//! Chunk generators and the worker pool that runs them off the game thread.
use basalt_world::Chunk;
use std::collections::HashSet;
use std::sync::{mpsc, Arc, Mutex};

pub trait Generator: Send + Sync {
    fn generate(&self, cx: i32, cz: i32) -> Chunk;
}

/// Vanilla-matching overworld terrain (crates/worldgen).
pub struct Vanilla(pub basalt_worldgen::Generator);

impl Generator for Vanilla {
    fn generate(&self, cx: i32, cz: i32) -> Chunk {
        let g = self.0.generate_chunk(cx, cz);
        Chunk::from_generated(cx, cz, &g.blocks, &g.biomes)
    }
}

/// Classic superflat: bedrock, two dirt, grass block; plains biome.
pub struct Flat { layers: Vec<u16>, biome: u8 }

impl Flat {
    pub fn new(plains_biome: u8) -> Flat {
        let s = |n: &str| crate::game::block_default(n).unwrap_or(1) as u16;
        Flat { layers: vec![s("bedrock"), s("dirt"), s("dirt"), s("grass_block")], biome: plains_biome }
    }
}

impl Generator for Flat {
    fn generate(&self, cx: i32, cz: i32) -> Chunk {
        let mut blocks = vec![[0u16; 4096]; basalt_world::SECTIONS];
        for (y, &b) in self.layers.iter().enumerate() {
            for i in 0..256 { blocks[0][y << 8 | i] = b; }
        }
        let biomes = vec![[self.biome; 64]; basalt_world::SECTIONS];
        Chunk::from_generated(cx, cz, &blocks, &biomes)
    }
}

/// Generates chunks on worker threads; the game thread polls finished ones.
pub struct GenPool {
    pub gen: Arc<dyn Generator>,
    jobs: mpsc::Sender<(i32, i32)>,
    done: mpsc::Receiver<Chunk>,
    pub pending: HashSet<(i32, i32)>,
}

impl GenPool {
    pub fn new(gen: Arc<dyn Generator>) -> GenPool {
        let (jtx, jrx) = mpsc::channel::<(i32, i32)>();
        let (dtx, drx) = mpsc::channel();
        let jrx = Arc::new(Mutex::new(jrx));
        let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).saturating_sub(2).clamp(1, 12);
        for i in 0..n {
            let (jrx, dtx, gen) = (jrx.clone(), dtx.clone(), gen.clone());
            std::thread::Builder::new().name(format!("gen{i}")).spawn(move || loop {
                let job = jrx.lock().unwrap().recv();
                let Ok((cx, cz)) = job else { return };
                if dtx.send(gen.generate(cx, cz)).is_err() { return; }
            }).unwrap();
        }
        GenPool { gen, jobs: jtx, done: drx, pending: HashSet::new() }
    }
    pub fn request(&mut self, cx: i32, cz: i32) {
        if self.pending.insert((cx, cz)) { let _ = self.jobs.send((cx, cz)); }
    }
    pub fn finished(&mut self) -> Vec<Chunk> {
        let v: Vec<Chunk> = self.done.try_iter().collect();
        for c in &v { self.pending.remove(&(c.x, c.z)); }
        v
    }
}

//! Overworld world generation for Basalt, driven entirely by the worldgen JSON that the
//! official data generator emits (noise settings, density functions, noises, biomes, ...).
//!
//! Stages ([`Stage`]): noise terrain with aquifers and ore veins, multi-noise biomes,
//! surface rules (+ iceberg / eroded-badlands extensions), cave and canyon carvers, and the
//! features implemented so far (ores, disks, freeze_top_layer). Structures, trees and other
//! vegetation are not generated. Measure against a vanilla world with the `compare` example.
//!
//! ```ignore
//! let g = basalt_worldgen::Generator::new(12345);
//! let chunk = g.generate_chunk(0, 0);
//! ```

pub mod aquifer;
pub mod biome;
pub mod carver;
pub mod density;
pub mod features;
pub mod noise;
pub mod region;
pub mod random;
pub mod surface;

use aquifer::{Aquifer, AquiferNodes, Fluid};
use anyhow::{anyhow, Context, Result};
use basalt_data::BlockState;
use density::{Eval, Graph, GraphBuilder, Mode, Node, NodeId};
use random::{PosFactory, Rng, Xoroshiro};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const MIN_Y: i32 = -64;
pub const HEIGHT: i32 = 384;
pub const SECTIONS: usize = 24;

/// Output of [`Generator::generate_chunk`].
pub struct GeneratedChunk {
    /// 24 sections (y = -64 upwards), index `y<<8 | z<<4 | x`, global block-state ids.
    pub blocks: Vec<[u16; 4096]>,
    /// 24 sections, index `y<<4 | z<<2 | x` (4×4×4 cells), biome registry ids.
    pub biomes: Vec<[u8; 64]>,
}

impl GeneratedChunk {
    pub fn get(&self, x: usize, y: i32, z: usize) -> u16 {
        let yy = (y - MIN_Y) as usize;
        self.blocks[yy >> 4][((yy & 15) << 8) | (z << 4) | x]
    }
}

/// Order in which the chunk and its neighbours are decorated.
/// (Vanilla's order depends on thread scheduling; measured differences are tiny.)
const DECORATION_ORDER: [(i32, i32); 9] = [(0, 0), (-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)];

/// Generation stages, in order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Stage {
    Noise,
    Surface,
    Carvers,
    Features,
}

pub(crate) struct Router {
    final_density: NodeId,
    vein_toggle: NodeId,
    vein_ridged: NodeId,
    vein_gap: NodeId,
    pub temperature: NodeId,
    pub vegetation: NodeId,
    pub continents: NodeId,
    pub erosion: NodeId,
    pub depth: NodeId,
    pub ridges: NodeId,
    aq: AquiferNodes,
}

#[derive(Clone, Copy)]
pub(crate) struct Blocks {
    lava: u16,
    air: u16,
    copper_ore: u16,
    raw_copper: u16,
    granite: u16,
    iron_ore: u16,
    raw_iron: u16,
    tuff: u16,
}

fn state(name: &str) -> Result<u16> {
    BlockState::parse(name).map(|s| s.0 as u16).ok_or_else(|| anyhow!("unknown block state {name}"))
}

/// Parse a `{"Name": .., "Properties": {..}}` block state object.
pub(crate) fn state_from_json(v: &Value) -> Result<u16> {
    let name = v["Name"].as_str().ok_or_else(|| anyhow!("block state without Name"))?;
    let mut s = BlockState::from_name(name).ok_or_else(|| anyhow!("unknown block {name}"))?;
    if let Some(p) = v.get("Properties").and_then(|p| p.as_object()) {
        for (k, val) in p {
            s = s.with(k, val.as_str().unwrap_or("")).ok_or_else(|| anyhow!("bad property {k} for {name}"))?;
        }
    }
    Ok(s.0 as u16)
}

/// Biome names in registry order (sorted file names of data/minecraft/worldgen/biome).
pub fn biome_names(data_dir: &Path) -> Result<Vec<String>> {
    let dir = data_dir.join("data/minecraft/worldgen/biome");
    let mut v: Vec<String> = std::fs::read_dir(&dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".json")).map(|s| s.to_string()))
        .collect();
    v.sort();
    Ok(v)
}

/// Overworld chunk generator. Cheap to share between threads.
pub struct Generator {
    pub seed: i64,
    graph: Graph,
    router: Router,
    aquifer_rand: PosFactory,
    ore_rand: PosFactory,
    sea_level: i32,
    aquifers_enabled: bool,
    ore_veins_enabled: bool,
    default_block: u16,
    default_fluid: u16,
    blocks: Blocks,
    interp_args: Vec<NodeId>,
    pub biome_names: Vec<String>,
    biomes: biome::BiomeSource,
    surface: surface::SurfaceSystem,
    carvers: Vec<Option<carver::Carver>>,
    /// Per biome: indices into `carvers`.
    biome_carvers: Vec<Vec<usize>>,
    trig: carver::Trig,
    features: features::Features,
    protos: std::sync::Mutex<ProtoCache>,
}

/// A chunk generated up to the carver stage (input for decoration).
pub struct Proto {
    pub blocks: Vec<[u16; 4096]>,
    pub biomes: Vec<[u8; 64]>,
}

struct ProtoCache {
    map: HashMap<(i32, i32), std::sync::Arc<Proto>>,
    order: std::collections::VecDeque<(i32, i32)>,
    cap: usize,
}

/// Target for the carvers: the chunk's blocks plus aquifer and surface access.
struct CarveChunk<'a, 'b, 'g> {
    env: &'b mut Env<'a, 'g>,
    blocks: &'b mut [[u16; 4096]],
    heights: [i32; 256],
    corners: [i32; 4],
}

impl carver::CarveTarget for CarveChunk<'_, '_, '_> {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        let yy = y - MIN_Y;
        self.blocks[(yy >> 4) as usize][(((yy & 15) << 8) | (z << 4) | x) as usize]
    }
    fn set(&mut self, x: i32, y: i32, z: i32, state: u16) {
        let yy = y - MIN_Y;
        self.blocks[(yy >> 4) as usize][(((yy & 15) << 8) | (z << 4) | x) as usize] = state;
    }
    fn carve_state(&mut self, x: i32, y: i32, z: i32) -> Option<u16> {
        let g = self.env.g;
        self.env.e.set_mode(Mode::Direct);
        match self.env.aq.compute(self.env.e, x, y, z, 0.0)? {
            Fluid::Air => Some(g.blocks.air),
            Fluid::Water => Some(g.default_fluid),
            Fluid::Lava => Some(g.blocks.lava),
        }
    }
    fn top_material(&mut self, x: i32, y: i32, z: i32, fluid: bool) -> Option<u16> {
        let g = self.env.g;
        g.surface.top_material(self.env, x, y, z, fluid, &self.heights, self.corners)
    }
    fn is_fluid(&self, state: u16) -> bool { state == self.env.g.default_fluid || state == self.env.g.blocks.lava }
}

/// Environment handed to the surface rules.
struct Env<'a, 'g> {
    g: &'a Generator,
    e: &'a mut Eval<'g>,
    aq: &'a mut Aquifer,
    cache: HashMap<(i32, i32, i32), u8>,
    last: Option<u32>,
}

impl surface::SurfaceEnv for Env<'_, '_> {
    fn noise_biome(&mut self, qx: i32, qy: i32, qz: i32) -> u8 {
        if let Some(b) = self.cache.get(&(qx, qy, qz)) { return *b; }
        self.e.set_mode(Mode::Direct);
        let t = self.g.climate(self.e, qx * 4, qy * 4, qz * 4);
        let (b, leaf) = self.g.biomes.search(&t, self.last);
        self.last = Some(leaf);
        self.cache.insert((qx, qy, qz), b);
        b
    }
    fn preliminary_surface(&mut self, x: i32, z: i32) -> i32 {
        self.e.set_mode(Mode::Direct);
        self.aq.preliminary_surface(self.e, x, z)
    }
}

/// Default location of the data generator output.
pub fn default_data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("BASALT_DATA") { return PathBuf::from(p); }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data");
    if manifest.exists() { manifest } else { PathBuf::from("data") }
}

impl Generator {
    /// Overworld generator using ./data (or $BASALT_DATA). Panics if the data is missing.
    pub fn new(seed: i64) -> Generator { Self::with_data_dir(seed, default_data_dir()) }

    pub fn with_data_dir(seed: i64, data_dir: impl AsRef<Path>) -> Generator {
        Self::try_new(seed, data_dir.as_ref()).expect("failed to load worldgen data")
    }

    pub fn try_new(seed: i64, data_dir: &Path) -> Result<Generator> {
        let wg = data_dir.join("data/minecraft/worldgen");
        let settings_path = wg.join("noise_settings/overworld.json");
        let settings: Value = serde_json::from_str(&std::fs::read_to_string(&settings_path).with_context(|| format!("reading {}", settings_path.display()))?)?;
        let rand = Xoroshiro::new(seed).fork_positional();
        let mut b = GraphBuilder::new(&wg, rand);
        let r = &settings["noise_router"];
        let mut p = |k: &str| -> Result<NodeId> { b.parse(&r[k]).with_context(|| format!("noise_router.{k}")) };
        let aq = AquiferNodes {
            barrier: p("barrier")?, floodedness: p("fluid_level_floodedness")?, spread: p("fluid_level_spread")?, lava: p("lava")?,
            erosion: p("erosion")?, depth: p("depth")?, prelim: p("preliminary_surface_level")?,
        };
        let router = Router {
            final_density: p("final_density")?, vein_toggle: p("vein_toggle")?, vein_ridged: p("vein_ridged")?, vein_gap: p("vein_gap")?,
            temperature: p("temperature")?, vegetation: p("vegetation")?, continents: p("continents")?, erosion: aq.erosion,
            depth: aq.depth, ridges: p("ridges")?, aq,
        };
        let graph = b.finish();
        let mut interp_args = vec![0; graph.n_interp as usize];
        for n in &graph.nodes {
            if let Node::Interpolated(a, s) = n { interp_args[*s as usize] = *a; }
        }
        let blocks = Blocks {
            lava: state("lava")?, air: state("air")?,
            copper_ore: state("copper_ore")?, raw_copper: state("raw_copper_block")?, granite: state("granite")?,
            iron_ore: state("deepslate_iron_ore")?, raw_iron: state("raw_iron_block")?, tuff: state("tuff")?,
        };
        let biome_names = biome_names(data_dir)?;
        let biomes = biome::BiomeSource::load(data_dir, &biome_names)?;
        let default_block = state_from_json(&settings["default_block"])?;
        let sea_level = settings["sea_level"].as_i64().unwrap_or(63) as i32;
        let mut carvers = Vec::new();
        let mut carver_names: Vec<String> = Vec::new();
        let mut biome_carvers = Vec::new();
        for n in &biome_names {
            let v: Value = serde_json::from_str(&std::fs::read_to_string(wg.join("biome").join(format!("{n}.json")))?)?;
            let list: Vec<String> = match &v["carvers"] {
                Value::String(s) => vec![s.clone()],
                Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect(),
                _ => Vec::new(),
            };
            let mut idx = Vec::new();
            for c in list {
                let i = match carver_names.iter().position(|x| *x == c) {
                    Some(i) => i,
                    None => {
                        // Carvers of other dimensions (e.g. nether_cave) are not supported: keep a slot.
                        carvers.push(carver::Carver::load(&data_dir.join("data/minecraft"), &c).ok());
                        carver_names.push(c);
                        carvers.len() - 1
                    }
                };
                idx.push(i);
            }
            biome_carvers.push(idx);
        }
        let features = features::Features::load(&data_dir.join("data/minecraft"), &biome_names, &biomes.possible_biomes())?;
        let surface = surface::SurfaceSystem::load(&wg, &settings, rand, &biome_names, default_block, sea_level, random::biome_zoom_seed(seed))?;
        Ok(Generator {
            seed, biome_names, biomes, surface, carvers, biome_carvers, trig: carver::Trig::new(), features,
            protos: std::sync::Mutex::new(ProtoCache { map: HashMap::new(), order: Default::default(), cap: 1024 }),
            aquifer_rand: rand.from_hash("minecraft:aquifer").fork_positional(),
            ore_rand: rand.from_hash("minecraft:ore").fork_positional(),
            sea_level,
            aquifers_enabled: settings["aquifers_enabled"].as_bool().unwrap_or(true),
            ore_veins_enabled: settings["ore_veins_enabled"].as_bool().unwrap_or(true),
            default_block,
            default_fluid: state_from_json(&settings["default_fluid"])?,
            graph, router, blocks, interp_args,
        })
    }

    /// Generate the terrain of one chunk.
    pub fn generate_chunk(&self, cx: i32, cz: i32) -> GeneratedChunk {
        self.generate_stages(cx, cz, Stage::Features)
    }

    /// Set how many carver-stage chunks are kept for decorating neighbours (default 1024,
    /// about 200 KB each).
    pub fn set_proto_cache_size(&self, n: usize) { self.protos.lock().unwrap().cap = n.max(25); }

    /// Chunk at the carver stage, cached (shared by the decoration of its neighbours).
    pub fn proto(&self, cx: i32, cz: i32) -> std::sync::Arc<Proto> {
        if let Some(p) = self.protos.lock().unwrap().map.get(&(cx, cz)) { return p.clone(); }
        let c = self.generate_stages(cx, cz, Stage::Carvers);
        let p = std::sync::Arc::new(Proto { blocks: c.blocks, biomes: c.biomes });
        let mut cache = self.protos.lock().unwrap();
        if !cache.map.contains_key(&(cx, cz)) {
            cache.map.insert((cx, cz), p.clone());
            cache.order.push_back((cx, cz));
            while cache.order.len() > cache.cap {
                if let Some(k) = cache.order.pop_front() { cache.map.remove(&k); }
            }
        }
        p
    }

    /// Decorate chunk (cx, cz): runs the features of it and its 8 neighbours over the
    /// carver-stage 5×5 chunk area around it.
    fn decorated(&self, cx: i32, cz: i32) -> GeneratedChunk {
        let mut blocks = Vec::with_capacity(25);
        let mut biomes = Vec::with_capacity(25);
        for dz in -2..=2 {
            for dx in -2..=2 {
                let p = self.proto(cx + dx, cz + dz);
                blocks.push(p.blocks.clone());
                biomes.push(p.biomes.clone());
            }
        }
        let own_biomes = biomes[12].clone();
        let mut reg = region::Region::new(self, cx - 2, cz - 2, 5, blocks, biomes);
        for (dx, dz) in DECORATION_ORDER {
            let (nx, nz) = (cx + dx, cz + dz);
            let mut set = vec![false; self.biome_names.len()];
            for ex in -1..=1 {
                for ez in -1..=1 { reg.chunk_biomes(nx + ex, nz + ez, &mut set); }
            }
            let list: Vec<u8> = set.iter().enumerate().filter(|(_, v)| **v).map(|(i, _)| i as u8).collect();
            reg.write = ((nx - 1) * 16, (nx + 2) * 16 - 1, (nz - 1) * 16, (nz + 2) * 16 - 1);
            self.features.decorate(self.seed, nx, nz, &list, &mut reg);
        }
        GeneratedChunk { blocks: reg.take_chunk(cx, cz), biomes: own_biomes }
    }

    /// Feature types present in the data that are not implemented yet (type → count).
    pub fn unsupported_features(&self) -> &std::collections::BTreeMap<String, usize> { &self.features.unsupported }

    /// Generate up to (and including) the given stage.
    pub fn generate_stages(&self, cx: i32, cz: i32, stage: Stage) -> GeneratedChunk {
        if stage >= Stage::Features { return self.decorated(cx, cz); }
        let mut e = Eval::new(&self.graph);
        let mut aq = Aquifer::new(self.router.aq, self.aquifer_rand, self.sea_level, self.aquifers_enabled);
        let mut blocks = self.fill_noise(&mut e, &mut aq, cx, cz);
        let biomes = self.fill_biomes(&mut e, cx, cz);
        if stage >= Stage::Surface {
            let mut cache = HashMap::new();
            for (si, sec) in biomes.iter().enumerate() {
                for (j, b) in sec.iter().enumerate() {
                    cache.insert((cx * 4 + (j & 3) as i32, si as i32 * 4 + (j >> 4) as i32 + MIN_Y / 4, cz * 4 + ((j >> 2) & 3) as i32), *b);
                }
            }
            let mut env = Env { g: self, e: &mut e, aq: &mut aq, cache, last: None };
            self.surface.build(&mut env, cx, cz, &mut blocks);
            if stage >= Stage::Carvers {
                self.carve(&mut env, cx, cz, &mut blocks);
            }
        }
        GeneratedChunk { blocks, biomes }
    }

    fn carve(&self, env: &mut Env, cx: i32, cz: i32, blocks: &mut [[u16; 4096]]) {
        let (heights, corners) = self.surface.column_info(env, cx, cz, blocks);
        let mut ctx = carver::CarveCtx {
            trig: &self.trig, cx, cz, mask: vec![0u64; 16 * 16 * HEIGHT as usize / 64],
            grass: state("grass_block[snowy=false]").unwrap_or(0), mycelium: state("mycelium[snowy=false]").unwrap_or(0),
            dirt: state("dirt").unwrap_or(0), lava: self.blocks.lava,
        };
        let mut target = CarveChunk { env, blocks, heights, corners };
        let mut r = random::JavaRandom::new(0);
        for dx in -8..=8 {
            for dz in -8..=8 {
                let (sx, sz) = (cx + dx, cz + dz);
                use surface::SurfaceEnv;
                let b = target.env.noise_biome(sx * 4, 0, sz * 4);
                for (l, ci) in self.biome_carvers[b as usize].iter().enumerate() {
                    let Some(c) = &self.carvers[*ci] else { continue };
                    if carver::start(&mut r, self.seed, l as i32, sx, sz, c) {
                        ctx.carve(c, &mut target, &mut r, sx, sz);
                    }
                }
            }
        }
    }

    pub fn debug_biome(&self, qx: i32, qy: i32, qz: i32) {
        let mut e = Eval::new(&self.graph);
        let t = self.climate(&mut e, qx * 4, qy * 4, qz * 4);
        println!("target {t:?}");
        for (f, i) in self.biomes.ranked(&t, 6) {
            let en = &self.biomes.entries[i];
            println!("  {f} #{i} {} {:?} off {}", self.biome_names[en.biome as usize], en.p, en.offset);
        }
    }

    fn climate(&self, e: &mut Eval, x: i32, y: i32, z: i32) -> biome::Target {
        let r = &self.router;
        let q = |e: &mut Eval, n: NodeId| biome::quantize(e.eval(n, x, y, z));
        [q(e, r.temperature), q(e, r.vegetation), q(e, r.continents), q(e, r.erosion), q(e, r.depth), q(e, r.ridges)]
    }

    /// Biome at a quart position (block coordinates / 4).
    pub fn noise_biome(&self, e: &mut Eval, qx: i32, qy: i32, qz: i32) -> u8 {
        e.set_mode(Mode::Direct);
        let t = self.climate(e, qx * 4, qy * 4, qz * 4);
        self.biomes.search(&t, None).0
    }

    /// Biome stage: 4×4×4 cells for the whole chunk.
    pub fn fill_biomes(&self, e: &mut Eval, cx: i32, cz: i32) -> Vec<[u8; 64]> {
        e.set_mode(Mode::Direct);
        let mut out = vec![[0u8; 64]; SECTIONS];
        let mut cols = [[0i64; 6]; 16];
        for qx in 0..4 {
            for qz in 0..4 {
                cols[(qx * 4 + qz) as usize] = self.climate(e, cx * 16 + qx * 4, 0, cz * 16 + qz * 4);
            }
        }
        // Same visiting order as vanilla (section, x, y, z) so warm-started ties resolve alike.
        let mut last = None;
        for (si, sec) in out.iter_mut().enumerate() {
            for qx in 0..4 {
                for qyl in 0..4 {
                    for qz in 0..4 {
                        let qy = si as i32 * 4 + qyl;
                        let (x, z) = (cx * 16 + qx * 4, cz * 16 + qz * 4);
                        let mut t = cols[(qx * 4 + qz) as usize];
                        t[4] = biome::quantize(e.eval(self.router.depth, x, MIN_Y + qy * 4, z));
                        let (b, leaf) = self.biomes.search(&t, last);
                        last = Some(leaf);
                        sec[((qyl << 4) | (qz << 2) | qx) as usize] = b;
                    }
                }
            }
        }
        out
    }

    /// Noise stage: stone / water / lava / air (+ ore veins).
    pub fn fill_noise(&self, e: &mut Eval, aq: &mut Aquifer, cx: i32, cz: i32) -> Vec<[u16; 4096]> {
        let x0 = cx * 16;
        let z0 = cz * 16;
        const CW: i32 = 4;
        const CH: i32 = 8;
        const NX: usize = 5;
        const NY: usize = (HEIGHT / CH) as usize + 1;
        let ns = self.interp_args.len();
        // corners[slot][ix][iz][iy]
        let mut corners = vec![0f64; ns * NX * NX * NY];
        let idx = |s: usize, ix: usize, iz: usize, iy: usize| ((s * NX + ix) * NX + iz) * NY + iy;
        e.set_mode(Mode::Direct);
        for ix in 0..NX {
            for iz in 0..NX {
                for iy in 0..NY {
                    let x = x0 + ix as i32 * CW;
                    let z = z0 + iz as i32 * CW;
                    let y = MIN_Y + iy as i32 * CH;
                    for (s, a) in self.interp_args.iter().enumerate() {
                        corners[idx(s, ix, iz, iy)] = e.eval(*a, x, y, z);
                    }
                }
            }
        }
        let mut out = vec![[self.blocks.air; 4096]; SECTIONS];
        let bl = self.blocks;
        for lx in 0..16i32 {
            let ix = (lx / CW) as usize;
            let fx = (lx % CW) as f64 / CW as f64;
            for lz in 0..16i32 {
                let iz = (lz / CW) as usize;
                let fz = (lz % CW) as f64 / CW as f64;
                for yy in 0..HEIGHT {
                    let iy = (yy / CH) as usize;
                    let fy = (yy % CH) as f64 / CH as f64;
                    e.set_mode(Mode::Interpolated);
                    for s in 0..ns {
                        let c = |dx: usize, dy: usize, dz: usize| corners[idx(s, ix + dx, iz + dz, iy + dy)];
                        let v00 = noise::lerp(fy, c(0, 0, 0), c(0, 1, 0));
                        let v10 = noise::lerp(fy, c(1, 0, 0), c(1, 1, 0));
                        let v01 = noise::lerp(fy, c(0, 0, 1), c(0, 1, 1));
                        let v11 = noise::lerp(fy, c(1, 0, 1), c(1, 1, 1));
                        let v0 = noise::lerp(fx, v00, v10);
                        let v1 = noise::lerp(fx, v01, v11);
                        e.interp[s] = noise::lerp(fz, v0, v1);
                    }
                    let x = x0 + lx;
                    let z = z0 + lz;
                    let y = MIN_Y + yy;
                    let d = e.eval(self.router.final_density, x, y, z);
                    let st = match aq.compute(e, x, y, z, d) {
                        Some(Fluid::Air) => bl.air,
                        Some(Fluid::Water) => self.default_fluid,
                        Some(Fluid::Lava) => bl.lava,
                        None => {
                            if self.ore_veins_enabled { self.ore_vein(e, x, y, z).unwrap_or(self.default_block) } else { self.default_block }
                        }
                    };
                    out[(yy >> 4) as usize][(((yy & 15) << 8) | (lz << 4) | lx) as usize] = st;
                }
            }
        }
        out
    }

    fn ore_vein(&self, e: &mut Eval, x: i32, y: i32, z: i32) -> Option<u16> {
        let d = e.eval(self.router.vein_toggle, x, y, z);
        let copper = d > 0.0;
        let (ore, raw, filler, min_y, max_y) = if copper {
            (self.blocks.copper_ore, self.blocks.raw_copper, self.blocks.granite, 0, 50)
        } else {
            (self.blocks.iron_ore, self.blocks.raw_iron, self.blocks.tuff, -60, -8)
        };
        let ad = d.abs();
        let j = max_y - y;
        let k = y - min_y;
        if k < 0 || j < 0 { return None; }
        let l = j.min(k);
        let f = density::clamped_map(l as f64, 0.0, 20.0, -0.2, 0.0);
        if ad + f < 0.4 { return None; }
        let mut rs = self.ore_rand.at(x, y, z);
        if rs.next_float() as f64 > 0.7f32 as f64 { return None; }
        if e.eval(self.router.vein_ridged, x, y, z) >= 0.0 { return None; }
        let g = density::clamped_map(ad, 0.4, 0.6, 0.1, 0.3);
        if (rs.next_float() as f64) < g && e.eval(self.router.vein_gap, x, y, z) > -0.3f32 as f64 {
            return Some(if rs.next_float() < 0.02 { raw } else { ore });
        }
        Some(filler)
    }

    pub fn stone_id(&self) -> u16 { self.default_block }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gen() -> Option<Generator> {
        let d = default_data_dir();
        if !d.join("data/minecraft/worldgen").exists() { return None; }
        Some(Generator::with_data_dir(12345, d))
    }

    #[test]
    fn generator_is_send_sync() {
        fn check<T: Send + Sync>() {}
        check::<Generator>();
    }

    #[test]
    fn generates_terrain() {
        let Some(g) = gen() else { return };
        let c = g.generate_chunk(0, 0);
        assert_eq!(c.blocks.len(), 24);
        // Bottom is solid, top is air.
        assert_ne!(c.get(0, -63, 0), 0);
        assert_eq!(c.get(0, 319, 0), 0);
    }
}

//! Chunk generators. `Flat` is a stand-in until crates/worldgen lands.
use basalt_world::Chunk;

pub trait Generator: Send {
    fn generate(&self, cx: i32, cz: i32) -> Chunk;
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

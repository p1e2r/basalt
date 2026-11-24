//! The bot's view of the world: chunk columns decoded from level_chunk_with_light and kept up to
//! date by block_update / section_blocks_update.
use anyhow::Result;
use basalt_protocol::Reader;
use basalt_world::PalettedContainer;
use std::collections::HashMap;

pub const MIN_Y: i32 = -64;
pub const SECTIONS: usize = 24;

pub struct Column {
    pub blocks: Vec<PalettedContainer>,
    pub biomes: Vec<PalettedContainer>,
}

/// `min_y` / `sections`: the dimension layout (overworld -64 / 24, nether 0 / 16).
pub struct World {
    pub chunks: HashMap<(i32, i32), Column>,
    pub min_y: i32,
    pub sections: usize,
}

impl Default for World {
    fn default() -> Self {
        World {
            chunks: HashMap::new(),
            min_y: MIN_Y,
            sections: SECTIONS,
        }
    }
}

impl Column {
    /// Decode the body of level_chunk_with_light after the two i32 chunk coordinates.
    pub fn decode(r: &mut Reader, sections: usize) -> Result<Column> {
        let nh = r.varint()?;
        for _ in 0..nh {
            let _ty = r.varint()?;
            let n = r.varint()?;
            r.take(n.max(0) as usize * 8)?;
        }
        let size = r.varint()?.max(0) as usize;
        let data = r.take(size)?;
        let mut d = Reader::new(data);
        let (mut blocks, mut biomes) = (Vec::with_capacity(sections), Vec::with_capacity(sections));
        for _ in 0..sections {
            let _non_air = d.i16()?;
            let _fluid = d.i16()?;
            blocks.push(PalettedContainer::read_network(&mut d, true)?);
            biomes.push(PalettedContainer::read_network(&mut d, false)?);
        }
        // block entities and light follow; the bot doesn't need them
        Ok(Column { blocks, biomes })
    }
}

impl World {
    /// Empty world with the layout of a dimension id (`minecraft:the_nether`: y 0..256).
    pub fn for_dim(dim: &str) -> World {
        match dim.trim_start_matches("minecraft:") {
            "the_nether" => World {
                min_y: 0,
                sections: 16,
                ..Default::default()
            },
            _ => World::default(),
        }
    }
    pub fn loaded(&self, x: i32, z: i32) -> bool {
        self.chunks.contains_key(&(x >> 4, z >> 4))
    }
    pub fn get(&self, x: i32, y: i32, z: i32) -> u32 {
        if !(self.min_y..self.min_y + 16 * self.sections as i32).contains(&y) {
            return 0;
        }
        match self.chunks.get(&(x >> 4, z >> 4)) {
            Some(c) => c.blocks[((y - self.min_y) >> 4) as usize]
                .get(((y & 15) << 8 | (z & 15) << 4 | (x & 15)) as usize),
            None => 0,
        }
    }
    pub fn set(&mut self, x: i32, y: i32, z: i32, s: u32) {
        if !(self.min_y..self.min_y + 16 * self.sections as i32).contains(&y) {
            return;
        }
        if let Some(c) = self.chunks.get_mut(&(x >> 4, z >> 4)) {
            c.blocks[((y - self.min_y) >> 4) as usize]
                .set(((y & 15) << 8 | (z & 15) << 4 | (x & 15)) as usize, s);
        }
    }
    pub fn name(&self, x: i32, y: i32, z: i32) -> &'static str {
        basalt_data::BlockState(self.get(x, y, z)).name()
    }
    /// Full state string, e.g. `minecraft:oak_log[axis=x]`.
    pub fn state_string(&self, x: i32, y: i32, z: i32) -> String {
        basalt_data::BlockState(self.get(x, y, z)).to_string()
    }
}

//! World storage for Basalt: paletted chunk sections, lighting, heightmaps, the network
//! chunk encoding and Anvil region file persistence (overworld dimensions).

pub mod anvil;
pub mod chunk;
pub mod light;
pub mod paletted;
pub mod props;

pub use chunk::{Chunk, HeightmapKind, Heightmaps, Layout, LightArray, Section};
pub use paletted::{Kind, PalettedContainer};

use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;

include!(concat!(env!("OUT_DIR"), "/biomes.rs"));

pub const MIN_Y: i32 = -64;
pub const HEIGHT: i32 = 384;
pub const SECTIONS: usize = 24;
/// Light sections: one below and one above the block sections.
pub const LIGHT_SECTIONS: usize = SECTIONS + 2;

/// Network id of a biome ("plains" or "minecraft:plains").
pub fn biome_id(name: &str) -> Option<u32> {
    let n = name.trim_start_matches("minecraft:");
    BIOMES.iter().position(|b| *b == n).map(|i| i as u32)
}

/// A dimension's loaded chunks plus optional region-file persistence.
pub struct World {
    pub chunks: HashMap<(i32, i32), Chunk>,
    /// Directory holding `r.X.Z.mca` files (e.g. `world/region`); `None` = in-memory only.
    pub dir: Option<PathBuf>,
    /// Dimension layout every chunk of this world gets.
    pub layout: Layout,
    regions: HashMap<(i32, i32), anvil::RegionFile>,
}

impl World {
    /// `dir` is the region directory itself (the one containing `r.X.Z.mca`).
    pub fn new(dir: Option<PathBuf>) -> World {
        Self::new_in(dir, Layout::OVERWORLD)
    }

    /// A world of a dimension with the given layout (e.g. [`Layout::NETHER`]).
    pub fn new_in(dir: Option<PathBuf>, layout: Layout) -> World {
        World { chunks: HashMap::new(), dir, layout, regions: HashMap::new() }
    }

    pub fn chunk(&self, cx: i32, cz: i32) -> Option<&Chunk> { self.chunks.get(&(cx, cz)) }
    pub fn chunk_mut(&mut self, cx: i32, cz: i32) -> Option<&mut Chunk> { self.chunks.get_mut(&(cx, cz)) }
    pub fn is_loaded(&self, cx: i32, cz: i32) -> bool { self.chunks.contains_key(&(cx, cz)) }
    /// Coordinates of all chunks currently in memory.
    pub fn loaded_chunks(&self) -> Vec<(i32, i32)> { self.chunks.keys().copied().collect() }

    /// Insert (or replace) a chunk. Does not compute light; call `light_chunk` afterwards.
    pub fn insert(&mut self, mut c: Chunk) { if c.layout != self.layout { c.set_layout(self.layout); } self.chunks.insert((c.x, c.z), c); }

    fn region(&mut self, cx: i32, cz: i32, create: bool) -> Result<Option<&mut anvil::RegionFile>> {
        let Some(dir) = &self.dir else { return Ok(None) };
        let key = (cx >> 5, cz >> 5);
        let reopen = match self.regions.get(&key) { Some(r) => create && !r.is_writable(), None => true };
        if reopen {
            let path = anvil::region_path(dir, cx, cz);
            if !create && !path.exists() { return Ok(None); }
            // Loading opens read-only; the first save reopens for writing.
            let r = if create { anvil::RegionFile::open(&path)? } else { anvil::RegionFile::open_read(&path)? };
            self.regions.insert(key, r);
        }
        Ok(self.regions.get_mut(&key))
    }

    /// Load chunk from its region file if present. Returns `Ok(false)` if not on disk (or
    /// not fully generated), in which case the caller generates it. Already-loaded → `true`.
    pub fn load(&mut self, cx: i32, cz: i32) -> Result<bool> {
        if self.chunks.contains_key(&(cx, cz)) { return Ok(true); }
        let Some(r) = self.region(cx, cz, false)? else { return Ok(false) };
        let Some(bytes) = r.read((cx & 31) as usize, (cz & 31) as usize)? else { return Ok(false) };
        let (c, status) = anvil::chunk_from_bytes_in(&bytes, self.layout)?;
        if status.trim_start_matches("minecraft:") != "full" { return Ok(false); }
        if (c.x, c.z) != (cx, cz) { anyhow::bail!("chunk at {cx},{cz} claims to be {},{}", c.x, c.z); }
        self.chunks.insert((cx, cz), c);
        Ok(true)
    }

    /// Block state at world coords; 0 (air) if unloaded or out of range.
    pub fn get_block(&self, x: i32, y: i32, z: i32) -> u32 {
        match self.chunks.get(&(x >> 4, z >> 4)) {
            Some(c) => c.get_block((x & 15) as usize, y, (z & 15) as usize),
            None => 0,
        }
    }

    /// (sky, block) light at world coords; (15, 0) above the world, (0, 0) if unloaded.
    pub fn light(&self, x: i32, y: i32, z: i32) -> (u8, u8) {
        match self.chunks.get(&(x >> 4, z >> 4)) {
            Some(c) => (c.sky_light((x & 15) as usize, y, (z & 15) as usize), c.block_light((x & 15) as usize, y, (z & 15) as usize)),
            None => (0, 0),
        }
    }

    pub fn biome(&self, x: i32, y: i32, z: i32) -> u32 {
        self.chunks.get(&(x >> 4, z >> 4)).map(|c| c.biome((x & 15) as usize, y, (z & 15) as usize)).unwrap_or(0)
    }

    fn take_window(&mut self, cx: i32, cz: i32) -> light::Window {
        let chunks = std::array::from_fn(|i| self.chunks.remove(&(cx + (i % 3) as i32 - 1, cz + (i / 3) as i32 - 1)));
        light::Window::new(cx, cz, chunks)
    }

    fn put_window(&mut self, w: light::Window) -> Vec<(i32, i32)> {
        let changed = w.changed();
        for c in w.chunks.into_iter().flatten() { self.chunks.insert((c.x, c.z), c); }
        changed
    }

    /// Set a block and update lighting incrementally (sky + block light, across chunk
    /// borders). Returns the chunk coords whose light changed. No-op if unloaded.
    pub fn set_block(&mut self, x: i32, y: i32, z: i32, state: u32) -> Vec<(i32, i32)> {
        if !self.layout.contains_y(y) { return Vec::new(); }
        let (cx, cz) = (x >> 4, z >> 4);
        let Some(c) = self.chunks.get_mut(&(cx, cz)) else { return Vec::new() };
        let old = c.set_block((x & 15) as usize, y, (z & 15) as usize, state);
        let (po, pn) = (props::props(old), props::props(state));
        if old == state || (po.opacity == pn.opacity && po.emission == pn.emission) { return Vec::new(); }
        let mut w = self.take_window(cx, cz);
        w.block_changed(x, y, z);
        self.put_window(w)
    }

    /// Full light computation for a chunk, considering loaded neighbours.
    pub fn light_chunk(&mut self, cx: i32, cz: i32) { self.light_chunk_changed(cx, cz); }

    /// Like `light_chunk`, returning every chunk whose light changed (the chunk itself and
    /// any loaded neighbours that received light from it).
    pub fn light_chunk_changed(&mut self, cx: i32, cz: i32) -> Vec<(i32, i32)> {
        if !self.chunks.contains_key(&(cx, cz)) { return Vec::new(); }
        let mut w = self.take_window(cx, cz);
        w.light_center();
        self.put_window(w)
    }

    fn save_chunk(&mut self, cx: i32, cz: i32) -> Result<()> {
        if self.dir.is_none() { return Ok(()); }
        let Some(c) = self.chunks.get(&(cx, cz)) else { return Ok(()) };
        if !c.dirty { return Ok(()); }
        let bytes = anvil::chunk_to_bytes(c);
        let r = self.region(cx, cz, true)?.expect("dir is set");
        r.write((cx & 31) as usize, (cz & 31) as usize, &bytes)?;
        if let Some(c) = self.chunks.get_mut(&(cx, cz)) { c.dirty = false; }
        Ok(())
    }

    /// Write all dirty chunks to their region files (no-op for in-memory worlds).
    pub fn save_all(&mut self) -> Result<()> {
        if self.dir.is_none() { return Ok(()); }
        let dirty: Vec<(i32, i32)> = self.chunks.iter().filter(|(_, c)| c.dirty).map(|(k, _)| *k).collect();
        for (cx, cz) in dirty { self.save_chunk(cx, cz)?; }
        for r in self.regions.values_mut() { r.flush()?; }
        Ok(())
    }

    /// Save (if dirty) and drop a chunk from memory.
    pub fn unload(&mut self, cx: i32, cz: i32) -> Result<()> {
        self.save_chunk(cx, cz)?;
        self.chunks.remove(&(cx, cz));
        Ok(())
    }
}

#[cfg(test)]
mod tests;

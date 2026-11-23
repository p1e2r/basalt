//! Chunk columns: sections, light, heightmaps, block entities, and the network encoding.

use crate::paletted::{write_varint, Kind, PalettedContainer};
use crate::props::props;
use crate::{HEIGHT, LIGHT_SECTIONS, MIN_Y, SECTIONS};
use basalt_nbt::Tag;

/// 16x16x16 nibble array (2048 bytes) stored lazily: `None` means every entry is `fill`.
/// Index `y<<8 | z<<4 | x`; even indices in the low nibble.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightArray {
    data: Option<Box<[u8; 2048]>>,
    fill: u8,
}

impl LightArray {
    pub fn filled(v: u8) -> Self { LightArray { data: None, fill: v & 15 } }
    pub fn from_bytes(b: &[u8]) -> Self {
        let mut a = Box::new([0u8; 2048]);
        a.copy_from_slice(&b[..2048]);
        let mut l = LightArray { data: Some(a), fill: 0 };
        l.normalize();
        l
    }
    #[inline]
    pub fn get(&self, i: usize) -> u8 {
        match &self.data { Some(a) => (a[i >> 1] >> ((i & 1) * 4)) & 15, None => self.fill }
    }
    #[inline]
    pub fn set(&mut self, i: usize, v: u8) {
        let v = v & 15;
        if self.data.is_none() {
            if v == self.fill { return; }
            let f = self.fill | self.fill << 4;
            self.data = Some(Box::new([f; 2048]));
        }
        let a = self.data.as_mut().unwrap();
        let sh = (i & 1) * 4;
        a[i >> 1] = (a[i >> 1] & !(15 << sh)) | (v << sh);
    }
    /// `Some(v)` if every entry equals `v`.
    pub fn uniform(&self) -> Option<u8> {
        match &self.data {
            None => Some(self.fill),
            Some(a) => {
                let b = a[0];
                if b >> 4 == b & 15 && a.iter().all(|x| *x == b) { Some(b & 15) } else { None }
            }
        }
    }
    /// Collapse to the uniform representation if possible.
    pub fn normalize(&mut self) {
        if self.data.is_some() {
            if let Some(v) = self.uniform() { *self = LightArray::filled(v); }
        }
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        match &self.data { Some(a) => a.to_vec(), None => vec![self.fill | self.fill << 4; 2048] }
    }
}

/// One 16-block-tall section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub blocks: PalettedContainer,
    pub biomes: PalettedContainer,
    /// Number of non-air blocks (maintained by `Chunk::set_block`).
    pub non_air: u16,
    /// Number of blocks containing a fluid.
    pub fluids: u16,
}

impl Section {
    pub fn empty(biome: u32) -> Section {
        Section { blocks: PalettedContainer::blocks_filled(0), biomes: PalettedContainer::biomes_filled(biome), non_air: 0, fluids: 0 }
    }
    pub fn new(blocks: PalettedContainer, biomes: PalettedContainer) -> Section {
        let mut s = Section { blocks, biomes, non_air: 0, fluids: 0 };
        s.recount();
        s
    }
    /// Recompute `non_air` / `fluids` from the block data.
    pub fn recount(&mut self) {
        let (mut a, mut f) = (0u16, 0u16);
        if let Some(v) = self.blocks.single() {
            let p = props(v);
            if !p.is_air() { a = 4096; }
            if p.has_fluid() { f = 4096; }
        } else {
            for v in self.blocks.values() {
                let p = props(v);
                a += !p.is_air() as u16;
                f += p.has_fluid() as u16;
            }
        }
        self.non_air = a;
        self.fluids = f;
    }
}

/// Heightmap kinds; the discriminant is the network id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeightmapKind { WorldSurface = 1, OceanFloor = 3, MotionBlocking = 4, MotionBlockingNoLeaves = 5 }

impl HeightmapKind {
    pub const ALL: [HeightmapKind; 4] = [HeightmapKind::WorldSurface, HeightmapKind::OceanFloor, HeightmapKind::MotionBlocking, HeightmapKind::MotionBlockingNoLeaves];
    pub fn disk_name(self) -> &'static str {
        match self {
            HeightmapKind::WorldSurface => "WORLD_SURFACE",
            HeightmapKind::OceanFloor => "OCEAN_FLOOR",
            HeightmapKind::MotionBlocking => "MOTION_BLOCKING",
            HeightmapKind::MotionBlockingNoLeaves => "MOTION_BLOCKING_NO_LEAVES",
        }
    }
    /// Does this state count for the heightmap?
    pub fn matches(self, state: u32) -> bool {
        let p = props(state);
        match self {
            HeightmapKind::WorldSurface => !p.is_air(),
            HeightmapKind::OceanFloor => p.blocks_motion(),
            HeightmapKind::MotionBlocking => p.blocks_motion() || p.has_fluid(),
            HeightmapKind::MotionBlockingNoLeaves => (p.blocks_motion() || p.has_fluid()) && !p.is_leaves(),
        }
    }
}

/// Per-column heights: `highest matching y - MIN_Y + 1` (0 = no matching block). Index `z*16+x`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heightmaps {
    pub world_surface: [u16; 256],
    pub ocean_floor: [u16; 256],
    pub motion_blocking: [u16; 256],
    pub motion_blocking_no_leaves: [u16; 256],
}

impl Default for Heightmaps {
    fn default() -> Self { Heightmaps { world_surface: [0; 256], ocean_floor: [0; 256], motion_blocking: [0; 256], motion_blocking_no_leaves: [0; 256] } }
}

impl Heightmaps {
    pub fn get(&self, k: HeightmapKind) -> &[u16; 256] {
        match k {
            HeightmapKind::WorldSurface => &self.world_surface,
            HeightmapKind::OceanFloor => &self.ocean_floor,
            HeightmapKind::MotionBlocking => &self.motion_blocking,
            HeightmapKind::MotionBlockingNoLeaves => &self.motion_blocking_no_leaves,
        }
    }
    pub fn get_mut(&mut self, k: HeightmapKind) -> &mut [u16; 256] {
        match k {
            HeightmapKind::WorldSurface => &mut self.world_surface,
            HeightmapKind::OceanFloor => &mut self.ocean_floor,
            HeightmapKind::MotionBlocking => &mut self.motion_blocking,
            HeightmapKind::MotionBlockingNoLeaves => &mut self.motion_blocking_no_leaves,
        }
    }
    /// Packed form: 9-bit entries, 7 per long, 37 longs.
    pub fn packed(&self, k: HeightmapKind) -> Vec<u64> {
        crate::paletted::pack(self.get(k).iter().map(|v| *v as u32), heightmap_bits())
    }
}

/// Bits per heightmap entry: ceil(log2(HEIGHT + 1)) = 9.
pub fn heightmap_bits() -> u8 { crate::paletted::ceil_log2(HEIGHT as usize + 1) }

#[derive(Debug, Clone)]
pub struct Chunk {
    pub x: i32,
    pub z: i32,
    /// `SECTIONS` sections, bottom-up from `MIN_Y`.
    pub sections: Vec<Section>,
    /// `LIGHT_SECTIONS` (= SECTIONS + 2) sky light arrays; index 0 is the section below the world.
    pub sky_light: Vec<LightArray>,
    pub block_light: Vec<LightArray>,
    pub heightmaps: Heightmaps,
    /// Block entities in the on-disk form (compound with `id`, `x`, `y`, `z`, ...; world coords).
    pub block_entities: Vec<Tag>,
    /// Unrecognised top-level tags from disk (structures, ticks, ...), written back unchanged.
    pub extra: basalt_nbt::Compound,
    /// Needs saving.
    pub dirty: bool,
    /// Light has been computed (or loaded).
    pub lit: bool,
}

#[inline]
fn block_index(lx: usize, y: i32, lz: usize) -> (usize, usize) {
    let ry = (y - MIN_Y) as usize;
    (ry >> 4, (ry & 15) << 8 | lz << 4 | lx)
}

impl Chunk {
    pub fn empty(x: i32, z: i32) -> Chunk {
        let plains = crate::biome_id("plains").unwrap_or(0);
        Self::from_sections(x, z, (0..SECTIONS).map(|_| Section::empty(plains)).collect())
    }

    /// Construct from sections; heightmaps are computed, light is set to "unlit"
    /// (sky 15 above the world, 0 elsewhere) until `World::light_chunk` runs.
    pub fn from_sections(x: i32, z: i32, sections: Vec<Section>) -> Chunk {
        assert_eq!(sections.len(), SECTIONS);
        let mut sky_light: Vec<LightArray> = (0..LIGHT_SECTIONS).map(|_| LightArray::filled(0)).collect();
        sky_light[LIGHT_SECTIONS - 1] = LightArray::filled(15);
        let mut c = Chunk {
            x, z, sections, sky_light,
            block_light: (0..LIGHT_SECTIONS).map(|_| LightArray::filled(0)).collect(),
            heightmaps: Heightmaps::default(), block_entities: Vec::new(), extra: Default::default(),
            dirty: true, lit: false,
        };
        c.recompute_heightmaps();
        c
    }

    /// From a generator: per section blocks index `y<<8|z<<4|x` (24 sections bottom-up),
    /// biomes index `y<<4|z<<2|x`.
    pub fn from_generated(x: i32, z: i32, blocks: &[[u16; 4096]], biomes: &[[u8; 64]]) -> Chunk {
        let plains = crate::biome_id("plains").unwrap_or(0);
        let sections = (0..SECTIONS).map(|i| {
            let b = match blocks.get(i) {
                Some(b) => { let v: Vec<u32> = b.iter().map(|s| *s as u32).collect(); PalettedContainer::from_values(Kind::Blocks, &v) }
                None => PalettedContainer::blocks_filled(0),
            };
            let bi = match biomes.get(i) {
                Some(b) => { let v: Vec<u32> = b.iter().map(|s| *s as u32).collect(); PalettedContainer::from_values(Kind::Biomes, &v) }
                None => PalettedContainer::biomes_filled(plains),
            };
            Section::new(b, bi)
        }).collect();
        Self::from_sections(x, z, sections)
    }

    #[inline]
    pub fn in_range(y: i32) -> bool { (MIN_Y..MIN_Y + HEIGHT).contains(&y) }

    /// Block state at local x/z and world y (air outside the build range).
    #[inline]
    pub fn get_block(&self, lx: usize, y: i32, lz: usize) -> u32 {
        if !Self::in_range(y) { return 0; }
        let (s, i) = block_index(lx & 15, y, lz & 15);
        self.sections[s].blocks.get(i)
    }

    /// Set a block; maintains section counts and heightmaps, marks dirty. Does not touch
    /// light (use `World::set_block` for that). Returns the old state.
    pub fn set_block(&mut self, lx: usize, y: i32, lz: usize, state: u32) -> u32 {
        if !Self::in_range(y) { return 0; }
        let (lx, lz) = (lx & 15, lz & 15);
        let (s, i) = block_index(lx, y, lz);
        let sec = &mut self.sections[s];
        let old = sec.blocks.set(i, state);
        if old == state { return old; }
        let (po, pn) = (props(old), props(state));
        sec.non_air = (sec.non_air as i32 - !po.is_air() as i32 + !pn.is_air() as i32) as u16;
        sec.fluids = (sec.fluids as i32 - po.has_fluid() as i32 + pn.has_fluid() as i32) as u16;
        self.dirty = true;
        let col = lz * 16 + lx;
        let h = (y - MIN_Y + 1) as u16;
        for k in HeightmapKind::ALL {
            let cur = self.heightmaps.get(k)[col];
            if k.matches(state) {
                if h > cur { self.heightmaps.get_mut(k)[col] = h; }
            } else if h == cur {
                let nh = self.scan_height(k, lx, lz, y - 1);
                self.heightmaps.get_mut(k)[col] = nh;
            }
        }
        old
    }

    fn scan_height(&self, k: HeightmapKind, lx: usize, lz: usize, from_y: i32) -> u16 {
        let mut y = from_y;
        while y >= MIN_Y {
            let (s, i) = block_index(lx, y, lz);
            let sec = &self.sections[s];
            if let Some(v) = sec.blocks.single() {
                if !k.matches(v) { y = MIN_Y + (s as i32) * 16 - 1; continue; }
                return (y - MIN_Y + 1) as u16;
            }
            if k.matches(sec.blocks.get(i)) { return (y - MIN_Y + 1) as u16; }
            y -= 1;
        }
        0
    }

    pub fn recompute_heightmaps(&mut self) {
        for k in HeightmapKind::ALL {
            for lz in 0..16 {
                for lx in 0..16 {
                    let h = self.scan_height(k, lx, lz, MIN_Y + HEIGHT - 1);
                    self.heightmaps.get_mut(k)[lz * 16 + lx] = h;
                }
            }
        }
    }

    /// Reset light to the unlit state (sky 15 above the world, 0 elsewhere).
    pub fn clear_light(&mut self) {
        for a in self.block_light.iter_mut() { *a = LightArray::filled(0); }
        for a in self.sky_light.iter_mut() { *a = LightArray::filled(0); }
        self.sky_light[LIGHT_SECTIONS - 1] = LightArray::filled(15);
        self.lit = false;
        self.dirty = true;
    }

    /// Recompute all section block counts.
    pub fn recount(&mut self) { for s in &mut self.sections { s.recount(); } }

    /// Biome id at local x/z, world y (clamped to the build range).
    pub fn biome(&self, lx: usize, y: i32, lz: usize) -> u32 {
        let y = y.clamp(MIN_Y, MIN_Y + HEIGHT - 1);
        let ry = (y - MIN_Y) as usize;
        self.sections[ry >> 4].biomes.get(((ry & 15) >> 2) << 4 | ((lz & 15) >> 2) << 2 | ((lx & 15) >> 2))
    }

    pub fn set_biome(&mut self, lx: usize, y: i32, lz: usize, biome: u32) {
        if !Self::in_range(y) { return; }
        let ry = (y - MIN_Y) as usize;
        self.sections[ry >> 4].biomes.set(((ry & 15) >> 2) << 4 | ((lz & 15) >> 2) << 2 | ((lx & 15) >> 2), biome);
        self.dirty = true;
    }

    /// Light-array index and offset for world y (`None` outside the light range).
    #[inline]
    pub(crate) fn light_index(lx: usize, y: i32, lz: usize) -> Option<(usize, usize)> {
        let ly = y - MIN_Y + 16;
        if ly < 0 || ly >= (LIGHT_SECTIONS as i32) * 16 { return None; }
        let ly = ly as usize;
        Some((ly >> 4, (ly & 15) << 8 | (lz & 15) << 4 | (lx & 15)))
    }

    /// Sky light at local x/z, world y (15 above the light range, 0 below).
    pub fn sky_light(&self, lx: usize, y: i32, lz: usize) -> u8 {
        match Self::light_index(lx, y, lz) {
            Some((s, i)) => self.sky_light[s].get(i),
            None => if y >= MIN_Y { 15 } else { 0 },
        }
    }
    pub fn block_light(&self, lx: usize, y: i32, lz: usize) -> u8 {
        match Self::light_index(lx, y, lz) { Some((s, i)) => self.block_light[s].get(i), None => 0 }
    }

    /// Block entities converted to the network form: (packed xz, y, type id, nbt).
    pub fn network_block_entities(&self) -> Vec<(u8, i16, i32, Tag)> {
        self.block_entities.iter().filter_map(|be| {
            let id = be.get("id")?.as_str()?;
            let ty = basalt_data::registry_id(basalt_data::registries::BLOCK_ENTITY_TYPE, id)?;
            let x = be.get("x")?.as_i64()? as i32;
            let y = be.get("y")?.as_i64()? as i32;
            let z = be.get("z")?.as_i64()? as i32;
            let mut c = be.as_compound()?.clone();
            for k in ["id", "x", "y", "z", "keepPacked"] { c.remove(k); }
            // containers sync nothing (measured: vanilla's chunk packet carries no NBT for chests, furnaces, barrels)
            if matches!(id.trim_start_matches("minecraft:"), "chest" | "trapped_chest" | "barrel" | "furnace" | "blast_furnace" | "smoker") { c.clear(); }
            Some((((x & 15) << 4 | (z & 15)) as u8, y as i16, ty, Tag::Compound(c)))
        }).collect()
    }

    /// Body of clientbound level_chunk_with_light after the two i32 coords:
    /// heightmaps, section data, block entities, light.
    pub fn write_network(&self, w: &mut Vec<u8>) {
        // Heightmaps: vanilla sends MOTION_BLOCKING_NO_LEAVES(5), MOTION_BLOCKING(4), WORLD_SURFACE(1).
        let kinds = [HeightmapKind::MotionBlockingNoLeaves, HeightmapKind::MotionBlocking, HeightmapKind::WorldSurface];
        write_varint(w, kinds.len() as i32);
        for k in kinds {
            write_varint(w, k as i32);
            let longs = self.heightmaps.packed(k);
            write_varint(w, longs.len() as i32);
            for l in longs { w.extend_from_slice(&l.to_be_bytes()); }
        }
        let mut data = Vec::with_capacity(4096);
        for s in &self.sections {
            data.extend_from_slice(&(s.non_air as i16).to_be_bytes());
            data.extend_from_slice(&(s.fluids as i16).to_be_bytes());
            s.blocks.write_network(&mut data);
            s.biomes.write_network(&mut data);
        }
        write_varint(w, data.len() as i32);
        w.extend_from_slice(&data);
        let bes = self.network_block_entities();
        write_varint(w, bes.len() as i32);
        for (xz, y, ty, nbt) in bes {
            w.push(xz);
            w.extend_from_slice(&y.to_be_bytes());
            write_varint(w, ty);
            // vanilla sends containers/furnaces with no NBT at all (a single TAG_End byte), not an empty compound
            if nbt.as_compound().map(|c| c.is_empty()).unwrap_or(false) { w.push(0); } else { basalt_nbt::write_network(w, &nbt); }
        }
        self.write_light(w);
    }

    /// Body of clientbound light_update after the two varint coords (also the light tail
    /// of level_chunk_with_light): sky mask, block mask, empty sky mask, empty block mask,
    /// sky arrays, block arrays. Sections that are all zero go in the empty masks;
    /// everything else (including fully lit sky = 0xFF bytes) is sent as an array.
    pub fn write_light(&self, w: &mut Vec<u8>) {
        let masks = |arrs: &[LightArray]| {
            let (mut has, mut empty) = (0u64, 0u64);
            for (i, a) in arrs.iter().enumerate() {
                if a.uniform() == Some(0) { empty |= 1 << i; } else { has |= 1 << i; }
            }
            (has, empty)
        };
        let (sky, sky_empty) = masks(&self.sky_light);
        let (blk, blk_empty) = masks(&self.block_light);
        for m in [sky, blk, sky_empty, blk_empty] { write_bitset(w, m); }
        for arrs in [&self.sky_light, &self.block_light] {
            let n = arrs.iter().filter(|a| a.uniform() != Some(0)).count();
            write_varint(w, n as i32);
            for a in arrs.iter().filter(|a| a.uniform() != Some(0)) {
                write_varint(w, 2048);
                w.extend_from_slice(&a.to_bytes());
            }
        }
    }
}

fn write_bitset(w: &mut Vec<u8>, m: u64) {
    if m == 0 { write_varint(w, 0); } else { write_varint(w, 1); w.extend_from_slice(&m.to_be_bytes()); }
}

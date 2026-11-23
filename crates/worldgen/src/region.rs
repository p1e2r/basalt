//! A mutable window of several generated chunks used for feature decoration, with
//! heightmaps kept up to date and block-property flags.

use crate::density::Eval;
use crate::{Generator, HEIGHT, MIN_Y};
use basalt_data::BlockState;
use std::collections::HashMap;
use std::sync::OnceLock;

pub const F_AIR: u8 = 1;
/// Water or lava (source or flowing), or waterlogged.
pub const F_FLUID: u8 = 2;
/// Blocks motion (has a sizeable collision shape).
pub const F_MOTION: u8 = 4;
pub const F_LEAVES: u8 = 8;
pub const F_WATER: u8 = 16;
pub const F_LAVA: u8 = 32;
/// Full, sturdy cube (top face full).
pub const F_FULL: u8 = 64;
/// Plants and other blocks that features may overwrite freely ("replaceable" plants).
pub const F_PLANT: u8 = 128;

/// Per-state flags derived from block kinds in the data reports.
pub fn flags() -> &'static [u8] {
    static T: OnceLock<Vec<u8>> = OnceLock::new();
    T.get_or_init(|| {
        (0..basalt_data::blocks::STATE_COUNT as u32).map(|i| {
            let s = BlockState(i);
            let b = s.block();
            let kind = b.kind;
            let name = b.name;
            let mut f = 0;
            if s.is_air() { return F_AIR; }
            let waterlogged = s.get("waterlogged") == Some("true");
            if name == "water" || name == "bubble_column" || waterlogged || matches!(kind, "seagrass" | "tall_seagrass" | "kelp" | "kelp_plant") { f |= F_FLUID | F_WATER; }
            if name == "lava" { f |= F_FLUID | F_LAVA; }
            let leaves = kind.ends_with("leaves");
            if leaves { f |= F_LEAVES; }
            let no_collision = matches!(kind,
                "liquid" | "bubble_column" | "flower" | "tall_flower" | "bush" | "tall_grass" | "double_plant" | "sapling"
                | "mushroom" | "seagrass" | "tall_seagrass" | "kelp" | "kelp_plant" | "vine" | "glow_lichen" | "multiface" | "sugar_cane"
                | "snow_layer" | "cave_vines" | "cave_vines_plant" | "hanging_roots" | "spore_blossom" | "small_dripleaf" | "flower_bed"
                | "leaf_litter" | "dry_vegetation" | "short_dry_grass" | "tall_dry_grass" | "firefly_bush" | "sweet_berry_bush" | "lily_pad"
                | "coral_plant" | "coral_fan" | "coral_wall_fan" | "base_coral_plant" | "base_coral_fan" | "base_coral_wall_fan" | "sea_pickle"
                | "torch" | "wall_torch" | "redstone_torch" | "redstone_wall_torch" | "button" | "carpet" | "wool_carpet" | "mossy_carpet"
                | "pressure_plate" | "weighted_pressure_plate" | "rail" | "powered_rail" | "detector_rail" | "redstone_wire" | "crop"
                | "carrot" | "potato" | "beetroot" | "stem" | "attached_stem" | "fire" | "soul_fire" | "web" | "ladder" | "standing_sign"
                | "wall_sign" | "banner" | "wall_banner" | "lever" | "tripwire" | "trip_wire_hook" | "nether_sprouts" | "nether_roots"
                | "nether_fungus" | "weeping_vines" | "weeping_vines_plant" | "twisting_vines" | "twisting_vines_plant" | "hanging_moss"
                | "sculk_vein" | "frogspawn" | "eyeblossom" | "cactus_flower" | "bamboo_sapling" | "pitcher_crop" | "torchflower_crop"
                | "structure_void" | "light" | "end_portal" | "nether_portal" | "air" | "sulfur_spike" | "pointed_dripstone"
                | "amethyst_cluster" | "cocoa" | "big_dripleaf_stem" | "nether_wart");
            if !no_collision { f |= F_MOTION; }
            let not_full = no_collision || matches!(kind, "slab" | "stair" | "fence" | "fence_gate" | "wall" | "iron_bars" | "stained_glass_pane"
                | "door" | "trapdoor" | "bed" | "chest" | "trapped_chest" | "ender_chest" | "cactus" | "big_dripleaf" | "lantern" | "chain"
                | "end_rod" | "lightning_rod" | "bell" | "anvil" | "cake" | "candle" | "skull" | "wall_skull" | "flower_pot" | "azalea"
                | "dirt_path" | "farmland" | "honey" | "soul_sand" | "mud" | "scaffolding" | "turtle_egg" | "sniffer_egg" | "decorated_pot"
                | "brewing_stand" | "cauldron" | "layered_cauldron" | "lava_cauldron" | "hopper" | "conduit" | "dragon_egg" | "grindstone"
                | "lectern" | "stonecutter" | "enchantment_table" | "daylight_detector" | "campfire" | "composter" | "powder_snow"
                | "mangrove_propagule" | "creaking_heart" | "piston_head" | "moving_piston");
            if !not_full && !leaves { f |= F_FULL; }
            let plant = matches!(kind, "flower" | "tall_flower" | "bush" | "tall_grass" | "double_plant" | "dry_vegetation"
                | "short_dry_grass" | "tall_dry_grass" | "firefly_bush" | "seagrass" | "tall_seagrass" | "vine" | "glow_lichen" | "multiface"
                | "snow_layer" | "leaf_litter" | "flower_bed" | "nether_sprouts" | "nether_roots" | "hanging_roots" | "fire");
            if plant { f |= F_PLANT; }
            f
        }).collect()
    })
}

#[inline]
pub fn flag(state: u16, f: u8) -> bool { flags()[state as usize] & f != 0 }

/// Heightmap kinds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hm {
    WorldSurface = 0,
    OceanFloor = 1,
    MotionBlocking = 2,
    MotionBlockingNoLeaves = 3,
    /// The worldgen variants are not updated during decoration (vanilla keeps them at
    /// their carver-stage values), only the final ones are.
    WorldSurfaceWg = 4,
    OceanFloorWg = 5,
}

const HM_ALL: [Hm; 6] = [Hm::WorldSurface, Hm::OceanFloor, Hm::MotionBlocking, Hm::MotionBlockingNoLeaves, Hm::WorldSurfaceWg, Hm::OceanFloorWg];

impl Hm {
    pub fn parse(s: &str) -> Option<Hm> {
        Some(match s {
            "WORLD_SURFACE" => Hm::WorldSurface,
            "OCEAN_FLOOR" => Hm::OceanFloor,
            "WORLD_SURFACE_WG" => Hm::WorldSurfaceWg,
            "OCEAN_FLOOR_WG" => Hm::OceanFloorWg,
            "MOTION_BLOCKING" => Hm::MotionBlocking,
            "MOTION_BLOCKING_NO_LEAVES" => Hm::MotionBlockingNoLeaves,
            _ => return None,
        })
    }
    #[inline]
    fn counts(self, s: u16) -> bool {
        let f = flags()[s as usize];
        match self {
            Hm::WorldSurface | Hm::WorldSurfaceWg => f & F_AIR == 0,
            Hm::OceanFloor | Hm::OceanFloorWg => f & F_MOTION != 0,
            Hm::MotionBlocking => f & (F_MOTION | F_FLUID) != 0,
            Hm::MotionBlockingNoLeaves => f & (F_MOTION | F_FLUID) != 0 && f & F_LEAVES == 0,
        }
    }
}

/// Chunks [cx0, cx0+n) × [cz0, cz0+n), fully generated up to the carver stage.
pub struct Region<'g> {
    pub g: &'g Generator,
    pub e: Eval<'g>,
    pub cx0: i32,
    pub cz0: i32,
    pub n: i32,
    pub blocks: Vec<Vec<[u16; 4096]>>,
    pub biomes: Vec<Vec<[u8; 64]>>,
    heights: Vec<[[i32; 256]; 6]>,
    /// Inclusive block bounds that may be written (min x, max x, min z, max z).
    pub write: (i32, i32, i32, i32),
    prelim: HashMap<(i32, i32), i32>,
    biome_cache: HashMap<(i32, i32, i32), u8>,
    last_leaf: Option<u32>,
}

impl<'g> Region<'g> {
    pub fn new(g: &'g Generator, cx0: i32, cz0: i32, n: i32, blocks: Vec<Vec<[u16; 4096]>>, biomes: Vec<Vec<[u8; 64]>>) -> Self {
        let mut r = Region {
            g, e: Eval::new(&g.graph), cx0, cz0, n, blocks, biomes, heights: Vec::new(), write: (0, -1, 0, -1),
            prelim: HashMap::new(), biome_cache: HashMap::new(), last_leaf: None,
        };
        for c in 0..(n * n) as usize {
            let mut hm = [[MIN_Y; 256]; 6];
            for (t, kind) in HM_ALL.iter().enumerate() {
                for col in 0..256 {
                    hm[t][col] = scan(&r.blocks[c], col as i32 & 15, col as i32 >> 4, MIN_Y + HEIGHT - 1, *kind);
                }
            }
            r.heights.push(hm);
        }
        r
    }

    #[inline]
    fn chunk_of(&self, x: i32, z: i32) -> Option<usize> {
        let (cx, cz) = ((x >> 4) - self.cx0, (z >> 4) - self.cz0);
        if cx < 0 || cz < 0 || cx >= self.n || cz >= self.n { None } else { Some((cz * self.n + cx) as usize) }
    }

    /// Block at a position; None outside the region or build height.
    #[inline]
    pub fn get(&self, x: i32, y: i32, z: i32) -> Option<u16> {
        if !(MIN_Y..MIN_Y + HEIGHT).contains(&y) { return None; }
        let c = self.chunk_of(x, z)?;
        let yy = y - MIN_Y;
        Some(self.blocks[c][(yy >> 4) as usize][(((yy & 15) << 8) | ((z & 15) << 4) | (x & 15)) as usize])
    }

    /// Block or air outside.
    #[inline]
    pub fn block(&self, x: i32, y: i32, z: i32) -> u16 { self.get(x, y, z).unwrap_or(0) }

    pub fn can_write(&self, x: i32, y: i32, z: i32) -> bool {
        let w = self.write;
        (MIN_Y..MIN_Y + HEIGHT).contains(&y) && x >= w.0 && x <= w.1 && z >= w.2 && z <= w.3
    }

    /// Set a block (ignored outside the writable area). Returns whether it was written.
    pub fn set(&mut self, x: i32, y: i32, z: i32, s: u16) -> bool {
        if !self.can_write(x, y, z) { return false; }
        let Some(c) = self.chunk_of(x, z) else { return false };
        let yy = y - MIN_Y;
        self.blocks[c][(yy >> 4) as usize][(((yy & 15) << 8) | ((z & 15) << 4) | (x & 15)) as usize] = s;
        let col = (((z & 15) << 4) | (x & 15)) as usize;
        for (t, kind) in HM_ALL.iter().enumerate().take(4) {
            let h = self.heights[c][t][col];
            if kind.counts(s) {
                if y >= h { self.heights[c][t][col] = y + 1; }
            } else if y == h - 1 {
                self.heights[c][t][col] = scan(&self.blocks[c], x & 15, z & 15, y - 1, *kind);
            }
        }
        true
    }

    /// Heightmap value (one above the top counted block); estimated outside the region.
    pub fn height(&mut self, kind: Hm, x: i32, z: i32) -> i32 {
        match self.chunk_of(x, z) {
            Some(c) => self.heights[c][kind as usize][(((z & 15) << 4) | (x & 15)) as usize],
            None => self.preliminary(x, z),
        }
    }

    pub fn preliminary(&mut self, x: i32, z: i32) -> i32 {
        let k = (x & !3, z & !3);
        if let Some(v) = self.prelim.get(&k) { return *v; }
        self.e.set_mode(crate::density::Mode::Direct);
        let v = self.e.eval(self.g.router.aq.prelim, k.0, 0, k.1).floor() as i32;
        self.prelim.insert(k, v);
        v
    }

    /// Noise biome at a quart position.
    pub fn noise_biome(&mut self, qx: i32, qy: i32, qz: i32) -> u8 {
        let qyy = qy - MIN_Y / 4;
        if (0..HEIGHT / 4).contains(&qyy) {
            if let Some(c) = self.chunk_of(qx * 4, qz * 4) {
                return self.biomes[c][(qyy >> 2) as usize][(((qyy & 3) << 4) | ((qz & 3) << 2) | (qx & 3)) as usize];
            }
        }
        if let Some(b) = self.biome_cache.get(&(qx, qy, qz)) { return *b; }
        self.e.set_mode(crate::density::Mode::Direct);
        let t = self.g.climate(&mut self.e, qx * 4, qy * 4, qz * 4);
        let (b, leaf) = self.g.biomes.search(&t, self.last_leaf);
        self.last_leaf = Some(leaf);
        self.biome_cache.insert((qx, qy, qz), b);
        b
    }

    /// Biome at a block position (with the vanilla zoom).
    pub fn biome(&mut self, x: i32, y: i32, z: i32) -> u8 {
        let g = self.g;
        g.surface.zoomed_biome(self, x, y, z)
    }

    /// Biomes present in a chunk of the region.
    pub fn chunk_biomes(&self, cx: i32, cz: i32, out: &mut [bool]) {
        if let Some(c) = self.chunk_of(cx * 16, cz * 16) {
            for sec in &self.biomes[c] { for b in sec { out[*b as usize] = true; } }
        }
    }

    /// Take a chunk's blocks out of the region.
    pub fn take_chunk(&mut self, cx: i32, cz: i32) -> Vec<[u16; 4096]> {
        let c = self.chunk_of(cx * 16, cz * 16).expect("chunk in region");
        std::mem::take(&mut self.blocks[c])
    }
}

impl crate::surface::SurfaceEnv for Region<'_> {
    fn noise_biome(&mut self, qx: i32, qy: i32, qz: i32) -> u8 { Region::noise_biome(self, qx, qy, qz) }
    fn preliminary_surface(&mut self, x: i32, z: i32) -> i32 { self.preliminary(x, z) }
}

fn scan(b: &[[u16; 4096]], lx: i32, lz: i32, from: i32, kind: Hm) -> i32 {
    let mut y = from;
    while y >= MIN_Y {
        let yy = y - MIN_Y;
        if kind.counts(b[(yy >> 4) as usize][(((yy & 15) << 8) | (lz << 4) | lx) as usize]) { return y + 1; }
        y -= 1;
    }
    MIN_Y
}

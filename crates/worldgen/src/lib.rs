//! Overworld world generation for Basalt, driven entirely by the worldgen JSON that the
//! official data generator emits (noise settings, density functions, noises, biomes, ...).
//!
//! ```ignore
//! let g = basalt_worldgen::Generator::new(12345);
//! let chunk = g.generate_chunk(0, 0);
//! ```

pub mod aquifer;
pub mod biome;
pub mod density;
pub mod noise;
pub mod random;

use aquifer::{Aquifer, AquiferNodes, Fluid};
use anyhow::{anyhow, Context, Result};
use basalt_data::BlockState;
use density::{Eval, Graph, GraphBuilder, Mode, Node, NodeId};
use random::{PosFactory, Rng, Xoroshiro};
use serde_json::Value;
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
        Ok(Generator {
            seed, biome_names, biomes,
            aquifer_rand: rand.from_hash("minecraft:aquifer").fork_positional(),
            ore_rand: rand.from_hash("minecraft:ore").fork_positional(),
            sea_level: settings["sea_level"].as_i64().unwrap_or(63) as i32,
            aquifers_enabled: settings["aquifers_enabled"].as_bool().unwrap_or(true),
            ore_veins_enabled: settings["ore_veins_enabled"].as_bool().unwrap_or(true),
            default_block: state_from_json(&settings["default_block"])?,
            default_fluid: state_from_json(&settings["default_fluid"])?,
            graph, router, blocks, interp_args,
        })
    }

    /// Generate the terrain of one chunk.
    pub fn generate_chunk(&self, cx: i32, cz: i32) -> GeneratedChunk {
        let mut e = Eval::new(&self.graph);
        let blocks = self.fill_noise(&mut e, cx, cz);
        let biomes = self.fill_biomes(&mut e, cx, cz);
        GeneratedChunk { blocks, biomes }
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
    pub fn fill_noise(&self, e: &mut Eval, cx: i32, cz: i32) -> Vec<[u16; 4096]> {
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
        let mut aq = Aquifer::new(self.router.aq, self.aquifer_rand, self.sea_level, self.aquifers_enabled);
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

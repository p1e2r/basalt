//! Surface rules (noise_settings.surface_rule): replaces the default block near the
//! surface with biome-dependent blocks, adds bedrock and deepslate.

use crate::density::load_noise_params;
use crate::noise::{lerp2, NormalNoise, PerlinSimplexNoise};
use crate::random::{lcg_next, JavaRandom, PosFactory, Rng};
use crate::state_from_json;
use anyhow::{anyhow, bail, Result};
use basalt_data::BlockState;
use serde_json::Value;
use std::path::Path;

const WAY_BELOW_MIN_Y: i32 = -2032 << 4;

#[derive(Debug, Clone, Copy)]
pub enum Anchor {
    Absolute(i32),
    AboveBottom(i32),
    BelowTop(i32),
}

impl Anchor {
    fn parse(v: &Value) -> Result<Anchor> {
        if let Some(a) = v.get("absolute").and_then(|x| x.as_i64()) { return Ok(Anchor::Absolute(a as i32)); }
        if let Some(a) = v.get("above_bottom").and_then(|x| x.as_i64()) { return Ok(Anchor::AboveBottom(a as i32)); }
        if let Some(a) = v.get("below_top").and_then(|x| x.as_i64()) { return Ok(Anchor::BelowTop(a as i32)); }
        bail!("bad vertical anchor {v}")
    }
    fn resolve(&self) -> i32 {
        match *self {
            Anchor::Absolute(y) => y,
            Anchor::AboveBottom(n) => crate::MIN_Y + n,
            Anchor::BelowTop(n) => crate::MIN_Y + crate::HEIGHT - 1 - n,
        }
    }
}

#[derive(Debug)]
pub enum Cond {
    Biome(Vec<bool>),
    Noise { noise: usize, min: f64, max: f64, is_3d: bool },
    VerticalGradient { rand: PosFactory, true_at: Anchor, false_at: Anchor },
    YAbove { anchor: Anchor, mult: i32, add_stone: bool },
    Water { offset: i32, mult: i32, add_stone: bool },
    Temperature,
    Steep,
    Not(Box<Cond>),
    Hole,
    AbovePreliminarySurface,
    StoneDepth { offset: i32, add_surface: bool, secondary_range: i32, ceiling: bool },
}

#[derive(Debug)]
pub enum Rule {
    Block(u16),
    Sequence(Vec<Rule>),
    Condition(Cond, Box<Rule>),
    Bandlands,
}

/// Access to generator state the rules need.
pub trait SurfaceEnv {
    /// Biome at a quart position.
    fn noise_biome(&mut self, qx: i32, qy: i32, qz: i32) -> u8;
    /// Preliminary surface level of the column containing (x, z).
    fn preliminary_surface(&mut self, x: i32, z: i32) -> i32;
}

pub struct BiomeClimate {
    pub temperature: f32,
    pub frozen: bool,
}

pub struct SurfaceSystem {
    rule: Rule,
    noises: Vec<NormalNoise>,
    noise_names: Vec<String>,
    surface: usize,
    secondary: usize,
    band_offset: usize,
    root: PosFactory,
    bands: Vec<u16>,
    climate: Vec<BiomeClimate>,
    temperature_noise: PerlinSimplexNoise,
    frozen_noise: PerlinSimplexNoise,
    info_noise: PerlinSimplexNoise,
    default_block: u16,
    zoom_seed: i64,
    sea_level: i32,
    biome_names: Vec<String>,
    wg_root: std::path::PathBuf,
    pub ids: Ids,
}

#[derive(Clone, Copy)]
pub struct Ids {
    pub air: u16,
    pub water: u16,
    pub packed_ice: u16,
    pub snow_block: u16,
    pub ice: u16,
    pub frozen_ocean: u8,
    pub deep_frozen_ocean: u8,
    pub eroded_badlands: u8,
}

/// Per-column state.
struct Ctx<'a> {
    x: i32,
    y: i32,
    z: i32,
    surface_depth: i32,
    secondary: Option<f64>,
    min_surface: Option<i32>,
    stone_above: i32,
    stone_below: i32,
    water_height: i32,
    biome: Option<u8>,
    steep: Option<bool>,
    heights: &'a [i32; 256],
    corners: [i32; 4],
}

impl SurfaceSystem {
    pub fn load(wg: &Path, settings: &Value, root: PosFactory, biome_names: &[String], default_block: u16, sea_level: i32, zoom_seed: i64) -> Result<SurfaceSystem> {
        let mut climate = Vec::new();
        for n in biome_names {
            let p = wg.join("biome").join(format!("{n}.json"));
            let v: Value = serde_json::from_str(&std::fs::read_to_string(&p)?)?;
            climate.push(BiomeClimate {
                temperature: v["temperature"].as_f64().unwrap_or(0.5) as f32,
                frozen: v.get("temperature_modifier").and_then(|m| m.as_str()).map(|m| m.ends_with("frozen")).unwrap_or(false),
            });
        }
        let bid = |n: &str| biome_names.iter().position(|b| b == n).unwrap_or(usize::MAX) as u8;
        let st = |n: &str| BlockState::parse(n).map(|s| s.0 as u16).ok_or_else(|| anyhow!("unknown block {n}"));
        let ids = Ids {
            air: st("air")?, water: st("water")?, packed_ice: st("packed_ice")?, snow_block: st("snow_block")?, ice: st("ice")?,
            frozen_ocean: bid("frozen_ocean"), deep_frozen_ocean: bid("deep_frozen_ocean"), eroded_badlands: bid("eroded_badlands"),
        };
        let mut s = SurfaceSystem {
            rule: Rule::Sequence(Vec::new()),
            noises: Vec::new(),
            noise_names: Vec::new(),
            surface: 0,
            secondary: 0,
            band_offset: 0,
            root,
            bands: Vec::new(),
            climate,
            temperature_noise: PerlinSimplexNoise::new(&mut JavaRandom::new(1234), &[0]),
            frozen_noise: PerlinSimplexNoise::new(&mut JavaRandom::new(3456), &[-2, -1, 0]),
            info_noise: PerlinSimplexNoise::new(&mut JavaRandom::new(2345), &[0]),
            default_block,
            zoom_seed,
            sea_level,
            biome_names: biome_names.to_vec(),
            wg_root: wg.to_path_buf(),
            ids,
        };
        s.surface = s.noise("minecraft:surface")?;
        s.secondary = s.noise("minecraft:surface_secondary")?;
        s.band_offset = s.noise("minecraft:clay_bands_offset")?;
        s.bands = make_bands(&mut root.from_hash("minecraft:clay_bands"))?;
        s.rule = s.parse_rule(&settings["surface_rule"])?;
        Ok(s)
    }

    fn noise(&mut self, name: &str) -> Result<usize> {
        let name = if name.contains(':') { name.to_string() } else { format!("minecraft:{name}") };
        if let Some(i) = self.noise_names.iter().position(|n| *n == name) { return Ok(i); }
        let p = load_noise_params(&self.wg_root, &name)?;
        self.noises.push(NormalNoise::new(&mut self.root.from_hash(&name), &p));
        self.noise_names.push(name);
        Ok(self.noises.len() - 1)
    }

    fn parse_rule(&mut self, v: &Value) -> Result<Rule> {
        let ty = v["type"].as_str().unwrap_or("").trim_start_matches("minecraft:");
        Ok(match ty {
            "block" => Rule::Block(state_from_json(&v["result_state"])?),
            "sequence" => Rule::Sequence(v["sequence"].as_array().ok_or_else(|| anyhow!("sequence"))?.iter().map(|r| self.parse_rule(r)).collect::<Result<_>>()?),
            "condition" => Rule::Condition(self.parse_cond(&v["if_true"])?, Box::new(self.parse_rule(&v["then_run"])?)),
            "bandlands" => Rule::Bandlands,
            t => bail!("unsupported surface rule {t}"),
        })
    }

    fn parse_cond(&mut self, v: &Value) -> Result<Cond> {
        let ty = v["type"].as_str().unwrap_or("").trim_start_matches("minecraft:");
        let int = |k: &str| v.get(k).and_then(|x| x.as_i64()).unwrap_or(0) as i32;
        let boolean = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
        Ok(match ty {
            "biome" => {
                let mut set = vec![false; self.biome_names.len()];
                let list: Vec<&str> = match &v["biome_is"] {
                    Value::String(s) => vec![s.as_str()],
                    Value::Array(a) => a.iter().filter_map(|x| x.as_str()).collect(),
                    _ => bail!("biome_is"),
                };
                for b in list {
                    let n = b.trim_start_matches("minecraft:");
                    if let Some(i) = self.biome_names.iter().position(|x| x == n) { set[i] = true; }
                }
                Cond::Biome(set)
            }
            "noise_threshold" => Cond::Noise {
                noise: self.noise(v["noise"].as_str().ok_or_else(|| anyhow!("noise"))?)?,
                min: v["min_threshold"].as_f64().unwrap_or(f64::MIN),
                max: v["max_threshold"].as_f64().unwrap_or(f64::MAX),
                is_3d: boolean("is_3d"),
            },
            "vertical_gradient" => {
                let name = v["random_name"].as_str().ok_or_else(|| anyhow!("random_name"))?;
                let name = if name.contains(':') { name.to_string() } else { format!("minecraft:{name}") };
                Cond::VerticalGradient {
                    rand: self.root.from_hash(&name).fork_positional(),
                    true_at: Anchor::parse(&v["true_at_and_below"])?,
                    false_at: Anchor::parse(&v["false_at_and_above"])?,
                }
            }
            "y_above" => Cond::YAbove { anchor: Anchor::parse(&v["anchor"])?, mult: int("surface_depth_multiplier"), add_stone: boolean("add_stone_depth") },
            "water" => Cond::Water { offset: int("offset"), mult: int("surface_depth_multiplier"), add_stone: boolean("add_stone_depth") },
            "temperature" => Cond::Temperature,
            "steep" => Cond::Steep,
            "not" => Cond::Not(Box::new(self.parse_cond(&v["invert"])?)),
            "hole" => Cond::Hole,
            "above_preliminary_surface" => Cond::AbovePreliminarySurface,
            "stone_depth" => Cond::StoneDepth {
                offset: int("offset"), add_surface: boolean("add_surface_depth"), secondary_range: int("secondary_depth_range"),
                ceiling: v["surface_type"].as_str() == Some("ceiling"),
            },
            t => bail!("unsupported surface condition {t}"),
        })
    }

    /// Biome at a block position with the vanilla "fuzzy" zoom from quart cells.
    pub fn zoomed_biome(&self, env: &mut dyn SurfaceEnv, x: i32, y: i32, z: i32) -> u8 {
        let i = x - 2;
        let j = y - 2;
        let k = z - 2;
        let (l, m, n) = (i >> 2, j >> 2, k >> 2);
        let d = (i & 3) as f64 / 4.0;
        let e = (j & 3) as f64 / 4.0;
        let f = (k & 3) as f64 / 4.0;
        let mut best = 0;
        let mut g = f64::INFINITY;
        for p in 0..8 {
            let bx = p & 4 == 0;
            let by = p & 2 == 0;
            let bz = p & 1 == 0;
            let q = if bx { l } else { l + 1 };
            let r = if by { m } else { m + 1 };
            let s = if bz { n } else { n + 1 };
            let h = if bx { d } else { d - 1.0 };
            let t = if by { e } else { e - 1.0 };
            let u = if bz { f } else { f - 1.0 };
            let v = fiddled_distance(self.zoom_seed, q, r, s, h, t, u);
            if g > v { best = p; g = v; }
        }
        let q = if best & 4 == 0 { l } else { l + 1 };
        let r = if best & 2 == 0 { m } else { m + 1 };
        let s = if best & 1 == 0 { n } else { n + 1 };
        env.noise_biome(q, r, s)
    }

    fn surface_depth(&self, x: i32, z: i32) -> i32 {
        let d = self.noises[self.surface].get(x as f64, 0.0, z as f64);
        (d * 2.75 + 3.0 + self.root.at(x, 0, z).next_double() * 0.25) as i32
    }

    fn band(&self, x: i32, y: i32, z: i32) -> u16 {
        let i = (self.noises[self.band_offset].get(x as f64, 0.0, z as f64) * 4.0 + 0.5).floor() as i32;
        let n = self.bands.len() as i32;
        self.bands[(y + i + n).rem_euclid(n) as usize]
    }

    fn cold_enough_to_snow(&self, biome: u8, x: i32, y: i32, z: i32) -> bool {
        let c = &self.climate[biome as usize];
        let mut t = c.temperature;
        if c.frozen {
            let d = self.frozen_noise.get(x as f64 * 0.05, z as f64 * 0.05, false) * 7.0;
            let e = self.info_noise.get(x as f64 * 0.2, z as f64 * 0.2, false);
            if d + e < 0.3 {
                let g = self.info_noise.get(x as f64 * 0.09, z as f64 * 0.09, false);
                if g < 0.8 { t = 0.2; }
            }
        }
        let i = self.sea_level + 17;
        if y > i {
            let g = (self.temperature_noise.get((x as f32 / 8.0) as f64, (z as f32 / 8.0) as f64, false) * 8.0) as f32;
            t -= (g + y as f32 - i as f32) * 0.05 / 40.0;
        }
        t < 0.15
    }

    fn test(&self, c: &Cond, ctx: &mut Ctx, env: &mut dyn SurfaceEnv) -> bool {
        match c {
            Cond::Biome(set) => {
                let b = match ctx.biome { Some(b) => b, None => { let b = self.zoomed_biome(env, ctx.x, ctx.y, ctx.z); ctx.biome = Some(b); b } };
                set[b as usize]
            }
            Cond::Noise { noise, min, max, is_3d } => {
                let y = if *is_3d { ctx.y as f64 } else { 0.0 };
                let v = self.noises[*noise].get(ctx.x as f64, y, ctx.z as f64);
                v >= *min && v <= *max
            }
            Cond::VerticalGradient { rand, true_at, false_at } => {
                let i = true_at.resolve();
                let j = false_at.resolve();
                let k = ctx.y;
                if k <= i { return true; }
                if k >= j { return false; }
                let d = crate::aquifer::map(k as f64, i as f64, j as f64, 1.0, 0.0);
                (rand.at(ctx.x, k, ctx.z).next_float() as f64) < d
            }
            Cond::YAbove { anchor, mult, add_stone } => {
                ctx.y + if *add_stone { ctx.stone_above } else { 0 } >= anchor.resolve() + ctx.surface_depth * mult
            }
            Cond::Water { offset, mult, add_stone } => {
                ctx.water_height == i32::MIN
                    || ctx.y + if *add_stone { ctx.stone_above } else { 0 } >= ctx.water_height + offset + ctx.surface_depth * mult
            }
            Cond::Temperature => {
                let b = match ctx.biome { Some(b) => b, None => { let b = self.zoomed_biome(env, ctx.x, ctx.y, ctx.z); ctx.biome = Some(b); b } };
                self.cold_enough_to_snow(b, ctx.x, ctx.y, ctx.z)
            }
            Cond::Steep => {
                if let Some(s) = ctx.steep { return s; }
                let i = ctx.x & 15;
                let j = ctx.z & 15;
                let h = |x: i32, z: i32| ctx.heights[(z * 16 + x) as usize];
                let k = (j - 1).max(0);
                let l = (j + 1).min(15);
                let m = h(i, k);
                let n = h(i, l);
                let s = if n >= m + 4 { true } else {
                    let o = (i - 1).max(0);
                    let p = (i + 1).min(15);
                    h(o, j) >= h(p, j) + 4
                };
                ctx.steep = Some(s);
                s
            }
            Cond::Not(c) => !self.test(c, ctx, env),
            Cond::Hole => ctx.surface_depth <= 0,
            Cond::AbovePreliminarySurface => ctx.y >= self.min_surface_level(ctx),
            Cond::StoneDepth { offset, add_surface, secondary_range, ceiling } => {
                let i = if *ceiling { ctx.stone_below } else { ctx.stone_above };
                let j = if *add_surface { ctx.surface_depth } else { 0 };
                let k = if *secondary_range == 0 { 0 } else {
                    let s = match ctx.secondary { Some(s) => s, None => {
                        let s = self.noises[self.secondary].get(ctx.x as f64, 0.0, ctx.z as f64);
                        ctx.secondary = Some(s);
                        s
                    } };
                    crate::aquifer::map(s, -1.0, 1.0, 0.0, *secondary_range as f64) as i32
                };
                i <= 1 + offset + j + k
            }
        }
    }

    fn min_surface_level(&self, ctx: &mut Ctx) -> i32 {
        if let Some(m) = ctx.min_surface { return m; }
        let fx = ((ctx.x & 15) as f32 / 16.0) as f64;
        let fz = ((ctx.z & 15) as f32 / 16.0) as f64;
        let c = ctx.corners;
        let k = lerp2(fx, fz, c[0] as f64, c[1] as f64, c[2] as f64, c[3] as f64).floor() as i32;
        let m = k + ctx.surface_depth - 8;
        ctx.min_surface = Some(m);
        m
    }

    fn apply(&self, r: &Rule, ctx: &mut Ctx, env: &mut dyn SurfaceEnv) -> Option<u16> {
        match r {
            Rule::Block(b) => Some(*b),
            Rule::Sequence(rs) => rs.iter().find_map(|r| self.apply(r, ctx, env)),
            Rule::Condition(c, then) => if self.test(c, ctx, env) { self.apply(then, ctx, env) } else { None },
            Rule::Bandlands => Some(self.band(ctx.x, ctx.y, ctx.z)),
        }
    }

    /// Run the surface rules over a chunk filled by the noise stage.
    pub fn build(&self, env: &mut dyn SurfaceEnv, cx: i32, cz: i32, blocks: &mut [[u16; 4096]]) {
        let x0 = cx * 16;
        let z0 = cz * 16;
        let get = |b: &[[u16; 4096]], x: i32, y: i32, z: i32| -> u16 {
            let yy = y - crate::MIN_Y;
            if !(0..crate::HEIGHT).contains(&yy) { return self.ids.air; }
            b[(yy >> 4) as usize][(((yy & 15) << 8) | (z << 4) | x) as usize]
        };
        let is_air = |id: u16| BlockState(id as u32).is_air();
        let is_fluid = |id: u16| { let n = BlockState(id as u32).name(); n == "water" || n == "lava" };
        // WORLD_SURFACE_WG heightmap: one above the highest non-air block.
        let mut heights = [crate::MIN_Y; 256];
        for x in 0..16 {
            for z in 0..16 {
                let mut y = crate::MIN_Y + crate::HEIGHT - 1;
                while y >= crate::MIN_Y && is_air(get(blocks, x, y, z)) { y -= 1; }
                heights[(z * 16 + x) as usize] = y + 1;
            }
        }
        let corners = [
            env.preliminary_surface(x0, z0), env.preliminary_surface(x0 + 16, z0),
            env.preliminary_surface(x0, z0 + 16), env.preliminary_surface(x0 + 16, z0 + 16),
        ];
        for lx in 0..16 {
            for lz in 0..16 {
                let x = x0 + lx;
                let z = z0 + lz;
                let top = heights[(lz * 16 + lx) as usize] + 1;
                let mut ctx = Ctx {
                    x, y: 0, z, surface_depth: self.surface_depth(x, z), secondary: None, min_surface: None,
                    stone_above: 0, stone_below: 0, water_height: i32::MIN, biome: None, steep: None, heights: &heights, corners,
                };
                let mut q = 0;
                let mut r = i32::MIN;
                let mut s = i32::MAX;
                let bottom = crate::MIN_Y;
                let mut y = top;
                while y >= bottom {
                    let st = get(blocks, lx, y, lz);
                    if is_air(st) { q = 0; r = i32::MIN; y -= 1; continue; }
                    if is_fluid(st) { if r == i32::MIN { r = y + 1; } y -= 1; continue; }
                    if s >= y {
                        s = WAY_BELOW_MIN_Y;
                        let mut v = y - 1;
                        while v >= bottom - 1 {
                            let b2 = get(blocks, lx, v, lz);
                            if is_air(b2) || is_fluid(b2) { s = v + 1; break; }
                            v -= 1;
                        }
                    }
                    q += 1;
                    ctx.y = y;
                    ctx.stone_above = q;
                    ctx.stone_below = y - s + 1;
                    ctx.water_height = r;
                    ctx.biome = None;
                    if st == self.default_block {
                        if let Some(b) = self.apply(&self.rule, &mut ctx, env) {
                            let yy = y - crate::MIN_Y;
                            blocks[(yy >> 4) as usize][(((yy & 15) << 8) | (lz << 4) | lx) as usize] = b;
                        }
                    }
                    y -= 1;
                }
            }
        }
    }
}

fn fiddle(l: i64) -> f64 { ((l >> 24).rem_euclid(1024) as f64 / 1024.0 - 0.5) * 0.9 }

fn fiddled_distance(seed: i64, x: i32, y: i32, z: i32, dx: f64, dy: f64, dz: f64) -> f64 {
    let mut m = lcg_next(seed, x as i64);
    m = lcg_next(m, y as i64);
    m = lcg_next(m, z as i64);
    m = lcg_next(m, x as i64);
    m = lcg_next(m, y as i64);
    m = lcg_next(m, z as i64);
    let g = fiddle(m);
    m = lcg_next(m, seed);
    let h = fiddle(m);
    m = lcg_next(m, seed);
    let o = fiddle(m);
    (dz + o) * (dz + o) + (dy + h) * (dy + h) + (dx + g) * (dx + g)
}

fn make_bands<R: Rng>(r: &mut R) -> Result<Vec<u16>> {
    let st = |n: &str| BlockState::parse(n).map(|s| s.0 as u16).ok_or_else(|| anyhow!("unknown block {n}"));
    let terracotta = st("terracotta")?;
    let mut b = vec![terracotta; 192];
    let mut i = 0usize;
    while i < 192 {
        i += r.next_int_bounded(5) as usize + 1;
        if i < 192 { b[i] = st("orange_terracotta")?; }
        i += 1;
    }
    make_band(r, &mut b, 1, st("yellow_terracotta")?);
    make_band(r, &mut b, 2, st("brown_terracotta")?);
    make_band(r, &mut b, 1, st("red_terracotta")?);
    let j = r.next_int_between(9, 15);
    let mut k = 0;
    let mut l = 0usize;
    let white = st("white_terracotta")?;
    let light = st("light_gray_terracotta")?;
    while k < j && l < 192 {
        b[l] = white;
        if l > 1 && r.next_bool() { b[l - 1] = light; }
        if l + 1 < 192 && r.next_bool() { b[l + 1] = light; }
        k += 1;
        l += r.next_int_bounded(16) as usize + 4;
    }
    Ok(b)
}

fn make_band<R: Rng>(r: &mut R, b: &mut [u16], min_width: i32, state: u16) {
    let i = r.next_int_between(6, 15);
    for _ in 0..i {
        let k = min_width + r.next_int_bounded(3);
        let l = r.next_int_bounded(b.len() as i32) as usize;
        let mut m = 0;
        while l + m < b.len() && (m as i32) < k {
            b[l + m] = state;
            m += 1;
        }
    }
}

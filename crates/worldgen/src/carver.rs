//! Cave and canyon carvers (configured_carver JSON), seeded per start chunk with the
//! legacy Java LCG, carving through the noise aquifer.

use crate::random::{set_large_feature_seed, JavaRandom, Rng};
use anyhow::{anyhow, bail, Result};
use basalt_data::BlockState;
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;

/// Mth.sin/cos: 65536-entry float lookup table.
pub struct Trig {
    table: Vec<f32>,
}

impl Trig {
    pub fn new() -> Self {
        Self { table: (0..65536).map(|i| (i as f64 * std::f64::consts::PI * 2.0 / 65536.0).sin() as f32).collect() }
    }
    #[inline]
    pub fn sin(&self, d: f64) -> f32 { self.table[((d * 10430.378350470453) as i64 & 65535) as usize] }
    #[inline]
    pub fn cos(&self, d: f64) -> f32 { self.table[((d * 10430.378350470453 + 16384.0) as i64 & 65535) as usize] }
}

impl Default for Trig {
    fn default() -> Self { Self::new() }
}

#[derive(Clone, Copy, Debug)]
pub enum FloatProvider {
    Constant(f32),
    Uniform(f32, f32),
    Trapezoid { min: f32, max: f32, plateau: f32 },
}

impl FloatProvider {
    fn parse(v: &Value) -> Result<Self> {
        if let Some(f) = v.as_f64() { return Ok(FloatProvider::Constant(f as f32)); }
        let f = |k: &str| v[k].as_f64().map(|x| x as f32).ok_or_else(|| anyhow!("float provider {k}"));
        match v["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
            "constant" => Ok(FloatProvider::Constant(f("value")?)),
            "uniform" => Ok(FloatProvider::Uniform(f("min_inclusive")?, f("max_exclusive")?)),
            "trapezoid" => Ok(FloatProvider::Trapezoid { min: f("min")?, max: f("max")?, plateau: f("plateau")? }),
            t => bail!("unsupported float provider {t}"),
        }
    }
    pub fn sample<R: Rng>(&self, r: &mut R) -> f32 {
        match *self {
            FloatProvider::Constant(c) => c,
            FloatProvider::Uniform(a, b) => r.next_float() * (b - a) + a,
            FloatProvider::Trapezoid { min, max, plateau } => {
                let f = max - min;
                let g = (f - plateau) / 2.0;
                let h = f - g;
                min + r.next_float() * h + r.next_float() * g
            }
        }
    }
}

fn anchor(v: &Value) -> Result<i32> {
    if let Some(a) = v.get("absolute").and_then(|x| x.as_i64()) { return Ok(a as i32); }
    if let Some(a) = v.get("above_bottom").and_then(|x| x.as_i64()) { return Ok(crate::MIN_Y + a as i32); }
    if let Some(a) = v.get("below_top").and_then(|x| x.as_i64()) { return Ok(crate::MIN_Y + crate::HEIGHT - 1 - a as i32); }
    bail!("bad anchor {v}")
}

#[derive(Clone, Copy, Debug)]
pub struct UniformHeight {
    min: i32,
    max: i32,
}

impl UniformHeight {
    fn parse(v: &Value) -> Result<Self> {
        match v["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
            "uniform" => Ok(UniformHeight { min: anchor(&v["min_inclusive"])?, max: anchor(&v["max_inclusive"])? }),
            t => bail!("unsupported height provider {t}"),
        }
    }
    fn sample<R: Rng>(&self, r: &mut R) -> i32 {
        if self.min > self.max { return self.min; }
        r.next_int_bounded(self.max - self.min + 1) + self.min
    }
}

#[derive(Debug)]
pub enum Kind {
    Cave { horizontal: FloatProvider, vertical: FloatProvider, floor: FloatProvider },
    Canyon {
        vertical_rotation: FloatProvider,
        distance_factor: FloatProvider,
        thickness: FloatProvider,
        width_smoothness: i32,
        horizontal_radius_factor: FloatProvider,
        vertical_radius_default_factor: f32,
        vertical_radius_center_factor: f32,
    },
}

#[derive(Debug)]
pub struct Carver {
    pub probability: f32,
    y: UniformHeight,
    y_scale: FloatProvider,
    lava_level: i32,
    replaceable: Vec<bool>,
    kind: Kind,
}

/// Resolve a block tag (recursively) into block names.
pub fn resolve_tag(tags: &Path, name: &str, out: &mut HashSet<String>) -> Result<()> {
    let n = name.trim_start_matches('#').trim_start_matches("minecraft:");
    let p = tags.join(format!("{n}.json"));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).map_err(|e| anyhow!("{}: {e}", p.display()))?)?;
    for e in v["values"].as_array().ok_or_else(|| anyhow!("tag values"))? {
        let s = e.as_str().or_else(|| e.get("id").and_then(|i| i.as_str())).unwrap_or("");
        if s.starts_with('#') { resolve_tag(tags, s, out)?; } else { out.insert(s.trim_start_matches("minecraft:").to_string()); }
    }
    Ok(())
}

impl Carver {
    pub fn load(data: &Path, name: &str) -> Result<Carver> {
        let n = name.trim_start_matches("minecraft:");
        let p = data.join("worldgen/configured_carver").join(format!("{n}.json"));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&p)?)?;
        let c = &v["config"];
        let mut set = HashSet::new();
        let rep = c["replaceable"].as_str().ok_or_else(|| anyhow!("replaceable"))?;
        if rep.starts_with('#') { resolve_tag(&data.join("tags/block"), rep, &mut set)?; } else { set.insert(rep.trim_start_matches("minecraft:").to_string()); }
        let replaceable = (0..basalt_data::blocks::STATE_COUNT as u32).map(|i| set.contains(BlockState(i).name())).collect();
        let kind = match v["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
            "cave" => Kind::Cave {
                horizontal: FloatProvider::parse(&c["horizontal_radius_multiplier"])?,
                vertical: FloatProvider::parse(&c["vertical_radius_multiplier"])?,
                floor: FloatProvider::parse(&c["floor_level"])?,
            },
            "canyon" => {
                let s = &c["shape"];
                Kind::Canyon {
                    vertical_rotation: FloatProvider::parse(&c["vertical_rotation"])?,
                    distance_factor: FloatProvider::parse(&s["distance_factor"])?,
                    thickness: FloatProvider::parse(&s["thickness"])?,
                    width_smoothness: s["width_smoothness"].as_i64().unwrap_or(3) as i32,
                    horizontal_radius_factor: FloatProvider::parse(&s["horizontal_radius_factor"])?,
                    vertical_radius_default_factor: s["vertical_radius_default_factor"].as_f64().unwrap_or(1.0) as f32,
                    vertical_radius_center_factor: s["vertical_radius_center_factor"].as_f64().unwrap_or(0.0) as f32,
                }
            }
            t => bail!("unsupported carver type {t}"),
        };
        Ok(Carver {
            probability: c["probability"].as_f64().unwrap_or(0.0) as f32,
            y: UniformHeight::parse(&c["y"])?,
            y_scale: FloatProvider::parse(&c["yScale"])?,
            lava_level: anchor(&c["lava_level"])?,
            replaceable,
            kind,
        })
    }
}

/// What a carver needs from the chunk being carved.
pub trait CarveTarget {
    fn get(&self, x: i32, y: i32, z: i32) -> u16;
    fn set(&mut self, x: i32, y: i32, z: i32, state: u16);
    /// Fluid (or air) for a carved position; None leaves the block (aquifer barrier).
    fn carve_state(&mut self, x: i32, y: i32, z: i32) -> Option<u16>;
    /// Surface block to put on exposed dirt below a carved block.
    fn top_material(&mut self, x: i32, y: i32, z: i32, fluid: bool) -> Option<u16>;
    fn is_fluid(&self, state: u16) -> bool;
}

pub struct CarveCtx<'a> {
    pub trig: &'a Trig,
    pub cx: i32,
    pub cz: i32,
    pub mask: Vec<u64>,
    pub grass: u16,
    pub mycelium: u16,
    pub dirt: u16,
    pub lava: u16,
}

#[derive(Clone, Copy)]
enum Skip<'a> {
    Cave(f64),
    Canyon(&'a [f32]),
}

impl CarveCtx<'_> {
    fn mask_test_set(&mut self, x: i32, y: i32, z: i32) -> bool {
        let i = (((y - crate::MIN_Y) as usize) << 8) | ((z as usize) << 4) | x as usize;
        let was = self.mask[i >> 6] & (1 << (i & 63)) != 0;
        self.mask[i >> 6] |= 1 << (i & 63);
        was
    }

    #[allow(clippy::too_many_arguments)]
    fn carve_ellipsoid(&mut self, c: &Carver, t: &mut dyn CarveTarget, x: f64, y: f64, z: f64, hr: f64, vr: f64, skip: Skip) {
        let mx = (self.cx * 16 + 7) as f64;
        let mz = (self.cz * 16 + 7) as f64;
        let f = 16.0 + hr * 2.0;
        if (x - mx).abs() > f || (z - mz).abs() > f { return; }
        let i = self.cx * 16;
        let j = self.cz * 16;
        let k = ((x - hr).floor() as i32 - i - 1).max(0);
        let l = ((x + hr).floor() as i32 - i).min(15);
        let m = ((y - vr).floor() as i32 - 1).max(crate::MIN_Y + 1);
        let o = ((y + vr).floor() as i32 + 1).min(crate::MIN_Y + crate::HEIGHT - 1 - 7);
        let p = ((z - hr).floor() as i32 - j - 1).max(0);
        let q = ((z + hr).floor() as i32 - j).min(15);
        for r in k..=l {
            let s = i + r;
            let g = (s as f64 + 0.5 - x) / hr;
            for tt in p..=q {
                let u = j + tt;
                let h = (u as f64 + 0.5 - z) / hr;
                if g * g + h * h >= 1.0 { continue; }
                let mut surface = false;
                let mut v = o;
                while v > m {
                    let w = (v as f64 - 0.5 - y) / vr;
                    let skipped = match skip {
                        Skip::Cave(floor) => w <= floor || g * g + w * w + h * h >= 1.0,
                        Skip::Canyon(fs) => (g * g + h * h) * fs[(v - crate::MIN_Y - 1) as usize] as f64 + w * w / 6.0 >= 1.0,
                    };
                    if !skipped && !self.mask_test_set(r, v, tt) {
                        self.carve_block(c, t, r, v, tt, &mut surface);
                    }
                    v -= 1;
                }
            }
        }
    }

    fn carve_block(&mut self, c: &Carver, t: &mut dyn CarveTarget, lx: i32, y: i32, lz: i32, surface: &mut bool) {
        let st = t.get(lx, y, lz);
        if st == self.grass || st == self.mycelium { *surface = true; }
        if !c.replaceable[st as usize] { return; }
        let wx = self.cx * 16 + lx;
        let wz = self.cz * 16 + lz;
        let ns = if y <= c.lava_level { Some(self.lava) } else { t.carve_state(wx, y, wz) };
        let Some(ns) = ns else { return };
        t.set(lx, y, lz, ns);
        if *surface && y - 1 >= crate::MIN_Y && t.get(lx, y - 1, lz) == self.dirt {
            let fl = t.is_fluid(ns);
            if let Some(top) = t.top_material(wx, y - 1, wz, fl) { t.set(lx, y - 1, lz, top); }
        }
    }

    fn can_reach(&self, x: f64, z: f64, branch: i32, count: i32, thickness: f32) -> bool {
        let f = x - (self.cx * 16 + 7) as f64;
        let g = z - (self.cz * 16 + 7) as f64;
        let h = (count - branch) as f64;
        let i = (thickness + 2.0 + 16.0) as f64;
        f * f + g * g - h * h <= i * i
    }

    /// Run one carver started in chunk (sx, sz) with the prepared random.
    pub fn carve(&mut self, c: &Carver, t: &mut dyn CarveTarget, r: &mut JavaRandom, sx: i32, sz: i32) {
        let range = (4 * 2 - 1) * 16;
        match &c.kind {
            Kind::Cave { horizontal, vertical, floor } => {
                let bound = r.next_int_bounded(15) + 1;
                let b2 = r.next_int_bounded(bound) + 1;
                let n = r.next_int_bounded(b2);
                for _ in 0..n {
                    let x = (sx * 16 + r.next_int_bounded(16)) as f64;
                    let y = c.y.sample(r) as f64;
                    let z = (sz * 16 + r.next_int_bounded(16)) as f64;
                    let hm = horizontal.sample(r) as f64;
                    let vm = vertical.sample(r) as f64;
                    let fl = floor.sample(r) as f64;
                    let skip = Skip::Cave(fl);
                    let mut m = 1;
                    if r.next_int_bounded(4) == 0 {
                        let ys = c.y_scale.sample(r) as f64;
                        let o = 1.0 + r.next_float() * 6.0;
                        let f = 1.5 + (self.trig.sin(std::f32::consts::FRAC_PI_2 as f64) * o) as f64;
                        let g = f * ys;
                        self.carve_ellipsoid(c, t, x + 1.0, y, z, f, g, skip);
                        m += r.next_int_bounded(4);
                    }
                    for _ in 0..m {
                        let yaw = r.next_float() * (std::f64::consts::PI * 2.0) as f32;
                        let pitch = (r.next_float() - 0.5) / 4.0;
                        let thick = cave_thickness(r);
                        let count = range - r.next_int_bounded(range / 4);
                        let seed = r.next_long();
                        self.tunnel(c, t, seed, x, y, z, hm, vm, thick, yaw, pitch, 0, count, 1.0, skip);
                    }
                }
            }
            Kind::Canyon { vertical_rotation, distance_factor, thickness, .. } => {
                let x = (sx * 16 + r.next_int_bounded(16)) as f64;
                let y = c.y.sample(r) as f64;
                let z = (sz * 16 + r.next_int_bounded(16)) as f64;
                let yaw = r.next_float() * (std::f64::consts::PI * 2.0) as f32;
                let pitch = vertical_rotation.sample(r);
                let ys = c.y_scale.sample(r) as f64;
                let th = thickness.sample(r);
                let count = (range as f32 * distance_factor.sample(r)) as i32;
                let seed = r.next_long();
                self.canyon(c, t, seed, x, y, z, th, yaw, pitch, 0, count, ys);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn tunnel(&mut self, c: &Carver, t: &mut dyn CarveTarget, seed: i64, mut x: f64, mut y: f64, mut z: f64, hm: f64, vm: f64,
              thickness: f32, mut yaw: f32, mut pitch: f32, branch: i32, count: i32, y_scale: f64, skip: Skip) {
        let mut r = JavaRandom::new(seed);
        let split = r.next_int_bounded(count / 2) + count / 4;
        let steep = r.next_int_bounded(6) == 0;
        let mut f = 0.0f32;
        let mut g = 0.0f32;
        for j in branch..count {
            let d = 1.5 + (self.trig.sin((std::f32::consts::PI * j as f32 / count as f32) as f64) * thickness) as f64;
            let e = d * y_scale;
            let h = self.trig.cos(pitch as f64);
            x += (self.trig.cos(yaw as f64) * h) as f64;
            y += self.trig.sin(pitch as f64) as f64;
            z += (self.trig.sin(yaw as f64) * h) as f64;
            pitch *= if steep { 0.92 } else { 0.7 };
            pitch += g * 0.1;
            yaw += f * 0.1;
            g *= 0.9;
            f *= 0.75;
            g += (r.next_float() - r.next_float()) * r.next_float() * 2.0;
            f += (r.next_float() - r.next_float()) * r.next_float() * 4.0;
            if j == split && thickness > 1.0 {
                let s1 = r.next_long();
                let t1 = r.next_float() * 0.5 + 0.5;
                self.tunnel(c, t, s1, x, y, z, hm, vm, t1, yaw - std::f32::consts::FRAC_PI_2, pitch / 3.0, j, count, 1.0, skip);
                let s2 = r.next_long();
                let t2 = r.next_float() * 0.5 + 0.5;
                self.tunnel(c, t, s2, x, y, z, hm, vm, t2, yaw + std::f32::consts::FRAC_PI_2, pitch / 3.0, j, count, 1.0, skip);
                return;
            }
            if r.next_int_bounded(4) == 0 { continue; }
            if !self.can_reach(x, z, j, count, thickness) { return; }
            self.carve_ellipsoid(c, t, x, y, z, d * hm, e * vm, skip);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn canyon(&mut self, c: &Carver, t: &mut dyn CarveTarget, seed: i64, mut x: f64, mut y: f64, mut z: f64, thickness: f32,
              mut yaw: f32, mut pitch: f32, branch: i32, count: i32, y_scale: f64) {
        let Kind::Canyon { width_smoothness, horizontal_radius_factor, vertical_radius_default_factor, vertical_radius_center_factor, .. } = &c.kind else { return };
        let mut r = JavaRandom::new(seed);
        let mut fs = vec![0f32; crate::HEIGHT as usize];
        let mut wf = 1.0f32;
        for (j, v) in fs.iter_mut().enumerate() {
            if j == 0 || r.next_int_bounded(*width_smoothness) == 0 { wf = 1.0 + r.next_float() * r.next_float(); }
            *v = wf * wf;
        }
        let mut f = 0.0f32;
        let mut g = 0.0f32;
        for i in branch..count {
            let mut d = 1.5 + (self.trig.sin((i as f32 * std::f32::consts::PI / count as f32) as f64) * thickness) as f64;
            let mut e = d * y_scale;
            d *= horizontal_radius_factor.sample(&mut r) as f64;
            let ff = 1.0 - (0.5 - i as f32 / count as f32).abs() * 2.0;
            let gg = vertical_radius_default_factor + vertical_radius_center_factor * ff;
            e = gg as f64 * e * (r.next_float() * (1.0 - 0.75) + 0.75) as f64;
            let h = self.trig.cos(pitch as f64);
            let jj = self.trig.sin(pitch as f64);
            x += (self.trig.cos(yaw as f64) * h) as f64;
            y += jj as f64;
            z += (self.trig.sin(yaw as f64) * h) as f64;
            pitch *= 0.7;
            pitch += g * 0.05;
            yaw += f * 0.05;
            g *= 0.8;
            f *= 0.5;
            g += (r.next_float() - r.next_float()) * r.next_float() * 2.0;
            f += (r.next_float() - r.next_float()) * r.next_float() * 4.0;
            if r.next_int_bounded(4) == 0 { continue; }
            if !self.can_reach(x, z, i, count, thickness) { return; }
            self.carve_ellipsoid(c, t, x, y, z, d, e, Skip::Canyon(&fs));
        }
    }
}

fn cave_thickness<R: Rng>(r: &mut R) -> f32 {
    let mut f = r.next_float() * 2.0 + r.next_float();
    if r.next_int_bounded(10) == 0 { f *= r.next_float() * r.next_float() * 3.0 + 1.0; }
    f
}

/// Prepare the random for a start chunk and test the carver's probability.
pub fn start(r: &mut JavaRandom, seed: i64, index: i32, sx: i32, sz: i32, c: &Carver) -> bool {
    set_large_feature_seed(r, seed.wrapping_add(index as i64), sx, sz);
    r.next_float() <= c.probability
}

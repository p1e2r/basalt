//! Lakes, springs, monster rooms and geodes.

use super::provider::{IntProvider, StateProvider};
use super::{block_set, Features, Pred, DIRS, R};
use crate::noise::{NoiseParams, NormalNoise};
use crate::random::Rng;
use crate::region::{flag, Region, F_AIR, F_FULL, F_MOTION, F_WATER};
use crate::state_from_json;
use anyhow::{anyhow, Result};
use basalt_data::BlockState;
use serde_json::Value;
use std::sync::Arc;

fn st(name: &str) -> u16 {
    BlockState::parse(name).map(|s| s.0 as u16).unwrap_or(0)
}
fn is_solid(s: u16) -> bool {
    flag(s, F_MOTION)
}
fn is_liquid_block(s: u16) -> bool {
    let n = BlockState(s as u32).name();
    n == "water" || n == "lava"
}

pub struct Lake {
    fluid: StateProvider,
    barrier: StateProvider,
    can_place: Pred,
    replace_air_fluid: Pred,
    replace_barrier: Pred,
}

impl Lake {
    pub fn parse(f: &mut Features, c: &Value) -> Result<Lake> {
        Ok(Lake {
            fluid: StateProvider::parse(&f.tags, &c["fluid"])?,
            barrier: StateProvider::parse(&f.tags, &c["barrier"])?,
            can_place: if c.get("can_place_feature").is_some() {
                Pred::parse(&f.tags, &c["can_place_feature"])?
            } else {
                Pred::True
            },
            replace_air_fluid: Pred::parse(&f.tags, &c["can_replace_with_air_or_fluid"])?,
            replace_barrier: Pred::parse(&f.tags, &c["can_replace_with_barrier"])?,
        })
    }

    pub fn place(&self, f: &Features, reg: &mut Region, r: &mut R, x: i32, y: i32, z: i32) -> bool {
        if y <= crate::MIN_Y + 4 {
            return false;
        }
        if !self.can_place.test(f, reg, x, y, z) {
            return false;
        }
        let (ox, oy, oz) = (x, y - 4, z);
        let mut bs = [false; 2048];
        let i = r.next_int_bounded(4) + 4;
        for _ in 0..i {
            let d = r.next_double() * 6.0 + 3.0;
            let e = r.next_double() * 4.0 + 2.0;
            let ff = r.next_double() * 6.0 + 3.0;
            let g = r.next_double() * (16.0 - d - 2.0) + 1.0 + d / 2.0;
            let h = r.next_double() * (8.0 - e - 4.0) + 2.0 + e / 2.0;
            let k = r.next_double() * (16.0 - ff - 2.0) + 1.0 + ff / 2.0;
            for l in 1..15 {
                for m in 1..15 {
                    for n in 1..7 {
                        let o = (l as f64 - g) / (d / 2.0);
                        let p = (n as f64 - h) / (e / 2.0);
                        let q = (m as f64 - k) / (ff / 2.0);
                        if o * o + p * p + q * q < 1.0 {
                            bs[((l * 16 + m) * 8 + n) as usize] = true;
                        }
                    }
                }
            }
        }
        let fluid = self.fluid.get(f, reg, r, ox, oy, oz).unwrap_or(0);
        let at = |j: i32, s: i32, t: i32| bs[((j * 16 + s) * 8 + t) as usize];
        let edge = |j: i32, s: i32, t: i32| {
            !at(j, s, t)
                && ((j < 15 && at(j + 1, s, t))
                    || (j > 0 && at(j - 1, s, t))
                    || (s < 15 && at(j, s + 1, t))
                    || (s > 0 && at(j, s - 1, t))
                    || (t < 7 && at(j, s, t + 1))
                    || (t > 0 && at(j, s, t - 1)))
        };
        for j in 0..16 {
            for s in 0..16 {
                for t in 0..8 {
                    if !edge(j, s, t) {
                        continue;
                    }
                    let b = reg.block(ox + j, oy + t, oz + s);
                    if t >= 4 && is_liquid_block(b) {
                        return false;
                    }
                    if t >= 4 || is_solid(b) || b == fluid {
                        continue;
                    }
                    return false;
                }
            }
        }
        for j in 0..16 {
            for s in 0..16 {
                for t in 0..8 {
                    if !at(j, s, t) {
                        continue;
                    }
                    let p = (ox + j, oy + t, oz + s);
                    if !self.replace_air_fluid.test(f, reg, p.0, p.1, p.2) {
                        continue;
                    }
                    reg.set(p.0, p.1, p.2, if t >= 4 { 0 } else { fluid });
                }
            }
        }
        let barrier = self.barrier.get(f, reg, r, ox, oy, oz).unwrap_or(0);
        if !flag(barrier, F_AIR) {
            for j in 0..16 {
                for s in 0..16 {
                    for t in 0..8 {
                        if !edge(j, s, t) {
                            continue;
                        }
                        if t >= 4 && r.next_int_bounded(2) == 0 {
                            continue;
                        }
                        let p = (ox + j, oy + t, oz + s);
                        let b = reg.block(p.0, p.1, p.2);
                        if !is_solid(b) || !self.replace_barrier.test(f, reg, p.0, p.1, p.2) {
                            continue;
                        }
                        reg.set(p.0, p.1, p.2, barrier);
                    }
                }
            }
        }
        if flag(fluid, F_WATER) {
            for j in 0..16 {
                for s in 0..16 {
                    let (px, py, pz) = (ox + j, oy + 4, oz + s);
                    let biome = reg.biome(px, py, pz);
                    let b = reg.block(px, py - 1, pz);
                    if BlockState(b as u32).name() == "water"
                        && reg.g.surface.cold_enough_to_snow(biome, px, py - 1, pz)
                        && self.replace_air_fluid.test(f, reg, px, py - 1, pz)
                    {
                        reg.set(px, py - 1, pz, st("ice"));
                    }
                }
            }
        }
        true
    }
}

pub struct Spring {
    state: u16,
    valid: Arc<Vec<bool>>,
    requires_below: bool,
    rock: i32,
    hole: i32,
}

impl Spring {
    pub fn parse(f: &mut Features, c: &Value) -> Result<Spring> {
        let s = &c["state"];
        let name = s["Name"].as_str().ok_or_else(|| anyhow!("spring state"))?;
        Ok(Spring {
            state: BlockState::from_name(name)
                .map(|b| b.0 as u16)
                .ok_or_else(|| anyhow!("fluid {name}"))?,
            valid: block_set(&f.tags, &c["valid_blocks"]),
            requires_below: c
                .get("requires_block_below")
                .and_then(|x| x.as_bool())
                .unwrap_or(true),
            rock: c.get("rock_count").and_then(|x| x.as_i64()).unwrap_or(4) as i32,
            hole: c.get("hole_count").and_then(|x| x.as_i64()).unwrap_or(1) as i32,
        })
    }

    pub fn place(&self, reg: &mut Region, x: i32, y: i32, z: i32) -> bool {
        let v = |reg: &Region, p: (i32, i32, i32)| self.valid[reg.block(p.0, p.1, p.2) as usize];
        if !v(reg, (x, y + 1, z)) {
            return false;
        }
        if self.requires_below && !v(reg, (x, y - 1, z)) {
            return false;
        }
        let here = reg.block(x, y, z);
        if !flag(here, F_AIR) && !self.valid[here as usize] {
            return false;
        }
        let ns = [
            (x - 1, y, z),
            (x + 1, y, z),
            (x, y, z - 1),
            (x, y, z + 1),
            (x, y - 1, z),
        ];
        let j = ns.iter().filter(|p| v(reg, **p)).count() as i32;
        let i = ns
            .iter()
            .filter(|p| flag(reg.block(p.0, p.1, p.2), F_AIR))
            .count() as i32;
        if j == self.rock && i == self.hole {
            reg.set(x, y, z, self.state);
            return true;
        }
        false
    }
}

pub fn monster_room(f: &Features, reg: &mut Region, r: &mut R, x: i32, y: i32, z: i32) -> bool {
    let cannot = f.tag("features_cannot_replace");
    let safe = |reg: &mut Region, p: (i32, i32, i32), s: u16| {
        if !cannot[reg.block(p.0, p.1, p.2) as usize] {
            reg.set(p.0, p.1, p.2, s);
        }
    };
    let j = r.next_int_bounded(2) + 2;
    let (k, l) = (-j - 1, j + 1);
    let o = r.next_int_bounded(2) + 2;
    let (p, q) = (-o - 1, o + 1);
    let mut cnt = 0;
    for s in k..=l {
        for t in -1..=4 {
            for u in p..=q {
                let bp = (x + s, y + t, z + u);
                let solid = is_solid(reg.block(bp.0, bp.1, bp.2));
                if t == -1 && !solid {
                    return false;
                }
                if t == 4 && !solid {
                    return false;
                }
                if (s == k || s == l || u == p || u == q)
                    && t == 0
                    && flag(reg.block(bp.0, bp.1, bp.2), F_AIR)
                    && flag(reg.block(bp.0, bp.1 + 1, bp.2), F_AIR)
                {
                    cnt += 1;
                }
            }
        }
    }
    if !(1..=5).contains(&cnt) {
        return false;
    }
    let cave_air = st("cave_air");
    let cobble = st("cobblestone");
    let mossy = st("mossy_cobblestone");
    let chest_id = st("chest");
    for s in k..=l {
        let mut t = 3;
        while t >= -1 {
            for u in p..=q {
                let bp = (x + s, y + t, z + u);
                let b = reg.block(bp.0, bp.1, bp.2);
                let name = BlockState(b as u32).name();
                if s == k || t == -1 || u == p || s == l || t == 4 || u == q {
                    if bp.1 >= crate::MIN_Y && !is_solid(reg.block(bp.0, bp.1 - 1, bp.2)) {
                        reg.set(bp.0, bp.1, bp.2, cave_air);
                        continue;
                    }
                    if !is_solid(b) || name == "chest" {
                        continue;
                    }
                    if t == -1 && r.next_int_bounded(4) != 0 {
                        safe(reg, bp, mossy);
                        continue;
                    }
                    safe(reg, bp, cobble);
                    continue;
                }
                if name == "chest" || name == "spawner" {
                    continue;
                }
                safe(reg, bp, cave_air);
            }
            t -= 1;
        }
    }
    'outer: for _ in 0..2 {
        for _ in 0..3 {
            let u = x + r.next_int_bounded(j * 2 + 1) - j;
            let w = z + r.next_int_bounded(o * 2 + 1) - o;
            let bp = (u, y, w);
            if !flag(reg.block(bp.0, bp.1, bp.2), F_AIR) {
                continue;
            }
            let hor = [(0, -1), (1, 0), (0, 1), (-1, 0)];
            let n = hor
                .iter()
                .filter(|(dx, dz)| is_solid(reg.block(u + dx, y, w + dz)))
                .count();
            if n != 1 {
                continue;
            }
            // Chest facing: away from the single solid-render neighbour.
            let mut dir: Option<usize> = None;
            let mut amb = false;
            for (i, (dx, dz)) in hor.iter().enumerate() {
                let b = reg.block(u + dx, y, w + dz);
                if b == chest_id {
                    break;
                }
                if !flag(b, F_FULL) {
                    continue;
                }
                if dir.is_some() {
                    amb = true;
                    break;
                }
                dir = Some(i);
            }
            let facing = match dir {
                Some(i) if !amb => ["south", "west", "north", "east"][i],
                _ => "north",
            };
            let chest = BlockState(chest_id as u32)
                .with("facing", facing)
                .map(|b| b.0 as u16)
                .unwrap_or(chest_id);
            safe(reg, bp, chest);
            let _ = r.next_long();
            continue 'outer;
        }
    }
    safe(reg, (x, y, z), st("spawner"));
    let _ = r.next_int_bounded(4);
    true
}

pub struct Geode {
    filling: StateProvider,
    inner: StateProvider,
    alt_inner: StateProvider,
    middle: StateProvider,
    outer: StateProvider,
    inner_placements: Vec<u16>,
    cannot_replace: Arc<Vec<bool>>,
    invalid: Arc<Vec<bool>>,
    layers: [f64; 4],
    crack_chance: f64,
    base_crack: f64,
    crack_offset: i32,
    noise_mult: f64,
    potential_chance: f64,
    alt_chance: f64,
    require_alt: bool,
    wall: IntProvider,
    points: IntProvider,
    point_offset: IntProvider,
    min_off: i32,
    max_off: i32,
    invalid_threshold: i32,
}

impl Geode {
    pub fn parse(f: &mut Features, c: &Value) -> Result<Geode> {
        let b = &c["blocks"];
        let l = &c["layers"];
        let cr = &c["crack"];
        let num = |v: &Value, k: &str, d: f64| v.get(k).and_then(|x| x.as_f64()).unwrap_or(d);
        let ip = |v: &Value, k: &str, d: IntProvider| -> Result<IntProvider> {
            if v.get(k).is_some() {
                IntProvider::parse(&v[k])
            } else {
                Ok(d)
            }
        };
        Ok(Geode {
            filling: StateProvider::parse(&f.tags, &b["filling_provider"])?,
            inner: StateProvider::parse(&f.tags, &b["inner_layer_provider"])?,
            alt_inner: StateProvider::parse(&f.tags, &b["alternate_inner_layer_provider"])?,
            middle: StateProvider::parse(&f.tags, &b["middle_layer_provider"])?,
            outer: StateProvider::parse(&f.tags, &b["outer_layer_provider"])?,
            inner_placements: b["inner_placements"]
                .as_array()
                .map(|a| a.iter().filter_map(|x| state_from_json(x).ok()).collect())
                .unwrap_or_default(),
            cannot_replace: block_set(&f.tags, &b["cannot_replace"]),
            invalid: block_set(&f.tags, &b["invalid_blocks"]),
            layers: [
                num(l, "filling", 1.7),
                num(l, "inner_layer", 2.2),
                num(l, "middle_layer", 3.2),
                num(l, "outer_layer", 4.2),
            ],
            crack_chance: num(cr, "generate_crack_chance", 1.0),
            base_crack: num(cr, "base_crack_size", 2.0),
            crack_offset: num(cr, "crack_point_offset", 2.0) as i32,
            noise_mult: num(c, "noise_multiplier", 0.05),
            potential_chance: num(c, "use_potential_placements_chance", 0.35),
            alt_chance: num(c, "use_alternate_layer0_chance", 0.0),
            require_alt: c
                .get("placements_require_layer0_alternate")
                .and_then(|x| x.as_bool())
                .unwrap_or(true),
            wall: ip(c, "outer_wall_distance", IntProvider::Uniform(4, 5))?,
            points: ip(c, "distribution_points", IntProvider::Uniform(3, 4))?,
            point_offset: ip(c, "point_offset", IntProvider::Uniform(1, 2))?,
            min_off: num(c, "min_gen_offset", -16.0) as i32,
            max_off: num(c, "max_gen_offset", 16.0) as i32,
            invalid_threshold: num(c, "invalid_blocks_threshold", 1.0) as i32,
        })
    }

    pub fn place(&self, f: &Features, reg: &mut Region, r: &mut R, x: i32, y: i32, z: i32) -> bool {
        let k = self.points.sample(r);
        let noise = NormalNoise::new_seeded_legacy(
            reg.g.seed,
            &NoiseParams {
                first_octave: -4,
                amplitudes: vec![1.0],
            },
        );
        let d = k as f64 / self.wall.max() as f64;
        let e = 1.0 / self.layers[0].sqrt();
        let ff = 1.0 / (self.layers[1] + d).sqrt();
        let g = 1.0 / (self.layers[2] + d).sqrt();
        let h = 1.0 / (self.layers[3] + d).sqrt();
        let l =
            1.0 / (self.base_crack + r.next_double() / 2.0 + if k > 3 { d } else { 0.0 }).sqrt();
        let crack = (r.next_float() as f64) < self.crack_chance;
        let mut m = 0;
        let mut points: Vec<((i32, i32, i32), i32)> = Vec::new();
        for _ in 0..k {
            let o = self.wall.sample(r);
            let p = self.wall.sample(r);
            let q = self.wall.sample(r);
            let bp = (x + o, y + p, z + q);
            let b = reg.block(bp.0, bp.1, bp.2);
            if flag(b, F_AIR) || self.invalid[b as usize] {
                m += 1;
                if m > self.invalid_threshold {
                    return false;
                }
            }
            points.push((bp, self.point_offset.sample(r)));
        }
        let mut cracks: Vec<(i32, i32, i32)> = Vec::new();
        if crack {
            let rr = r.next_int_bounded(4);
            let s = k * 2 + 1;
            let (dx, dz) = match rr {
                0 => (s, 0),
                1 => (0, s),
                2 => (s, s),
                _ => (0, 0),
            };
            for dy in [7, 5, 1] {
                cracks.push((x + dx, y + dy, z + dz));
            }
        }
        let ok = |reg: &Region, p: (i32, i32, i32)| {
            !self.cannot_replace[reg.block(p.0, p.1, p.2) as usize]
        };
        let mut potential = Vec::new();
        for bz in z + self.min_off..=z + self.max_off {
            for by in y + self.min_off..=y + self.max_off {
                for bx in x + self.min_off..=x + self.max_off {
                    let t = noise.get(bx as f64, by as f64, bz as f64) * self.noise_mult;
                    let dist = |p: (i32, i32, i32)| {
                        let (a, b, c) = (bx - p.0, by - p.1, bz - p.2);
                        (a * a + b * b + c * c) as f64
                    };
                    let u: f64 = points
                        .iter()
                        .map(|(p, off)| 1.0 / (dist(*p) + *off as f64).sqrt() + t)
                        .sum();
                    let v: f64 = cracks
                        .iter()
                        .map(|p| 1.0 / (dist(*p) + self.crack_offset as f64).sqrt() + t)
                        .sum();
                    if u < h {
                        continue;
                    }
                    let bp = (bx, by, bz);
                    if crack && v >= l && u < e {
                        if ok(reg, bp) {
                            reg.set(bx, by, bz, 0);
                        }
                        continue;
                    }
                    if u >= e {
                        if let Some(s) = self.filling.get(f, reg, r, bx, by, bz) {
                            if ok(reg, bp) {
                                reg.set(bx, by, bz, s);
                            }
                        }
                        continue;
                    }
                    if u >= ff {
                        let alt = (r.next_float() as f64) < self.alt_chance;
                        let prov = if alt { &self.alt_inner } else { &self.inner };
                        if let Some(s) = prov.get(f, reg, r, bx, by, bz) {
                            if ok(reg, bp) {
                                reg.set(bx, by, bz, s);
                            }
                        }
                        if (self.require_alt && !alt)
                            || (r.next_float() as f64) >= self.potential_chance
                        {
                            continue;
                        }
                        potential.push(bp);
                        continue;
                    }
                    if u >= g {
                        if let Some(s) = self.middle.get(f, reg, r, bx, by, bz) {
                            if ok(reg, bp) {
                                reg.set(bx, by, bz, s);
                            }
                        }
                        continue;
                    }
                    if let Some(s) = self.outer.get(f, reg, r, bx, by, bz) {
                        if ok(reg, bp) {
                            reg.set(bx, by, bz, s);
                        }
                    }
                }
            }
        }
        if self.inner_placements.is_empty() {
            return true;
        }
        for bp in potential {
            let mut s = self.inner_placements
                [r.next_int_bounded(self.inner_placements.len() as i32) as usize];
            for (di, d) in DIRS.iter().enumerate() {
                let face = ["down", "up", "north", "south", "west", "east"][di];
                let bs = BlockState(s as u32);
                if bs.get("facing").is_some() {
                    if let Some(n) = bs.with("facing", face) {
                        s = n.0 as u16;
                    }
                }
                let q = (bp.0 + d.0, bp.1 + d.1, bp.2 + d.2);
                let b2 = reg.block(q.0, q.1, q.2);
                let src = BlockState(b2 as u32).name() == "water"
                    && BlockState(b2 as u32).get("level") == Some("0")
                    || BlockState(b2 as u32).get("waterlogged") == Some("true");
                let bs = BlockState(s as u32);
                if bs.get("waterlogged").is_some() {
                    if let Some(n) = bs.with("waterlogged", if src { "true" } else { "false" }) {
                        s = n.0 as u16;
                    }
                }
                let grow = flag(b2, F_AIR)
                    || (BlockState(b2 as u32).name() == "water"
                        && BlockState(b2 as u32).get("level") == Some("0"));
                if !grow {
                    continue;
                }
                if ok(reg, q) {
                    reg.set(q.0, q.1, q.2, s);
                }
                break;
            }
        }
        true
    }
}

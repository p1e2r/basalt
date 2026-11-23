//! Configured feature types.

use super::provider::{IntProvider, StateProvider};
use super::{block_set, Features, Pred, R};
use crate::random::Rng;
use crate::region::{flag, Hm, Region, F_AIR, F_FULL, F_WATER};
use crate::state_from_json;
use anyhow::{anyhow, bail, Result};
use basalt_data::BlockState;
use serde_json::Value;
use std::sync::Arc;

pub struct Ore {
    size: i32,
    discard: f32,
    /// (replaceable state set, result state)
    targets: Vec<(Arc<Vec<bool>>, u16)>,
    scattered: bool,
}

pub struct Disk {
    half_height: i32,
    radius: IntProvider,
    provider: StateProvider,
    target: Pred,
}

pub enum Kind {
    Unsupported,
    Ore(Ore),
    Disk(Disk),
    Freeze,
    SimpleBlock(StateProvider),
    RandomSelector(Vec<(f32, usize)>, usize),
    SimpleRandomSelector(Vec<usize>),
    RandomBoolean(usize, usize),
    Sequence(Vec<usize>),
    BlockColumn { layers: Vec<(IntProvider, StateProvider)>, up: bool, allowed: Pred, prioritize_tip: bool },
    Seagrass(f32),
    Kelp,
    Tree(Box<super::tree::Tree>),
    HugeMushroom { red: bool, cap: StateProvider, stem: StateProvider, radius: i32, can_place_on: Arc<Vec<bool>> },
    Vines,
}

fn rule_test(f: &Features, v: &Value) -> Result<Arc<Vec<bool>>> {
    Ok(match v["predicate_type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
        "tag_match" => f.tags.get(v["tag"].as_str().unwrap_or("")),
        "block_match" => block_set(&f.tags, &v["block"]),
        t => bail!("unsupported rule test {t}"),
    })
}

pub fn parse(f: &mut Features, ty: &str, c: &Value) -> Result<Kind> {
    Ok(match ty {
        "ore" | "scattered_ore" => {
            let mut targets = Vec::new();
            for t in c["targets"].as_array().ok_or_else(|| anyhow!("targets"))? {
                targets.push((rule_test(f, &t["target"])?, state_from_json(&t["state"])?));
            }
            Kind::Ore(Ore {
                size: c["size"].as_i64().unwrap_or(0) as i32,
                discard: c["discard_chance_on_air_exposure"].as_f64().unwrap_or(0.0) as f32,
                targets,
                scattered: ty == "scattered_ore",
            })
        }
        "disk" => Kind::Disk(Disk {
            half_height: c["half_height"].as_i64().unwrap_or(0) as i32,
            radius: IntProvider::parse(&c["radius"])?,
            provider: StateProvider::parse(&f.tags, &c["state_provider"])?,
            target: Pred::parse(&f.tags, &c["target"])?,
        }),
        "freeze_top_layer" => Kind::Freeze,
        "simple_block" => Kind::SimpleBlock(StateProvider::parse(&f.tags, &c["to_place"])?),
        "random_selector" => {
            let mut v = Vec::new();
            for e in c["features"].as_array().ok_or_else(|| anyhow!("features"))? {
                let ch = e["chance"].as_f64().unwrap_or(0.0) as f32;
                v.push((ch, f.placed_ref(&e["feature"])));
            }
            Kind::RandomSelector(v, f.placed_ref(&c["default"]))
        }
        "simple_random_selector" => {
            let list = match &c["features"] {
                Value::Array(a) => a.clone(),
                other => vec![other.clone()],
            };
            Kind::SimpleRandomSelector(list.iter().map(|e| f.placed_ref(e)).collect())
        }
        "random_boolean_selector" => Kind::RandomBoolean(f.placed_ref(&c["feature_true"]), f.placed_ref(&c["feature_false"])),
        "sequence" => Kind::Sequence(c["features"].as_array().ok_or_else(|| anyhow!("features"))?.iter().map(|e| f.placed_ref(e)).collect()),
        "block_column" => {
            let mut layers = Vec::new();
            for l in c["layers"].as_array().ok_or_else(|| anyhow!("layers"))? {
                layers.push((IntProvider::parse(&l["height"])?, StateProvider::parse(&f.tags, &l["provider"])?));
            }
            Kind::BlockColumn {
                layers, up: c["direction"].as_str() != Some("down"),
                allowed: Pred::parse(&f.tags, &c["allowed_placement"])?,
                prioritize_tip: c["prioritize_tip"].as_bool().unwrap_or(false),
            }
        }
        "seagrass" => Kind::Seagrass(c["probability"].as_f64().unwrap_or(0.0) as f32),
        "kelp" => Kind::Kelp,
        "vines" => Kind::Vines,
        "tree" => Kind::Tree(Box::new(super::tree::Tree::parse(f, c)?)),
        "huge_red_mushroom" | "huge_brown_mushroom" => Kind::HugeMushroom {
            red: ty == "huge_red_mushroom",
            cap: StateProvider::parse(&f.tags, &c["cap_provider"])?,
            stem: StateProvider::parse(&f.tags, &c["stem_provider"])?,
            radius: c.get("foliage_radius").and_then(|x| x.as_i64()).unwrap_or(2) as i32,
            can_place_on: block_set(&f.tags, &c["can_place_on"]),
        },
        t => bail!("unsupported feature type {t}"),
    })
}

/// Place a configured feature; returns whether it placed anything.
pub fn place(f: &Features, reg: &mut Region, r: &mut R, id: usize, x: i32, y: i32, z: i32) -> bool {
    match &f.configured[id] {
        Kind::Unsupported => false,
        Kind::Ore(o) => ore(f, o, reg, r, x, y, z),
        Kind::Disk(d) => disk(f, d, reg, r, x, y, z),
        Kind::Freeze => { freeze(reg, x, z); true }
        Kind::SimpleBlock(p) => simple_block(f, p, reg, r, x, y, z),
        Kind::RandomSelector(list, default) => {
            for (ch, pid) in list {
                if r.next_float() < *ch { return f.place_placed(reg, r, *pid, None, x, y, z); }
            }
            f.place_placed(reg, r, *default, None, x, y, z)
        }
        Kind::SimpleRandomSelector(list) => {
            let i = r.next_int_bounded(list.len() as i32) as usize;
            f.place_placed(reg, r, list[i], None, x, y, z)
        }
        Kind::RandomBoolean(a, b) => {
            let t = r.next_bool();
            f.place_placed(reg, r, if t { *a } else { *b }, None, x, y, z)
        }
        Kind::Sequence(list) => {
            for pid in list { if !f.place_placed(reg, r, *pid, None, x, y, z) { return false; } }
            true
        }
        Kind::BlockColumn { layers, up, allowed, prioritize_tip } => block_column(f, layers, *up, allowed, *prioritize_tip, reg, r, x, y, z),
        Kind::Seagrass(p) => seagrass(f, *p, reg, r, x, y, z),
        Kind::Kelp => kelp(f, reg, r, x, y, z),
        Kind::Vines => vines(f, reg, x, y, z),
        Kind::Tree(t) => t.place(f, reg, r, x, y, z),
        Kind::HugeMushroom { red, cap, stem, radius, can_place_on } => super::tree::huge_mushroom(f, *red, cap, stem, *radius, can_place_on, reg, r, x, y, z),
    }
}

#[allow(clippy::too_many_arguments)]
fn ore(f: &Features, c: &Ore, reg: &mut Region, r: &mut R, ox: i32, oy: i32, oz: i32) -> bool {
    if c.scattered { return scattered_ore(c, reg, r, ox, oy, oz); }
    let trig = &f.trig;
    let fl = r.next_float() * std::f32::consts::PI;
    let g = c.size as f32 / 8.0;
    let i = ((c.size as f32 / 16.0 * 2.0 + 1.0) / 2.0).ceil() as i32;
    let gs = (fl as f64).sin() * g as f64;
    let gc = (fl as f64).cos() * g as f64;
    let d = ox as f64 + gs;
    let e = ox as f64 - gs;
    let h = oz as f64 + gc;
    let j = oz as f64 - gc;
    let l = (oy + r.next_int_bounded(3) - 2) as f64;
    let m = (oy + r.next_int_bounded(3) - 2) as f64;
    let gi = g.ceil() as i32;
    let n = ox - gi - i;
    let o = oy - 2 - i;
    let p = oz - gi - i;
    let q = 2 * (gi + i);
    let rr = 2 * (2 + i);
    let mut ok = false;
    'outer: for s in n..=n + q {
        for t in p..=p + q {
            if o <= reg.height(Hm::OceanFloor, s, t) { ok = true; break 'outer; }
        }
    }
    if !ok { return false; }
    let k = c.size as usize;
    let mut ds = vec![0f64; k * 4];
    for nn in 0..k {
        let ff = nn as f32 / k as f32;
        let px = d + ff as f64 * (e - d);
        let py = l + ff as f64 * (m - l);
        let pz = h + ff as f64 * (j - h);
        let s = r.next_double() * k as f64 / 16.0;
        let rad = (((trig.sin((std::f32::consts::PI * ff) as f64) + 1.0) as f64) * s + 1.0) / 2.0;
        ds[nn * 4] = px;
        ds[nn * 4 + 1] = py;
        ds[nn * 4 + 2] = pz;
        ds[nn * 4 + 3] = rad;
    }
    for a in 0..k.saturating_sub(1) {
        if ds[a * 4 + 3] <= 0.0 { continue; }
        for b in a + 1..k {
            if ds[b * 4 + 3] <= 0.0 { continue; }
            let px = ds[a * 4] - ds[b * 4];
            let py = ds[a * 4 + 1] - ds[b * 4 + 1];
            let pz = ds[a * 4 + 2] - ds[b * 4 + 2];
            let s = ds[a * 4 + 3] - ds[b * 4 + 3];
            if s * s > px * px + py * py + pz * pz {
                if s > 0.0 { ds[b * 4 + 3] = -1.0; } else { ds[a * 4 + 3] = -1.0; }
            }
        }
    }
    let width = q;
    let height = rr;
    let mut maxi = 0i64;
    for t in 0..k {
        let rad = ds[t * 4 + 3];
        if rad < 0.0 { continue; }
        let a = ((ds[t * 4] + rad).floor() as i32).max(n) - n;
        let b = ((ds[t * 4 + 1] + rad).floor() as i32).max(o) - o;
        let c2 = ((ds[t * 4 + 2] + rad).floor() as i32).max(p) - p;
        maxi = maxi.max(a as i64 + b as i64 * width as i64 + c2 as i64 * (width * height) as i64);
    }
    // Visited-position set, indexed like vanilla (indices may alias, which is kept).
    let mut bits = vec![false; maxi as usize + 1];
    let (bx0, bx1, bz0, bz1) = reg.write;
    let mut placed = 0;
    for t in 0..k {
        let rad = ds[t * 4 + 3];
        if rad < 0.0 { continue; }
        let (cx, cy, cz) = (ds[t * 4], ds[t * 4 + 1], ds[t * 4 + 2]);
        let u = ((cx - rad).floor() as i32).max(n);
        let v = ((cy - rad).floor() as i32).max(o);
        let ww = ((cz - rad).floor() as i32).max(p);
        let aa = ((cx + rad).floor() as i32).max(u);
        let ab = ((cy + rad).floor() as i32).max(v);
        let ac = ((cz + rad).floor() as i32).max(ww);
        // Spheres entirely outside the writable area do nothing (and draw no randoms).
        if aa < bx0 || u > bx1 || ac < bz0 || ww > bz1 { continue; }
        for ad in u..=aa {
            let ae = (ad as f64 + 0.5 - cx) / rad;
            if ae * ae >= 1.0 { continue; }
            for af in v..=ab {
                let ag = (af as f64 + 0.5 - cy) / rad;
                if ae * ae + ag * ag >= 1.0 { continue; }
                for ah in ww..=ac {
                    let ai = (ah as f64 + 0.5 - cz) / rad;
                    if ae * ae + ag * ag + ai * ai >= 1.0 || !(crate::MIN_Y..crate::MIN_Y + crate::HEIGHT).contains(&af) { continue; }
                    let aj = (ad - n + (af - o) * width + (ah - p) * width * height) as usize;
                    if bits[aj] { continue; }
                    bits[aj] = true;
                    if !reg.can_write(ad, af, ah) { continue; }
                    let cur = reg.block(ad, af, ah);
                    for (set, st) in &c.targets {
                        if !set[cur as usize] { continue; }
                        let ok = if c.discard <= 0.0 { true } else if c.discard >= 1.0 { false } else { r.next_float() >= c.discard }
                            || !adjacent_air(reg, ad, af, ah);
                        if !ok { continue; }
                        reg.set(ad, af, ah, *st);
                        placed += 1;
                        break;
                    }
                }
            }
        }
    }
    placed > 0
}

fn scattered_ore(c: &Ore, reg: &mut Region, r: &mut R, x: i32, y: i32, z: i32) -> bool {
    let i = r.next_int_bounded(c.size + 1);
    for j in 0..i {
        let mag = j.min(7) as f32;
        let mut off = || ((r.next_float() - r.next_float()) * mag + 0.5).floor() as i32;
        let (dx, dy, dz) = (off(), off(), off());
        let (px, py, pz) = (x + dx, y + dy, z + dz);
        let cur = reg.block(px, py, pz);
        for (set, st) in &c.targets {
            if !set[cur as usize] { continue; }
            let ok = if c.discard <= 0.0 { true } else if c.discard >= 1.0 { false } else { r.next_float() >= c.discard } || !adjacent_air(reg, px, py, pz);
            if ok { reg.set(px, py, pz, *st); break; }
        }
    }
    true
}

pub fn adjacent_air(reg: &Region, x: i32, y: i32, z: i32) -> bool {
    super::DIRS.iter().any(|(dx, dy, dz)| reg.get(x + dx, y + dy, z + dz).map(|b| flag(b, F_AIR)).unwrap_or(false))
}

#[allow(clippy::too_many_arguments)]
fn disk(f: &Features, d: &Disk, reg: &mut Region, r: &mut R, ox: i32, oy: i32, oz: i32) -> bool {
    let top = oy + d.half_height;
    let bottom = oy - d.half_height - 1;
    let l = d.radius.sample(r);
    let mut any = false;
    for z in oz - l..=oz + l {
        for x in ox - l..=ox + l {
            let (n, o) = (x - ox, z - oz);
            if n * n + o * o > l * l { continue; }
            let mut k = top;
            while k > bottom {
                if d.target.test(f, reg, x, k, z) {
                    if let Some(st) = d.provider.get(f, reg, r, x, k, z) { reg.set(x, k, z, st); any = true; }
                }
                k -= 1;
            }
        }
    }
    any
}

/// "freeze_top_layer": ice on cold water, snow on cold ground (one chunk of columns).
fn freeze(reg: &mut Region, ox: i32, oz: i32) {
    let g = reg.g;
    let ids = g.surface.ids;
    let snow = BlockState::from_name("snow").map(|s| s.0 as u16).unwrap_or(0);
    for i in 0..16 {
        for j in 0..16 {
            let (x, z) = (ox + i, oz + j);
            let n = reg.height(Hm::MotionBlocking, x, z);
            let biome = reg.biome(x, n, z);
            let below = reg.block(x, n - 1, z);
            if BlockState(below as u32).name() == "water" && BlockState(below as u32).get("level") == Some("0")
                && g.surface.cold_enough_to_snow(biome, x, n - 1, z) {
                reg.set(x, n - 1, z, ids.ice);
            }
            if !g.surface.cold_enough_to_snow(biome, x, n, z) { continue; }
            let here = reg.block(x, n, z);
            if !(flag(here, F_AIR) || BlockState(here as u32).name() == "snow") { continue; }
            let b = reg.block(x, n - 1, z);
            let bn = BlockState(b as u32).name();
            let survives = match bn {
                "ice" | "packed_ice" | "barrier" => false,
                "honey_block" | "soul_sand" | "mud" => true,
                _ => flag(b, F_FULL) || flag(b, crate::region::F_LEAVES),
            };
            if !survives { continue; }
            reg.set(x, n, z, snow);
            let bs = BlockState(b as u32);
            if bs.get("snowy").is_some() {
                if let Some(v) = bs.with("snowy", "true") { reg.set(x, n - 1, z, v.0 as u16); }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn simple_block(f: &Features, p: &StateProvider, reg: &mut Region, r: &mut R, x: i32, y: i32, z: i32) -> bool {
    let Some(s) = p.get(f, reg, r, x, y, z) else { return false };
    if !f.can_survive(reg, s, x, y, z) { return false; }
    let st = BlockState(s as u32);
    if st.get("half") == Some("lower") && matches!(st.block().kind, "double_plant" | "tall_flower" | "tall_grass" | "tall_seagrass") {
        if !flag(reg.block(x, y + 1, z), F_AIR) { return false; }
        reg.set(x, y, z, s);
        if let Some(u) = st.with("half", "upper") { reg.set(x, y + 1, z, u.0 as u16); }
    } else {
        reg.set(x, y, z, s);
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn block_column(f: &Features, layers: &[(IntProvider, StateProvider)], up: bool, allowed: &Pred, prioritize_tip: bool, reg: &mut Region, r: &mut R, x: i32, y: i32, z: i32) -> bool {
    let n = layers.len();
    let mut is: Vec<i32> = layers.iter().map(|(h, _)| h.sample(r)).collect();
    let j: i32 = is.iter().sum();
    if j == 0 { return false; }
    let dy = if up { 1 } else { -1 };
    let mut k = 0;
    while k < j {
        if !allowed.test(f, reg, x, y + (k + 1) * dy, z) { break; }
        k += 1;
    }
    if k < j {
        let mut l = j - k;
        if prioritize_tip {
            let mut i = 0;
            while i < n && l > 0 { let m = is[i].min(l); is[i] -= m; l -= m; i += 1; }
        } else {
            let mut i = n as i32 - 1;
            while i >= 0 && l > 0 { let m = is[i as usize].min(l); is[i as usize] -= m; l -= m; i -= 1; }
        }
    }
    let mut yy = y;
    for (i, (_, p)) in layers.iter().enumerate() {
        for _ in 0..is[i] {
            if let Some(s) = p.get(f, reg, r, x, yy, z) { reg.set(x, yy, z, s); }
            yy += dy;
        }
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn seagrass(f: &Features, prob: f32, reg: &mut Region, r: &mut R, x: i32, _y: i32, z: i32) -> bool {
    let i = r.next_int_bounded(8) - r.next_int_bounded(8);
    let j = r.next_int_bounded(8) - r.next_int_bounded(8);
    let (px, pz) = (x + i, z + j);
    let py = reg.height(Hm::OceanFloor, px, pz);
    let here = reg.block(px, py, pz);
    if BlockState(here as u32).name() != "water" { return false; }
    let tall = r.next_double() < prob as f64;
    let name = if tall { "tall_seagrass" } else { "seagrass" };
    let s = BlockState::from_name(name).map(|s| s.0 as u16).unwrap_or(0);
    if !f.can_survive(reg, s, px, py, pz) { return false; }
    if tall {
        if BlockState(reg.block(px, py + 1, pz) as u32).name() != "water" { return false; }
        let lower = BlockState(s as u32).with("half", "lower").map(|b| b.0 as u16).unwrap_or(s);
        let upper = BlockState(s as u32).with("half", "upper").map(|b| b.0 as u16).unwrap_or(s);
        reg.set(px, py, pz, lower);
        reg.set(px, py + 1, pz, upper);
    } else {
        reg.set(px, py, pz, s);
    }
    true
}

fn kelp(f: &Features, reg: &mut Region, r: &mut R, x: i32, _y: i32, z: i32) -> bool {
    let mut i = 0;
    let mut y = reg.height(Hm::OceanFloor, x, z);
    let water = |reg: &Region, y: i32| BlockState(reg.block(x, y, z) as u32).name() == "water";
    if !water(reg, y) { return false; }
    let kelp = BlockState::from_name("kelp").unwrap();
    let plant = BlockState::from_name("kelp_plant").map(|s| s.0 as u16).unwrap_or(0);
    let k = 1 + r.next_int_bounded(10);
    for l in 0..=k {
        if water(reg, y) && water(reg, y + 1) && f.can_survive(reg, plant, x, y, z) {
            if l == k {
                let age = r.next_int_bounded(4) + 20;
                reg.set(x, y, z, kelp.with("age", &age.to_string()).map(|b| b.0 as u16).unwrap_or(kelp.0 as u16));
                i += 1;
            } else {
                reg.set(x, y, z, plant);
            }
        } else if l > 0 {
            if f.can_survive(reg, kelp.0 as u16, x, y - 1, z) && BlockState(reg.block(x, y - 2, z) as u32).name() != "kelp" {
                let age = r.next_int_bounded(4) + 20;
                reg.set(x, y - 1, z, kelp.with("age", &age.to_string()).map(|b| b.0 as u16).unwrap_or(kelp.0 as u16));
                i += 1;
            }
            break;
        }
        y += 1;
    }
    i > 0
}

fn vines(_f: &Features, reg: &mut Region, x: i32, y: i32, z: i32) -> bool {
    if !flag(reg.block(x, y, z), F_AIR) { return false; }
    let vine = BlockState::from_name("vine").unwrap();
    for (d, (dx, dy, dz)) in [("up", (0, 1, 0)), ("north", (0, 0, -1)), ("south", (0, 0, 1)), ("west", (-1, 0, 0)), ("east", (1, 0, 0))] {
        let n = reg.block(x + dx, y + dy, z + dz);
        if flag(n, F_FULL) {
            if let Some(s) = vine.with(d, "true") { reg.set(x, y, z, s.0 as u16); return true; }
        }
    }
    false
}

pub fn is_water(s: u16) -> bool { flag(s, F_WATER) }

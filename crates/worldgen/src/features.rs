//! Feature decoration (currently: the ore features of the UNDERGROUND_ORES step).
//!
//! Placed features are ordered globally per step the same way for every chunk (a
//! topological merge of each biome's feature lists), and every placed feature gets its
//! own random seeded from the chunk's decoration seed, its global index and its step.

use crate::carver::{resolve_tag, Trig};
use crate::random::{set_decoration_seed, set_feature_seed, BitXoroshiro, Rng};
use crate::state_from_json;
use anyhow::{anyhow, bail, Result};
use basalt_data::BlockState;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

pub const ORE_STEP: usize = 6;

#[derive(Debug, Clone)]
pub enum IntProvider {
    Constant(i32),
    Uniform(i32, i32),
}

impl IntProvider {
    fn parse(v: &Value) -> Result<Self> {
        if let Some(i) = v.as_i64() { return Ok(IntProvider::Constant(i as i32)); }
        match v["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
            "constant" => Ok(IntProvider::Constant(v["value"].as_i64().unwrap_or(0) as i32)),
            "uniform" => Ok(IntProvider::Uniform(v["min_inclusive"].as_i64().unwrap_or(0) as i32, v["max_inclusive"].as_i64().unwrap_or(0) as i32)),
            t => bail!("unsupported int provider {t}"),
        }
    }
    fn sample<R: Rng>(&self, r: &mut R) -> i32 {
        match *self {
            IntProvider::Constant(c) => c,
            IntProvider::Uniform(a, b) => r.next_int_bounded(b - a + 1) + a,
        }
    }
}

fn anchor(v: &Value) -> Result<i32> {
    if let Some(a) = v.get("absolute").and_then(|x| x.as_i64()) { return Ok(a as i32); }
    if let Some(a) = v.get("above_bottom").and_then(|x| x.as_i64()) { return Ok(crate::MIN_Y + a as i32); }
    if let Some(a) = v.get("below_top").and_then(|x| x.as_i64()) { return Ok(crate::MIN_Y + crate::HEIGHT - 1 - a as i32); }
    bail!("bad anchor {v}")
}

#[derive(Debug, Clone)]
pub enum Height {
    Uniform(i32, i32),
    Trapezoid(i32, i32, i32),
}

impl Height {
    fn parse(v: &Value) -> Result<Self> {
        let lo = anchor(&v["min_inclusive"])?;
        let hi = anchor(&v["max_inclusive"])?;
        match v["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
            "uniform" => Ok(Height::Uniform(lo, hi)),
            "trapezoid" => Ok(Height::Trapezoid(lo, hi, v.get("plateau").and_then(|p| p.as_i64()).unwrap_or(0) as i32)),
            t => bail!("unsupported height provider {t}"),
        }
    }
    fn sample<R: Rng>(&self, r: &mut R) -> i32 {
        match *self {
            Height::Uniform(i, j) => if i > j { i } else { r.next_int_bounded(j - i + 1) + i },
            Height::Trapezoid(i, j, plateau) => {
                if i > j { return i; }
                let k = j - i;
                if plateau >= k { return r.next_int_bounded(j - i + 1) + i; }
                let l = (k - plateau) / 2;
                let m = k - l;
                i + r.next_int_bounded(m + 1) + r.next_int_bounded(l + 1)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum Modifier {
    Count(IntProvider),
    InSquare,
    HeightRange(Height),
    Biome,
    Rarity(i32),
    /// true: OCEAN_FLOOR_WG, false: WORLD_SURFACE_WG
    Heightmap(bool),
    RandomOffset(IntProvider, IntProvider),
    Filter(Pred),
}

/// Block predicates.
#[derive(Debug, Clone)]
pub enum Pred {
    True,
    MatchingBlocks((i32, i32, i32), HashSet<String>),
    MatchingFluids((i32, i32, i32), bool, bool),
    Solid((i32, i32, i32)),
    Not(Box<Pred>),
    AnyOf(Vec<Pred>),
    AllOf(Vec<Pred>),
}

fn names_of(v: &Value) -> HashSet<String> {
    match v {
        Value::String(s) => [s.trim_start_matches("minecraft:").to_string()].into_iter().collect(),
        Value::Array(a) => a.iter().filter_map(|x| x.as_str()).map(|s| s.trim_start_matches("minecraft:").to_string()).collect(),
        _ => HashSet::new(),
    }
}

impl Pred {
    fn parse(v: &Value) -> Result<Pred> {
        let off = v.get("offset").and_then(|o| o.as_array()).map(|a| {
            let g = |i: usize| a.get(i).and_then(|x| x.as_i64()).unwrap_or(0) as i32;
            (g(0), g(1), g(2))
        }).unwrap_or((0, 0, 0));
        Ok(match v["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
            "true" => Pred::True,
            "matching_blocks" => Pred::MatchingBlocks(off, names_of(&v["blocks"])),
            "matching_fluids" => {
                let n = names_of(&v["fluids"]);
                Pred::MatchingFluids(off, n.contains("water") || n.contains("flowing_water"), n.contains("lava") || n.contains("flowing_lava"))
            }
            "solid" => Pred::Solid(off),
            "not" => Pred::Not(Box::new(Pred::parse(&v["predicate"])?)),
            "any_of" => Pred::AnyOf(v["predicates"].as_array().ok_or_else(|| anyhow!("predicates"))?.iter().map(Pred::parse).collect::<Result<_>>()?),
            "all_of" => Pred::AllOf(v["predicates"].as_array().ok_or_else(|| anyhow!("predicates"))?.iter().map(Pred::parse).collect::<Result<_>>()?),
            t => bail!("unsupported block predicate {t}"),
        })
    }

    /// `approx`: estimate unknown blocks outside the chunk instead of failing.
    fn test(&self, w: &mut dyn OreWorld, x: i32, y: i32, z: i32, approx: bool) -> bool {
        let get = |w: &mut dyn OreWorld, o: (i32, i32, i32)| {
            let (px, py, pz) = (x + o.0, y + o.1, z + o.2);
            match w.get(px, py, pz) { Some(b) => Some(b), None if approx => Some(w.get_approx(px, py, pz)), None => None }
        };
        match self {
            Pred::True => true,
            Pred::MatchingBlocks(o, set) => get(w, *o).map(|b| set.contains(BlockState(b as u32).name())).unwrap_or(false),
            Pred::MatchingFluids(o, water, lava) => get(w, *o).map(|b| {
                let st = BlockState(b as u32);
                let wl = st.get("waterlogged") == Some("true");
                (*water && (st.name() == "water" || wl)) || (*lava && st.name() == "lava")
            }).unwrap_or(false),
            Pred::Solid(o) => get(w, *o).map(|b| { let st = BlockState(b as u32); !st.is_air() && !matches!(st.name(), "water" | "lava") }).unwrap_or(false),
            Pred::Not(p) => !p.test(w, x, y, z, approx),
            Pred::AnyOf(ps) => ps.iter().any(|p| p.test(w, x, y, z, approx)),
            Pred::AllOf(ps) => ps.iter().all(|p| p.test(w, x, y, z, approx)),
        }
    }
}

#[derive(Debug)]
pub struct Disk {
    half_height: i32,
    radius: IntProvider,
    rules: Vec<(Pred, u16)>,
    fallback: u16,
    target: Pred,
}

#[derive(Debug)]
pub enum Kind {
    Ore(Ore),
    Disk(Disk),
}

fn simple_state(v: &Value) -> Result<u16> {
    match v["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
        "simple_state_provider" => state_from_json(&v["state"]),
        t => bail!("unsupported state provider {t}"),
    }
}

#[derive(Debug)]
pub struct Ore {
    size: i32,
    discard: f32,
    /// (replaceable state set, result state)
    targets: Vec<(usize, u16)>,
}

#[derive(Debug)]
pub struct PlacedOre {
    modifiers: Vec<Modifier>,
    kind: Kind,
}

pub struct Features {
    /// Per step: global placed feature names in decoration order.
    pub steps: Vec<Vec<String>>,
    /// Per biome, per step: global indices of the biome's features.
    biome_steps: Vec<Vec<Vec<u32>>>,
    /// Per biome: set of (step, index) present.
    biome_has: Vec<HashSet<(u32, u32)>>,
    /// Supported ore features of the ore step, by global index.
    ores: HashMap<u32, PlacedOre>,
    /// Replaceable block-state sets used by ore targets.
    sets: Vec<Vec<bool>>,
    /// State returned by `OreWorld::get_approx` for solid ground.
    pub approx_stone: u16,
}

/// Something that ore placement can read and write.
pub trait OreWorld {
    /// Block at a world position, or None if unknown (outside the chunk being built).
    fn get(&self, x: i32, y: i32, z: i32) -> Option<u16>;
    /// Inclusive x/z block bounds of the writable area (min x, max x, min z, max z).
    fn bounds(&self) -> (i32, i32, i32, i32);
    fn set(&mut self, x: i32, y: i32, z: i32, state: u16);
    /// OCEAN_FLOOR_WG height of a column (possibly approximated outside the chunk).
    fn ocean_floor(&mut self, x: i32, z: i32) -> i32;
    /// Zoomed biome at a block position.
    fn biome(&mut self, x: i32, y: i32, z: i32) -> u8;
    /// Best guess for a block outside the chunk (used by placement filters only).
    fn get_approx(&mut self, x: i32, y: i32, z: i32) -> u16;
    /// WORLD_SURFACE_WG height of a column.
    fn world_surface(&mut self, x: i32, z: i32) -> i32 { self.ocean_floor(x, z) }
}

impl Features {
    /// `biome_order`: biome ids in the order the biome source lists them.
    pub fn load(data: &Path, biome_names: &[String], biome_order: &[u8]) -> Result<Features> {
        let wg = data.join("worldgen");
        let mut lists: Vec<Vec<Vec<String>>> = Vec::new();
        for n in biome_names {
            let v: Value = serde_json::from_str(&std::fs::read_to_string(wg.join("biome").join(format!("{n}.json")))?)?;
            let steps = v["features"].as_array().map(|a| a.iter().map(|s| match s {
                Value::Array(x) => x.iter().filter_map(|f| f.as_str().map(|f| f.to_string())).collect(),
                Value::String(f) => vec![f.clone()],
                _ => Vec::new(),
            }).collect()).unwrap_or_default();
            lists.push(steps);
        }
        // Topological merge (feature identity = name; tie-break by step, then first-seen order).
        let mut first_seen: HashMap<String, usize> = HashMap::new();
        let mut graph: BTreeMap<(usize, usize), BTreeSet<(usize, usize)>> = BTreeMap::new();
        let mut names: HashMap<(usize, usize), String> = HashMap::new();
        let mut nsteps = 0;
        for &b in biome_order {
            let steps = &lists[b as usize];
            nsteps = nsteps.max(steps.len());
            let mut seq = Vec::new();
            for (j, st) in steps.iter().enumerate() {
                for f in st {
                    let n = first_seen.len();
                    let idx = *first_seen.entry(f.clone()).or_insert(n);
                    names.insert((j, idx), f.clone());
                    seq.push((j, idx));
                }
            }
            for k in 0..seq.len() {
                let e = graph.entry(seq[k]).or_default();
                if k + 1 < seq.len() { e.insert(seq[k + 1]); }
            }
        }
        let mut visited = BTreeSet::new();
        let mut out = Vec::new();
        fn dfs(g: &BTreeMap<(usize, usize), BTreeSet<(usize, usize)>>, visited: &mut BTreeSet<(usize, usize)>, inprog: &mut BTreeSet<(usize, usize)>, out: &mut Vec<(usize, usize)>, n: (usize, usize)) -> bool {
            if visited.contains(&n) { return false; }
            if inprog.contains(&n) { return true; }
            inprog.insert(n);
            if let Some(ch) = g.get(&n) {
                for c in ch { if dfs(g, visited, inprog, out, *c) { return true; } }
            }
            inprog.remove(&n);
            visited.insert(n);
            out.push(n);
            false
        }
        let keys: Vec<(usize, usize)> = graph.keys().copied().collect();
        for k in keys {
            let mut inprog = BTreeSet::new();
            if visited.contains(&k) { continue; }
            if dfs(&graph, &mut visited, &mut inprog, &mut out, k) { bail!("feature order cycle"); }
        }
        out.reverse();
        let mut steps: Vec<Vec<String>> = vec![Vec::new(); nsteps];
        for n in &out { steps[n.0].push(names[n].clone()); }
        let index: Vec<HashMap<&str, u32>> = steps.iter().map(|s| s.iter().enumerate().map(|(i, n)| (n.as_str(), i as u32)).collect()).collect();
        let mut biome_steps = Vec::new();
        let mut biome_has = Vec::new();
        for l in &lists {
            let mut bs = Vec::new();
            let mut has = HashSet::new();
            for (j, st) in l.iter().enumerate() {
                let v: Vec<u32> = st.iter().filter_map(|f| index[j].get(f.as_str()).copied()).collect();
                for i in &v { has.insert((j as u32, *i)); }
                bs.push(v);
            }
            biome_steps.push(bs);
            biome_has.push(has);
        }
        let mut f = Features { steps, biome_steps, biome_has, ores: HashMap::new(), sets: Vec::new(), approx_stone: BlockState::from_name("stone").map(|s| s.0 as u16).unwrap_or(1) };
        let mut set_idx: HashMap<String, usize> = HashMap::new();
        if f.steps.len() > ORE_STEP {
            for (i, name) in f.steps[ORE_STEP].clone().iter().enumerate() {
                if let Ok(Some(p)) = f.load_ore(data, name, &mut set_idx) { f.ores.insert(i as u32, p); }
            }
        }
        Ok(f)
    }

    fn load_ore(&mut self, data: &Path, name: &str, sets: &mut HashMap<String, usize>) -> Result<Option<PlacedOre>> {
        let wg = data.join("worldgen");
        let pn = name.trim_start_matches("minecraft:");
        let pv: Value = serde_json::from_str(&std::fs::read_to_string(wg.join("placed_feature").join(format!("{pn}.json")))?)?;
        let cv: Value = match &pv["feature"] {
            Value::String(s) => serde_json::from_str(&std::fs::read_to_string(wg.join("configured_feature").join(format!("{}.json", s.trim_start_matches("minecraft:"))))?)?,
            v => v.clone(),
        };
        let ty = cv["type"].as_str().unwrap_or("").trim_start_matches("minecraft:").to_string();
        if ty != "ore" && ty != "disk" { return Ok(None); }
        let mut modifiers = Vec::new();
        for m in pv["placement"].as_array().ok_or_else(|| anyhow!("placement"))? {
            modifiers.push(match m["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
                "count" => Modifier::Count(IntProvider::parse(&m["count"])?),
                "in_square" => Modifier::InSquare,
                "height_range" => Modifier::HeightRange(Height::parse(&m["height"])?),
                "biome" => Modifier::Biome,
                "rarity_filter" => Modifier::Rarity(m["chance"].as_i64().unwrap_or(1) as i32),
                "heightmap" => match m["heightmap"].as_str() {
                    Some("OCEAN_FLOOR_WG") => Modifier::Heightmap(true),
                    Some("WORLD_SURFACE_WG") => Modifier::Heightmap(false),
                    _ => return Ok(None),
                },
                "random_offset" => Modifier::RandomOffset(IntProvider::parse(&m["xz_spread"])?, IntProvider::parse(&m["y_spread"])?),
                "block_predicate_filter" => Modifier::Filter(Pred::parse(&m["predicate"])?),
                _ => return Ok(None),
            });
        }
        let c = &cv["config"];
        if ty == "disk" {
            let sp = &c["state_provider"];
            let (rules, fallback) = match sp["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
                "rule_based_state_provider" => {
                    let mut rules = Vec::new();
                    for r in sp["rules"].as_array().ok_or_else(|| anyhow!("rules"))? {
                        rules.push((Pred::parse(&r["if_true"])?, simple_state(&r["then"])?));
                    }
                    (rules, simple_state(&sp["fallback"])?)
                }
                _ => (Vec::new(), simple_state(sp)?),
            };
            return Ok(Some(PlacedOre {
                modifiers,
                kind: Kind::Disk(Disk {
                    half_height: c["half_height"].as_i64().unwrap_or(0) as i32,
                    radius: IntProvider::parse(&c["radius"])?,
                    rules, fallback,
                    target: Pred::parse(&c["target"])?,
                }),
            }));
        }
        let mut targets = Vec::new();
        for t in c["targets"].as_array().ok_or_else(|| anyhow!("targets"))? {
            let state = state_from_json(&t["state"])?;
            let tg = &t["target"];
            let key = match tg["predicate_type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
                "tag_match" => format!("#{}", tg["tag"].as_str().unwrap_or("")),
                "block_match" => tg["block"].as_str().unwrap_or("").to_string(),
                _ => return Ok(None),
            };
            let si = match sets.get(&key) {
                Some(i) => *i,
                None => {
                    let mut names = HashSet::new();
                    if key.starts_with('#') { resolve_tag(&data.join("tags/block"), &key, &mut names)?; } else { names.insert(key.trim_start_matches("minecraft:").to_string()); }
                    self.sets.push((0..basalt_data::blocks::STATE_COUNT as u32).map(|i| names.contains(BlockState(i).name())).collect());
                    sets.insert(key, self.sets.len() - 1);
                    self.sets.len() - 1
                }
            };
            targets.push((si, state));
        }
        Ok(Some(PlacedOre {
            modifiers,
            kind: Kind::Ore(Ore {
                size: c["size"].as_i64().unwrap_or(0) as i32,
                discard: c["discard_chance_on_air_exposure"].as_f64().unwrap_or(0.0) as f32,
                targets,
            }),
        }))
    }

    /// Run the ore step for the chunk (ncx, ncz). `biomes` is the set of biomes present
    /// in that chunk and its 8 neighbours.
    pub fn decorate_ores(&self, trig: &Trig, seed: i64, ncx: i32, ncz: i32, biomes: &[u8], w: &mut dyn OreWorld) {
        let mut r = BitXoroshiro::new(0);
        let deco = set_decoration_seed(&mut r, seed, ncx * 16, ncz * 16);
        let mut idx: BTreeSet<u32> = BTreeSet::new();
        for b in biomes {
            if let Some(st) = self.biome_steps[*b as usize].get(ORE_STEP) { idx.extend(st.iter().copied()); }
        }
        for p in idx {
            let Some(po) = self.ores.get(&p) else { continue };
            set_feature_seed(&mut r, deco, p as i32, ORE_STEP as i32);
            self.place(trig, po, p, &mut r, 0, ncx * 16, crate::MIN_Y, ncz * 16, w);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn place(&self, trig: &Trig, po: &PlacedOre, p: u32, r: &mut BitXoroshiro, mi: usize, x: i32, y: i32, z: i32, w: &mut dyn OreWorld) {
        if mi == po.modifiers.len() {
            match &po.kind {
                Kind::Ore(o) => self.ore(trig, o, r, x, y, z, w),
                Kind::Disk(d) => disk(d, r, x, y, z, w),
            }
            return;
        }
        match &po.modifiers[mi] {
            Modifier::Count(c) => {
                let n = c.sample(r);
                for _ in 0..n { self.place(trig, po, p, r, mi + 1, x, y, z, w); }
            }
            Modifier::InSquare => {
                let nx = r.next_int_bounded(16) + x;
                let nz = r.next_int_bounded(16) + z;
                self.place(trig, po, p, r, mi + 1, nx, y, nz, w);
            }
            Modifier::HeightRange(h) => {
                let ny = h.sample(r);
                self.place(trig, po, p, r, mi + 1, x, ny, z, w);
            }
            Modifier::Biome => {
                let b = w.biome(x, y, z);
                if self.biome_has[b as usize].contains(&(ORE_STEP as u32, p)) { self.place(trig, po, p, r, mi + 1, x, y, z, w); }
            }
            Modifier::Rarity(c) => {
                if r.next_float() < 1.0 / *c as f32 { self.place(trig, po, p, r, mi + 1, x, y, z, w); }
            }
            Modifier::Heightmap(ocean) => {
                let h = if *ocean { w.ocean_floor(x, z) } else { w.world_surface(x, z) };
                if h > crate::MIN_Y { self.place(trig, po, p, r, mi + 1, x, h, z, w); }
            }
            Modifier::RandomOffset(xz, ys) => {
                let nx = x + xz.sample(r);
                let ny = y + ys.sample(r);
                let nz = z + xz.sample(r);
                self.place(trig, po, p, r, mi + 1, nx, ny, nz, w);
            }
            Modifier::Filter(pr) => {
                if pr.test(w, x, y, z, true) { self.place(trig, po, p, r, mi + 1, x, y, z, w); }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn ore(&self, trig: &Trig, c: &Ore, r: &mut BitXoroshiro, ox: i32, oy: i32, oz: i32, w: &mut dyn OreWorld) {
        let f = r.next_float() * std::f32::consts::PI;
        let g = c.size as f32 / 8.0;
        let i = ((c.size as f32 / 16.0 * 2.0 + 1.0) / 2.0).ceil() as i32;
        let gs = (f as f64).sin() * g as f64;
        let gc = (f as f64).cos() * g as f64;
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
                if o <= w.ocean_floor(s, t) { ok = true; break 'outer; }
            }
        }
        if !ok { return; }
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
        // Visited-position set, indexed like vanilla (indices may alias, which is kept).
        let mut maxi = 0i64;
        for t in 0..k {
            let rad = ds[t * 4 + 3];
            if rad < 0.0 { continue; }
            let a = ((ds[t * 4] + rad).floor() as i32).max(n) - n;
            let b = ((ds[t * 4 + 1] + rad).floor() as i32).max(o) - o;
            let c2 = ((ds[t * 4 + 2] + rad).floor() as i32).max(p) - p;
            maxi = maxi.max(a as i64 + b as i64 * width as i64 + c2 as i64 * (width * height) as i64);
        }
        let mut bits = vec![false; maxi as usize + 1];
        let no_rng = c.discard <= 0.0 || c.discard >= 1.0;
        let (bx0, bx1, bz0, bz1) = w.bounds();
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
            // Spheres that cannot touch the chunk and draw no randoms can be skipped.
            if no_rng && (aa < bx0 || u > bx1 || ac < bz0 || ww > bz1) { continue; }
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
                        let cur = w.get(ad, af, ah);
                        // Unknown blocks (outside the chunk) are estimated; they only matter
                        // for the randoms the air-exposure check draws.
                        let test = match cur { Some(b) => b, None if no_rng => continue, None => w.get_approx(ad, af, ah) };
                        for (si, st) in &c.targets {
                            let pass = self.sets[*si][test as usize] || (cur.is_none() && test == self.approx_stone);
                            if !pass { continue; }
                            let place = if c.discard <= 0.0 { true } else if c.discard >= 1.0 { false } else { r.next_float() >= c.discard }
                                || !adjacent_air(w, ad, af, ah);
                            if !place { continue; }
                            if cur.is_some() { w.set(ad, af, ah, *st); }
                            break;
                        }
                    }
                }
            }
        }
    }
}

fn disk<R: Rng>(d: &Disk, r: &mut R, ox: i32, oy: i32, oz: i32, w: &mut dyn OreWorld) {
    let top = oy + d.half_height;
    let bottom = oy - d.half_height - 1;
    let l = d.radius.sample(r);
    for z in oz - l..=oz + l {
        for x in ox - l..=ox + l {
            let (n, o) = (x - ox, z - oz);
            if n * n + o * o > l * l { continue; }
            let mut k = top;
            while k > bottom {
                if d.target.test(w, x, k, z, false) {
                    let st = d.rules.iter().find(|(p, _)| p.test(w, x, k, z, false)).map(|(_, s)| *s).unwrap_or(d.fallback);
                    w.set(x, k, z, st);
                }
                k -= 1;
            }
        }
    }
}

fn adjacent_air(w: &dyn OreWorld, x: i32, y: i32, z: i32) -> bool {
    for (dx, dy, dz) in [(0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1), (-1, 0, 0), (1, 0, 0)] {
        if let Some(b) = w.get(x + dx, y + dy, z + dz) {
            if BlockState(b as u32).is_air() { return true; }
        }
    }
    false
}

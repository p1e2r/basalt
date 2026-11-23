//! Feature decoration driven by placed_feature / configured_feature JSON.
//!
//! Placed features are ordered globally per step the same way for every chunk (a
//! topological merge of each biome's feature lists), and every placed feature gets its
//! own random seeded from the chunk's decoration seed, its global index and its step.

pub mod kinds;
pub mod provider;
pub mod tree;

use crate::carver::resolve_tag;
use crate::random::{set_decoration_seed, set_feature_seed, BitXoroshiro, Rng};
use crate::region::{flag, Hm, Region, F_AIR, F_FLUID, F_FULL, F_LAVA, F_MOTION, F_WATER};
use crate::state_from_json;
use anyhow::{anyhow, bail, Result};
use basalt_data::BlockState;
use provider::IntProvider;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub type R = BitXoroshiro;

pub fn anchor(v: &Value) -> Result<i32> {
    if let Some(a) = v.get("absolute").and_then(|x| x.as_i64()) { return Ok(a as i32); }
    if let Some(a) = v.get("above_bottom").and_then(|x| x.as_i64()) { return Ok(crate::MIN_Y + a as i32); }
    if let Some(a) = v.get("below_top").and_then(|x| x.as_i64()) { return Ok(crate::MIN_Y + crate::HEIGHT - 1 - a as i32); }
    bail!("bad anchor {v}")
}

#[derive(Debug, Clone)]
pub enum Height {
    Constant(i32),
    Uniform(i32, i32),
    Trapezoid(i32, i32, i32),
    BiasedToBottom(i32, i32, i32),
    VeryBiasedToBottom(i32, i32, i32),
}

impl Height {
    fn parse(v: &Value) -> Result<Self> {
        if v.get("type").is_none() { return Ok(Height::Constant(anchor(v)?)); }
        let ty = v["type"].as_str().unwrap_or("").trim_start_matches("minecraft:");
        if ty == "constant" { return Ok(Height::Constant(anchor(&v["value"])?)); }
        let lo = anchor(&v["min_inclusive"])?;
        let hi = anchor(&v["max_inclusive"])?;
        let inner = v.get("inner").and_then(|p| p.as_i64()).unwrap_or(1) as i32;
        match ty {
            "uniform" => Ok(Height::Uniform(lo, hi)),
            "trapezoid" => Ok(Height::Trapezoid(lo, hi, v.get("plateau").and_then(|p| p.as_i64()).unwrap_or(0) as i32)),
            "biased_to_bottom" => Ok(Height::BiasedToBottom(lo, hi, inner)),
            "very_biased_to_bottom" => Ok(Height::VeryBiasedToBottom(lo, hi, inner)),
            t => bail!("unsupported height provider {t}"),
        }
    }
    fn sample(&self, r: &mut R) -> i32 {
        match *self {
            Height::Constant(c) => c,
            Height::Uniform(i, j) => if i > j { i } else { r.next_int_bounded(j - i + 1) + i },
            Height::Trapezoid(i, j, plateau) => {
                if i > j { return i; }
                let k = j - i;
                if plateau >= k { return r.next_int_bounded(j - i + 1) + i; }
                let l = (k - plateau) / 2;
                let m = k - l;
                i + r.next_int_bounded(m + 1) + r.next_int_bounded(l + 1)
            }
            Height::BiasedToBottom(i, j, inner) => {
                if j - i - inner + 1 <= 0 { return i; }
                let k = r.next_int_bounded(j - i - inner + 1);
                r.next_int_bounded(k + inner) + i
            }
            Height::VeryBiasedToBottom(i, j, inner) => {
                if j - i - inner + 1 <= 0 { return i; }
                let k = r.next_int_between(i + inner, j);
                let l = r.next_int_between(i, k - 1);
                r.next_int_between(i, l - 1 + inner)
            }
        }
    }
}

/// Block predicates.
#[derive(Debug, Clone)]
pub enum Pred {
    True,
    Blocks((i32, i32, i32), Arc<Vec<bool>>),
    Fluids((i32, i32, i32), bool, bool),
    Solid((i32, i32, i32)),
    Replaceable((i32, i32, i32)),
    WouldSurvive((i32, i32, i32), u16),
    Sturdy((i32, i32, i32), usize),
    InsideWorld((i32, i32, i32)),
    Not(Box<Pred>),
    AnyOf(Vec<Pred>),
    AllOf(Vec<Pred>),
}

pub const DIRS: [(i32, i32, i32); 6] = [(0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1), (-1, 0, 0), (1, 0, 0)];
pub fn dir_index(s: &str) -> Option<usize> { ["down", "up", "north", "south", "west", "east"].iter().position(|d| *d == s) }

/// Block tag and support-rule data shared by features.
pub struct Tags {
    root: PathBuf,
    cache: Mutex<HashMap<String, Arc<Vec<bool>>>>,
}

impl Tags {
    pub fn new(data: &Path) -> Self { Tags { root: data.join("tags/block"), cache: Mutex::new(HashMap::new()) } }
    pub fn get(&self, tag: &str) -> Arc<Vec<bool>> {
        let key = tag.trim_start_matches('#').trim_start_matches("minecraft:").to_string();
        if let Some(v) = self.cache.lock().unwrap().get(&key) { return v.clone(); }
        let mut names = HashSet::new();
        let _ = resolve_tag(&self.root, &key, &mut names);
        let v = Arc::new(names_to_set(&names));
        self.cache.lock().unwrap().insert(key, v.clone());
        v
    }
}

pub fn names_to_set(names: &HashSet<String>) -> Vec<bool> {
    (0..basalt_data::blocks::STATE_COUNT as u32).map(|i| names.contains(BlockState(i).name())).collect()
}

fn names_of(v: &Value) -> HashSet<String> {
    match v {
        Value::String(s) => [s.trim_start_matches("minecraft:").to_string()].into_iter().collect(),
        Value::Array(a) => a.iter().filter_map(|x| x.as_str()).map(|s| s.trim_start_matches("minecraft:").to_string()).collect(),
        _ => HashSet::new(),
    }
}

/// Block sets referenced by a "blocks" value (block names, a tag "#..", or a list).
pub fn block_set(tags: &Tags, v: &Value) -> Arc<Vec<bool>> {
    if let Some(s) = v.as_str() { if s.starts_with('#') { return tags.get(s); } }
    let mut names = names_of(v);
    if let Some(a) = v.as_array() {
        for t in a.iter().filter_map(|x| x.as_str()).filter(|s| s.starts_with('#')) {
            let key = t.trim_start_matches('#');
            let _ = resolve_tag(&tags.root, key, &mut names);
        }
    }
    Arc::new(names_to_set(&names))
}

fn offset(v: &Value) -> (i32, i32, i32) {
    v.get("offset").and_then(|o| o.as_array()).map(|a| {
        let g = |i: usize| a.get(i).and_then(|x| x.as_i64()).unwrap_or(0) as i32;
        (g(0), g(1), g(2))
    }).unwrap_or((0, 0, 0))
}

impl Pred {
    pub fn parse(tags: &Tags, v: &Value) -> Result<Pred> {
        let off = offset(v);
        Ok(match v["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
            "true" => Pred::True,
            "matching_blocks" => Pred::Blocks(off, block_set(tags, &v["blocks"])),
            "matching_block_tag" => Pred::Blocks(off, tags.get(v["tag"].as_str().unwrap_or(""))),
            "matching_fluids" => {
                let n = names_of(&v["fluids"]);
                Pred::Fluids(off, n.contains("water") || n.contains("flowing_water"), n.contains("lava") || n.contains("flowing_lava"))
            }
            "solid" => Pred::Solid(off),
            "replaceable" => Pred::Replaceable(off),
            "would_survive" => Pred::WouldSurvive(off, state_from_json(&v["state"])?),
            "has_sturdy_face" => Pred::Sturdy(off, dir_index(v["direction"].as_str().unwrap_or("up")).unwrap_or(1)),
            "inside_world_bounds" => Pred::InsideWorld(off),
            "not" => Pred::Not(Box::new(Pred::parse(tags, &v["predicate"])?)),
            "any_of" => Pred::AnyOf(v["predicates"].as_array().ok_or_else(|| anyhow!("predicates"))?.iter().map(|p| Pred::parse(tags, p)).collect::<Result<_>>()?),
            "all_of" => Pred::AllOf(v["predicates"].as_array().ok_or_else(|| anyhow!("predicates"))?.iter().map(|p| Pred::parse(tags, p)).collect::<Result<_>>()?),
            "unobstructed" => Pred::True,
            t => bail!("unsupported block predicate {t}"),
        })
    }

    pub fn test(&self, f: &Features, reg: &mut Region, x: i32, y: i32, z: i32) -> bool {
        let at = |o: &(i32, i32, i32)| (x + o.0, y + o.1, z + o.2);
        match self {
            Pred::True => true,
            Pred::Blocks(o, set) => { let p = at(o); set[reg.block(p.0, p.1, p.2) as usize] }
            Pred::Fluids(o, water, lava) => {
                let p = at(o);
                let b = reg.block(p.0, p.1, p.2);
                (*water && flag(b, F_WATER)) || (*lava && flag(b, F_LAVA))
            }
            Pred::Solid(o) => { let p = at(o); flag(reg.block(p.0, p.1, p.2), F_MOTION) }
            Pred::Replaceable(o) => { let p = at(o); f.replaceable[reg.block(p.0, p.1, p.2) as usize] }
            Pred::WouldSurvive(o, s) => { let p = at(o); f.can_survive(reg, *s, p.0, p.1, p.2) }
            Pred::Sturdy(o, _d) => { let p = at(o); flag(reg.block(p.0, p.1, p.2), F_FULL) }
            Pred::InsideWorld(o) => { let p = at(o); (crate::MIN_Y..crate::MIN_Y + crate::HEIGHT).contains(&p.1) }
            Pred::Not(p) => !p.test(f, reg, x, y, z),
            Pred::AnyOf(ps) => ps.iter().any(|p| p.test(f, reg, x, y, z)),
            Pred::AllOf(ps) => ps.iter().all(|p| p.test(f, reg, x, y, z)),
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
    Heightmap(Hm),
    RandomOffset(IntProvider, IntProvider),
    Filter(Pred),
    SurfaceWaterDepth(i32),
    SurfaceRelative(Hm, i32, i32),
    EnvironmentScan { up: bool, max_steps: i32, target: Pred, allowed: Pred },
    NoiseThresholdCount { level: f64, below: i32, above: i32 },
    NoiseBasedCount { ratio: i32, factor: f64, offset: f64 },
    Fixed(Vec<(i32, i32, i32)>),
}

pub struct Placed {
    pub modifiers: Vec<Modifier>,
    pub feature: usize,
}

pub use kinds::Kind;

pub struct Features {
    /// Per step: global placed feature names in decoration order.
    pub steps: Vec<Vec<String>>,
    /// Per step: placed feature id of each global index.
    step_placed: Vec<Vec<usize>>,
    /// Per biome, per step: global indices of the biome's features.
    biome_steps: Vec<Vec<Vec<u32>>>,
    /// Per biome: set of (step, index) present.
    biome_has: Vec<HashSet<(u32, u32)>>,
    pub placed: Vec<Placed>,
    pub configured: Vec<Kind>,
    pub tags: Tags,
    pub replaceable: Arc<Vec<bool>>,
    /// Types of configured features that are not implemented (name → count).
    pub unsupported: BTreeMap<String, usize>,
    placed_names: HashMap<String, usize>,
    configured_names: HashMap<String, usize>,
    root: PathBuf,
    pub trig: crate::carver::Trig,
    support: HashMap<&'static str, Arc<Vec<bool>>>,
}

const SUPPORT_TAGS: &[&str] = &[
    "supports_vegetation", "supports_mangrove_propagule", "supports_azalea", "supports_dry_vegetation", "overrides_mushroom_light_requirement",
    "supports_sugar_cane", "supports_sugar_cane_adjacently", "supports_cactus", "supports_lily_pad", "cannot_support_seagrass", "cannot_support_kelp",
    "cannot_support_snow_layer", "support_override_snow_layer", "supports_crops", "supports_bamboo", "supports_small_dripleaf",
    "supports_big_dripleaf", "logs", "leaves", "substrate_overworld", "replaceable_by_mushrooms", "replaceable_by_trees", "cannot_replace_below_tree_trunk",
];

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
        type Node = (usize, usize);
        fn dfs(g: &BTreeMap<Node, BTreeSet<Node>>, visited: &mut BTreeSet<Node>, inprog: &mut BTreeSet<Node>, out: &mut Vec<Node>, n: Node) -> bool {
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
        let mut visited = BTreeSet::new();
        let mut out = Vec::new();
        let keys: Vec<Node> = graph.keys().copied().collect();
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
        let tags = Tags::new(data);
        let replaceable = tags.get("replaceable");
        let mut f = Features {
            steps: steps.clone(), step_placed: Vec::new(), biome_steps, biome_has, placed: Vec::new(), configured: Vec::new(),
            tags, replaceable, unsupported: BTreeMap::new(), placed_names: HashMap::new(), configured_names: HashMap::new(),
            root: wg, trig: crate::carver::Trig::new(), support: HashMap::new(),
        };
        for t in SUPPORT_TAGS { let v = f.tags.get(t); f.support.insert(t, v); }
        for st in &steps {
            let mut v = Vec::new();
            for name in st { v.push(f.placed_ref(&Value::String(name.clone()))); }
            f.step_placed.push(v);
        }
        Ok(f)
    }

    fn read(&self, dir: &str, name: &str) -> Result<Value> {
        let p = self.root.join(dir).join(format!("{}.json", name.trim_start_matches("minecraft:")));
        Ok(serde_json::from_str(&std::fs::read_to_string(&p).map_err(|e| anyhow!("{}: {e}", p.display()))?)?)
    }

    /// Placed feature by name or inline object.
    pub fn placed_ref(&mut self, v: &Value) -> usize {
        if let Some(n) = v.as_str() {
            if let Some(i) = self.placed_names.get(n) { return *i; }
            let pv = self.read("placed_feature", n).unwrap_or(Value::Null);
            let i = self.placed_value(&pv);
            self.placed_names.insert(n.to_string(), i);
            return i;
        }
        self.placed_value(v)
    }

    fn placed_value(&mut self, v: &Value) -> usize {
        let feature = self.configured_ref(&v["feature"]);
        let mut modifiers = Vec::new();
        let mut ok = true;
        for m in v["placement"].as_array().cloned().unwrap_or_default() {
            match self.modifier(&m) {
                Ok(md) => modifiers.push(md),
                Err(e) => { ok = false; *self.unsupported.entry(format!("placement: {e}")).or_default() += 1; }
            }
        }
        let feature = if ok { feature } else { self.add_configured(Kind::Unsupported) };
        self.placed.push(Placed { modifiers, feature });
        self.placed.len() - 1
    }

    fn add_configured(&mut self, k: Kind) -> usize { self.configured.push(k); self.configured.len() - 1 }

    /// Configured feature by name or inline object.
    pub fn configured_ref(&mut self, v: &Value) -> usize {
        if let Some(n) = v.as_str() {
            if let Some(i) = self.configured_names.get(n) { return *i; }
            // Reserve the slot first (features can be shared).
            let cv = self.read("configured_feature", n).unwrap_or(Value::Null);
            let i = self.configured_value(&cv);
            self.configured_names.insert(n.to_string(), i);
            return i;
        }
        self.configured_value(v)
    }

    fn configured_value(&mut self, v: &Value) -> usize {
        let ty = v["type"].as_str().unwrap_or("").trim_start_matches("minecraft:").to_string();
        let k = match kinds::parse(self, &ty, &v["config"]) {
            Ok(k) => k,
            Err(e) => { *self.unsupported.entry(format!("{ty}: {e}")).or_default() += 1; Kind::Unsupported }
        };
        self.add_configured(k)
    }

    fn modifier(&mut self, m: &Value) -> Result<Modifier> {
        Ok(match m["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
            "count" => Modifier::Count(IntProvider::parse(&m["count"])?),
            "in_square" => Modifier::InSquare,
            "height_range" => Modifier::HeightRange(Height::parse(&m["height"])?),
            "biome" => Modifier::Biome,
            "rarity_filter" => Modifier::Rarity(m["chance"].as_i64().unwrap_or(1) as i32),
            "heightmap" => Modifier::Heightmap(Hm::parse(m["heightmap"].as_str().unwrap_or("")).ok_or_else(|| anyhow!("heightmap"))?),
            "random_offset" => Modifier::RandomOffset(IntProvider::parse(&m["xz_spread"])?, IntProvider::parse(&m["y_spread"])?),
            "block_predicate_filter" => Modifier::Filter(Pred::parse(&self.tags, &m["predicate"])?),
            "surface_water_depth_filter" => Modifier::SurfaceWaterDepth(m["max_water_depth"].as_i64().unwrap_or(0) as i32),
            "surface_relative_threshold_filter" => Modifier::SurfaceRelative(
                Hm::parse(m["heightmap"].as_str().unwrap_or("")).ok_or_else(|| anyhow!("heightmap"))?,
                m.get("min_inclusive").and_then(|x| x.as_i64()).unwrap_or(i32::MIN as i64) as i32,
                m.get("max_inclusive").and_then(|x| x.as_i64()).unwrap_or(i32::MAX as i64) as i32,
            ),
            "environment_scan" => Modifier::EnvironmentScan {
                up: m["direction_of_search"].as_str() == Some("up"),
                max_steps: m["max_steps"].as_i64().unwrap_or(1) as i32,
                target: Pred::parse(&self.tags, &m["target_condition"])?,
                allowed: if m.get("allowed_search_condition").is_some() { Pred::parse(&self.tags, &m["allowed_search_condition"])? } else { Pred::True },
            },
            "noise_threshold_count" => Modifier::NoiseThresholdCount {
                level: m["noise_level"].as_f64().unwrap_or(0.0), below: m["below_noise"].as_i64().unwrap_or(0) as i32,
                above: m["above_noise"].as_i64().unwrap_or(0) as i32,
            },
            "noise_based_count" => Modifier::NoiseBasedCount {
                ratio: m["noise_to_count_ratio"].as_i64().unwrap_or(0) as i32, factor: m["noise_factor"].as_f64().unwrap_or(1.0),
                offset: m.get("noise_offset").and_then(|x| x.as_f64()).unwrap_or(0.0),
            },
            "fixed_placement" => Modifier::Fixed(m["positions"].as_array().map(|a| a.iter().filter_map(|p| {
                let p = p.as_array()?;
                Some((p[0].as_i64()? as i32, p[1].as_i64()? as i32, p[2].as_i64()? as i32))
            }).collect()).unwrap_or_default()),
            t => bail!("{t}"),
        })
    }

    /// Run all decoration steps for chunk (ncx, ncz). `biomes`: biome ids present in
    /// that chunk and its 8 neighbours. The region's writable area must be set by the caller.
    pub fn decorate(&self, seed: i64, ncx: i32, ncz: i32, biomes: &[u8], reg: &mut Region) {
        let mut r = BitXoroshiro::new(0);
        let deco = set_decoration_seed(&mut r, seed, ncx * 16, ncz * 16);
        for step in 0..self.steps.len() {
            let mut idx: BTreeSet<u32> = BTreeSet::new();
            for b in biomes {
                if let Some(st) = self.biome_steps[*b as usize].get(step) { idx.extend(st.iter().copied()); }
            }
            for p in idx {
                let pid = self.step_placed[step][p as usize];
                if matches!(self.configured[self.placed[pid].feature], Kind::Unsupported) { continue; }
                set_feature_seed(&mut r, deco, p as i32, step as i32);
                self.place_placed(reg, &mut r, pid, Some((step as u32, p)), ncx * 16, crate::MIN_Y, ncz * 16);
            }
        }
    }

    /// Place a placed feature at an origin; returns whether anything was placed.
    #[allow(clippy::too_many_arguments)]
    pub fn place_placed(&self, reg: &mut Region, r: &mut R, pid: usize, top: Option<(u32, u32)>, x: i32, y: i32, z: i32) -> bool {
        self.modifiers(reg, r, pid, top, 0, x, y, z)
    }

    #[allow(clippy::too_many_arguments)]
    fn modifiers(&self, reg: &mut Region, r: &mut R, pid: usize, top: Option<(u32, u32)>, mi: usize, x: i32, y: i32, z: i32) -> bool {
        let pf = &self.placed[pid];
        if mi == pf.modifiers.len() {
            return kinds::place(self, reg, r, pf.feature, x, y, z);
        }
        let next = |s: &Self, reg: &mut Region, r: &mut R, x, y, z| s.modifiers(reg, r, pid, top, mi + 1, x, y, z);
        match &pf.modifiers[mi] {
            Modifier::Count(c) => {
                let n = c.sample(r);
                let mut any = false;
                for _ in 0..n { any |= next(self, reg, r, x, y, z); }
                any
            }
            Modifier::InSquare => {
                let nx = r.next_int_bounded(16) + x;
                let nz = r.next_int_bounded(16) + z;
                next(self, reg, r, nx, y, nz)
            }
            Modifier::HeightRange(h) => { let ny = h.sample(r); next(self, reg, r, x, ny, z) }
            Modifier::Biome => {
                let Some(t) = top else { return false };
                let b = reg.biome(x, y, z);
                if let Some(dbg) = debug_filter() {
                    let name = &self.steps[t.0 as usize][t.1 as usize];
                    if name.contains(dbg.as_str()) {
                        eprintln!("[feat] {name} at {x} {y} {z} biome {} pass {}", reg.g.biome_names[b as usize], self.biome_has[b as usize].contains(&t));
                    }
                }
                if self.biome_has[b as usize].contains(&t) { next(self, reg, r, x, y, z) } else { false }
            }
            Modifier::Rarity(c) => if r.next_float() < 1.0 / *c as f32 { next(self, reg, r, x, y, z) } else { false },
            Modifier::Heightmap(hm) => {
                let h = reg.height(*hm, x, z);
                if h > crate::MIN_Y { next(self, reg, r, x, h, z) } else { false }
            }
            Modifier::RandomOffset(xz, ys) => {
                let nx = x + xz.sample(r);
                let ny = y + ys.sample(r);
                let nz = z + xz.sample(r);
                next(self, reg, r, nx, ny, nz)
            }
            Modifier::Filter(p) => if p.test(self, reg, x, y, z) { next(self, reg, r, x, y, z) } else { false },
            Modifier::SurfaceWaterDepth(d) => {
                let i = reg.height(Hm::OceanFloor, x, z);
                let j = reg.height(Hm::WorldSurface, x, z);
                if j - i <= *d { next(self, reg, r, x, y, z) } else { false }
            }
            Modifier::SurfaceRelative(hm, lo, hi) => {
                let h = reg.height(*hm, x, z) as i64;
                if h + *lo as i64 <= y as i64 && y as i64 <= h + *hi as i64 { next(self, reg, r, x, y, z) } else { false }
            }
            Modifier::EnvironmentScan { up, max_steps, target, allowed } => {
                let dy = if *up { 1 } else { -1 };
                let mut yy = y;
                if !allowed.test(self, reg, x, yy, z) { return false; }
                for _ in 0..*max_steps {
                    if target.test(self, reg, x, yy, z) { return next(self, reg, r, x, yy, z); }
                    yy += dy;
                    if !(crate::MIN_Y..crate::MIN_Y + crate::HEIGHT).contains(&yy) { return false; }
                    if !allowed.test(self, reg, x, yy, z) { break; }
                }
                if target.test(self, reg, x, yy, z) { next(self, reg, r, x, yy, z) } else { false }
            }
            Modifier::NoiseThresholdCount { level, below, above } => {
                let d = reg.g.surface.info_noise.get(x as f64 / 200.0, z as f64 / 200.0, false);
                let n = if d < *level { *below } else { *above };
                let mut any = false;
                for _ in 0..n { any |= next(self, reg, r, x, y, z); }
                any
            }
            Modifier::NoiseBasedCount { ratio, factor, offset } => {
                let d = reg.g.surface.info_noise.get(x as f64 / factor, z as f64 / factor, false);
                let n = ((d + offset) * *ratio as f64).ceil() as i32;
                let mut any = false;
                for _ in 0..n.max(0) { any |= next(self, reg, r, x, y, z); }
                any
            }
            Modifier::Fixed(ps) => {
                let mut any = false;
                for p in ps { any |= next(self, reg, r, p.0, p.1, p.2); }
                any
            }
        }
    }

    /// Block-specific survival rules (data-driven via the supports_* tags where possible).
    pub fn can_survive(&self, reg: &mut Region, s: u16, x: i32, y: i32, z: i32) -> bool {
        let st = BlockState(s as u32);
        let kind = st.block().kind;
        let name = st.name();
        let below = reg.block(x, y - 1, z);
        let t = |n: &str| self.tag(n)[below as usize];
        let upper_half = st.get("half") == Some("upper");
        if upper_half { return BlockState(below as u32).name() == name; }
        match kind {
            "sapling" | "flower" | "tall_flower" | "bush" | "tall_grass" | "double_plant" | "firefly_bush" | "sweet_berry_bush"
            | "flower_bed" | "leaf_litter" | "eyeblossom" => {
                if name == "mangrove_propagule" { return t("supports_mangrove_propagule"); }
                t("supports_vegetation")
            }
            "azalea" => t("supports_azalea"),
            "dry_vegetation" | "short_dry_grass" | "tall_dry_grass" => t("supports_dry_vegetation"),
            "mushroom" => t("overrides_mushroom_light_requirement") || flag(below, F_FULL),
            "sugar_cane" => {
                if BlockState(below as u32).name() == "sugar_cane" { return true; }
                if !t("supports_sugar_cane") { return false; }
                [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dz)| {
                    let n = reg.block(x + dx, y - 1, z + dz);
                    flag(n, F_WATER) || self.tag("supports_sugar_cane_adjacently")[n as usize]
                })
            }
            "cactus" => {
                if [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dz)| { let n = reg.block(x + dx, y, z + dz); flag(n, F_MOTION) || flag(n, F_LAVA) }) { return false; }
                t("supports_cactus") || BlockState(below as u32).name() == "cactus"
            }
            "lily_pad" => (flag(below, F_WATER) && BlockState(below as u32).name() == "water") || t("supports_lily_pad"),
            "seagrass" | "tall_seagrass" | "sea_pickle" | "coral_plant" | "coral_fan" => flag(below, F_FULL) && !t("cannot_support_seagrass"),
            "kelp" | "kelp_plant" => {
                let bn = BlockState(below as u32).name();
                bn == "kelp" || bn == "kelp_plant" || (flag(below, F_FULL) && !t("cannot_support_kelp"))
            }
            "snow_layer" => {
                if t("cannot_support_snow_layer") { return false; }
                t("support_override_snow_layer") || flag(below, F_FULL)
            }
            "crop" | "carrot" | "potato" | "beetroot" => t("supports_crops"),
            "bamboo_sapling" | "bamboo_stalk" => t("supports_bamboo"),
            "small_dripleaf" => t("supports_small_dripleaf"),
            "big_dripleaf" => t("supports_big_dripleaf") || BlockState(below as u32).name() == "big_dripleaf_stem",
            _ => true,
        }
    }

    /// A preloaded tag (see SUPPORT_TAGS).
    pub fn tag(&self, n: &str) -> &Vec<bool> { &self.support[n] }

    pub fn is_air(&self, s: u16) -> bool { flag(s, F_AIR) }
    pub fn is_fluid(&self, s: u16) -> bool { flag(s, F_FLUID) }
}

pub fn debug_filter() -> Option<&'static String> {
    static D: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    D.get_or_init(|| std::env::var("BASALT_DEBUG_FEATURE").ok()).as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heights() {
        let mut r = R::new(5);
        for _ in 0..200 {
            let v = Height::Trapezoid(0, 10, 2).sample(&mut r);
            assert!((0..=10).contains(&v));
        }
    }
}

//! Multi-noise biome source: climate parameters sampled from the noise router at
//! quart resolution, matched against the overworld parameter list by nearest distance.

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::path::Path;

#[inline]
pub fn quantize(v: f64) -> i64 { ((v as f32) * 10000.0f32) as i64 }

#[derive(Clone, Copy, Debug)]
pub struct Param {
    pub min: i64,
    pub max: i64,
}

impl Param {
    #[inline]
    fn dist(&self, v: i64) -> i64 {
        let l = v - self.max;
        let m = self.min - v;
        if l > 0 { l } else { m.max(0) }
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    /// temperature, humidity, continentalness, erosion, depth, weirdness
    pub p: [Param; 6],
    pub offset: i64,
    pub biome: u8,
}

pub struct BiomeSource {
    pub entries: Vec<Entry>,
    tree: Vec<TreeNode>,
    root: u32,
}

/// R-tree node: a bounding box over 7 dimensions (6 climate + offset) and either
/// children (inner node) or an entry index (leaf).
#[derive(Clone, Debug)]
struct TreeNode {
    space: [Param; 7],
    children: Vec<u32>,
    leaf: Option<u32>,
}

const CHILDREN_PER_NODE: usize = 6;

fn bounds(nodes: &[TreeNode], ids: &[u32]) -> [Param; 7] {
    let mut sp = [Param { min: i64::MAX, max: i64::MIN }; 7];
    for &i in ids {
        for d in 0..7 {
            sp[d].min = sp[d].min.min(nodes[i as usize].space[d].min);
            sp[d].max = sp[d].max.max(nodes[i as usize].space[d].max);
        }
    }
    sp
}

fn center(p: &Param) -> i64 { (p.min + p.max) / 2 }

fn sort_nodes(nodes: &[TreeNode], ids: &mut [u32], dim: usize, absolute: bool) {
    ids.sort_by(|a, b| {
        for k in 0..7 {
            let d = (dim + k) % 7;
            let mut ca = center(&nodes[*a as usize].space[d]);
            let mut cb = center(&nodes[*b as usize].space[d]);
            if absolute { ca = ca.abs(); cb = cb.abs(); }
            match ca.cmp(&cb) { std::cmp::Ordering::Equal => continue, o => return o }
        }
        std::cmp::Ordering::Equal
    });
}

fn bucketize(ids: &[u32]) -> Vec<Vec<u32>> {
    let size = 6f64.powf(((ids.len() as f64 - 0.01).ln() / 6f64.ln()).floor()) as usize;
    let mut out = Vec::new();
    let mut cur = Vec::new();
    for &i in ids {
        cur.push(i);
        if cur.len() >= size { out.push(std::mem::take(&mut cur)); }
    }
    if !cur.is_empty() { out.push(cur); }
    out
}

fn cost(sp: &[Param; 7]) -> i64 { sp.iter().map(|p| (p.max - p.min).abs()).sum() }

fn build(nodes: &mut Vec<TreeNode>, mut ids: Vec<u32>) -> u32 {
    if ids.len() == 1 { return ids[0]; }
    if ids.len() <= CHILDREN_PER_NODE {
        ids.sort_by_key(|i| nodes[*i as usize].space.iter().map(|p| center(p).abs()).sum::<i64>());
        let space = bounds(nodes, &ids);
        nodes.push(TreeNode { space, children: ids, leaf: None });
        return (nodes.len() - 1) as u32;
    }
    let mut best = i64::MAX;
    let mut best_dim = 0;
    let mut best_buckets: Vec<Vec<u32>> = Vec::new();
    for d in 0..7 {
        sort_nodes(nodes, &mut ids, d, false);
        let buckets = bucketize(&ids);
        let c: i64 = buckets.iter().map(|b| cost(&bounds(nodes, b))).sum();
        if best > c { best = c; best_dim = d; best_buckets = buckets; }
    }
    // Sort the buckets (as subtrees) by their bounding boxes.
    let mut tmp: Vec<u32> = Vec::new();
    for b in &best_buckets {
        let space = bounds(nodes, b);
        nodes.push(TreeNode { space, children: b.clone(), leaf: None });
        tmp.push((nodes.len() - 1) as u32);
    }
    sort_nodes(nodes, &mut tmp, best_dim, true);
    let children: Vec<u32> = tmp.iter().map(|t| {
        let ch = nodes[*t as usize].children.clone();
        build(nodes, ch)
    }).collect();
    let space = bounds(nodes, &children);
    nodes.push(TreeNode { space, children, leaf: None });
    (nodes.len() - 1) as u32
}

#[inline]
fn node_dist(n: &TreeNode, t: &[i64; 7]) -> i64 {
    let mut s = 0;
    for d in 0..7 { let v = n.space[d].dist(t[d]); s += v * v; }
    s
}

/// Climate target: temperature, humidity, continentalness, erosion, depth, weirdness.
pub type Target = [i64; 6];

impl BiomeSource {
    pub fn load(data_dir: &Path, biome_names: &[String]) -> Result<BiomeSource> {
        let p = data_dir.join("reports/biome_parameters/minecraft/overworld.json");
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?)?;
        let mut entries = Vec::new();
        for b in v["biomes"].as_array().ok_or_else(|| anyhow!("biomes"))? {
            let name = b["biome"].as_str().unwrap_or("").trim_start_matches("minecraft:");
            let id = biome_names.iter().position(|n| n == name).ok_or_else(|| anyhow!("unknown biome {name}"))?;
            let pr = &b["parameters"];
            let param = |k: &str| -> Result<Param> {
                let x = &pr[k];
                if let Some(f) = x.as_f64() { let q = quantize(f); return Ok(Param { min: q, max: q }); }
                let a = x.as_array().ok_or_else(|| anyhow!("param {k}"))?;
                Ok(Param { min: quantize(a[0].as_f64().unwrap_or(0.0)), max: quantize(a[1].as_f64().unwrap_or(0.0)) })
            };
            entries.push(Entry {
                p: [param("temperature")?, param("humidity")?, param("continentalness")?, param("erosion")?, param("depth")?, param("weirdness")?],
                offset: quantize(pr["offset"].as_f64().unwrap_or(0.0)),
                biome: id as u8,
            });
        }
        let mut tree: Vec<TreeNode> = entries.iter().enumerate().map(|(i, e)| {
            let mut space = [Param { min: e.offset, max: e.offset }; 7];
            space[..6].copy_from_slice(&e.p);
            TreeNode { space, children: Vec::new(), leaf: Some(i as u32) }
        }).collect();
        let ids: Vec<u32> = (0..tree.len() as u32).collect();
        let root = build(&mut tree, ids);
        Ok(BiomeSource { entries, tree, root })
    }

    /// R-tree search with a warm-start leaf (as vanilla does); returns (biome, leaf).
    pub fn search(&self, t: &Target, last: Option<u32>) -> (u8, u32) {
        let mut tv = [0i64; 7];
        tv[..6].copy_from_slice(t);
        let leaf = self.search_node(self.root, &tv, last);
        (self.entries[self.tree[leaf as usize].leaf.unwrap() as usize].biome, leaf)
    }

    fn search_node(&self, node: u32, t: &[i64; 7], closest: Option<u32>) -> u32 {
        let n = &self.tree[node as usize];
        if n.leaf.is_some() { return node; }
        let mut l = match closest { Some(c) => node_dist(&self.tree[c as usize], t), None => i64::MAX };
        let mut leaf = closest;
        for &c in &n.children {
            let m = node_dist(&self.tree[c as usize], t);
            if l > m {
                let leaf2 = self.search_node(c, t, leaf);
                let nn = if c == leaf2 { m } else { node_dist(&self.tree[leaf2 as usize], t) };
                if l > nn { l = nn; leaf = Some(leaf2); }
            }
        }
        leaf.unwrap()
    }

    /// Distinct biomes in the order the parameter list first mentions them.
    pub fn possible_biomes(&self) -> Vec<u8> {
        let mut v: Vec<u8> = Vec::new();
        for e in &self.entries { if !v.contains(&e.biome) { v.push(e.biome); } }
        v
    }

    /// Brute-force nearest entry.
    pub fn find(&self, t: &Target) -> u8 {
        let mut best = i64::MAX;
        let mut res = 0;
        for e in &self.entries {
            let d = self.fitness(e, t);
            if d < best { best = d; res = e.biome; }
        }
        res
    }

    /// The `n` best entries (fitness, entry index) for a target.
    pub fn ranked(&self, t: &Target, n: usize) -> Vec<(i64, usize)> {
        let mut v: Vec<(i64, usize)> = self.entries.iter().enumerate().map(|(i, e)| (self.fitness(e, t), i)).collect();
        v.sort();
        v.truncate(n);
        v
    }

    #[inline]
    fn fitness(&self, e: &Entry, t: &Target) -> i64 {
        let mut s = e.offset * e.offset;
        for i in 0..6 { let d = e.p[i].dist(t[i]); s += d * d; }
        s
    }
}

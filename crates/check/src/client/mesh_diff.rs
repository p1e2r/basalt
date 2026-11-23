//! `basalt-check client-mesh`: block-meshing parity of basalt-client against the vanilla client.
//!
//! Generates block-meshing scenes deterministically from (seed, n), records them in
//! target/check/cases/client-mesh/<seed>-<n>.json, has the vanilla client mesh them through the
//! test-only oracle mod (tools/oracle-mod, schema in its SCHEMA.md; vanilla output cached under
//! vanilla/oracle/cache — gitignored, it contains Mojang-derived geometry) and basalt-client
//! (`--mesh-dump`), matches quads order-independently by geometry and prints mismatch classes
//! with counts, the worst blocks and a minimal repro scene per class, then
//! `RESULT client_mesh quads=<n> scenes=<m> mismatches=<k>`.
//!
//! usage: basalt-check client-mesh [--seed S] [--n N] [--states STRIDE] [--batch B]
//!          [--oracle-dir vanilla/oracle] [--no-launch] [--client target/release/basalt-client]
//!          [--jar vanilla/client.jar] [--out target/check] [--repro-only CLASS] [--show K]
//! Scene modes: every block state alone (full sky light) and every state with random face
//! neighbours + random light (STRIDE > 1 samples every STRIDE-th state), plus N random
//! 26-neighbour combination scenes.
use super::oracle::Oracle;
use super::rng::Rng;
use anyhow::{bail, Context, Result};
use basalt_data::BlockState;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

pub struct Opts {
    pub seed: u64, pub n: usize, pub stride: usize, pub batch: usize,
    pub oracle_dir: PathBuf, pub launch: bool, pub client: PathBuf, pub jar: PathBuf, pub out: PathBuf, pub show: usize,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts { seed: 1, n: 20000, stride: 1, batch: 4000, oracle_dir: "vanilla/oracle".into(), launch: true,
            client: "target/release/basalt-client".into(), jar: "vanilla/client.jar".into(), out: "target/check".into(), show: 3 }
    }
}

const DIRS: [(&str, [i32; 3]); 6] = [("down", [0, -1, 0]), ("up", [0, 1, 0]), ("north", [0, 0, -1]), ("south", [0, 0, 1]), ("west", [-1, 0, 0]), ("east", [1, 0, 0])];

// ------------------------------------------------------------------------ scene generation

fn all_states() -> Vec<BlockState> {
    (0..basalt_data::blocks::STATE_COUNT as u32).map(BlockState).filter(|s| !s.is_air()).collect()
}

/// One neighbour from the palette {air, opaque cube, leaves, glass, same block, slab, stairs, water}.
fn neighbour(r: &mut Rng, target: BlockState) -> String {
    match r.below(8) {
        0 => "minecraft:air".into(),
        1 => "minecraft:stone".into(),
        2 => "minecraft:oak_leaves[distance=7,persistent=true,waterlogged=false]".into(),
        3 => "minecraft:glass".into(),
        4 => target.to_string(),
        5 => format!("minecraft:stone_slab[type={},waterlogged=false]", r.pick(&["bottom", "top"])),
        6 => format!("minecraft:oak_stairs[facing={},half={},shape=straight,waterlogged=false]", r.pick(&["north", "south", "west", "east"]), r.pick(&["bottom", "top"])),
        _ => "minecraft:water[level=0]".into(),
    }
}

fn random_light(r: &mut Rng, radius: i32) -> Value {
    let mut cells = Vec::new();
    for dy in -radius..=radius { for dz in -radius..=radius { for dx in -radius..=radius {
        if r.chance(0.5) { cells.push(json!([dx, dy, dz, r.below(16), r.below(16)])); }
    } } }
    json!({"default": [15, 0], "cells": cells})
}

fn random_tint(r: &mut Rng) -> Value {
    let c = |r: &mut Rng| (r.next_u64() & 0xFFFFFF) as u32;
    json!({"grass": c(r), "foliage": c(r), "dry_foliage": c(r), "water": c(r)})
}

/// The deterministic scene list for (seed, n, stride).
pub fn generate(seed: u64, n: usize, stride: usize) -> Vec<Value> {
    let states = all_states();
    let mut r = Rng::new(seed);
    let mut scenes = Vec::new();
    let mut id = 0u64;
    let stride = stride.max(1);
    // A: every state alone, full sky light
    for s in states.iter().step_by(stride) {
        scenes.push(json!({"id": id, "mode": "iso", "blocks": [[0, 0, 0, s.to_string()]], "light": {"default": [15, 0]}}));
        id += 1;
    }
    // B: every state with random face neighbours, light and biome colours
    for s in states.iter().step_by(stride) {
        let mut blocks = vec![json!([0, 0, 0, s.to_string()])];
        for (_, d) in DIRS { let nb = neighbour(&mut r, *s); if !nb.ends_with(":air") { blocks.push(json!([d[0], d[1], d[2], nb])); } }
        scenes.push(json!({"id": id, "mode": "nbr", "blocks": blocks, "light": random_light(&mut r, 1), "tint": random_tint(&mut r)}));
        id += 1;
    }
    // C: random targets with all 26 neighbours (faces from the palette, edges/corners air/opaque/glass/leaves)
    for _ in 0..n {
        let s = *r.pick(&states);
        let mut blocks = vec![json!([0, 0, 0, s.to_string()])];
        for dy in -1..=1i32 { for dz in -1..=1i32 { for dx in -1..=1i32 {
            let manhattan = dx.abs() + dy.abs() + dz.abs();
            if manhattan == 0 { continue; }
            let nb = if manhattan == 1 { neighbour(&mut r, s) } else {
                match r.below(5) { 0 | 1 => "minecraft:air".into(), 2 => "minecraft:stone".into(), 3 => "minecraft:glass".into(), _ => "minecraft:oak_leaves[distance=7,persistent=true,waterlogged=false]".into() }
            };
            if !nb.ends_with(":air") { blocks.push(json!([dx, dy, dz, nb])); }
        } } }
        let light = if r.chance(0.3) { json!({"default": [15, 0]}) } else { random_light(&mut r, 1) };
        scenes.push(json!({"id": id, "mode": "combo", "blocks": blocks, "light": light, "tint": random_tint(&mut r)}));
        id += 1;
    }
    scenes
}

fn fnv(b: &[u8]) -> u64 { b.iter().fold(0xcbf29ce484222325u64, |h, &x| (h ^ x as u64).wrapping_mul(0x100000001b3)) }

// ------------------------------------------------------------------------ diff

#[derive(Clone, Debug)]
struct Q { src: String, layer: String, cull: Option<String>, tex: String, ti: i64, tint: Option<u32>, shade: bool, v: [[f64; 7]; 4] }

fn parse_q(v: &Value) -> Option<Q> {
    let mut verts = [[0.0; 7]; 4];
    let vs = v["v"].as_array()?;
    if vs.len() != 4 { return None; }
    for (i, x) in vs.iter().enumerate() { let a = x.as_array()?; for k in 0..7 { verts[i][k] = a.get(k)?.as_f64()?; } }
    Some(Q {
        src: v["src"].as_str()?.into(), layer: v["layer"].as_str()?.into(), cull: v["cull"].as_str().map(String::from),
        tex: v["tex"].as_str()?.trim_start_matches("minecraft:").into(), ti: v["ti"].as_i64().unwrap_or(-1),
        tint: v["tint"].as_u64().map(|t| t as u32), shade: v["shade"].as_bool().unwrap_or(true), v: verts,
    })
}

fn rp(x: f64) -> i64 { (x * 1000.0).round() as i64 }

fn normal_dir(q: &Q) -> usize {
    let p = |i: usize| [q.v[i][0], q.v[i][1], q.v[i][2]];
    let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let cross = |a: [f64; 3], b: [f64; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let mut n = cross(sub(p(1), p(0)), sub(p(2), p(0)));
    if n.iter().map(|c| c.abs()).sum::<f64>() < 1e-9 { n = cross(sub(p(2), p(0)), sub(p(3), p(0))); }
    let ax = (0..3).max_by(|&a, &b| n[a].abs().partial_cmp(&n[b].abs()).unwrap()).unwrap();
    match (ax, n[ax] >= 0.0) { (1, false) => 0, (1, true) => 1, (2, false) => 2, (2, true) => 3, (0, false) => 4, _ => 5 }
}

fn key(q: &Q) -> (String, usize, [[i64; 3]; 4]) {
    let mut ps: Vec<[i64; 3]> = q.v.iter().map(|v| [rp(v[0]), rp(v[1]), rp(v[2])]).collect();
    ps.sort();
    (q.src.clone(), normal_dir(q), [ps[0], ps[1], ps[2], ps[3]])
}

fn rgb(c: f64) -> [i32; 3] { let c = c as u64; [(c >> 16 & 255) as i32, (c >> 8 & 255) as i32, (c & 255) as i32] }
fn eff_tint(q: &Q) -> u32 { if q.ti >= 0 { q.tint.unwrap_or(0xFFFFFF) } else { 0xFFFFFF } }

/// Mismatch classes of a geometrically matched (vanilla, basalt) quad pair.
fn compare(v: &Q, b: &Q) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    // pair vertices by position
    let mut map = [usize::MAX; 4]; let mut used = [false; 4];
    for i in 0..4 { for j in 0..4 { if !used[j] && (0..3).all(|k| rp(v.v[i][k]) == rp(b.v[j][k])) { map[i] = j; used[j] = true; break; } } }
    if map.contains(&usize::MAX) { return vec![("geometry", "vertex positions do not pair up".into())]; }
    if v.tex != b.tex { out.push(("texture", format!("{} vs {}", v.tex, b.tex))); }
    if v.layer != b.layer { out.push(("layer", format!("{} vs {}", v.layer, b.layer))); }
    if v.tex == b.tex {
        let du = (0..4).map(|i| (v.v[i][3] - b.v[map[i]][3]).abs().max((v.v[i][4] - b.v[map[i]][4]).abs())).fold(0.0, f64::max);
        if du > 0.002 { out.push(("uv", format!("max |duv| {du:.4}: vanilla {:?} basalt {:?}", (0..4).map(|i| [v.v[i][3], v.v[i][4]]).collect::<Vec<_>>(), (0..4).map(|i| [b.v[map[i]][3], b.v[map[i]][4]]).collect::<Vec<_>>()))); }
        else if du > 0.0005 { out.push(("uv-inset", format!("max |duv| {du:.4}"))); }
    }
    let (tv, tb) = (eff_tint(v), eff_tint(b));
    let tint_ok = (0..3).all(|k| (rgb(tv as f64)[k] - rgb(tb as f64)[k]).abs() <= 2);
    if !tint_ok { out.push(("tint", format!("{tv:06x} vs {tb:06x} (ti {} vs {})", v.ti, b.ti))); }
    if v.shade != b.shade { out.push(("shade", format!("shade flag {} vs {}", v.shade, b.shade))); }
    if tint_ok {
        let cv: Vec<[i32; 3]> = (0..4).map(|i| rgb(v.v[i][5])).collect();
        let cb: Vec<[i32; 3]> = (0..4).map(|i| rgb(b.v[map[i]][5])).collect();
        let bad = (0..4).any(|i| (0..3).any(|k| (cv[i][k] - cb[i][k]).abs() > 3));
        if bad {
            let uniform = |c: &Vec<[i32; 3]>| c.iter().all(|x| x == &c[0]);
            let class = if uniform(&cv) && uniform(&cb) { "shade" } else { "ao" };
            out.push((class, format!("vertex colours vanilla {:?} basalt {:?}", cv, cb)));
        }
    }
    let lt = |c: f64| { let c = c as u64; ((c >> 16 & 0xFFFF) as i64, (c & 0xFFFF) as i64) };
    let lbad = (0..4).any(|i| { let (s1, b1) = lt(v.v[i][6]); let (s2, b2) = lt(b.v[map[i]][6]); (s1 - s2).abs() > 2 || (b1 - b2).abs() > 2 });
    if lbad {
        let f = |q: &Q, idx: &dyn Fn(usize) -> usize| (0..4).map(|i| { let (s, bl) = lt(q.v[idx(i)][6]); format!("{:.2}/{:.2}", s as f64 / 16.0, bl as f64 / 16.0) }).collect::<Vec<_>>().join(" ");
        out.push(("light", format!("sky/block vanilla [{}] basalt [{}]", f(v, &|i| i), f(b, &|i| map[i]))));
    }
    match (&v.cull, &b.cull) {
        (Some(c), _) if c == "?" => {}
        (a, b2) if a != b2 => out.push(("cullface", format!("cull face {:?} vs {:?}", a, b2))),
        _ => {}
    }
    out
}

#[derive(Default)]
struct ClassStat { count: usize, blocks: HashMap<String, usize>, repro: Option<(usize, u64, String)>, examples: Vec<(u64, String)> }

#[derive(Default)]
struct Report { classes: BTreeMap<String, ClassStat>, vanilla_quads: usize, basalt_quads: usize, scenes: usize, mismatched_quads: usize }

impl Report {
    fn add(&mut self, class: &str, scene: &Value, detail: String, show: usize) {
        let st = self.classes.entry(class.to_string()).or_default();
        st.count += 1;
        let target = scene["blocks"][0][3].as_str().unwrap_or("?");
        let name = target.split('[').next().unwrap_or(target).trim_start_matches("minecraft:").to_string();
        *st.blocks.entry(name).or_default() += 1;
        let nb = scene["blocks"].as_array().map(|a| a.len()).unwrap_or(0);
        let id = scene["id"].as_u64().unwrap_or(0);
        if st.repro.as_ref().map(|r| nb < r.0).unwrap_or(true) { st.repro = Some((nb, id, format!("{target}: {detail}"))); }
        if st.examples.len() < show && !st.examples.iter().any(|e| e.0 == id) { st.examples.push((id, format!("{target}: {detail}"))); }
    }
}

fn diff_scene(rep: &mut Report, scene: &Value, van: &Value, bas: &Value, show: usize) {
    rep.scenes += 1;
    if let Some(e) = van.get("error").and_then(|e| e.as_str()) { rep.add("vanilla-error", scene, e.to_string(), show); return; }
    if let Some(e) = bas.get("error").and_then(|e| e.as_str()) { rep.add("basalt-error", scene, e.to_string(), show); }
    let vq: Vec<Q> = van["quads"].as_array().map(|a| a.iter().filter_map(parse_q).collect()).unwrap_or_default();
    let bq: Vec<Q> = bas["quads"].as_array().map(|a| a.iter().filter_map(parse_q).collect()).unwrap_or_default();
    rep.vanilla_quads += vq.len(); rep.basalt_quads += bq.len();
    // blocks with several weighted models: vanilla picks one by position, so geometry/UV/texture
    // differences there are reported as "variant" rather than as mesher bugs
    let random = bas["random_variant"].as_bool().unwrap_or(false);
    let iso = scene["blocks"].as_array().map(|a| a.len() <= 1).unwrap_or(true);
    let add = |rep: &mut Report, class: &'static str, detail: String| {
        let class = match class {
            "uv" | "texture" | "missing-quad" | "extra-quad" | "culling-over" | "culling-under" | "cullface" | "geometry" if random => "variant",
            // colour differences with no neighbours cannot be ambient occlusion
            "ao" if iso => "shade",
            "shade" if !iso => "ao",
            c => c,
        };
        rep.add(class, scene, detail, show);
    };
    let mut pool: HashMap<(String, usize, [[i64; 3]; 4]), Vec<usize>> = HashMap::new();
    for (i, q) in bq.iter().enumerate() { pool.entry(key(q)).or_default().push(i); }
    let mut used = vec![false; bq.len()];
    let neighbour_solid = |dir: &str| -> bool {
        let Some(d) = DIRS.iter().find(|d| d.0 == dir).map(|d| d.1) else { return false };
        scene["blocks"].as_array().map(|a| a.iter().any(|b| b[0].as_i64() == Some(d[0] as i64) && b[1].as_i64() == Some(d[1] as i64) && b[2].as_i64() == Some(d[2] as i64))).unwrap_or(false)
    };
    let mut unmatched_v = Vec::new();
    for q in &vq {
        let k = key(q);
        let cand = pool.get_mut(&k).and_then(|c| {
            // prefer a candidate with the same texture (stacked overlay quads share geometry)
            let pos = c.iter().position(|&j| bq[j].tex == q.tex && bq[j].layer == q.layer).or(if c.is_empty() { None } else { Some(0) })?;
            Some(c.remove(pos))
        });
        match cand {
            Some(j) => {
                used[j] = true;
                let cls = compare(q, &bq[j]);
                if !cls.is_empty() { rep.mismatched_quads += 1; }
                for (c, d) in cls { add(rep, c, format!("[{} {}] {d}", q.tex, DIRS[k.1].0)); }
            }
            None => unmatched_v.push(q),
        }
    }
    for q in unmatched_v {
        rep.mismatched_quads += 1;
        let over = q.cull.as_deref().map(|c| c != "?" && neighbour_solid(c)).unwrap_or(false);
        let pos: Vec<[f64; 3]> = q.v.iter().map(|v| [v[0], v[1], v[2]]).collect();
        add(rep, if over { "culling-over" } else { "missing-quad" }, format!("[{} {} cull {:?}] {:?}", q.tex, DIRS[normal_dir(q)].0, q.cull, pos));
    }
    for (j, q) in bq.iter().enumerate() {
        if used[j] { continue; }
        rep.mismatched_quads += 1;
        let under = q.cull.as_deref().map(|c| neighbour_solid(c)).unwrap_or(false);
        let pos: Vec<[f64; 3]> = q.v.iter().map(|v| [v[0], v[1], v[2]]).collect();
        add(rep, if under { "culling-under" } else { "extra-quad" }, format!("[{} {} cull {:?}] {:?}", q.tex, DIRS[normal_dir(q)].0, q.cull, pos));
    }
}

// ------------------------------------------------------------------------ driver

pub fn parse_args(args: &[String]) -> Result<Opts> {
    let mut o = Opts::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut v = || it.next().cloned().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--seed" => o.seed = v()?.parse()?, "--n" => o.n = v()?.parse()?, "--states" => o.stride = v()?.parse()?,
            "--batch" => o.batch = v()?.parse::<usize>()?.max(1), "--oracle-dir" => o.oracle_dir = v()?.into(),
            "--no-launch" => o.launch = false, "--client" => o.client = v()?.into(), "--jar" => o.jar = v()?.into(),
            "--out" => o.out = v()?.into(), "--show" => o.show = v()?.parse()?,
            other => bail!("client-mesh: unknown argument {other}"),
        }
    }
    Ok(o)
}

/// Run the harness; returns the number of mismatching quads.
pub fn run(o: &Opts) -> Result<usize> {
    let t0 = Instant::now();
    let scenes = generate(o.seed, o.n, o.stride);
    let tag = format!("{}-{}{}", o.seed, o.n, if o.stride > 1 { format!("-s{}", o.stride) } else { String::new() });
    let case_dir = o.out.join("cases/client-mesh");
    std::fs::create_dir_all(&case_dir)?;
    let case = serde_json::to_vec(&json!({"seed": o.seed, "n": o.n, "stride": o.stride, "scenes": scenes}))?;
    std::fs::write(case_dir.join(format!("{tag}.json")), &case)?;
    let hash = fnv(&case);
    let work = o.out.join("client-mesh").join(&tag);
    let req_dir = work.join("req"); let bas_dir = work.join("basalt");
    let _ = std::fs::remove_dir_all(&req_dir); let _ = std::fs::remove_dir_all(&bas_dir);
    std::fs::create_dir_all(&req_dir)?;
    let batches: Vec<(String, Vec<u8>)> = scenes.chunks(o.batch).enumerate().map(|(i, c)| {
        let id = format!("cm-{tag}-{:08x}-{i:03}", hash as u32);
        let body = serde_json::to_vec(&json!({"id": id, "kind": "mesh", "options": {"ao": true, "cull": true}, "scenes": c})).unwrap();
        (id, body)
    }).collect();
    for (id, b) in &batches { std::fs::write(req_dir.join(format!("{id}.json")), b)?; }
    eprintln!("client-mesh: {} scenes in {} batches (case {})", scenes.len(), batches.len(), case_dir.join(format!("{tag}.json")).display());

    // basalt side runs concurrently with the vanilla oracle
    if !o.client.exists() { bail!("{} missing: cargo build --release -p basalt-client", o.client.display()); }
    let mut basalt = Command::new(&o.client).arg("--jar").arg(&o.jar).arg("--mesh-dump").arg(&req_dir).arg("--out").arg(&bas_dir)
        .env("RUST_LOG", "warn").spawn().context("starting basalt-client --mesh-dump")?;

    // vanilla side (cached by request hash)
    let cache = o.oracle_dir.join("cache/client-mesh").join(&tag);
    std::fs::create_dir_all(&cache)?;
    let need: Vec<&(String, Vec<u8>)> = batches.iter().filter(|(id, _)| !cache.join(format!("{id}.json")).exists()).collect();
    if !need.is_empty() {
        let oracle = Oracle::connect(&o.oracle_dir, o.launch)?;
        for (id, body) in need {
            let t = Instant::now();
            let p = oracle.request_bytes(id, body, Duration::from_secs(900))?;
            std::fs::rename(&p, cache.join(format!("{id}.json"))).or_else(|_| std::fs::copy(&p, cache.join(format!("{id}.json"))).map(|_| ()))?;
            eprintln!("client-mesh: vanilla {id} in {:.1?}", t.elapsed());
        }
    } else { eprintln!("client-mesh: vanilla output cached in {}", cache.display()); }
    let st = basalt.wait()?;
    if !st.success() { bail!("basalt-client --mesh-dump failed ({st})"); }

    // diff
    let mut rep = Report::default();
    let by_id: HashMap<u64, &Value> = scenes.iter().map(|s| (s["id"].as_u64().unwrap(), s)).collect();
    for (id, _) in &batches {
        let rd = |p: &Path| -> Result<Value> { Ok(serde_json::from_slice(&std::fs::read(p).with_context(|| format!("reading {}", p.display()))?)?) };
        let van = rd(&cache.join(format!("{id}.json")))?;
        let bas = rd(&bas_dir.join(format!("{id}.json")))?;
        if let Some(e) = van.get("error") { bail!("vanilla oracle failed on {id}: {e}"); }
        let bmap: HashMap<u64, &Value> = bas["scenes"].as_array().map(|a| a.iter().map(|s| (s["id"].as_u64().unwrap_or(u64::MAX), s)).collect()).unwrap_or_default();
        let empty = json!({"quads": [], "error": "scene missing from basalt output"});
        for vs in van["scenes"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            let sid = vs["id"].as_u64().unwrap_or(u64::MAX);
            let Some(scene) = by_id.get(&sid) else { continue };
            diff_scene(&mut rep, scene, vs, bmap.get(&sid).copied().unwrap_or(&empty), o.show);
        }
    }

    // report
    let mut classes: Vec<(&String, &ClassStat)> = rep.classes.iter().collect();
    classes.sort_by(|a, b| b.1.count.cmp(&a.1.count));
    println!("client-mesh: {} scenes, vanilla {} quads, basalt {} quads, {} mismatching quads ({:.1?})", rep.scenes, rep.vanilla_quads, rep.basalt_quads, rep.mismatched_quads, t0.elapsed());
    println!("{:16} {:>9}  top blocks", "class", "count");
    let repro_dir = work.join("repro");
    let _ = std::fs::remove_dir_all(&repro_dir); std::fs::create_dir_all(&repro_dir)?;
    let mut json_classes = serde_json::Map::new();
    for (c, st) in &classes {
        let mut top: Vec<(&String, &usize)> = st.blocks.iter().collect();
        top.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        println!("{:16} {:>9}  {}", c, st.count, top.iter().take(6).map(|(b, n)| format!("{b}:{n}")).collect::<Vec<_>>().join(" "));
        if let Some((_, sid, detail)) = &st.repro {
            println!("    repro scene {sid}: {}", truncate(detail, 300));
            let scene = by_id[sid];
            std::fs::write(repro_dir.join(format!("{c}.json")), serde_json::to_vec_pretty(&json!({"id": format!("repro-{c}"), "kind": "mesh", "scenes": [scene]}))?)?;
        }
        for (sid, d) in &st.examples { println!("    e.g. scene {sid}: {}", truncate(d, 200)); }
        json_classes.insert(c.to_string(), json!({"count": st.count, "top_blocks": top.iter().take(40).map(|(b, n)| json!([b, n])).collect::<Vec<_>>(),
            "repro": st.repro.as_ref().map(|r| json!({"scene": r.1, "detail": r.2}))}));
    }
    std::fs::write(work.join("report.json"), serde_json::to_vec_pretty(&json!({"seed": o.seed, "n": o.n, "stride": o.stride, "scenes": rep.scenes,
        "vanilla_quads": rep.vanilla_quads, "basalt_quads": rep.basalt_quads, "mismatches": rep.mismatched_quads, "classes": json_classes}))?)?;
    println!("report {} (repro request files in {})", work.join("report.json").display(), repro_dir.display());
    println!("RESULT client_mesh quads={} scenes={} mismatches={}", rep.vanilla_quads, rep.scenes, rep.mismatched_quads);
    Ok(rep.mismatched_quads)
}

fn truncate(s: &str, n: usize) -> String { if s.len() <= n { s.to_string() } else { let mut e = n; while !s.is_char_boundary(e) { e -= 1; } format!("{}...", &s[..e]) } }

/// Entry point for `basalt-check client-mesh ARGS`; returns the process exit code.
pub fn main(args: &[String]) -> i32 {
    match parse_args(args).and_then(|o| run(&o)) {
        Ok(0) => 0,
        Ok(_) => 1,
        Err(e) => { eprintln!("client-mesh: {e:#}"); 2 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generation_is_deterministic() {
        let a = generate(5, 20, 500); let b = generate(5, 20, 500);
        assert_eq!(serde_json::to_string(&a).unwrap(), serde_json::to_string(&b).unwrap());
        assert!(a.len() > 20);
    }
    #[test]
    fn quads_match_regardless_of_order() {
        let q = |x: f64, c: u64| json!({"src": "block", "layer": "solid", "dir": "up", "tex": "minecraft:block/stone", "ti": -1, "shade": true,
            "v": [[x, 1, 0, 0, 0, c, 0], [x, 1, 1, 0, 1, c, 0], [x + 1.0, 1, 1, 1, 1, c, 0], [x + 1.0, 1, 0, 1, 0, c, 0]]});
        let scene = json!({"id": 0, "blocks": [[0, 0, 0, "minecraft:stone"]]});
        let van = json!({"quads": [q(0.0, 0xFFFFFFFFu64), q(2.0, 0xFFFFFFFFu64)]});
        let bas = json!({"quads": [q(2.0, 0xFFFFFFFFu64), q(0.0, 0xFF808080u64)]});
        let mut rep = Report::default();
        diff_scene(&mut rep, &scene, &van, &bas, 1);
        assert_eq!(rep.mismatched_quads, 1);
        assert_eq!(rep.classes.keys().cloned().collect::<Vec<_>>(), vec!["shade".to_string()]);
    }
}

//! Structure parity against a vanilla world's region files.
//!
//! cargo run -p basalt-worldgen --release --example structures -- <cmd> <region dir> <R> [seed]
//!   count   REGION R        — chunks in -R..R-1 by status (`full=<n> starts=<n>`)
//!   dump    REGION R        — vanilla structure starts in the square (id chunk pieces bbox)
//!   compare REGION R SEED   — Basalt's structure starts vs vanilla's in the square, per structure
//!                             type, plus block-exact % inside the vanilla structure boxes of fully
//!                             generated chunks. Last line `RESULT ...`.
//!   adapt   REGION R SEED NOSTRUCT_REGION OUT — terrain adaptation measuring data: for the starts of
//!                             $ONLY in the square, OUT.pieces.json (vanilla piece boxes, ground delta,
//!                             projection, junctions) and OUT.bin (per block near the pieces: x y z i32,
//!                             vanilla state with structures u16, without u16, Basalt's pre-adaptation
//!                             density f32, Basalt's state without structures u16) + OUT.states.json.

#![allow(clippy::type_complexity)]

use anyhow::{bail, Result};
use basalt_nbt::Tag;
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::Path;

pub fn read_nbt(dir: &Path, cx: i32, cz: i32) -> Result<Option<Tag>> {
    let path = dir.join(format!("r.{}.{}.mca", cx >> 5, cz >> 5));
    let Ok(data) = std::fs::read(&path) else {
        return Ok(None);
    };
    if data.len() < 8192 {
        return Ok(None);
    }
    let i = (((cx & 31) + (cz & 31) * 32) * 4) as usize;
    let off =
        ((data[i] as usize) << 16 | (data[i + 1] as usize) << 8 | data[i + 2] as usize) * 4096;
    if off == 0 || off + 5 > data.len() {
        return Ok(None);
    }
    let len = u32::from_be_bytes(data[off..off + 4].try_into()?) as usize;
    let comp = data[off + 4];
    let raw = &data[off + 5..off + 4 + len];
    let mut buf = Vec::new();
    match comp {
        1 => {
            flate2::read::GzDecoder::new(raw).read_to_end(&mut buf)?;
        }
        2 => {
            flate2::read::ZlibDecoder::new(raw).read_to_end(&mut buf)?;
        }
        3 => buf.extend_from_slice(raw),
        c => bail!("unsupported compression {c}"),
    }
    let (_, root) = basalt_nbt::Reader::new(&buf).named()?;
    Ok(Some(root))
}

#[derive(Debug, Clone)]
pub struct VStart {
    pub id: String,
    pub cx: i32,
    pub cz: i32,
    pub pieces: Vec<(String, [i32; 6])>,
}

/// Structure starts saved in the chunks of the square (any status).
fn vanilla_starts(dir: &Path, r: i32) -> Result<(Vec<VStart>, HashMap<String, usize>)> {
    let (a, b, _) = vanilla_starts_ex(dir, r)?;
    Ok((a, b))
}

fn vanilla_starts_ex(
    dir: &Path,
    r: i32,
) -> Result<(
    Vec<VStart>,
    HashMap<String, usize>,
    std::collections::HashSet<(i32, i32)>,
)> {
    let mut have = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut status: HashMap<String, usize> = HashMap::new();
    let (ox, oz) = center();
    for cx in ox - r..ox + r {
        for cz in oz - r..oz + r {
            let Some(root) = read_nbt(dir, cx, cz)? else {
                continue;
            };
            let st = root
                .get("Status")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            if st != "minecraft:empty" {
                have.insert((cx, cz));
            }
            *status.entry(st).or_default() += 1;
            let Some(Tag::Compound(starts)) = root.get("structures").and_then(|s| s.get("starts"))
            else {
                continue;
            };
            for (id, s) in starts {
                if s.get("id").and_then(|i| i.as_str()) == Some("INVALID") {
                    continue;
                }
                let mut pieces = Vec::new();
                for c in s
                    .get("Children")
                    .and_then(|c| c.as_list())
                    .map(|l| l.as_slice())
                    .unwrap_or(&[])
                {
                    let pid = c
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();
                    let rot = c
                        .get("Rot")
                        .or(c.get("Rotation"))
                        .or(c.get("rotation"))
                        .and_then(|x| x.as_str())
                        .unwrap_or("");
                    let name = c
                        .get("pool_element")
                        .and_then(|e| e.get("location"))
                        .and_then(|l| l.as_str())
                        .or_else(|| c.get("Template").and_then(|t| t.as_str()))
                        .unwrap_or("");
                    if std::env::var("RAW").is_ok() {
                        eprintln!("{id} {cx} {cz}: {:?}", c);
                    }
                    if let Some(Tag::IntArray(bb)) = c.get("BB") {
                        if bb.len() == 6 {
                            pieces.push((
                                format!("{pid} {name} {rot}"),
                                [bb[0], bb[1], bb[2], bb[3], bb[4], bb[5]],
                            ));
                        }
                    }
                }
                out.push(VStart {
                    id: id.trim_start_matches("minecraft:").to_string(),
                    cx,
                    cz,
                    pieces,
                });
            }
        }
    }
    Ok((out, status, have))
}

/// Square centre (chunk) from $CENTER=cx,cz (default 0,0).
fn center() -> (i32, i32) {
    std::env::var("CENTER")
        .ok()
        .and_then(|c| {
            let (a, b) = c.split_once(',')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        })
        .unwrap_or((0, 0))
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        bail!("usage: structures count|dump|compare REGION R [SEED]");
    }
    let dir = Path::new(&args[2]);
    let r: i32 = args[3].parse().unwrap_or(0);
    match args[1].as_str() {
        "count" => {
            let (starts, status) = vanilla_starts(dir, r)?;
            println!(
                "full={} starts={} statuses={:?}",
                status.get("minecraft:full").copied().unwrap_or(0),
                starts.len(),
                status
            );
        }
        "dump" => {
            let (starts, _) = vanilla_starts(dir, r)?;
            let detail = std::env::var("DETAIL").is_ok();
            for s in &starts {
                let bb = union(&s.pieces);
                println!(
                    "{} {} {} pieces={} bb={:?}",
                    s.id,
                    s.cx,
                    s.cz,
                    s.pieces.len(),
                    bb
                );
                if detail {
                    for p in &s.pieces {
                        println!("    {} {:?}", p.0, p.1);
                    }
                }
            }
            let mut per: BTreeMap<&str, usize> = BTreeMap::new();
            for s in &starts {
                *per.entry(&s.id).or_default() += 1;
            }
            println!("{per:?}");
        }
        "compare" => compare(
            dir,
            r,
            args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(12345),
        )?,
        "adapt" => adapt(dir, r, args[4].parse()?, Path::new(&args[5]), &args[6])?,
        "blocks" => {
            // blocks REGION R X0,Y0,Z0,X1,Y1,Z1: vanilla block states in the box ("x y z state" lines)
            let v: Vec<i32> = args[4].split(',').map(|x| x.parse().unwrap()).collect();
            for cx in (v[0] >> 4)..=(v[3] >> 4) {
                for cz in (v[2] >> 4)..=(v[5] >> 4) {
                    let Some(b) = vanilla_blocks(dir, cx, cz)? else {
                        continue;
                    };
                    for x in v[0].max(cx * 16)..=v[3].min(cx * 16 + 15) {
                        for z in v[2].max(cz * 16)..=v[5].min(cz * 16 + 15) {
                            for y in v[1]..=v[4] {
                                let i = (((y + 64) as usize) << 8)
                                    | (((z & 15) as usize) << 4)
                                    | (x & 15) as usize;
                                println!(
                                    "{x} {y} {z} {}",
                                    basalt_data::BlockState(b[i] as u32).to_string()
                                );
                            }
                        }
                    }
                }
            }
        }
        "bstarts" => {
            // bstarts CX,CZ R SEED ID (dir argument = "CX,CZ"): Basalt's starts of ID in the square
            let v: Vec<i32> = args[2].split(',').map(|x| x.parse().unwrap()).collect();
            let g = basalt_worldgen::Generator::new(args[4].parse()?);
            let s = g.structures().unwrap();
            for cx in v[0] - r..v[0] + r {
                for cz in v[1] - r..v[1] + r {
                    for st in g.structure_starts(cx, cz).iter() {
                        if s.defs[st.def].name.contains(args[5].as_str()) {
                            println!("{cx} {cz} {:?}", st.pieces.first().map(|p| (&p.kind, p.bb)));
                        }
                    }
                }
            }
        }
        "json" => {
            // json REGION R ID: the saved starts of ID in the square as JSON lines (all NBT fields)
            fn j(t: &Tag) -> serde_json::Value {
                use serde_json::Value as V;
                match t {
                    Tag::Byte(v) => V::from(*v),
                    Tag::Short(v) => V::from(*v),
                    Tag::Int(v) => V::from(*v),
                    Tag::Long(v) => V::from(*v),
                    Tag::Float(v) => V::from(*v),
                    Tag::Double(v) => V::from(*v),
                    Tag::String(s) => V::from(s.clone()),
                    Tag::ByteArray(a) => V::from(a.clone()),
                    Tag::IntArray(a) => V::from(a.clone()),
                    Tag::LongArray(a) => V::from(a.clone()),
                    Tag::List(l) => V::Array(l.iter().map(j).collect()),
                    Tag::Compound(c) => {
                        V::Object(c.iter().map(|(k, v)| (k.clone(), j(v))).collect())
                    }
                    Tag::End => V::Null,
                }
            }
            let (ox, oz) = center();
            for cx in ox - r..ox + r {
                for cz in oz - r..oz + r {
                    let Some(root) = read_nbt(dir, cx, cz)? else {
                        continue;
                    };
                    let Some(Tag::Compound(starts)) =
                        root.get("structures").and_then(|s| s.get("starts"))
                    else {
                        continue;
                    };
                    for (id, s) in starts {
                        if id.trim_start_matches("minecraft:") == args[4] {
                            let mut v = j(s);
                            v["cx"] = cx.into();
                            v["cz"] = cz.into();
                            println!("{v}");
                        }
                    }
                }
            }
        }
        "refs" => {
            // refs REGION R CX,CZ: the structure references saved in chunk CX,CZ (+ status)
            let v: Vec<i32> = args[4].split(',').map(|x| x.parse().unwrap()).collect();
            if let Some(root) = read_nbt(dir, v[0], v[1])? {
                println!("status {:?}", root.get("Status"));
                if let Some(Tag::Compound(rf)) =
                    root.get("structures").and_then(|s| s.get("References"))
                {
                    for (k, t) in rf {
                        println!("{k}: {:?}", t);
                    }
                }
            }
        }
        "scan" => {
            // scan ID R SEED (dir argument = id): Basalt's starts of ID in the placement regions
            // within R regions of the origin, nearest first ("cx cz block-x block-z")
            use basalt_worldgen::structure::Placement;
            let g = basalt_worldgen::Generator::new(args[4].parse()?);
            let s = g.structures().unwrap();
            let def = *s.by_name.get(args[2].as_str()).expect("structure id");
            let set = s.set_of(def).expect("set");
            let Placement::Spread(sp) = &s.sets[set].placement else {
                bail!("not a spread placement")
            };
            let mut out = Vec::new();
            for rx in -r..r {
                for rz in -r..r {
                    let (cx, cz) = basalt_worldgen::structure::spread_potential(
                        g.seed,
                        sp,
                        rx * sp.spacing,
                        rz * sp.spacing,
                    );
                    if !g.is_structure_chunk(set, cx, cz) {
                        continue;
                    }
                    if let Some(st) = g.select_start(set, cx, cz, false) {
                        if st.def == def {
                            out.push((cx * cx + cz * cz, cx, cz));
                        }
                    }
                }
            }
            out.sort();
            for (_, cx, cz) in out {
                println!("{cx} {cz} {} {}", cx * 16, cz * 16);
            }
        }
        "bblocks" => {
            // bblocks SEED X0,Y0,Z0,X1,Y1,Z1 (dir argument = seed): Basalt's blocks in the box
            // ("x y z state" lines like `blocks`); env BASALT_NO_STRUCTURES for bare terrain
            let g = basalt_worldgen::Generator::new(args[2].parse()?);
            let v: Vec<i32> = args[3].split(',').map(|x| x.parse().unwrap()).collect();
            for cx in (v[0] >> 4)..=(v[3] >> 4) {
                for cz in (v[2] >> 4)..=(v[5] >> 4) {
                    let b = g.generate_chunk(cx, cz);
                    for x in v[0].max(cx * 16)..=v[3].min(cx * 16 + 15) {
                        for z in v[2].max(cz * 16)..=v[5].min(cz * 16 + 15) {
                            for y in v[1]..=v[4] {
                                let s = b.get((x & 15) as usize, y, (z & 15) as usize);
                                println!(
                                    "{x} {y} {z} {}",
                                    basalt_data::BlockState(s as u32).to_string()
                                );
                            }
                        }
                    }
                }
            }
        }
        "placerand" => {
            // placerand SEED ID CX,CZ N (dir argument = seed): the placement random of structure
            // ID while decorating chunk CX,CZ (as place_structures seeds it): N raw 31-bit draws
            // shown as fractions
            use basalt_worldgen::random::{set_decoration_seed, set_feature_seed, Rng};
            let seed: i64 = args[2].parse()?;
            let g = basalt_worldgen::Generator::new(seed);
            let s = g.structures().unwrap();
            let def = *s.by_name.get(args[3].as_str()).expect("id");
            let step = s.defs[def].step;
            let idx = s.defs.iter().take(def).filter(|d| d.step == step).count();
            let v: Vec<i32> = args[4].split(',').map(|x| x.parse().unwrap()).collect();
            let mut r = basalt_worldgen::features::R::new(0);
            let deco = set_decoration_seed(&mut r, seed, v[0] * 16, v[1] * 16);
            set_feature_seed(&mut r, deco, idx as i32, step as i32);
            let n: usize = args[5].parse()?;
            let fl = std::env::var("FLOAT").is_ok();
            let f: Vec<String> = (0..n)
                .map(|_| {
                    if fl {
                        format!("{:.4}", r.next_float())
                    } else {
                        format!(
                            "{:.3}",
                            r.next_int_bounded(1 << 30) as f64 / (1u64 << 30) as f64
                        )
                    }
                })
                .collect();
            println!("step {step} idx {idx}: {}", f.join(" "));
        }
        "footprint" => {
            // footprint SEED X0,Z0,W,D (dir argument = seed): WORLD_SURFACE_WG first free y over
            // the area as a JSON grid [z][x], then OCEAN_FLOOR_WG
            let g = basalt_worldgen::Generator::new(args[2].parse()?);
            let v: Vec<i32> = args[3].split(',').map(|x| x.parse().unwrap()).collect();
            for of in [false, true] {
                let grid: Vec<Vec<i32>> = (0..v[3])
                    .map(|z| {
                        (0..v[2])
                            .map(|x| g.first_free(v[0] + x, v[1] + z, of))
                            .collect()
                    })
                    .collect();
                println!("{}", serde_json::json!(grid));
            }
        }
        "wsheight" => {
            // wsheight SEED X,Z... (dir argument = seed): WORLD_SURFACE_WG / OCEAN_FLOOR_WG first free y
            let g = basalt_worldgen::Generator::new(args[2].parse()?);
            for p in &args[3..] {
                let v: Vec<i32> = p.split(',').map(|x| x.parse().unwrap()).collect();
                println!(
                    "{p}: ws={} of={}",
                    g.first_free(v[0], v[1], false),
                    g.first_free(v[0], v[1], true)
                );
            }
        }
        "bpieces" => {
            // bpieces CX,CZ SEED (dir argument = "CX,CZ"): Basalt's pieces of the starts in that
            // chunk as JSON lines (box, ground delta, rigid, junctions) for checking against vanilla
            use basalt_worldgen::structure::PieceKind;
            let v: Vec<i32> = args[2].split(',').map(|x| x.parse().unwrap()).collect();
            let g = basalt_worldgen::Generator::new(args[3].parse()?);
            for st in g.structure_starts(v[0], v[1]).iter() {
                for p in &st.pieces {
                    if let PieceKind::Pool {
                        element,
                        ground_delta,
                        junctions,
                        ..
                    } = &p.kind
                    {
                        let js: Vec<_> = junctions
                            .iter()
                            .map(|j| serde_json::json!([j.x, j.y, j.z, j.dy, j.rigid]))
                            .collect();
                        let b = p.bb;
                        println!(
                            "{}",
                            serde_json::json!({"bb": [b.min[0], b.min[1], b.min[2], b.max[0], b.max[1], b.max[2]],
                                "gd": ground_delta, "rigid": element.rigid(), "junctions": js})
                        );
                    }
                }
            }
        }
        "probe" => {
            // probe SET: placement chunks of a set in the square with vanilla presence + biomes at several points
            let seed: i64 = args[5].parse()?;
            let g = basalt_worldgen::Generator::new(seed);
            let s = g.structures().unwrap();
            let set = s.sets.iter().position(|x| x.name == args[4]).expect("set");
            let (vs, _, have) = vanilla_starts_ex(dir, r)?;
            for cx in -r..r {
                for cz in -r..r {
                    if !have.contains(&(cx, cz)) || !g.is_structure_chunk(set, cx, cz) {
                        continue;
                    }
                    let v: Vec<&str> = vs
                        .iter()
                        .filter(|v| {
                            v.cx == cx
                                && v.cz == cz
                                && s.sets[set].entries.iter().any(|e| s.defs[e.0].name == v.id)
                        })
                        .map(|v| v.id.as_str())
                        .collect();
                    let mut line = format!("{cx} {cz} vanilla={v:?}");
                    for (dx, dz) in [(8, 8), (0, 0), (8, 0), (0, 8)] {
                        for y in [-60, 0, 50, 100] {
                            let b = g.biome_at(cx * 16 + dx, y, cz * 16 + dz);
                            line += &format!(" {dx},{y},{dz}={}", g.biome_names[b as usize]);
                        }
                    }
                    println!("{line}");
                }
            }
        }
        "template" => {
            let g = basalt_worldgen::Generator::new(0);
            let t = g
                .structures()
                .unwrap()
                .templates
                .get(&args[2])
                .expect("template");
            println!(
                "size {:?} blocks {} palettes {} entities {}",
                t.size,
                t.blocks.len(),
                t.palettes.len(),
                t.entities.len()
            );
            for j in &t.jigsaws {
                println!("  {:?}", j);
            }
        }
        "rand" => {
            // rand ID N...: for every vanilla start of ID, replay the start random: rotation + nextInt(N) per N
            use basalt_worldgen::random::{set_large_feature_seed, JavaRandom, Rng};
            let seed: i64 = args[5].parse()?;
            let (vs, _) = vanilla_starts(dir, r)?;
            for v in vs.iter().filter(|v| v.id == args[4]) {
                let mut line = format!(
                    "{} {} {:?}:",
                    v.cx,
                    v.cz,
                    v.pieces.first().map(|p| p.0.clone())
                );
                for n in &args[6..] {
                    let n: i32 = n.parse()?;
                    let mut rr = JavaRandom::new(0);
                    set_large_feature_seed(&mut rr, seed, v.cx, v.cz);
                    let a = rr.next_int_bounded(4);
                    let b = rr.next_int_bounded(n);
                    line += &format!(" rot={a} i{n}={b}");
                }
                println!("{line}");
            }
        }
        "locate" => {
            // locate ID X,Z SEED   (dir argument = id)
            let seed: i64 = args[4].parse()?;
            let g = basalt_worldgen::Generator::new(seed);
            let v: Vec<i32> = args[3].split(',').map(|x| x.parse().unwrap()).collect();
            let t = std::time::Instant::now();
            println!(
                "{:?} in {:?}",
                g.locate(&args[2], v[0], v[1], 100),
                t.elapsed()
            );
        }
        "raw" => {
            // raw ID SEED: first 8 draws of the start random as float / int10 / int4 / int3 interpretations
            use basalt_worldgen::random::{set_large_feature_seed, JavaRandom};
            let seed: i64 = args[5].parse()?;
            let (vs, _) = vanilla_starts(dir, r)?;
            for v in vs.iter().filter(|v| v.id.starts_with(args[4].as_str())) {
                let mut rr = JavaRandom::new(0);
                set_large_feature_seed(&mut rr, seed, v.cx, v.cz);
                let mut line = format!(
                    "{} {} {} {:?}:",
                    v.id,
                    v.cx,
                    v.cz,
                    v.pieces.first().map(|p| p.0.clone())
                );
                for _ in 0..8 {
                    let x = rr.next(32) as u32;
                    let f = (x >> 8) as f32 / (1 << 24) as f32;
                    let n31 = (x >> 1) as i32;
                    line += &format!(
                        " [{f:.3} i10={} i4={} i3={} i8={}]",
                        n31 % 10,
                        ((4i64 * n31 as i64) >> 31),
                        n31 % 3,
                        ((8i64 * n31 as i64) >> 31)
                    );
                }
                println!("{line}");
            }
        }
        "heights" => {
            // heights ID SEED: WG heights (surface, ocean floor) at the corners of each vanilla piece box
            let seed: i64 = args[5].parse()?;
            let g = basalt_worldgen::Generator::new(seed);
            let (vs, _) = vanilla_starts(dir, r)?;
            for v in vs.iter().filter(|v| v.id.starts_with(args[4].as_str())) {
                let Some((n, b)) = v.pieces.first() else {
                    continue;
                };
                let cs = [(b[0], b[2]), (b[3], b[2]), (b[0], b[5]), (b[3], b[5])];
                let ws: Vec<i32> = cs.iter().map(|c| g.first_free(c.0, c.1, false)).collect();
                let os: Vec<i32> = cs.iter().map(|c| g.first_free(c.0, c.1, true)).collect();
                let mut all = Vec::new();
                for x in b[0]..=b[3] {
                    for z in b[2]..=b[5] {
                        all.push(g.first_free(x, z, true));
                    }
                }
                all.sort();
                let avg = all.iter().map(|h| *h as i64).sum::<i64>() / all.len() as i64;
                println!(
                    "{} {} {} {n} bb={:?} ws={ws:?} of={os:?} min={} med={} avg={avg}",
                    v.id,
                    v.cx,
                    v.cz,
                    b,
                    all[0],
                    all[all.len() / 2]
                );
            }
        }
        "h" => {
            let g = basalt_worldgen::Generator::new(
                args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(12345),
            );
            for p in args[2].split(';') {
                let v: Vec<i32> = p.split(',').map(|x| x.parse().unwrap()).collect();
                println!(
                    "{p} surface={} floor={} biome={}",
                    g.first_free(v[0], v[1], false),
                    g.first_free(v[0], v[1], true),
                    g.biome_names[g.biome_at(v[0], 64, v[1]) as usize]
                );
            }
        }
        "biome" => {
            let g = basalt_worldgen::Generator::new(
                args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(12345),
            );
            for p in args[2].split(';') {
                let v: Vec<i32> = p.split(',').map(|x| x.parse().unwrap()).collect();
                let b = g.biome_at(v[0], v[1], v[2]);
                println!("{p} {}", g.biome_names[b as usize]);
            }
        }
        c => bail!("unknown command {c}"),
    }
    Ok(())
}

fn union(p: &[(String, [i32; 6])]) -> [i32; 6] {
    let mut b = [i32::MAX, i32::MAX, i32::MAX, i32::MIN, i32::MIN, i32::MIN];
    for (_, q) in p {
        for i in 0..3 {
            b[i] = b[i].min(q[i]);
            b[i + 3] = b[i + 3].max(q[i + 3]);
        }
    }
    b
}

fn compare(dir: &Path, r: i32, seed: i64) -> Result<()> {
    use basalt_worldgen::structure::PieceKind;
    let g = basalt_worldgen::Generator::new(seed);
    let (vs, _, have) = vanilla_starts_ex(dir, r)?;
    let mut van: HashMap<(i32, i32, String), &VStart> = HashMap::new();
    for v in &vs {
        van.insert((v.cx, v.cz, v.id.clone()), v);
    }
    let s = g.structures().expect("structures");
    let only = std::env::var("ONLY").ok();
    let verbose = std::env::var("VERBOSE").is_ok();
    // per type: (vanilla, basalt, matched, pieces vanilla, pieces matched)
    let mut per: BTreeMap<String, [usize; 5]> = BTreeMap::new();
    let mut seen = std::collections::HashSet::new();
    let (ox, oz) = center();
    for cx in ox - r..ox + r {
        for cz in oz - r..oz + r {
            if !have.contains(&(cx, cz)) {
                continue;
            }
            let starts = g.structure_starts(cx, cz);
            for st in starts.iter() {
                let id = s.defs[st.def].name.clone();
                if let Some(o) = &only {
                    if !id.contains(o.as_str()) {
                        continue;
                    }
                }
                let e = per.entry(id.clone()).or_default();
                e[1] += 1;
                if let Some(v) = van.get(&(cx, cz, id.clone())) {
                    seen.insert((cx, cz, id.clone()));
                    e[2] += 1;
                    e[3] += v.pieces.len();
                    let mut ok = 0;
                    for (i, p) in st.pieces.iter().enumerate() {
                        let name = match &p.kind {
                            PieceKind::Pool { element, .. } => element.name(),
                            PieceKind::Template { name, .. } => name.clone(),
                            _ => String::new(),
                        };
                        let bb = [
                            p.bb.min[0],
                            p.bb.min[1],
                            p.bb.min[2],
                            p.bb.max[0],
                            p.bb.max[1],
                            p.bb.max[2],
                        ];
                        let vm = v.pieces.get(i);
                        let m = vm
                            .map(|(n, b)| {
                                *b == bb && {
                                    let nn = n.split(' ').nth(1).unwrap_or("");
                                    name.is_empty() || nn == name || nn.is_empty()
                                }
                            })
                            .unwrap_or(false);
                        if m {
                            ok += 1;
                        }
                        if verbose && (!m || std::env::var("ALL").is_ok()) {
                            println!(
                                "  {id} {cx} {cz} piece {i}: basalt {name} {bb:?} | vanilla {:?}",
                                vm
                            );
                        }
                    }
                    e[4] += ok;
                    if verbose {
                        println!(
                            "{id} {cx} {cz}: pieces {}/{} (basalt {})",
                            ok,
                            v.pieces.len(),
                            st.pieces.len()
                        );
                    }
                } else if verbose {
                    println!("{id} {cx} {cz}: basalt only");
                }
            }
        }
    }
    for v in &vs {
        if v.cx < ox - r || v.cx >= ox + r || v.cz < oz - r || v.cz >= oz + r {
            continue;
        }
        if let Some(o) = &only {
            if !v.id.contains(o.as_str()) {
                continue;
            }
        }
        per.entry(v.id.clone()).or_default()[0] += 1;
        if verbose && !seen.contains(&(v.cx, v.cz, v.id.clone())) {
            println!("{} {} {}: vanilla only", v.id, v.cx, v.cz);
        }
    }
    let (mut tv, mut tb, mut tm) = (0, 0, 0);
    for (id, e) in &per {
        println!(
            "STRUCT {id:24} vanilla={} basalt={} matched={} pieces={}/{}",
            e[0], e[1], e[2], e[4], e[3]
        );
        tv += e[0];
        tb += e[1];
        tm += e[2];
    }
    let mut blocks = String::new();
    if std::env::var("NO_BLOCKS").is_err() {
        let max: usize = std::env::var("MAX_CHUNKS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(400);
        let sel: Vec<&VStart> = vs
            .iter()
            .filter(|v| seen.contains(&(v.cx, v.cz, v.id.clone())))
            .filter(|v| {
                only.as_ref()
                    .map(|o| v.id.contains(o.as_str()))
                    .unwrap_or(true)
            })
            .filter(|v| {
                std::env::var("BLOCK_TYPES")
                    .map(|t| t.split(',').any(|x| v.id.starts_with(x)))
                    .unwrap_or(true)
            })
            .collect();
        let bp = block_parity(dir, &g, &sel, max)?;
        let (mut m, mut t) = (0u64, 0u64);
        for (id, (a, b, c)) in &bp {
            println!(
                "BLOCKS {id:24} {:.2}% of {b} (without structures {:.2}%)",
                100.0 * *a as f64 / (*b).max(1) as f64,
                100.0 * *c as f64 / (*b).max(1) as f64
            );
            m += a;
            t += b;
        }
        blocks = format!(" blocks={:.2}", 100.0 * m as f64 / t.max(1) as f64);
    }
    println!(
        "RESULT seed={seed} chunks={} vanilla={tv} basalt={tb} matched={tm}{blocks}",
        (2 * r) * (2 * r)
    );
    Ok(())
}

/// Fully generated vanilla chunk blocks (24 sections from y=-64, index (y+64)<<8|z<<4|x).
fn vanilla_blocks(dir: &Path, cx: i32, cz: i32) -> Result<Option<Vec<u16>>> {
    use basalt_data::BlockState;
    let Some(root) = read_nbt(dir, cx, cz)? else {
        return Ok(None);
    };
    if root.get("Status").and_then(|s| s.as_str()) != Some("minecraft:full") {
        return Ok(None);
    }
    let mut blocks = vec![0u16; 24 * 4096];
    for sec in root
        .get("sections")
        .and_then(|s| s.as_list())
        .map(|l| l.as_slice())
        .unwrap_or(&[])
    {
        let sy = sec.get("Y").and_then(|y| y.as_i64()).unwrap_or(-100) as i32;
        let si = sy + 4;
        if !(0..24).contains(&si) {
            continue;
        }
        let Some(bs) = sec.get("block_states") else {
            continue;
        };
        let pal: Vec<u16> = bs
            .get("palette")
            .and_then(|p| p.as_list())
            .map(|l| {
                l.iter()
                    .map(|e| {
                        let name = e
                            .get("Name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("minecraft:air");
                        let mut st = BlockState::from_name(name).unwrap_or(BlockState::AIR);
                        if let Some(Tag::Compound(p)) = e.get("Properties") {
                            for (k, v) in p {
                                if let Some(n) = v.as_str().and_then(|v| st.with(k, v)) {
                                    st = n;
                                }
                            }
                        }
                        st.0 as u16
                    })
                    .collect()
            })
            .unwrap_or_default();
        if pal.is_empty() {
            continue;
        }
        let base = si as usize * 4096;
        let data: Vec<i64> = match bs.get("data") {
            Some(Tag::LongArray(d)) => d.clone(),
            _ => Vec::new(),
        };
        if pal.len() == 1 || data.is_empty() {
            for i in 0..4096 {
                blocks[base + i] = pal[0];
            }
            continue;
        }
        let bits = ((usize::BITS - (pal.len() - 1).leading_zeros()) as usize).max(4);
        let per = 64 / bits;
        let mask = (1u64 << bits) - 1;
        for i in 0..4096 {
            let v = ((data[i / per] as u64) >> ((i % per) * bits)) & mask;
            blocks[base + i] = pal.get(v as usize).copied().unwrap_or(0);
        }
    }
    Ok(Some(blocks))
}

/// Block-exact % of Basalt vs vanilla inside the vanilla piece boxes of the given starts
/// (fully generated chunks only). Returns (matching, total) per structure id.
fn block_parity(
    dir: &Path,
    g: &basalt_worldgen::Generator,
    starts: &[&VStart],
    max_chunks: usize,
) -> Result<BTreeMap<String, (u64, u64, u64)>> {
    let mut per: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();
    std::env::set_var("BASALT_NO_STRUCTURES", "1");
    let g0 = basalt_worldgen::Generator::new(g.seed);
    std::env::remove_var("BASALT_NO_STRUCTURES");
    let mut chunks: BTreeMap<(i32, i32), Vec<(String, [i32; 6])>> = BTreeMap::new();
    // HALO=n: also count the blocks within n of a piece box (outside every box) as "<id> halo"
    // (terrain adaptation reaches beyond the boxes); listed after the boxes of each chunk.
    let halo: i32 = std::env::var("HALO")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    for pass in 0..2 {
        if pass == 1 && halo == 0 {
            break;
        }
        for st in starts {
            for (_, bb) in &st.pieces {
                let (bb, id) = if pass == 0 {
                    (*bb, st.id.clone())
                } else {
                    let h = halo;
                    (
                        [
                            bb[0] - h,
                            bb[1] - h,
                            bb[2] - h,
                            bb[3] + h,
                            bb[4] + h,
                            bb[5] + h,
                        ],
                        format!("{} halo", st.id),
                    )
                };
                for cx in (bb[0] >> 4)..=(bb[3] >> 4) {
                    for cz in (bb[2] >> 4)..=(bb[5] >> 4) {
                        if pass == 1 && !chunks.contains_key(&(cx, cz)) {
                            continue;
                        }
                        chunks.entry((cx, cz)).or_default().push((id.clone(), bb));
                    }
                }
            }
        }
    }
    let detail = std::env::var("BLOCKS_DETAIL").is_ok();
    for (n, ((cx, cz), boxes)) in chunks.iter().enumerate() {
        if n >= max_chunks {
            break;
        }
        let Some(v) = vanilla_blocks(dir, *cx, *cz)? else {
            continue;
        };
        let b = g.generate_chunk(*cx, *cz);
        let b0 = g0.generate_chunk(*cx, *cz);
        let mut done = std::collections::HashSet::new();
        for (id, bb) in boxes {
            for x in bb[0].max(cx * 16)..=bb[3].min(cx * 16 + 15) {
                for z in bb[2].max(cz * 16)..=bb[5].min(cz * 16 + 15) {
                    for y in bb[1].max(-64)..=bb[4].min(319) {
                        if !done.insert((x, y, z)) {
                            continue;
                        }
                        let (lx, lz) = ((x & 15) as usize, (z & 15) as usize);
                        let vb = v[(((y + 64) as usize) << 8) | (lz << 4) | lx];
                        let ob = b.get(lx, y, lz);
                        let e = per.entry(id.clone()).or_default();
                        e.1 += 1;
                        if b0.get(lx, y, lz) == vb {
                            e.2 += 1;
                        }
                        if vb == ob {
                            e.0 += 1;
                        } else if detail {
                            println!(
                                "  diff {id} {x} {y} {z}: basalt {} vanilla {}",
                                basalt_data::BlockState(ob as u32).to_string(),
                                basalt_data::BlockState(vb as u32).to_string()
                            );
                        }
                    }
                }
            }
        }
    }
    Ok(per)
}

/// Terrain adaptation measuring dump (see the module doc, `adapt`).
fn adapt(dir: &Path, r: i32, seed: i64, dir0: &Path, out: &str) -> Result<()> {
    use std::io::Write;
    let only = std::env::var("ONLY").unwrap_or_default();
    let margin: i32 = std::env::var("MARGIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(16);
    std::env::set_var("BASALT_NO_STRUCTURES", "1");
    let g0 = basalt_worldgen::Generator::new(seed);
    let (ox, oz) = center();
    let mut pieces = Vec::new();
    let mut boxes: Vec<[i32; 6]> = Vec::new();
    for cx in ox - r..ox + r {
        for cz in oz - r..oz + r {
            let Some(root) = read_nbt(dir, cx, cz)? else {
                continue;
            };
            let Some(Tag::Compound(starts)) = root.get("structures").and_then(|s| s.get("starts"))
            else {
                continue;
            };
            for (id, s) in starts {
                if s.get("id").and_then(|i| i.as_str()) == Some("INVALID")
                    || !id.contains(only.as_str())
                {
                    continue;
                }
                for c in s
                    .get("Children")
                    .and_then(|c| c.as_list())
                    .map(|l| l.as_slice())
                    .unwrap_or(&[])
                {
                    let Some(Tag::IntArray(bb)) = c.get("BB") else {
                        continue;
                    };
                    let gi = |t: &Tag, k: &str| t.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
                    let mut js = Vec::new();
                    for j in c
                        .get("junctions")
                        .and_then(|c| c.as_list())
                        .map(|l| l.as_slice())
                        .unwrap_or(&[])
                    {
                        js.push(serde_json::json!([
                            gi(j, "source_x"),
                            gi(j, "source_ground_y"),
                            gi(j, "source_z"),
                            gi(j, "delta_y"),
                            j.get("dest_proj").and_then(|v| v.as_str()).unwrap_or("")
                        ]));
                    }
                    let el = c.get("pool_element");
                    pieces.push(serde_json::json!({
                        "start": format!("{id} {cx} {cz}"),
                        "bb": bb,
                        "gd": gi(c, "ground_level_delta"),
                        "proj": el.and_then(|e| e.get("projection")).and_then(|v| v.as_str()).unwrap_or(""),
                        "loc": el.and_then(|e| e.get("location")).and_then(|v| v.as_str())
                            .or_else(|| c.get("Template").and_then(|t| t.as_str())).unwrap_or(""),
                        "piece": c.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                        "junctions": js,
                    }));
                    boxes.push([bb[0], bb[1], bb[2], bb[3], bb[4], bb[5]]);
                }
            }
        }
    }
    std::fs::write(
        format!("{out}.pieces.json"),
        serde_json::to_string(&pieces)?,
    )?;
    // chunks within `margin` of a box, y range of the boxes in reach +- margin
    let mut chunks: BTreeMap<(i32, i32), (i32, i32)> = BTreeMap::new();
    for b in &boxes {
        for cx in ((b[0] - margin) >> 4)..=((b[3] + margin) >> 4) {
            for cz in ((b[2] - margin) >> 4)..=((b[5] + margin) >> 4) {
                let e = chunks.entry((cx, cz)).or_insert((i32::MAX, i32::MIN));
                e.0 = e.0.min(b[1] - margin);
                e.1 = e.1.max(b[4] + margin);
            }
        }
    }
    let mut f = std::io::BufWriter::new(std::fs::File::create(format!("{out}.bin"))?);
    let mut states: BTreeMap<u16, String> = BTreeMap::new();
    let mut n = 0usize;
    for (&(cx, cz), &(y0, y1)) in &chunks {
        let (Some(v), Some(v0)) = (vanilla_blocks(dir, cx, cz)?, vanilla_blocks(dir0, cx, cz)?)
        else {
            continue;
        };
        let d = g0.density_dump(cx, cz);
        let b0 = g0.generate_chunk(cx, cz);
        for y in y0.max(-64)..=y1.min(319) {
            for lz in 0..16usize {
                for lx in 0..16usize {
                    let i = (((y + 64) as usize) << 8) | (lz << 4) | lx;
                    let bs = b0.get(lx, y, lz);
                    for st in [v[i], v0[i], bs] {
                        states
                            .entry(st)
                            .or_insert_with(|| basalt_data::BlockState(st as u32).to_string());
                    }
                    f.write_all(&(cx * 16 + lx as i32).to_le_bytes())?;
                    f.write_all(&y.to_le_bytes())?;
                    f.write_all(&(cz * 16 + lz as i32).to_le_bytes())?;
                    f.write_all(&v[i].to_le_bytes())?;
                    f.write_all(&v0[i].to_le_bytes())?;
                    f.write_all(&(d[i] as f32).to_le_bytes())?;
                    f.write_all(&bs.to_le_bytes())?;
                    n += 1;
                }
            }
        }
    }
    std::fs::write(
        format!("{out}.states.json"),
        serde_json::to_string(&states)?,
    )?;
    println!(
        "pieces={} chunks={} samples={n}",
        pieces.len(),
        chunks.len()
    );
    Ok(())
}

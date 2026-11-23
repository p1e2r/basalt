//! Structure parity against a vanilla world's region files.
//!
//! cargo run -p basalt-worldgen --release --example structures -- <cmd> <region dir> <R> [seed]
//!   count   REGION R        — chunks in -R..R-1 by status (`full=<n> starts=<n>`)
//!   dump    REGION R        — vanilla structure starts in the square (id chunk pieces bbox)
//!   compare REGION R SEED   — Basalt's structure starts vs vanilla's in the square, per structure
//!                             type, plus block-exact % inside the vanilla structure boxes of fully
//!                             generated chunks. Last line `RESULT ...`.

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
                        " [{f:.3} i10={} i4={} i3={}]",
                        n31 % 10,
                        ((4i64 * n31 as i64) >> 31),
                        n31 % 3
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
    for st in starts {
        for (_, bb) in &st.pieces {
            for cx in (bb[0] >> 4)..=(bb[3] >> 4) {
                for cz in (bb[2] >> 4)..=(bb[5] >> 4) {
                    chunks
                        .entry((cx, cz))
                        .or_default()
                        .push((st.id.clone(), *bb));
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

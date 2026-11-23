//! Compare generated chunks against a vanilla world's region files.
//!
//! cargo run -p basalt-worldgen --release --example compare -- <region dir> <cx> <cz> <radius> [seed]
//!
//! The last line printed is always
//! `RESULT seed=<seed> chunks=<n> ms_per_chunk=<t> biomes=<%> shape=<%> exact=<%> features=<%>`.
//! Exit status: 0 on success, 2 if the region has no fully generated chunks in range,
//! 1 on usage / IO errors. Env: DATA dir via BASALT_DATA; debugging via DETAIL, SLICE,
//! HSLICE, BASES, PAIR, FPAIR, TOP, FTOP.
//!
//! Prints, per stage, the fraction of positions that match vanilla:
//!  * shape   — solid / air / fluid category per block
//!  * biome   — biome id per 4×4×4 cell
//!  * blocks  — exact block id, restricted to "terrain" positions (both sides natural terrain blocks)

use anyhow::{bail, Context, Result};
use basalt_data::BlockState;
use basalt_nbt::Tag;
use basalt_worldgen::{Dimension, GeneratedChunk, Generator};
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

// Dimension layout of the compared world (set from the region path: `.../the_nether/region` = nether).
static DIM_MIN_Y: AtomicI32 = AtomicI32::new(-64);
static DIM_SECTIONS: AtomicUsize = AtomicUsize::new(24);
#[allow(non_snake_case)]
fn MIN_Y() -> i32 { DIM_MIN_Y.load(Ordering::Relaxed) }
fn nsec() -> usize { DIM_SECTIONS.load(Ordering::Relaxed) }
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::time::Instant;

struct VanillaChunk {
    blocks: Vec<u16>, // 98304, index (y-MIN_Y())<<8 | z<<4 | x
    biomes: Vec<String>, // 24*64
    /// The chunk references or starts a structure.
    structure: bool,
}

fn read_nbt(dir: &Path, cx: i32, cz: i32) -> Result<Option<Tag>> {
    let path = dir.join(format!("r.{}.{}.mca", cx >> 5, cz >> 5));
    let Ok(data) = std::fs::read(&path) else { return Ok(None) };
    if data.len() < 8192 { return Ok(None); }
    let i = (((cx & 31) + (cz & 31) * 32) * 4) as usize;
    let off = ((data[i] as usize) << 16 | (data[i + 1] as usize) << 8 | data[i + 2] as usize) * 4096;
    if off == 0 { return Ok(None); }
    let len = u32::from_be_bytes(data[off..off + 4].try_into()?) as usize;
    let comp = data[off + 4];
    let raw = &data[off + 5..off + 4 + len];
    let mut buf = Vec::new();
    match comp {
        1 => { flate2::read::GzDecoder::new(raw).read_to_end(&mut buf)?; }
        2 => { flate2::read::ZlibDecoder::new(raw).read_to_end(&mut buf)?; }
        3 => buf.extend_from_slice(raw),
        c => bail!("unsupported compression {c}"),
    }
    let (_, root) = basalt_nbt::Reader::new(&buf).named()?;
    Ok(Some(root))
}

/// Bounding boxes of all structure pieces started in a chunk.
fn structure_boxes(root: &Tag) -> Vec<[i32; 6]> {
    let mut out = Vec::new();
    let Some(Tag::Compound(starts)) = root.get("structures").and_then(|s| s.get("starts")) else { return out };
    for st in starts.values() {
        for c in st.get("Children").and_then(|c| c.as_list()).map(|l| l.as_slice()).unwrap_or(&[]) {
            if let Some(Tag::IntArray(bb)) = c.get("BB") {
                if bb.len() == 6 { out.push([bb[0], bb[1], bb[2], bb[3], bb[4], bb[5]]); }
            }
        }
    }
    out
}

fn read_chunk(dir: &Path, cx: i32, cz: i32) -> Result<Option<VanillaChunk>> {
    let Some(root) = read_nbt(dir, cx, cz)? else { return Ok(None) };
    let status = root.get("Status").and_then(|s| s.as_str()).unwrap_or("");
    if status != "minecraft:full" { return Ok(None); }
    let mut blocks = vec![0u16; nsec() * 4096];
    let mut biomes = vec![String::new(); nsec() * 64];
    let sections = root.get("sections").and_then(|s| s.as_list()).context("sections")?;
    for s in sections {
        let sy = s.get("Y").and_then(|y| y.as_i64()).context("Y")? as i32;
        let si = sy - MIN_Y() / 16;
        if !(0..nsec() as i32).contains(&si) { continue; }
        let base = si as usize * 4096;
        if let Some(bs) = s.get("block_states") {
            let pal: Vec<u16> = bs.get("palette").and_then(|p| p.as_list()).context("palette")?.iter().map(|e| {
                let name = e.get("Name").and_then(|n| n.as_str()).unwrap_or("minecraft:air");
                let mut st = BlockState::from_name(name).unwrap_or(BlockState::AIR);
                if let Some(Tag::Compound(p)) = e.get("Properties") {
                    for (k, v) in p { if let Some(n) = v.as_str().and_then(|v| st.with(k, v)) { st = n; } }
                }
                st.0 as u16
            }).collect();
            let data: Vec<i64> = match bs.get("data") { Some(Tag::LongArray(d)) => d.clone(), _ => Vec::new() };
            unpack(&pal, &data, 4, 4096, &mut blocks[base..base + 4096]);
        }
        if let Some(bi) = s.get("biomes") {
            let pal: Vec<String> = bi.get("palette").and_then(|p| p.as_list()).context("biome palette")?.iter().map(|e| e.as_str().unwrap_or("").trim_start_matches("minecraft:").to_string()).collect();
            let data: Vec<i64> = match bi.get("data") { Some(Tag::LongArray(d)) => d.clone(), _ => Vec::new() };
            let idx: Vec<u16> = (0..pal.len() as u16).collect();
            let mut out = vec![0u16; 64];
            unpack(&idx, &data, 1, 64, &mut out);
            for (j, v) in out.iter().enumerate() { biomes[si as usize * 64 + j] = pal[*v as usize].clone(); }
        }
    }
    let nonempty = |k: &str| root.get("structures").and_then(|s| s.get(k)).and_then(|r| r.as_compound()).map(|c| !c.is_empty()).unwrap_or(false);
    let structure = nonempty("References") || nonempty("starts");
    Ok(Some(VanillaChunk { blocks, biomes, structure }))
}

fn unpack(pal: &[u16], data: &[i64], min_bits: u32, n: usize, out: &mut [u16]) {
    if pal.len() == 1 || data.is_empty() { out.iter_mut().for_each(|o| *o = pal[0]); return; }
    let bits = (usize::BITS - (pal.len() - 1).leading_zeros()).max(min_bits) as usize;
    let per = 64 / bits;
    let mask = (1u64 << bits) - 1;
    for i in 0..n {
        let l = data[i / per] as u64;
        let v = (l >> ((i % per) * bits)) & mask;
        out[i] = pal.get(v as usize).copied().unwrap_or(0);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Cat { Air, Fluid, Solid }

fn cat(id: u16) -> Cat {
    let s = BlockState(id as u32);
    let n = s.name();
    if s.is_air() { return Cat::Air; }
    if matches!(n, "water" | "lava" | "bubble_column" | "kelp" | "kelp_plant" | "seagrass" | "tall_seagrass") { return Cat::Fluid; }
    if s.get("waterlogged") == Some("true") { return Cat::Fluid; }
    Cat::Solid
}

/// Natural terrain blocks placed by noise + surface rules (not features).
fn terrain(id: u16) -> bool {
    let n = BlockState(id as u32).name();
    matches!(n, "stone" | "deepslate" | "bedrock" | "dirt" | "grass_block" | "sand" | "red_sand" | "sandstone" | "red_sandstone"
        | "gravel" | "coarse_dirt" | "podzol" | "mycelium" | "snow_block" | "powder_snow" | "packed_ice" | "ice" | "blue_ice" | "calcite"
        | "mud" | "clay" | "terracotta" | "white_terracotta" | "orange_terracotta" | "yellow_terracotta" | "brown_terracotta"
        | "red_terracotta" | "light_gray_terracotta" | "water" | "lava" | "air" | "cave_air" | "granite" | "tuff" | "copper_ore"
        | "deepslate_iron_ore" | "raw_copper_block" | "raw_iron_block" | "diorite" | "andesite" | "coal_ore" | "deepslate_coal_ore"
        | "iron_ore" | "deepslate_copper_ore" | "gold_ore" | "deepslate_gold_ore" | "redstone_ore" | "deepslate_redstone_ore"
        | "diamond_ore" | "deepslate_diamond_ore" | "lapis_ore" | "deepslate_lapis_ore" | "emerald_ore" | "deepslate_emerald_ore"
        // nether noise + surface rules
        | "netherrack" | "soul_sand" | "soul_soil" | "basalt" | "blackstone" | "crimson_nylium" | "warped_nylium")
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 5 { bail!("usage: compare <region dir> <cx> <cz> <radius> [seed]"); }
    let dir = Path::new(&a[1]);
    let (cx0, cz0, rad): (i32, i32, i32) = (a[2].parse()?, a[3].parse()?, a[4].parse()?);
    let seed: i64 = a.get(5).map(|s| s.parse()).transpose()?.unwrap_or(12345);
    let detail = std::env::var("DETAIL").is_ok();
    let t = Instant::now();
    let dim = if dir.to_string_lossy().contains("the_nether") { Dimension::Nether } else { Dimension::Overworld };
    DIM_MIN_Y.store(dim.min_y(), Ordering::Relaxed);
    DIM_SECTIONS.store(dim.sections(), Ordering::Relaxed);
    let g = Generator::new_dim(seed, dim);
    eprintln!("loaded generator in {:?}", t.elapsed());
    let biome_names = basalt_worldgen::biome_names(&basalt_worldgen::default_data_dir()).unwrap_or_default();
    let (mut shape_ok, mut shape_n, mut bio_ok, mut bio_n, mut blk_ok, mut blk_n) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let mut confusion: HashMap<(Cat, Cat), u64> = HashMap::new();
    let mut blk_conf: HashMap<(String, String), u64> = HashMap::new();
    let mut bio_conf: HashMap<(String, String), u64> = HashMap::new();
    let mut gen_time = 0f64;
    let mut chunks = 0;
    let mut boxes: Vec<[i32; 6]> = Vec::new();
    for cx in cx0 - rad - 8..=cx0 + rad + 8 {
        for cz in cz0 - rad - 8..=cz0 + rad + 8 {
            if let Ok(Some(t)) = read_nbt(dir, cx, cz) { boxes.extend(structure_boxes(&t)); }
        }
    }
    let mut pair_count = 0;
    let (mut feat_ok, mut feat_n, mut struct_chunks) = (0u64, 0u64, 0);
    // vanilla block name -> (vanilla count, matched, ours-wrong count)
    let mut feat_types: HashMap<&'static str, (u64, u64, u64)> = HashMap::new();
    for cx in cx0 - rad..=cx0 + rad {
        for cz in cz0 - rad..=cz0 + rad {
            let Some(v) = read_chunk(dir, cx, cz)? else { continue };
            let t = Instant::now();
            let c: GeneratedChunk = g.generate_chunk(cx, cz);
            gen_time += t.elapsed().as_secs_f64();
            chunks += 1;
            let (bx0, bz0) = (cx * 16, cz * 16);
            let cboxes: Vec<[i32; 6]> = boxes.iter().filter(|b| b[3] >= bx0 - 1 && b[0] <= bx0 + 16 && b[5] >= bz0 - 1 && b[2] <= bz0 + 16).copied().collect();
            if v.structure { struct_chunks += 1; }
            let (mut cok, mut cn) = (0, 0);
            for si in 0..nsec() {
                for i in 0..4096 {
                    let ours = c.blocks[si][i];
                    let theirs = v.blocks[si * 4096 + i];
                    let (a, b) = (cat(ours), cat(theirs));
                    // Features (trees, plants, snow layers) add solids in air above the terrain: ignore those.
                    let tn = BlockState(theirs as u32).name();
                    let on = BlockState(ours as u32).name();
                    let feature = (a != Cat::Solid && b == Cat::Solid && !terrain(theirs))
                        || (b != Cat::Solid && a == Cat::Solid && !terrain(ours))
                        || matches!(tn, "snow" | "ice") || matches!(on, "snow" | "ice");
                    if !feature {
                        shape_n += 1; cn += 1;
                        if a == b { shape_ok += 1; cok += 1; } else { *confusion.entry((a, b)).or_default() += 1; }
                    }
                    let (wx, wy, wz) = (cx * 16 + (i & 15) as i32, (si * 16 + (i >> 8)) as i32 + MIN_Y(), cz * 16 + ((i >> 4) & 15) as i32);
                    let in_structure = cboxes.iter().any(|b| wx >= b[0] - 1 && wx <= b[3] + 1 && wy >= b[1] - 1 && wy <= b[4] + 1 && wz >= b[2] - 1 && wz <= b[5] + 1);
                    if !in_structure {
                        feat_n += 1;
                        let e = feat_types.entry(BlockState(theirs as u32).name()).or_insert((0, 0, 0));
                        e.0 += 1;
                        if ours == theirs { feat_ok += 1; e.1 += 1; } else {
                            if let Ok(fp) = std::env::var("FPAIR") {
                                let (on, tn) = (BlockState(ours as u32).to_string(), BlockState(theirs as u32).to_string());
                                if (on.contains(&fp) || tn.contains(&fp)) && pair_count < std::env::var("FLIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(60) {
                                    pair_count += 1;
                                    println!("  at {wx} {wy} {wz}: ours {on} vanilla {tn}");
                                }
                            }
                            feat_types.entry(BlockState(ours as u32).name()).or_insert((0, 0, 0)).2 += 1;
                        }
                    }
                    if terrain(ours) && terrain(theirs) {
                        blk_n += 1;
                        if ours == theirs { blk_ok += 1; } else {
                            if let Ok(pair) = std::env::var("PAIR") {
                                let (pa, pb) = pair.split_once(',').unwrap();
                                if BlockState(ours as u32).name() == pa && BlockState(theirs as u32).name() == pb && pair_count < 30 {
                                    pair_count += 1;
                                    let bi = &v.biomes[si * 64 + (((i >> 8) & 15) >> 2 << 4) + ((i >> 4 & 15) >> 2 << 2) + ((i & 15) >> 2)];
                                    println!("  {pair} at {} {} {} ({bi})", cx * 16 + (i & 15) as i32, (si * 16 + (i >> 8)) as i32 + MIN_Y(), cz * 16 + ((i >> 4) & 15) as i32);
                                }
                            }
                            *blk_conf.entry((BlockState(ours as u32).name().to_string(), BlockState(theirs as u32).name().to_string())).or_default() += 1;
                        }
                    }
                }
                for j in 0..64 {
                    let ours = biome_names.get(c.biomes[si][j] as usize).cloned().unwrap_or_default();
                    let theirs = &v.biomes[si * 64 + j];
                    if theirs.is_empty() { continue; }
                    bio_n += 1;
                    if &ours == theirs { bio_ok += 1; } else {
                        if detail { println!("biome mismatch at quart {} {} {}: ours {ours} vanilla {theirs}", cx * 4 + (j & 3) as i32, (si * 4 + (j >> 4)) as i32 - 16, cz * 4 + ((j >> 2) & 3) as i32); } *bio_conf.entry((ours, theirs.clone())).or_default() += 1; }
                }
            }
            if std::env::var("BASES").is_ok() && cx == cx0 && cz == cz0 {
                let is_log = |id: u16| { let n = BlockState(id as u32).name(); n.ends_with("_log") || n.ends_with("mushroom_stem") };
                for (label, get) in [("ours", &(|i: usize| c.blocks[i >> 12][i & 4095]) as &dyn Fn(usize) -> u16), ("vanilla", &|i: usize| v.blocks[i])] {
                    let mut out = Vec::new();
                    for i in 4096..nsec() * 4096 {
                        if is_log(get(i)) && !is_log(get(i - 256)) {
                            out.push(format!("({},{},{} {})", cx * 16 + (i & 15) as i32, (i >> 8) as i32 + MIN_Y(), cz * 16 + ((i >> 4) & 15) as i32, BlockState(get(i) as u32).name()));
                        }
                    }
                    println!("{label} log bases: {}", out.join(" "));
                }
            }
            if let Ok(hs) = std::env::var("HSLICE") {
                if cx == cx0 && cz == cz0 {
                    let ch = |id: u16| {
                        let n = BlockState(id as u32).name();
                        if n.ends_with("_log") { 'L' } else if n.ends_with("_leaves") { 'l' } else if n == "snow" { 's' } else {
                            match cat(id) { Cat::Air => '.', Cat::Fluid => '~', Cat::Solid => if terrain(id) { '#' } else { 'f' } }
                        }
                    };
                    for ys in hs.split(',') {
                        let y: i32 = ys.parse().unwrap_or(64);
                        println!("y={y}  (rows z, cols x)");
                        for z in 0..16usize {
                            let mut l1 = String::new();
                            let mut l2 = String::new();
                            for x in 0..16usize {
                                let i = (((y - MIN_Y()) as usize) << 8) | (z << 4) | x;
                                l1.push(ch(c.blocks[i >> 12][i & 4095]));
                                l2.push(ch(v.blocks[i]));
                            }
                            println!("{z:3} {l1}  {l2}");
                        }
                    }
                }
            }
            if let Ok(sl) = std::env::var("SLICE") {
                if cx == cx0 && cz == cz0 {
                    let lx: usize = sl.parse().unwrap_or(0);
                    let ch = |id: u16| {
                        let n = BlockState(id as u32).name();
                        if n.ends_with("_log") { 'L' } else if n.ends_with("_leaves") { 'l' } else if n == "snow" { 's' } else {
                            match cat(id) { Cat::Air => '.', Cat::Fluid => '~', Cat::Solid => if terrain(id) { '#' } else { 'f' } }
                        }
                    };
                    for yy in (0..nsec() * 16).rev() {
                        let mut l1 = String::new();
                        let mut l2 = String::new();
                        for z in 0..16 {
                            let i = (yy << 8) | (z << 4) | lx;
                            l1.push(ch(c.blocks[i >> 12][i & 4095]));
                            l2.push(ch(v.blocks[i]));
                        }
                        if l1 != l2 || yy % 16 == 0 { println!("{:4} {l1}  {l2}", yy as i32 + MIN_Y()); }
                    }
                }
            }
            if detail { println!("chunk {cx},{cz}: shape {:.3}%", 100.0 * cok as f64 / cn as f64); }
        }
    }
    if chunks == 0 {
        println!("RESULT seed={seed} chunks=0");
        eprintln!("no fully generated vanilla chunks found in range");
        std::process::exit(2);
    }
    let pct = |a: u64, b: u64| if b == 0 { 0.0 } else { 100.0 * a as f64 / b as f64 };
    println!("chunks compared: {chunks}, generation {:.1} ms/chunk", 1000.0 * gen_time / chunks as f64);
    println!("shape  (solid/air/fluid): {:.3}% of {shape_n}", pct(shape_ok, shape_n));
    let mut cv: Vec<_> = confusion.into_iter().collect();
    cv.sort_by(|a, b| b.1.cmp(&a.1));
    for ((a, b), n) in cv.iter().take(6) { println!("    ours {a:?} vanilla {b:?}: {n}"); }
    println!("biomes (per 4x4x4 cell):  {:.3}% of {bio_n}", pct(bio_ok, bio_n));
    let mut bv: Vec<_> = bio_conf.into_iter().collect();
    bv.sort_by(|a, b| b.1.cmp(&a.1));
    for ((a, b), n) in bv.iter().take(8) { println!("    ours {a} vanilla {b}: {n}"); }
    println!("blocks (terrain, exact):  {:.3}% of {blk_n}", pct(blk_ok, blk_n));
    let mut kv: Vec<_> = blk_conf.into_iter().collect();
    kv.sort_by(|a, b| b.1.cmp(&a.1));
    for ((a, b), n) in kv.iter().take(std::env::var("TOP").ok().and_then(|t| t.parse().ok()).unwrap_or(12)) { println!("    ours {a} vanilla {b}: {n}"); }
    let _ = struct_chunks;
    println!("features (all blocks outside structure pieces, exact): {:.3}% of {feat_n}", pct(feat_ok, feat_n));
    let mut fv: Vec<_> = feat_types.into_iter().filter(|(_, v)| v.0 != v.1 || v.2 > 0).collect();
    fv.sort_by(|a, b| ((b.1).0 - (b.1).1 + (b.1).2).cmp(&((a.1).0 - (a.1).1 + (a.1).2)));
    println!("    {:<28} {:>9} {:>9} {:>9}", "block", "vanilla", "matched", "ours-only");
    for (n, (a, b, c)) in fv.iter().take(std::env::var("FTOP").ok().and_then(|t| t.parse().ok()).unwrap_or(25)) {
        println!("    {n:<28} {a:>9} {b:>9} {c:>9}");
    }
    // One stable, machine-parseable summary line (percentages with 3 decimals).
    println!(
        "RESULT seed={seed} chunks={chunks} ms_per_chunk={:.1} biomes={:.3} shape={:.3} exact={:.3} features={:.3}",
        1000.0 * gen_time / chunks as f64, pct(bio_ok, bio_n), pct(shape_ok, shape_n), pct(blk_ok, blk_n), pct(feat_ok, feat_n)
    );
    Ok(())
}

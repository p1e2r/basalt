use super::*;
use basalt_data::BlockState;
use basalt_protocol::Reader;

fn st(s: &str) -> u32 { BlockState::parse(s).unwrap_or_else(|| panic!("no block {s}")).0 }

/// Simple deterministic PRNG.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 { self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17; self.0 }
    fn below(&mut self, n: u64) -> u64 { self.next() % n }
}

// ---- copy of the client's decoder logic (crates/client/src/world.rs) ----
fn client_read_container(r: &mut Reader, entries: usize, max_indirect: u8, min_bits: u8) -> Vec<u32> {
    let bpe = r.u8().unwrap();
    if bpe == 0 { return vec![r.varint().unwrap() as u32; entries]; }
    let palette: Option<Vec<u32>> = if bpe <= max_indirect {
        let n = r.varint().unwrap();
        Some((0..n).map(|_| r.varint().unwrap() as u32).collect())
    } else { None };
    let bits = bpe.max(min_bits) as usize;
    let per = 64 / bits;
    let longs = entries.div_ceil(per);
    let raw = r.take(longs * 8).unwrap();
    let mask = (1u64 << bits) - 1;
    let mut out = Vec::with_capacity(entries);
    'outer: for l in 0..longs {
        let mut v = u64::from_be_bytes(raw[l * 8..l * 8 + 8].try_into().unwrap());
        for _ in 0..per {
            let idx = (v & mask) as u32; v >>= bits;
            out.push(match &palette { Some(p) => p[idx as usize], None => idx });
            if out.len() == entries { break 'outer; }
        }
    }
    out
}

fn read_bitset(r: &mut Reader) -> u64 {
    let n = r.varint().unwrap(); let mut v = 0u64;
    for i in 0..n { let l = r.i64().unwrap() as u64; if i == 0 { v = l; } }
    v
}

struct Decoded { blocks: Vec<Vec<u32>>, biomes: Vec<Vec<u32>>, counts: Vec<(i16, i16)>, heightmaps: Vec<(i32, Vec<i64>)>, nbe: i32, sky: Vec<Option<Vec<u8>>>, block: Vec<Option<Vec<u8>>>, sky_empty: u64 }

fn client_decode(buf: &[u8]) -> Decoded {
    let mut r = Reader::new(buf);
    let nh = r.varint().unwrap();
    let mut heightmaps = Vec::new();
    for _ in 0..nh { let t = r.varint().unwrap(); let n = r.varint().unwrap(); heightmaps.push((t, (0..n).map(|_| r.i64().unwrap()).collect())); }
    let size = r.varint().unwrap() as usize;
    let data = r.take(size).unwrap();
    let mut d = Reader::new(data);
    let (mut blocks, mut biomes, mut counts) = (vec![], vec![], vec![]);
    for _ in 0..SECTIONS {
        counts.push((d.i16().unwrap(), d.i16().unwrap()));
        blocks.push(client_read_container(&mut d, 4096, 8, 4));
        biomes.push(client_read_container(&mut d, 64, 3, 1));
    }
    assert_eq!(d.remaining(), 0, "section data size mismatch");
    let nbe = r.varint().unwrap();
    for _ in 0..nbe { r.u8().unwrap(); r.i16().unwrap(); r.varint().unwrap(); r.nbt().unwrap(); }
    let (sky, block, sky_empty) = client_decode_light(&mut r);
    assert_eq!(r.remaining(), 0, "trailing bytes");
    Decoded { blocks, biomes, counts, heightmaps, nbe, sky, block, sky_empty }
}

#[allow(clippy::type_complexity)]
fn client_decode_light(r: &mut Reader) -> (Vec<Option<Vec<u8>>>, Vec<Option<Vec<u8>>>, u64) {
    let sky_mask = read_bitset(r); let block_mask = read_bitset(r);
    let empty_sky = read_bitset(r); let empty_block = read_bitset(r);
    assert_eq!(sky_mask & empty_sky, 0);
    assert_eq!(block_mask & empty_block, 0);
    let n = LIGHT_SECTIONS;
    let mut read_arrays = |mask: u64| {
        let cnt = r.varint().unwrap();
        assert_eq!(cnt as u32, mask.count_ones());
        let mut out: Vec<Option<Vec<u8>>> = vec![None; n];
        let mut bits = (0..n).filter(|i| mask >> i & 1 == 1);
        for _ in 0..cnt {
            let len = r.varint().unwrap() as usize; assert_eq!(len, 2048);
            out[bits.next().unwrap()] = Some(r.take(len).unwrap().to_vec());
        }
        out
    };
    let sky = read_arrays(sky_mask); let block = read_arrays(block_mask);
    (sky, block, empty_sky)
}

fn nibble(a: &[u8], i: usize) -> u8 { (a[i >> 1] >> ((i & 1) * 4)) & 15 }

// ---------------- paletted containers ----------------

fn container_with(kind: Kind, distinct: usize, rng: &mut Rng) -> (PalettedContainer, Vec<u32>) {
    let reg = kind.registry_size() as u64;
    let pool: Vec<u32> = (0..distinct).map(|i| ((i as u64 * 7919 + 13) % reg) as u32).collect();
    let mut vals: Vec<u32> = (0..kind.len()).map(|i| pool[i % distinct]).collect();
    // Shuffle for good measure.
    for i in (1..vals.len()).rev() { let j = rng.below(i as u64 + 1) as usize; vals.swap(i, j); }
    let mut c = match kind { Kind::Blocks => PalettedContainer::blocks_filled(vals[0]), Kind::Biomes => PalettedContainer::biomes_filled(vals[0]) };
    for (i, v) in vals.iter().enumerate() { c.set(i, *v); }
    (c, vals)
}

#[test]
fn paletted_roundtrip_all_tiers() {
    let mut rng = Rng(0x1234_5678);
    for (kind, tiers) in [(Kind::Blocks, &[1usize, 2, 16, 17, 100, 256, 257, 1000, 4096][..]), (Kind::Biomes, &[1usize, 2, 3, 4, 8, 9, 20, 64][..])] {
        for &n in tiers {
            let (c, vals) = container_with(kind, n.min(kind.registry_size()), &mut rng);
            for (i, v) in vals.iter().enumerate() { assert_eq!(c.get(i), *v); }
            let mut w = Vec::new();
            c.write_network(&mut w);
            let bpe = w[0];
            let expect_bpe = match (kind, n) {
                (_, 1) => 0,
                (Kind::Blocks, n) if n <= 16 => 4,
                (Kind::Blocks, n) if n <= 256 => paletted::ceil_log2(n),
                (Kind::Blocks, _) => 15,
                (Kind::Biomes, n) if n <= 8 => paletted::ceil_log2(n),
                (Kind::Biomes, _) => 7,
            };
            assert_eq!(bpe, expect_bpe, "{kind:?} with {n} values");
            // Our own decoder.
            let mut r = Reader::new(&w);
            let d = PalettedContainer::read_network(&mut r, kind == Kind::Blocks).unwrap();
            assert_eq!(r.remaining(), 0);
            assert!(d.values().eq(vals.iter().copied()), "{kind:?} {n}");
            // Client decoder.
            let mut r = Reader::new(&w);
            let (max_ind, min_bits) = match kind { Kind::Blocks => (8, 4), Kind::Biomes => (3, 1) };
            let cd = client_read_container(&mut r, kind.len(), max_ind, min_bits);
            assert_eq!(r.remaining(), 0);
            assert_eq!(cd, vals);
            // Disk form.
            let (pal, data) = c.to_disk();
            assert_eq!(pal.len(), n);
            let dd = PalettedContainer::from_disk(kind, pal, &data).unwrap();
            assert!(dd.values().eq(vals.iter().copied()));
        }
    }
}

#[test]
fn paletted_set_returns_old_and_compacts() {
    let mut c = PalettedContainer::blocks_filled(0);
    assert_eq!(c.set(5, 1), 0);
    assert_eq!(c.set(5, 2), 1);
    assert_eq!(c.get(5), 2);
    assert_eq!(c.set(5, 0), 2);
    // Only air left: encodes as single value even though the palette is stale.
    let mut w = Vec::new();
    c.write_network(&mut w);
    assert_eq!(w, vec![0, 0]);
    // Many distinct values cycling through one slot never blow up the palette.
    for v in 0..10_000u32 { c.set(7, v % 5000); }
    assert!(c.palette().len() <= 4097);
    assert_eq!(c.get(7), 9999 % 5000);
}

// ---------------- chunk network encoding ----------------

fn test_chunk(cx: i32, cz: i32) -> Chunk {
    let mut rng = Rng(99);
    let stone = st("stone") as u16; let dirt = st("dirt") as u16; let grass = st("grass_block") as u16;
    let water = st("water") as u16;
    let mut blocks = vec![[0u16; 4096]; SECTIONS];
    for (si, sec) in blocks.iter_mut().enumerate() {
        for i in 0..4096 {
            let y = MIN_Y + si as i32 * 16 + (i >> 8) as i32;
            sec[i] = if y < 0 { if rng.below(10) == 0 { (rng.below(basalt_data::blocks::STATE_COUNT as u64)) as u16 } else { stone } }
                else if y < 60 { stone } else if y < 63 { dirt } else if y == 63 { grass } else if y < 66 && i & 15 == 0 { water } else { 0 };
        }
    }
    let plains = biome_id("plains").unwrap() as u8;
    let mut biomes = vec![[plains; 64]; SECTIONS];
    biomes[5][3] = biome_id("desert").unwrap() as u8;
    Chunk::from_generated(cx, cz, &blocks, &biomes)
}

#[test]
fn chunk_network_roundtrip() {
    let mut w = World::new(None);
    w.insert(test_chunk(0, 0));
    w.light_chunk(0, 0);
    let c = w.chunk(0, 0).unwrap();
    let mut buf = Vec::new();
    c.write_network(&mut buf);
    let d = client_decode(&buf);
    for si in 0..SECTIONS {
        assert!(c.sections[si].blocks.values().eq(d.blocks[si].iter().copied()), "section {si}");
        assert!(c.sections[si].biomes.values().eq(d.biomes[si].iter().copied()), "biomes {si}");
        assert_eq!(d.counts[si], (c.sections[si].non_air as i16, c.sections[si].fluids as i16));
    }
    assert_eq!(d.counts[SECTIONS - 1], (0, 0));
    assert_eq!(d.counts[0].0, 4096);
    assert_eq!(d.nbe, 0);
    // Heightmaps: 5, 4, 1 with 37 longs each; WORLD_SURFACE at y=65 for water columns, 63 elsewhere.
    assert_eq!(d.heightmaps.iter().map(|h| (h.0, h.1.len())).collect::<Vec<_>>(), vec![(5, 37), (4, 37), (1, 37)]);
    let ws: Vec<u64> = d.heightmaps[2].1.iter().map(|l| *l as u64).collect();
    let hv = paletted::unpack(&ws, 9, 256);
    assert_eq!(hv[0], (65 - MIN_Y + 1) as u32);
    assert_eq!(hv[1], (63 - MIN_Y + 1) as u32);
    // Light: sky above terrain is 15 (sent as 0xFF arrays), top extra section too.
    let top = d.sky[LIGHT_SECTIONS - 1].as_ref().expect("top sky array");
    assert!(top.iter().all(|b| *b == 0xFF));
    let li = ((100 - MIN_Y) >> 4) as usize + 1;
    assert!(d.sky[li].as_ref().unwrap().iter().all(|b| *b == 0xFF));
    // Deep underground sections are all-dark → empty mask.
    assert!(d.sky_empty & 1 << 2 != 0);
    for li in 0..LIGHT_SECTIONS {
        if let Some(a) = &d.sky[li] {
            for i in 0..4096 { assert_eq!(nibble(a, i), c.sky_light[li].get(i)); }
        } else { assert_eq!(c.sky_light[li].uniform(), Some(0)); }
        if let Some(a) = &d.block[li] {
            for i in 0..4096 { assert_eq!(nibble(a, i), c.block_light[li].get(i)); }
        } else { assert_eq!(c.block_light[li].uniform(), Some(0)); }
    }
    // Light-only packet decodes the same way.
    let mut lb = Vec::new();
    c.write_light(&mut lb);
    let mut r = Reader::new(&lb);
    let (sky2, _, _) = client_decode_light(&mut r);
    assert_eq!(r.remaining(), 0);
    assert_eq!(sky2, d.sky);
    // Our own container reader handles the section data too.
    let mut r = Reader::new(&buf);
    let nh = r.varint().unwrap();
    for _ in 0..nh { r.varint().unwrap(); let n = r.varint().unwrap(); r.take(n as usize * 8).unwrap(); }
    let _size = r.varint().unwrap();
    r.i16().unwrap(); r.i16().unwrap();
    let s0 = PalettedContainer::read_network(&mut r, true).unwrap();
    assert_eq!(s0, { let mut x = c.sections[0].blocks.clone(); x.compact(); PalettedContainer::from_values(Kind::Blocks, &x.values().collect::<Vec<_>>()) });
}

#[test]
fn chunk_counts_and_heightmaps_maintained() {
    let mut c = Chunk::empty(0, 0);
    assert_eq!(c.heightmaps.world_surface[0], 0);
    c.set_block(0, 10, 0, st("stone"));
    c.set_block(0, 20, 0, st("oak_leaves"));
    c.set_block(0, 30, 0, st("short_grass"));
    let sec = &c.sections[((10 - MIN_Y) >> 4) as usize];
    assert_eq!(sec.non_air, 1);
    let h = |y: i32| (y - MIN_Y + 1) as u16;
    assert_eq!(c.heightmaps.world_surface[0], h(30));
    assert_eq!(c.heightmaps.motion_blocking[0], h(20));
    assert_eq!(c.heightmaps.motion_blocking_no_leaves[0], h(10));
    assert_eq!(c.heightmaps.ocean_floor[0], h(20));
    c.set_block(0, 30, 0, 0);
    c.set_block(0, 20, 0, st("water"));
    assert_eq!(c.heightmaps.world_surface[0], h(20));
    assert_eq!(c.heightmaps.motion_blocking_no_leaves[0], h(20));
    assert_eq!(c.heightmaps.ocean_floor[0], h(10));
    assert_eq!(c.sections[((20 - MIN_Y) >> 4) as usize].fluids, 1);
    c.set_block(0, 10, 0, 0);
    assert_eq!(c.heightmaps.ocean_floor[0], 0);
    let mut fresh = c.clone();
    fresh.recompute_heightmaps();
    assert_eq!(fresh.heightmaps, c.heightmaps);
}

// ---------------- lighting ----------------

fn flat_world(r: i32, ground: i32) -> World {
    let mut w = World::new(None);
    let stone = st("stone");
    for cx in -r..=r {
        for cz in -r..=r {
            let mut c = Chunk::empty(cx, cz);
            for y in MIN_Y..=ground { for lz in 0..16 { for lx in 0..16 { c.set_block(lx, y, lz, stone); } } }
            w.insert(c);
        }
    }
    for cx in -r..=r { for cz in -r..=r { w.light_chunk(cx, cz); } }
    w
}

#[test]
fn light_open_sky() {
    let w = flat_world(1, 0);
    for y in 1..330 { assert_eq!(w.light(3, y, 5).0, 15, "y={y}"); }
    assert_eq!(w.light(3, 0, 5).0, 0);
    assert_eq!(w.light(3, -30, 5), (0, 0));
}

#[test]
fn light_torch_in_closed_cave() {
    let mut w = flat_world(1, 40);
    // Carve a 1-high horizontal tunnel x = -10..=10 at y=10, z=8, sealed by stone.
    for x in -10..=10 { w.set_block(x, 10, 8, 0); }
    assert_eq!(w.light(0, 10, 8), (0, 0));
    let changed = w.set_block(0, 10, 8, st("torch"));
    assert!(changed.contains(&(0, 0)));
    assert!(changed.contains(&(-1, 0)), "light crosses the chunk border: {changed:?}");
    for d in 0..=10 {
        assert_eq!(w.light(d, 10, 8).1, 14 - d as u8, "x={d}");
        assert_eq!(w.light(-d, 10, 8).1, 14 - d as u8, "x=-{d}");
    }
    assert_eq!(w.light(0, 11, 8).1, 0, "stone stays dark");
    assert_eq!(w.light(0, 10, 8).0, 0, "no sky light in the cave");
    // Removing the torch darkens the tunnel again.
    w.set_block(0, 10, 8, 0);
    for d in -10..=10 { assert_eq!(w.light(d, 10, 8).1, 0); }
}

#[test]
fn light_shading_and_unshading() {
    let mut w = flat_world(1, 0);
    w.set_block(5, 50, 5, st("stone"));
    // Directly below: no longer direct sky; lit from the sides at 14.
    assert_eq!(w.light(5, 49, 5).0, 14);
    assert_eq!(w.light(5, 1, 5).0, 14);
    assert_eq!(w.light(6, 49, 5).0, 15);
    // A 3x3 roof makes the centre column darker (13).
    for dx in -1..=1 { for dz in -1..=1 { w.set_block(5 + dx, 50, 5 + dz, st("stone")); } }
    assert_eq!(w.light(5, 49, 5).0, 13);
    assert_eq!(w.light(5, 1, 5).0, 13);
    // Leaves and water reduce sky light by 1 per block.
    w.set_block(20, 30, 20, st("oak_leaves"));
    assert_eq!(w.light(20, 30, 20).0, 14);
    // Remove the roof: back to full sky.
    for dx in -1..=1 { for dz in -1..=1 { w.set_block(5 + dx, 50, 5 + dz, 0); } }
    assert_eq!(w.light(5, 49, 5).0, 15);
    assert_eq!(w.light(5, 1, 5).0, 15);
}

#[test]
fn light_incremental_matches_full() {
    let mut w = flat_world(1, 20);
    let mut rng = Rng(7);
    let palette = [st("stone"), 0, 0, st("torch"), st("glowstone"), st("water"), st("oak_leaves"), st("glass")];
    for _ in 0..400 {
        let x = rng.below(40) as i32 - 20; let z = rng.below(40) as i32 - 20;
        let y = 10 + rng.below(20) as i32;
        w.set_block(x, y, z, palette[rng.below(palette.len() as u64) as usize]);
    }
    // Relight the centre from scratch and compare with the incrementally maintained values.
    let before: Vec<(u8, u8)> = (-8..24).flat_map(|x| (-8..24).flat_map(move |z| (0..40).map(move |y| (x, y, z)))).map(|(x, y, z)| w.light(x, y, z)).collect();
    let mut w2 = World::new(None);
    for (k, c) in &w.chunks { let mut c = c.clone(); c.clear_light(); w2.chunks.insert(*k, c); }
    for cx in -1..=1 { for cz in -1..=1 { w2.light_chunk(cx, cz); } }
    // Full relight done in a fixed order is itself order-independent only up to unloaded
    // neighbours, so compare only the centre chunk region against a final relight.
    w2.light_chunk(0, 0);
    let after: Vec<(u8, u8)> = (-8..24).flat_map(|x| (-8..24).flat_map(move |z| (0..40).map(move |y| (x, y, z)))).map(|(x, y, z)| w2.light(x, y, z)).collect();
    let diffs = before.iter().zip(&after).filter(|(a, b)| a != b).count();
    assert_eq!(diffs, 0, "{diffs} cells differ between incremental and full lighting");
}

// ---------------- anvil ----------------

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("basalt-world-{tag}-{}-{:?}", std::process::id(), std::time::SystemTime::now()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn anvil_roundtrip() {
    let dir = temp_dir("anvil");
    let mut w = World::new(Some(dir.clone()));
    for (cx, cz) in [(0, 0), (1, 0), (-1, -1), (31, 31), (32, 0), (-33, 5)] {
        w.insert(test_chunk(cx, cz));
    }
    for (cx, cz) in w.loaded_chunks() { w.light_chunk(cx, cz); }
    w.set_block(3, 70, 3, st("torch"));
    w.chunk_mut(0, 0).unwrap().block_entities.push(basalt_nbt::compound([
        ("id", basalt_nbt::Tag::String("minecraft:chest".into())), ("x", basalt_nbt::Tag::Int(1)), ("y", basalt_nbt::Tag::Int(64)), ("z", basalt_nbt::Tag::Int(2)),
    ]));
    w.save_all().unwrap();
    assert!(dir.join("r.0.0.mca").exists() && dir.join("r.-1.-1.mca").exists() && dir.join("r.-2.0.mca").exists());
    // Second save after a change reuses / reallocates sectors.
    w.set_block(4, 70, 4, st("glowstone"));
    w.save_all().unwrap();

    let mut w2 = World::new(Some(dir.clone()));
    for (cx, cz) in w.loaded_chunks() {
        assert!(w2.load(cx, cz).unwrap(), "chunk {cx},{cz} not found");
        let (a, b) = (w.chunk(cx, cz).unwrap(), w2.chunk(cx, cz).unwrap());
        for si in 0..SECTIONS {
            assert!(a.sections[si].blocks.values().eq(b.sections[si].blocks.values()), "{cx},{cz} s{si}");
            assert!(a.sections[si].biomes.values().eq(b.sections[si].biomes.values()));
            assert_eq!((a.sections[si].non_air, a.sections[si].fluids), (b.sections[si].non_air, b.sections[si].fluids));
        }
        for li in 0..LIGHT_SECTIONS {
            for i in 0..4096 {
                assert_eq!(a.sky_light[li].get(i), b.sky_light[li].get(i), "{cx},{cz} sky li{li} i{i}");
                assert_eq!(a.block_light[li].get(i), b.block_light[li].get(i), "{cx},{cz} block li{li}");
            }
        }
        assert_eq!(a.heightmaps, b.heightmaps);
        assert_eq!(a.block_entities, b.block_entities);
        assert!(b.lit && !b.dirty);
    }
    assert!(!w2.load(5, 5).unwrap());
    assert!(!w2.load(1000, 1000).unwrap());
    // Unload saves dirty chunks.
    w2.set_block(0, 100, 0, st("stone"));
    w2.unload(0, 0).unwrap();
    assert!(!w2.is_loaded(0, 0));
    let mut w3 = World::new(Some(dir.clone()));
    assert!(w3.load(0, 0).unwrap());
    assert_eq!(w3.get_block(0, 100, 0), st("stone"));
    // Network encoding from a loaded chunk includes the block entity.
    let mut buf = Vec::new();
    w3.chunk(0, 0).unwrap().write_network(&mut buf);
    assert_eq!(client_decode(&buf).nbe, 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn region_file_large_chunk_and_rewrite() {
    let dir = temp_dir("region");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("r.0.0.mca");
    let mut r = anvil::RegionFile::open(&p).unwrap();
    let mut rng = Rng(3);
    let big: Vec<u8> = (0..200_000).map(|_| rng.next() as u8).collect(); // incompressible, ~49 sectors
    r.write(1, 2, &big).unwrap();
    r.write(3, 4, b"small").unwrap();
    r.write(1, 2, b"now small").unwrap();
    r.write(5, 5, &big).unwrap(); // should reuse freed space
    drop(r);
    let mut r = anvil::RegionFile::open_read(&p).unwrap();
    assert_eq!(r.read(1, 2).unwrap().unwrap(), b"now small");
    assert_eq!(r.read(3, 4).unwrap().unwrap(), b"small");
    assert_eq!(r.read(5, 5).unwrap().unwrap(), big);
    assert!(r.read(0, 0).unwrap().is_none());
    let len = std::fs::metadata(&p).unwrap().len();
    assert_eq!(len % 4096, 0);
    assert!(len < (2 + 2 * 50 + 2) * 4096, "freed sectors were not reused (len {len})");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Reads the vanilla test server's world (read-only) if it exists on this machine.
#[test]
fn read_vanilla_region() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vanilla/srv/world");
    // worktrees have no vanilla/ of their own: BASALT_VANILLA_WORLD points at the shared one
    let base = std::env::var_os("BASALT_VANILLA_WORLD").map(PathBuf::from).filter(|_| !base.exists()).unwrap_or(base);
    let dir = [base.join("dimensions/minecraft/overworld/region"), base.join("region")].into_iter().find(|d| d.join("r.0.0.mca").exists());
    let Some(dir) = dir else { eprintln!("no vanilla world found; skipping"); return };
    let mut r = anvil::RegionFile::open_read(&dir.join("r.0.0.mca")).unwrap();
    let mut names = std::collections::BTreeMap::<&str, usize>::new();
    let mut loaded = 0;
    for lz in 0..32 {
        for lx in 0..32 {
            let Some(bytes) = r.read(lx, lz).unwrap() else { continue };
            let (c, status) = anvil::chunk_from_bytes(&bytes).unwrap();
            if status != "minecraft:full" { continue; }
            assert_eq!((c.x, c.z), (lx as i32, lz as i32));
            for s in &c.sections { for v in s.blocks.values() { *names.entry(BlockState(v).name()).or_default() += 1; } }
            // Re-encode for the network and to disk; both must round-trip.
            let mut buf = Vec::new();
            c.write_network(&mut buf);
            let d = client_decode(&buf);
            for si in 0..SECTIONS { assert!(c.sections[si].blocks.values().eq(d.blocks[si].iter().copied())); }
            let (c2, _) = anvil::chunk_from_bytes(&anvil::chunk_to_bytes(&c)).unwrap();
            for si in 0..SECTIONS { assert_eq!(c.sections[si].blocks.values().collect::<Vec<_>>(), c2.sections[si].blocks.values().collect::<Vec<_>>()); }
            loaded += 1;
            if loaded >= 16 { break; }
        }
        if loaded >= 16 { break; }
    }
    eprintln!("loaded {loaded} vanilla chunks; top blocks: {:?}", { let mut v: Vec<_> = names.iter().collect(); v.sort_by_key(|x| std::cmp::Reverse(*x.1)); v.into_iter().take(12).collect::<Vec<_>>() });
    assert!(loaded > 0);
    for n in ["stone", "air"] { assert!(names.contains_key(n), "{n} missing"); }
    assert!(names.contains_key("water") || names.contains_key("grass_block"));
    // Vanilla stored sky light: open air high up is 15.
    let mut w = World::new(Some(dir));
    assert!(w.load(0, 0).unwrap());
    assert_eq!(w.light(0, 300, 0).0, 15);
    // Compare our lighting with vanilla's stored light for a chunk whose neighbours are loaded.
    for (tx, tz) in [(5, 5), (10, 3), (20, 20), (2, 12)] {
    for cx in -1..=1 { for cz in -1..=1 { let _ = w.load(cx + tx, cz + tz); } }
    if w.is_loaded(tx, tz) {
        let vanilla = w.chunk(tx, tz).unwrap().clone();
        w.light_chunk(tx, tz);
        let ours = w.chunk(tx, tz).unwrap();
        let partial = (1..=SECTIONS).flat_map(|li| (0..4096).map(move |i| (li, i))).filter(|(li, i)| (1..15).contains(&ours.sky_light[*li].get(*i))).count();
        let surface = ours.biome(8, 64, 8);
        eprintln!("chunk {tx},{tz} biome {} partial sky cells {partial} blocklit {}", BIOMES[surface as usize], (1..=SECTIONS).flat_map(|li| (0..4096).map(move |i| (li, i))).filter(|(li, i)| ours.block_light[*li].get(*i) > 0).count());
        let (mut sky_diff, mut blk_diff, mut total) = (0, 0, 0);
        for li in 1..=SECTIONS {
            for i in 0..4096 {
                total += 1;
                sky_diff += (vanilla.sky_light[li].get(i) != ours.sky_light[li].get(i)) as usize;
                blk_diff += (vanilla.block_light[li].get(i) != ours.block_light[li].get(i)) as usize;
            }
        }
        eprintln!("light vs vanilla in chunk {tx},{tz}: sky differs {sky_diff}/{total}, block differs {blk_diff}/{total}");
        assert!(sky_diff * 20 < total, "sky light differs too much from vanilla");
    }
    }
}

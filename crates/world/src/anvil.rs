//! Anvil region files (`r.X.Z.mca`) and the on-disk chunk NBT format.
//!
//! Region layout: 4 KiB sector table (1024 big-endian u32: `sector_offset << 8 | sector_count`),
//! 4 KiB timestamp table, then chunk payloads: u32 length (including the type byte), u8
//! compression type (1 gzip, 2 zlib, 3 uncompressed, 4 lz4 — unsupported here; +128 means the
//! payload lives in an external `c.X.Z.mcc` file), data, padded to whole sectors.

use crate::chunk::{Chunk, HeightmapKind, LightArray, Section};
use crate::paletted::{Kind, PalettedContainer};
use crate::{biome_id, BIOMES, LIGHT_SECTIONS, MIN_Y, SECTIONS};
use anyhow::{anyhow, bail, Context, Result};
use basalt_data::BlockState;
use basalt_nbt::{Compound, Tag};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const SECTOR: usize = 4096;
/// Largest payload stored inline (sector count is a u8).
const MAX_INLINE_SECTORS: usize = 255;

pub const COMPRESSION_GZIP: u8 = 1;
pub const COMPRESSION_ZLIB: u8 = 2;
pub const COMPRESSION_NONE: u8 = 3;
pub const COMPRESSION_LZ4: u8 = 4;

/// Region file path for chunk (cx, cz) inside `dir`.
pub fn region_path(dir: &Path, cx: i32, cz: i32) -> PathBuf {
    dir.join(format!("r.{}.{}.mca", cx >> 5, cz >> 5))
}

/// An open region file. Reads and writes go straight to disk.
pub struct RegionFile {
    path: PathBuf,
    file: File,
    writable: bool,
    locations: [u32; 1024],
    timestamps: [u32; 1024],
}

impl RegionFile {
    /// Open an existing region file read-only.
    pub fn open_read(path: &Path) -> Result<RegionFile> {
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        Self::from_file(path, file, false)
    }

    /// Open (creating if needed) a region file for reading and writing.
    pub fn open(path: &Path) -> Result<RegionFile> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("open {}", path.display()))?;
        Self::from_file(path, file, true)
    }

    fn from_file(path: &Path, mut file: File, writable: bool) -> Result<RegionFile> {
        let len = file.metadata()?.len();
        let mut locations = [0u32; 1024];
        let mut timestamps = [0u32; 1024];
        if len >= (2 * SECTOR) as u64 {
            let mut hdr = vec![0u8; 2 * SECTOR];
            file.seek(SeekFrom::Start(0))?;
            file.read_exact(&mut hdr)?;
            for i in 0..1024 {
                locations[i] = u32::from_be_bytes(hdr[i * 4..i * 4 + 4].try_into().unwrap());
                timestamps[i] =
                    u32::from_be_bytes(hdr[SECTOR + i * 4..SECTOR + i * 4 + 4].try_into().unwrap());
            }
        } else if writable {
            file.set_len(0)?;
            file.seek(SeekFrom::Start(0))?;
            file.write_all(&[0u8; 2 * SECTOR])?;
        } else if len != 0 {
            bail!("{}: truncated region header", path.display());
        }
        Ok(RegionFile {
            path: path.to_path_buf(),
            file,
            writable,
            locations,
            timestamps,
        })
    }

    fn index(lx: usize, lz: usize) -> usize {
        (lx & 31) + (lz & 31) * 32
    }

    pub fn has_chunk(&self, lx: usize, lz: usize) -> bool {
        self.locations[Self::index(lx, lz)] != 0
    }
    pub fn timestamp(&self, lx: usize, lz: usize) -> u32 {
        self.timestamps[Self::index(lx, lz)]
    }

    fn external_path(&self, lx: usize, lz: usize) -> PathBuf {
        // Recover absolute chunk coords from the region file name.
        let name = self.path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let mut parts = name.split('.').skip(1);
        let rx: i32 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let rz: i32 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        self.path.with_file_name(format!(
            "c.{}.{}.mcc",
            rx * 32 + lx as i32,
            rz * 32 + lz as i32
        ))
    }

    /// Read and decompress chunk (lx, lz) (region-local, 0..32). `None` if absent.
    pub fn read(&mut self, lx: usize, lz: usize) -> Result<Option<Vec<u8>>> {
        let loc = self.locations[Self::index(lx, lz)];
        if loc == 0 {
            return Ok(None);
        }
        let (off, cnt) = ((loc >> 8) as u64 * SECTOR as u64, (loc & 0xFF) as usize);
        let mut head = [0u8; 5];
        self.file.seek(SeekFrom::Start(off))?;
        self.file.read_exact(&mut head)?;
        let len = u32::from_be_bytes(head[..4].try_into().unwrap()) as usize;
        let ty = head[4];
        let raw = if ty & 128 != 0 {
            std::fs::read(self.external_path(lx, lz)).context("read external chunk")?
        } else {
            if len == 0 || len + 4 > cnt * SECTOR {
                bail!("chunk {lx},{lz}: bad length {len} for {cnt} sectors")
            }
            let mut raw = vec![0u8; len - 1];
            self.file.read_exact(&mut raw)?;
            raw
        };
        Ok(Some(decompress(ty & 127, &raw)?))
    }

    /// Compress (zlib) and write chunk (lx, lz).
    pub fn write(&mut self, lx: usize, lz: usize, nbt: &[u8]) -> Result<()> {
        if !self.writable {
            bail!("region file opened read-only")
        }
        let comp = compress_zlib(nbt)?;
        let i = Self::index(lx, lz);
        let need = (comp.len() + 5).div_ceil(SECTOR);
        let external = need > MAX_INLINE_SECTORS;
        let mut payload = Vec::with_capacity(need.max(1) * SECTOR);
        if external {
            std::fs::write(self.external_path(lx, lz), &comp)?;
            payload.extend_from_slice(&1u32.to_be_bytes());
            payload.push(COMPRESSION_ZLIB | 128);
        } else {
            let _ = std::fs::remove_file(self.external_path(lx, lz));
            payload.extend_from_slice(&(comp.len() as u32 + 1).to_be_bytes());
            payload.push(COMPRESSION_ZLIB);
            payload.extend_from_slice(&comp);
        }
        let sectors = payload.len().div_ceil(SECTOR);
        payload.resize(sectors * SECTOR, 0);

        let old = self.locations[i];
        let (old_off, old_cnt) = ((old >> 8) as usize, (old & 0xFF) as usize);
        let off = if old != 0 && sectors <= old_cnt {
            old_off
        } else {
            // First fit among free sectors, else append.
            let total = (self.file.metadata()?.len() as usize)
                .div_ceil(SECTOR)
                .max(2);
            let mut used = vec![false; total];
            used[0] = true;
            used[1] = true;
            for (j, l) in self.locations.iter().enumerate() {
                if *l == 0 || j == i {
                    continue;
                }
                let (o, c) = ((*l >> 8) as usize, (*l & 0xFF) as usize);
                for u in used.iter_mut().take((o + c).min(total)).skip(o) {
                    *u = true;
                }
            }
            let mut found = None;
            let mut run = 0;
            for (s, u) in used.iter().enumerate() {
                if *u {
                    run = 0;
                } else {
                    run += 1;
                    if run == sectors {
                        found = Some(s + 1 - sectors);
                        break;
                    }
                }
            }
            found.unwrap_or_else(|| total - run)
        };
        self.file.seek(SeekFrom::Start((off * SECTOR) as u64))?;
        self.file.write_all(&payload)?;
        self.locations[i] = (off as u32) << 8 | sectors as u32;
        self.timestamps[i] = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as u32)
            .unwrap_or(0);
        self.file.seek(SeekFrom::Start((i * 4) as u64))?;
        self.file.write_all(&self.locations[i].to_be_bytes())?;
        self.file.seek(SeekFrom::Start((SECTOR + i * 4) as u64))?;
        self.file.write_all(&self.timestamps[i].to_be_bytes())?;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<()> {
        if self.writable {
            self.file.sync_data()?;
        }
        Ok(())
    }
    pub fn is_writable(&self) -> bool {
        self.writable
    }
}

pub fn decompress(ty: u8, raw: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    match ty {
        COMPRESSION_GZIP => {
            flate2::read::GzDecoder::new(raw).read_to_end(&mut out)?;
        }
        COMPRESSION_ZLIB => {
            flate2::read::ZlibDecoder::new(raw).read_to_end(&mut out)?;
        }
        COMPRESSION_NONE => out.extend_from_slice(raw),
        COMPRESSION_LZ4 => bail!("LZ4-compressed chunks are not supported"),
        t => bail!("unknown chunk compression type {t}"),
    }
    Ok(out)
}

pub fn compress_zlib(data: &[u8]) -> Result<Vec<u8>> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data)?;
    Ok(e.finish()?)
}

// ---------------- chunk <-> NBT ----------------

fn state_to_nbt(s: u32) -> Tag {
    let st = BlockState(s);
    let mut c = Compound::new();
    c.insert(
        "Name".into(),
        Tag::String(format!("minecraft:{}", st.name())),
    );
    let props = st.props();
    if !props.is_empty() {
        c.insert(
            "Properties".into(),
            Tag::Compound(
                props
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), Tag::String(v.to_string())))
                    .collect(),
            ),
        );
    }
    Tag::Compound(c)
}

fn state_from_nbt(t: &Tag) -> u32 {
    let Some(name) = t.get("Name").and_then(|n| n.as_str()) else {
        return 0;
    };
    let Some(mut st) = BlockState::from_name(name) else {
        log::warn!("unknown block {name}, using air");
        return 0;
    };
    if let Some(p) = t.get("Properties").and_then(|p| p.as_compound()) {
        for (k, v) in p {
            if let Some(v) = v.as_str() {
                match st.with(k, v) {
                    Some(n) => st = n,
                    None => log::warn!("{name}: bad property {k}={v}"),
                }
            }
        }
    }
    st.0
}

fn long_array(t: Option<&Tag>) -> &[i64] {
    match t {
        Some(Tag::LongArray(v)) => v,
        _ => &[],
    }
}

/// Serialise a chunk to the vanilla on-disk NBT (root compound, DataVersion WORLD_VERSION).
pub fn chunk_to_nbt(c: &Chunk) -> Tag {
    let mut root = c.extra.clone();
    let mut ins = |k: &str, v: Tag| {
        root.insert(k.to_string(), v);
    };
    ins("DataVersion", Tag::Int(basalt_data::WORLD_VERSION));
    ins("xPos", Tag::Int(c.x));
    ins("zPos", Tag::Int(c.z));
    ins("yPos", Tag::Int(c.layout.min_y() >> 4));
    ins("Status", Tag::String("minecraft:full".into()));
    ins("isLightOn", Tag::Byte(c.lit as i8));
    let mut sections = Vec::with_capacity(SECTIONS);
    for (i, s) in c
        .sections
        .iter()
        .enumerate()
        .skip(c.layout.first)
        .take(c.layout.count)
    {
        let mut sc = Compound::new();
        sc.insert("Y".into(), Tag::Byte(((MIN_Y >> 4) + i as i32) as i8));
        let (pal, data) = s.blocks.to_disk();
        let mut bs = Compound::new();
        bs.insert(
            "palette".into(),
            Tag::List(pal.iter().map(|v| state_to_nbt(*v)).collect()),
        );
        if !data.is_empty() {
            bs.insert("data".into(), Tag::LongArray(data));
        }
        sc.insert("block_states".into(), Tag::Compound(bs));
        let (pal, data) = s.biomes.to_disk();
        let mut bi = Compound::new();
        bi.insert(
            "palette".into(),
            Tag::List(
                pal.iter()
                    .map(|v| {
                        Tag::String(format!(
                            "minecraft:{}",
                            BIOMES.get(*v as usize).copied().unwrap_or("plains")
                        ))
                    })
                    .collect(),
            ),
        );
        if !data.is_empty() {
            bi.insert("data".into(), Tag::LongArray(data));
        }
        sc.insert("biomes".into(), Tag::Compound(bi));
        if c.lit {
            // Vanilla omits uniform sky layers; we keep every non-zero layer explicit except
            // fully lit ones (which the loader reconstructs from the heightmap) and zero ones.
            let sky = &c.sky_light[i + 1];
            if !matches!(sky.uniform(), Some(0) | Some(15)) {
                sc.insert(
                    "SkyLight".into(),
                    Tag::ByteArray(sky.to_bytes().into_iter().map(|b| b as i8).collect()),
                );
            }
            let bl = &c.block_light[i + 1];
            if bl.uniform() != Some(0) {
                sc.insert(
                    "BlockLight".into(),
                    Tag::ByteArray(bl.to_bytes().into_iter().map(|b| b as i8).collect()),
                );
            }
        }
        sections.push(Tag::Compound(sc));
    }
    ins("sections", Tag::List(sections));
    let mut hm = Compound::new();
    for k in HeightmapKind::ALL {
        hm.insert(
            k.disk_name().into(),
            Tag::LongArray(
                c.heightmaps_packed_rel(k)
                    .into_iter()
                    .map(|l| l as i64)
                    .collect(),
            ),
        );
    }
    ins("Heightmaps", Tag::Compound(hm));
    ins("block_entities", Tag::List(c.block_entities.clone()));
    for k in ["block_ticks", "fluid_ticks"] {
        root.entry(k.into()).or_insert(Tag::List(vec![]));
    }
    root.entry("PostProcessing".into())
        .or_insert_with(|| Tag::List((0..c.layout.count).map(|_| Tag::List(vec![])).collect()));
    root.entry("InhabitedTime".into()).or_insert(Tag::Long(0));
    root.entry("LastUpdate".into()).or_insert(Tag::Long(0));
    root.entry("structures".into()).or_insert_with(|| {
        basalt_nbt::compound([
            ("References", Tag::Compound(Compound::new())),
            ("starts", Tag::Compound(Compound::new())),
        ])
    });
    Tag::Compound(root)
}

/// Keys handled explicitly by `chunk_to_nbt` / `chunk_from_nbt`.
const KNOWN_KEYS: &[&str] = &[
    "DataVersion",
    "xPos",
    "zPos",
    "yPos",
    "Status",
    "isLightOn",
    "sections",
    "Heightmaps",
    "block_entities",
];

/// Parse a vanilla on-disk chunk. Heightmaps and block counts are recomputed from the blocks.
/// Missing sky light layers are reconstructed: sections entirely above every column's
/// highest light-blocking block are fully lit (15), others 0.
pub fn chunk_from_nbt(t: &Tag) -> Result<Chunk> {
    chunk_from_nbt_in(t, crate::Layout::OVERWORLD)
}

/// Like [`chunk_from_nbt`] for a chunk of a dimension with the given layout.
pub fn chunk_from_nbt_in(t: &Tag, layout: crate::Layout) -> Result<Chunk> {
    let root = t
        .as_compound()
        .ok_or_else(|| anyhow!("chunk root is not a compound"))?;
    let geti = |k: &str| root.get(k).and_then(|v| v.as_i64()).map(|v| v as i32);
    let x = geti("xPos").ok_or_else(|| anyhow!("missing xPos"))?;
    let z = geti("zPos").ok_or_else(|| anyhow!("missing zPos"))?;
    let plains = biome_id("plains").unwrap_or(0);
    let mut sections: Vec<Section> = (0..SECTIONS).map(|_| Section::empty(plains)).collect();
    let mut sky: Vec<Option<LightArray>> = (0..LIGHT_SECTIONS).map(|_| None).collect();
    let mut block: Vec<LightArray> = (0..LIGHT_SECTIONS).map(|_| LightArray::filled(0)).collect();
    let lit = geti("isLightOn").unwrap_or(0) != 0;
    for s in root
        .get("sections")
        .and_then(|s| s.as_list())
        .map(|v| v.as_slice())
        .unwrap_or(&[])
    {
        let Some(y) = s.get("Y").and_then(|v| v.as_i64()) else {
            continue;
        };
        let li = y as i32 - (MIN_Y >> 4) + 1;
        if li < 0 || li >= LIGHT_SECTIONS as i32 {
            continue;
        }
        let li = li as usize;
        if let Some(Tag::ByteArray(b)) = s.get("SkyLight") {
            if b.len() == 2048 {
                sky[li] = Some(LightArray::from_bytes(
                    &b.iter().map(|v| *v as u8).collect::<Vec<_>>(),
                ));
            }
        }
        if let Some(Tag::ByteArray(b)) = s.get("BlockLight") {
            if b.len() == 2048 {
                block[li] = LightArray::from_bytes(&b.iter().map(|v| *v as u8).collect::<Vec<_>>());
            }
        }
        if li == 0 || li > SECTIONS {
            continue;
        }
        let si = li - 1;
        let blocks = match s.get("block_states") {
            Some(bs) => {
                let pal: Vec<u32> = bs
                    .get("palette")
                    .and_then(|p| p.as_list())
                    .map(|l| l.iter().map(state_from_nbt).collect())
                    .unwrap_or_else(|| vec![0]);
                PalettedContainer::from_disk(Kind::Blocks, pal, long_array(bs.get("data")))
                    .with_context(|| format!("section {y} block_states"))?
            }
            None => PalettedContainer::blocks_filled(0),
        };
        let biomes = match s.get("biomes") {
            Some(bs) => {
                let pal: Vec<u32> = bs
                    .get("palette")
                    .and_then(|p| p.as_list())
                    .map(|l| {
                        l.iter()
                            .map(|n| n.as_str().and_then(biome_id).unwrap_or(plains))
                            .collect()
                    })
                    .unwrap_or_else(|| vec![plains]);
                PalettedContainer::from_disk(Kind::Biomes, pal, long_array(bs.get("data")))
                    .with_context(|| format!("section {y} biomes"))?
            }
            None => PalettedContainer::biomes_filled(plains),
        };
        sections[si] = Section::new(blocks, biomes);
    }
    let mut c = Chunk::from_sections(x, z, sections);
    // Highest block with any opacity, over all columns.
    let mut top = MIN_Y - 1;
    'outer: for y in (MIN_Y..MIN_Y + crate::HEIGHT).rev() {
        let s = &c.sections[((y - MIN_Y) >> 4) as usize];
        if let Some(v) = s.blocks.single() {
            if crate::props::opacity(v) == 0 {
                continue;
            }
        }
        for lz in 0..16 {
            for lx in 0..16 {
                if crate::props::opacity(c.get_block(lx, y, lz)) > 0 {
                    top = y;
                    break 'outer;
                }
            }
        }
    }
    for (li, s) in sky.into_iter().enumerate() {
        let bottom = MIN_Y + (li as i32 - 1) * 16;
        c.sky_light[li] = match s {
            Some(a) => a,
            None => LightArray::filled(if bottom > top { 15 } else { 0 }),
        };
    }
    c.block_light = block;
    c.set_layout(layout);
    c.lit = lit;
    if let Some(Tag::List(be)) = root.get("block_entities") {
        c.block_entities = be
            .iter()
            .filter(|t| t.as_compound().is_some())
            .cloned()
            .collect();
    }
    c.extra = root
        .iter()
        .filter(|(k, _)| !KNOWN_KEYS.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    c.dirty = false;
    Ok(c)
}

/// Encode chunk NBT to bytes (named root with empty name, as vanilla writes).
pub fn chunk_to_bytes(c: &Chunk) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 * 1024);
    basalt_nbt::write_named(&mut out, "", &chunk_to_nbt(c));
    out
}

pub fn chunk_from_bytes(b: &[u8]) -> Result<(Chunk, String)> {
    chunk_from_bytes_in(b, crate::Layout::OVERWORLD)
}

pub fn chunk_from_bytes_in(b: &[u8], layout: crate::Layout) -> Result<(Chunk, String)> {
    let (_, t) = basalt_nbt::Reader::new(b).named()?;
    let status = t
        .get("Status")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    Ok((chunk_from_nbt_in(&t, layout)?, status))
}

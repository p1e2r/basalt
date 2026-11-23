//! Structure templates (`data/minecraft/structure/**.nbt`), read at runtime from the official
//! server jar (or an extracted copy) — never bundled with Basalt.
//!
//! Lookup order: `$BASALT_STRUCTURES` (a directory or jar), `<data>/structure` (an extracted
//! `data/minecraft/structure` tree), `<data>/../vanilla/server.jar` (the bundler jar, the inner
//! `META-INF/versions/*/server-*.jar` is opened in memory) and `<data>/../vanilla/srv/versions/*/server-*.jar`.

use anyhow::{anyhow, bail, Context, Result};
use basalt_data::BlockState;
use basalt_nbt::Tag;
use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Horizontal / vertical directions in vanilla's 3D order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Dir {
    pub fn parse(s: &str) -> Option<Dir> {
        Some(match s {
            "down" => Dir::Down,
            "up" => Dir::Up,
            "north" => Dir::North,
            "south" => Dir::South,
            "west" => Dir::West,
            "east" => Dir::East,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            Dir::Down => "down",
            Dir::Up => "up",
            Dir::North => "north",
            Dir::South => "south",
            Dir::West => "west",
            Dir::East => "east",
        }
    }
    pub fn step(self) -> [i32; 3] {
        match self {
            Dir::Down => [0, -1, 0],
            Dir::Up => [0, 1, 0],
            Dir::North => [0, 0, -1],
            Dir::South => [0, 0, 1],
            Dir::West => [-1, 0, 0],
            Dir::East => [1, 0, 0],
        }
    }
    pub fn opposite(self) -> Dir {
        match self {
            Dir::Down => Dir::Up,
            Dir::Up => Dir::Down,
            Dir::North => Dir::South,
            Dir::South => Dir::North,
            Dir::West => Dir::East,
            Dir::East => Dir::West,
        }
    }
    pub fn rotate(self, rot: Rotation) -> Dir {
        if matches!(self, Dir::Up | Dir::Down) {
            return self;
        }
        let mut d = self;
        for _ in 0..rot as u8 {
            d = d.cw();
        }
        d
    }
    fn cw(self) -> Dir {
        match self {
            Dir::North => Dir::East,
            Dir::East => Dir::South,
            Dir::South => Dir::West,
            Dir::West => Dir::North,
            d => d,
        }
    }
    pub fn mirror_lr(self) -> Dir {
        match self {
            Dir::West => Dir::East,
            Dir::East => Dir::West,
            d => d,
        }
    }
    pub fn mirror_fb(self) -> Dir {
        match self {
            Dir::North => Dir::South,
            Dir::South => Dir::North,
            d => d,
        }
    }
}

/// Rotation in clockwise quarter turns (vanilla order NONE, CW_90, CW_180, CCW_90).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rotation {
    None = 0,
    Cw90 = 1,
    Cw180 = 2,
    Ccw90 = 3,
}

impl Rotation {
    pub const ALL: [Rotation; 4] = [
        Rotation::None,
        Rotation::Cw90,
        Rotation::Cw180,
        Rotation::Ccw90,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Rotation::None => "NONE",
            Rotation::Cw90 => "CLOCKWISE_90",
            Rotation::Cw180 => "CLOCKWISE_180",
            Rotation::Ccw90 => "COUNTERCLOCKWISE_90",
        }
    }
    pub fn parse(s: &str) -> Rotation {
        match s {
            "CLOCKWISE_90" => Rotation::Cw90,
            "CLOCKWISE_180" => Rotation::Cw180,
            "COUNTERCLOCKWISE_90" => Rotation::Ccw90,
            _ => Rotation::None,
        }
    }
    /// Rotate a template-local position around the origin.
    pub fn apply(self, p: [i32; 3]) -> [i32; 3] {
        match self {
            Rotation::None => p,
            Rotation::Cw90 => [-p[2], p[1], p[0]],
            Rotation::Cw180 => [-p[0], p[1], -p[2]],
            Rotation::Ccw90 => [p[2], p[1], -p[0]],
        }
    }
    pub fn then(self, o: Rotation) -> Rotation {
        Rotation::ALL[((self as u8 + o as u8) & 3) as usize]
    }
}

/// Axis-aligned inclusive block box.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BBox {
    pub min: [i32; 3],
    pub max: [i32; 3],
}

impl BBox {
    pub fn new(a: [i32; 3], b: [i32; 3]) -> BBox {
        BBox {
            min: [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])],
            max: [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])],
        }
    }
    pub fn moved(&self, d: [i32; 3]) -> BBox {
        BBox {
            min: [self.min[0] + d[0], self.min[1] + d[1], self.min[2] + d[2]],
            max: [self.max[0] + d[0], self.max[1] + d[1], self.max[2] + d[2]],
        }
    }
    pub fn contains(&self, p: [i32; 3]) -> bool {
        (0..3).all(|i| p[i] >= self.min[i] && p[i] <= self.max[i])
    }
    pub fn intersects(&self, o: &BBox) -> bool {
        (0..3).all(|i| self.max[i] >= o.min[i] && self.min[i] <= o.max[i])
    }
    pub fn inside(&self, o: &BBox) -> bool {
        (0..3).all(|i| self.min[i] >= o.min[i] && self.max[i] <= o.max[i])
    }
    pub fn union(&self, o: &BBox) -> BBox {
        BBox {
            min: [
                self.min[0].min(o.min[0]),
                self.min[1].min(o.min[1]),
                self.min[2].min(o.min[2]),
            ],
            max: [
                self.max[0].max(o.max[0]),
                self.max[1].max(o.max[1]),
                self.max[2].max(o.max[2]),
            ],
        }
    }
    pub fn encapsulate(&mut self, p: [i32; 3]) {
        for i in 0..3 {
            self.min[i] = self.min[i].min(p[i]);
            self.max[i] = self.max[i].max(p[i]);
        }
    }
    pub fn span(&self, i: usize) -> i32 {
        self.max[i] - self.min[i] + 1
    }
    pub fn center(&self) -> [i32; 3] {
        [
            (self.min[0] + self.max[0]).div_euclid(2),
            (self.min[1] + self.max[1]).div_euclid(2),
            (self.min[2] + self.max[2]).div_euclid(2),
        ]
    }
}

#[derive(Clone, Debug)]
pub struct Jigsaw {
    pub pos: [i32; 3],
    pub front: Dir,
    pub top: Dir,
    pub name: String,
    pub target: String,
    pub pool: String,
    pub final_state: String,
    pub rollable: bool,
    pub selection_priority: i32,
    pub placement_priority: i32,
}

#[derive(Clone, Debug)]
pub struct TBlock {
    pub pos: [i32; 3],
    /// Index into each palette.
    pub state: u32,
    pub nbt: Option<Tag>,
}

#[derive(Debug)]
pub struct Template {
    pub size: [i32; 3],
    pub palettes: Vec<Vec<u16>>,
    /// Blocks in vanilla's placement order (full blocks, then other blocks, then block entities;
    /// each sorted by y, x, z).
    pub blocks: Vec<TBlock>,
    /// Jigsaw blocks in template order (sorted by y, x, z).
    pub jigsaws: Vec<Jigsaw>,
    pub entities: Vec<([f64; 3], [i32; 3], Tag)>,
}

impl Template {
    /// Bounding box of the template placed at `pos` with `rot` (pivot = origin).
    pub fn bbox(&self, pos: [i32; 3], rot: Rotation) -> BBox {
        let a = rot.apply([0, 0, 0]);
        let b = rot.apply([self.size[0] - 1, self.size[1] - 1, self.size[2] - 1]);
        BBox::new(a, b).moved(pos)
    }
}

enum Source {
    Dir(PathBuf),
    Zip(Mutex<zip::ZipArchive<Cursor<Vec<u8>>>>),
}

pub struct Templates {
    src: Option<Source>,
    cache: Mutex<HashMap<String, Option<Arc<Template>>>>,
    /// Block-state ids that are "full blocks" for vanilla's template ordering heuristic.
    full: Vec<bool>,
}

fn open_zip(bytes: Vec<u8>) -> Result<zip::ZipArchive<Cursor<Vec<u8>>>> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes))?;
    // A bundler jar: open the inner versioned server jar.
    let inner = (0..z.len())
        .filter_map(|i| z.by_index(i).ok().map(|f| f.name().to_string()))
        .find(|n| n.starts_with("META-INF/versions/") && n.ends_with(".jar"));
    if let Some(n) = inner {
        let mut buf = Vec::new();
        z.by_name(&n)?.read_to_end(&mut buf)?;
        return Ok(zip::ZipArchive::new(Cursor::new(buf))?);
    }
    Ok(z)
}

fn find_source(data_dir: &Path) -> Option<Source> {
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("BASALT_STRUCTURES") {
        cands.push(PathBuf::from(p));
    }
    cands.push(data_dir.join("structure"));
    let root = data_dir.join("..");
    cands.push(root.join("vanilla/server.jar"));
    if let Ok(rd) = std::fs::read_dir(root.join("vanilla/srv/versions")) {
        for e in rd.flatten() {
            if let Ok(rd2) = std::fs::read_dir(e.path()) {
                for f in rd2.flatten() {
                    if f.path().extension().map(|x| x == "jar").unwrap_or(false) {
                        cands.push(f.path());
                    }
                }
            }
        }
    }
    for c in cands {
        if c.is_dir() {
            return Some(Source::Dir(c));
        }
        if c.is_file() {
            if let Ok(b) = std::fs::read(&c) {
                if let Ok(z) = open_zip(b) {
                    return Some(Source::Zip(Mutex::new(z)));
                }
            }
        }
    }
    None
}

impl Templates {
    pub fn new(data_dir: &Path) -> Templates {
        let full = crate::region::flags()
            .iter()
            .map(|f| f & crate::region::F_FULL != 0)
            .collect();
        Templates {
            src: find_source(data_dir),
            cache: Mutex::new(HashMap::new()),
            full,
        }
    }

    pub fn available(&self) -> bool {
        self.src.is_some()
    }

    fn read_raw(&self, name: &str) -> Result<Vec<u8>> {
        let rel = name.trim_start_matches("minecraft:");
        match self
            .src
            .as_ref()
            .ok_or_else(|| anyhow!("no structure template source (server jar) found"))?
        {
            Source::Dir(d) => Ok(std::fs::read(d.join(format!("{rel}.nbt")))?),
            Source::Zip(z) => {
                let mut z = z.lock().unwrap();
                let mut f = z
                    .by_name(&format!("data/minecraft/structure/{rel}.nbt"))
                    .with_context(|| format!("template {name}"))?;
                let mut buf = Vec::new();
                f.read_to_end(&mut buf)?;
                Ok(buf)
            }
        }
    }

    pub fn get(&self, name: &str) -> Option<Arc<Template>> {
        if let Some(t) = self.cache.lock().unwrap().get(name) {
            return t.clone();
        }
        let t = self.load(name).ok().map(Arc::new);
        self.cache
            .lock()
            .unwrap()
            .insert(name.to_string(), t.clone());
        t
    }

    fn load(&self, name: &str) -> Result<Template> {
        let raw = self.read_raw(name)?;
        let mut buf = Vec::new();
        flate2::read::GzDecoder::new(&raw[..]).read_to_end(&mut buf)?;
        let (_, root) = basalt_nbt::Reader::new(&buf).named()?;
        parse(&root, &self.full)
    }
}

fn int3(t: Option<&Tag>) -> Option<[i32; 3]> {
    let l = t?.as_list()?;
    if l.len() != 3 {
        return None;
    }
    Some([
        l[0].as_i64()? as i32,
        l[1].as_i64()? as i32,
        l[2].as_i64()? as i32,
    ])
}

fn state_of(e: &Tag) -> (u16, String) {
    let name = e
        .get("Name")
        .and_then(|n| n.as_str())
        .unwrap_or("minecraft:air");
    let mut st = BlockState::from_name(name).unwrap_or(BlockState::AIR);
    let mut orient = String::new();
    if let Some(Tag::Compound(p)) = e.get("Properties") {
        for (k, v) in p {
            if let Some(v) = v.as_str() {
                if k == "orientation" {
                    orient = v.to_string();
                }
                if let Some(n) = st.with(k, v) {
                    st = n;
                }
            }
        }
    }
    (st.0 as u16, orient)
}

fn parse(root: &Tag, full: &[bool]) -> Result<Template> {
    let size = int3(root.get("size")).ok_or_else(|| anyhow!("template without size"))?;
    let mut palettes = Vec::new();
    let mut orients: Vec<String> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut read_pal = |l: &Vec<Tag>, first: bool| -> Vec<u16> {
        l.iter()
            .map(|e| {
                let (s, o) = state_of(e);
                if first {
                    orients.push(o);
                    names.push(
                        e.get("Name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string(),
                    );
                }
                s
            })
            .collect()
    };
    if let Some(l) = root.get("palette").and_then(|p| p.as_list()) {
        palettes.push(read_pal(l, true));
    } else if let Some(ls) = root.get("palettes").and_then(|p| p.as_list()) {
        for (i, l) in ls.iter().enumerate() {
            if let Some(l) = l.as_list() {
                palettes.push(read_pal(l, i == 0));
            }
        }
    } else {
        bail!("template without palette");
    }
    let mut full_l = Vec::new();
    let mut other_l = Vec::new();
    let mut nbt_l = Vec::new();
    let mut jigsaws = Vec::new();
    for b in root
        .get("blocks")
        .and_then(|b| b.as_list())
        .map(|l| l.as_slice())
        .unwrap_or(&[])
    {
        let Some(pos) = int3(b.get("pos")) else {
            continue;
        };
        let state = b.get("state").and_then(|s| s.as_i64()).unwrap_or(0) as u32;
        let nbt = b.get("nbt").cloned();
        let sid = palettes[0].get(state as usize).copied().unwrap_or(0);
        if names
            .get(state as usize)
            .map(|n| n == "minecraft:jigsaw")
            .unwrap_or(false)
        {
            let o = &orients[state as usize];
            let (f, t) = o.split_once('_').unwrap_or(("north", "up"));
            let n = nbt.as_ref();
            let s = |k: &str| {
                n.and_then(|n| n.get(k))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let i = |k: &str| {
                n.and_then(|n| n.get(k))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0) as i32
            };
            let front = Dir::parse(f).unwrap_or(Dir::North);
            let joint = s("joint");
            let rollable = if joint.is_empty() {
                !matches!(front, Dir::Up | Dir::Down)
            } else {
                joint == "rollable"
            };
            jigsaws.push(Jigsaw {
                pos,
                front,
                top: Dir::parse(t).unwrap_or(Dir::Up),
                name: s("name"),
                target: s("target"),
                pool: s("pool"),
                final_state: {
                    let f = s("final_state");
                    if f.is_empty() {
                        "minecraft:air".into()
                    } else {
                        f
                    }
                },
                rollable,
                selection_priority: i("selection_priority"),
                placement_priority: i("placement_priority"),
            });
        }
        let tb = TBlock { pos, state, nbt };
        if tb.nbt.is_some() {
            nbt_l.push(tb);
        } else if full[sid as usize] {
            full_l.push(tb);
        } else {
            other_l.push(tb);
        }
    }
    let key = |b: &TBlock| (b.pos[1], b.pos[0], b.pos[2]);
    full_l.sort_by_key(key);
    other_l.sort_by_key(key);
    nbt_l.sort_by_key(key);
    jigsaws.sort_by_key(|j| (j.pos[1], j.pos[0], j.pos[2]));
    let mut blocks = full_l;
    blocks.extend(other_l);
    blocks.extend(nbt_l);
    let mut entities = Vec::new();
    for e in root
        .get("entities")
        .and_then(|b| b.as_list())
        .map(|l| l.as_slice())
        .unwrap_or(&[])
    {
        let pos = e
            .get("pos")
            .and_then(|p| p.as_list())
            .map(|l| {
                [
                    l[0].as_f64().unwrap_or(0.0),
                    l[1].as_f64().unwrap_or(0.0),
                    l[2].as_f64().unwrap_or(0.0),
                ]
            })
            .unwrap_or([0.0; 3]);
        let bp = int3(e.get("blockPos")).unwrap_or([0; 3]);
        if let Some(n) = e.get("nbt") {
            entities.push((pos, bp, n.clone()));
        }
    }
    Ok(Template {
        size,
        palettes,
        blocks,
        jigsaws,
        entities,
    })
}

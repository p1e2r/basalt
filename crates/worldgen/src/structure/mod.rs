//! Overworld structures: placement from `worldgen/structure_set`, structure starts and pieces
//! (jigsaw structures from `worldgen/template_pool` + templates read from the server jar, and
//! the template-based single structures), and placing the pieces into chunks during decoration.
//!
//! Every rule here was derived from the data files and checked against structure starts saved
//! by the vanilla server (see the `structures` example); rules are listed in NOTES.md.

#![allow(
    clippy::type_complexity,
    clippy::needless_range_loop,
    clippy::result_unit_err
)]

pub mod adapt;
pub mod jigsaw;
pub mod place;
pub mod template;

use crate::random::{set_large_feature_seed, JavaRandom, Rng};
use crate::Generator;
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use template::{BBox, Rotation, Templates};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reduction {
    Default,
    Legacy1,
    Legacy2,
    Legacy3,
}

#[derive(Clone, Debug)]
pub struct Spread {
    pub spacing: i32,
    pub separation: i32,
    pub salt: i32,
    pub triangular: bool,
    pub frequency: f32,
    pub reduction: Reduction,
    /// (other set index, chunk count)
    pub exclusion: Option<(usize, i32)>,
    pub locate_offset: [i32; 3],
}

#[derive(Clone, Debug)]
pub struct Rings {
    pub distance: i32,
    pub spread: i32,
    pub count: i32,
    pub salt: i32,
    pub preferred: Vec<bool>,
}

#[derive(Clone, Debug)]
pub enum Placement {
    Spread(Spread),
    Rings(Rings),
}

pub struct Set {
    pub name: String,
    pub placement: Placement,
    /// (structure index, weight)
    pub entries: Vec<(usize, i32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adaptation {
    None,
    Bury,
    BeardThin,
    BeardBox,
    Encapsulate,
}

#[derive(Clone, Debug)]
pub enum Kind {
    Jigsaw(Arc<jigsaw::JigsawDef>),
    BuriedTreasure,
    Shipwreck {
        beached: bool,
    },
    Igloo,
    SwampHut,
    DesertPyramid,
    JunglePyramid,
    OceanRuin {
        warm: bool,
        large_p: f32,
        cluster_p: f32,
    },
    RuinedPortal(Vec<PortalSetup>),
    Mineshaft {
        mesa: bool,
    },
    Monument,
    Mansion,
    Stronghold,
    /// Not generated in the overworld (fortress, bastion, end city, nether fossil).
    Other,
}

#[derive(Clone, Debug)]
pub struct PortalSetup {
    pub placement: String,
    pub air_pocket: f32,
    pub mossiness: f32,
    pub overgrown: bool,
    pub vines: bool,
    pub can_be_cold: bool,
    pub blackstone: bool,
    pub weight: f32,
}

pub struct Def {
    pub name: String,
    pub kind: Kind,
    pub biomes: Vec<bool>,
    /// Decoration step index (GenerationStep order).
    pub step: usize,
    pub adaptation: Adaptation,
}

/// A generated piece of a structure.
#[derive(Clone, Debug)]
pub struct Piece {
    pub bb: BBox,
    pub kind: PieceKind,
}

#[derive(Clone, Debug)]
pub enum PieceKind {
    /// A jigsaw pool element placed at `pos` with `rot`.
    Pool {
        element: Arc<jigsaw::Element>,
        pos: [i32; 3],
        rot: Rotation,
        ground_delta: i32,
        junctions: Vec<Junction>,
    },
    /// A single template at `pos` with `rot` (and optional mirror) and processing options.
    Template {
        name: String,
        pos: [i32; 3],
        rot: Rotation,
        mirror: bool,
        pivot: [i32; 3],
        opts: TemplateOpts,
    },
    /// Buried treasure chest.
    Chest { pos: [i32; 3], loot: String },
    /// Swamp hut (measured layout, place::swamp_hut) with its box corner and orientation
    /// (vanilla's `O`: 0 south, 1 west, 2 north, 3 east).
    SwampHut { origin: [i32; 3], o: u8 },
}

#[derive(Clone, Debug, Default)]
pub struct TemplateOpts {
    pub ignore_air: bool,
    pub integrity: f32,
    pub processors: Option<String>,
    /// Re-take y from the region's OCEAN_FLOOR_WG height at the origin when placing.
    pub floor_y: bool,
    /// Blocks set (world coordinates) after the template (measured variations, e.g. the igloo
    /// trapdoor without a laboratory).
    pub set: Vec<([i32; 3], u16)>,
}

#[derive(Clone, Copy, Debug)]
pub struct Junction {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub dy: i32,
    pub rigid: bool,
}

#[derive(Clone, Debug)]
pub struct Start {
    pub def: usize,
    pub cx: i32,
    pub cz: i32,
    pub pieces: Vec<Piece>,
}

impl Start {
    pub fn bbox(&self) -> BBox {
        let mut b = self.pieces[0].bb;
        for p in &self.pieces[1..] {
            b = b.union(&p.bb);
        }
        b
    }
}

pub const STEPS: [&str; 11] = [
    "raw_generation",
    "lakes",
    "local_modifications",
    "underground_structures",
    "surface_structures",
    "strongholds",
    "underground_ores",
    "underground_decoration",
    "fluid_springs",
    "vegetal_decoration",
    "top_layer_modification",
];

pub struct Structures {
    pub sets: Vec<Set>,
    pub defs: Vec<Def>,
    pub by_name: HashMap<String, usize>,
    pub templates: Templates,
    pub pools: jigsaw::Pools,
    pub processors: place::Processors,
    /// Placed-feature ids of the features used by feature pool elements.
    pub feature_ids: HashMap<String, usize>,
    data: PathBuf,
    starts: Mutex<HashMap<(i32, i32), Arc<Vec<Start>>>>,
    heights: Mutex<HashMap<(i32, i32, bool), i32>>,
    rings: Mutex<HashMap<usize, Arc<Vec<(i32, i32)>>>>,
    /// Terrain adaptation on (env BASALT_NO_BEARD turns it off, for measuring).
    pub beard: bool,
}

fn read_json(p: &Path) -> Result<Value> {
    serde_json::from_str(
        &std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?,
    )
    .with_context(|| format!("parsing {}", p.display()))
}

/// Resolve a biome tag (or single id / list) to a per-biome flag vector.
pub fn biome_set(data: &Path, names: &[String], v: &Value) -> Vec<bool> {
    let mut out = vec![false; names.len()];
    let mut seen = HashSet::new();
    fn add(data: &Path, names: &[String], v: &Value, out: &mut [bool], seen: &mut HashSet<String>) {
        match v {
            Value::Array(a) => {
                for x in a {
                    add(data, names, x, out, seen)
                }
            }
            Value::Object(o) => {
                if let Some(id) = o.get("id") {
                    add(data, names, id, out, seen)
                }
            }
            Value::String(s) => {
                if let Some(t) = s.strip_prefix('#') {
                    if !seen.insert(t.to_string()) {
                        return;
                    }
                    let rel = t.trim_start_matches("minecraft:");
                    if let Ok(tv) = read_json(&data.join(format!("tags/worldgen/biome/{rel}.json")))
                    {
                        add(data, names, &tv["values"], out, seen);
                    }
                } else if let Some(i) = names
                    .iter()
                    .position(|n| n == s.trim_start_matches("minecraft:"))
                {
                    out[i] = true;
                }
            }
            _ => {}
        }
    }
    add(data, names, v, &mut out, &mut seen);
    out
}

impl Structures {
    pub fn load(
        data_dir: &Path,
        biome_names: &[String],
        features: &mut crate::features::Features,
    ) -> Result<Structures> {
        let data = data_dir.join("data/minecraft");
        let wg = data.join("worldgen");
        let mut defs = Vec::new();
        let mut by_name = HashMap::new();
        let mut files: Vec<PathBuf> = std::fs::read_dir(wg.join("structure"))?
            .flatten()
            .map(|e| e.path())
            .collect();
        files.sort();
        let mut pools = jigsaw::Pools::new(&wg);
        for f in files {
            let name = f
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let v = read_json(&f)?;
            let ty = v["type"]
                .as_str()
                .unwrap_or("")
                .trim_start_matches("minecraft:");
            let kind = match ty {
                "jigsaw" => Kind::Jigsaw(Arc::new(jigsaw::JigsawDef::parse(&v, &mut pools)?)),
                "buried_treasure" => Kind::BuriedTreasure,
                "shipwreck" => Kind::Shipwreck {
                    beached: v["is_beached"].as_bool().unwrap_or(false),
                },
                "igloo" => Kind::Igloo,
                "swamp_hut" => Kind::SwampHut,
                "desert_pyramid" => Kind::DesertPyramid,
                "jungle_temple" => Kind::JunglePyramid,
                "ocean_ruin" => Kind::OceanRuin {
                    warm: v["biome_temp"].as_str() == Some("warm"),
                    large_p: v["large_probability"].as_f64().unwrap_or(0.3) as f32,
                    cluster_p: v["cluster_probability"].as_f64().unwrap_or(0.9) as f32,
                },
                "ruined_portal" => Kind::RuinedPortal(
                    v["setups"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .map(|s| PortalSetup {
                                    placement: s["placement"].as_str().unwrap_or("").to_string(),
                                    air_pocket: s["air_pocket_probability"].as_f64().unwrap_or(0.0)
                                        as f32,
                                    mossiness: s["mossiness"].as_f64().unwrap_or(0.0) as f32,
                                    overgrown: s["overgrown"].as_bool().unwrap_or(false),
                                    vines: s["vines"].as_bool().unwrap_or(false),
                                    can_be_cold: s["can_be_cold"].as_bool().unwrap_or(false),
                                    blackstone: s["replace_with_blackstone"]
                                        .as_bool()
                                        .unwrap_or(false),
                                    weight: s["weight"].as_f64().unwrap_or(1.0) as f32,
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                ),
                "mineshaft" => Kind::Mineshaft {
                    mesa: v["mineshaft_type"].as_str() == Some("mesa"),
                },
                "ocean_monument" => Kind::Monument,
                "woodland_mansion" => Kind::Mansion,
                "stronghold" => Kind::Stronghold,
                _ => Kind::Other,
            };
            let step = STEPS
                .iter()
                .position(|s| Some(*s) == v["step"].as_str())
                .unwrap_or(4);
            let adaptation = match v["terrain_adaptation"].as_str().unwrap_or("none") {
                "bury" => Adaptation::Bury,
                "beard_thin" => Adaptation::BeardThin,
                "beard_box" => Adaptation::BeardBox,
                "encapsulate" => Adaptation::Encapsulate,
                _ => Adaptation::None,
            };
            by_name.insert(name.clone(), defs.len());
            defs.push(Def {
                name,
                kind,
                biomes: biome_set(&data, biome_names, &v["biomes"]),
                step,
                adaptation,
            });
        }
        let mut set_files: Vec<PathBuf> = std::fs::read_dir(wg.join("structure_set"))?
            .flatten()
            .map(|e| e.path())
            .collect();
        set_files.sort();
        let mut raw = Vec::new();
        for f in &set_files {
            raw.push((
                f.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string(),
                read_json(f)?,
            ));
        }
        let set_index: HashMap<String, usize> = raw
            .iter()
            .enumerate()
            .map(|(i, (n, _))| (n.clone(), i))
            .collect();
        let mut sets = Vec::new();
        for (name, v) in &raw {
            let p = &v["placement"];
            let i = |k: &str, d: i64| p.get(k).and_then(|x| x.as_i64()).unwrap_or(d) as i32;
            let placement = match p["type"]
                .as_str()
                .unwrap_or("")
                .trim_start_matches("minecraft:")
            {
                "concentric_rings" => Placement::Rings(Rings {
                    distance: i("distance", 32),
                    spread: i("spread", 3),
                    count: i("count", 128),
                    salt: i("salt", 0),
                    preferred: biome_set(&data, biome_names, &p["preferred_biomes"]),
                }),
                _ => Placement::Spread(Spread {
                    spacing: i("spacing", 1),
                    separation: i("separation", 0),
                    salt: i("salt", 0),
                    triangular: p["spread_type"].as_str() == Some("triangular"),
                    frequency: p.get("frequency").and_then(|f| f.as_f64()).unwrap_or(1.0) as f32,
                    reduction: match p["frequency_reduction_method"]
                        .as_str()
                        .unwrap_or("default")
                    {
                        "legacy_type_1" => Reduction::Legacy1,
                        "legacy_type_2" => Reduction::Legacy2,
                        "legacy_type_3" => Reduction::Legacy3,
                        _ => Reduction::Default,
                    },
                    exclusion: p.get("exclusion_zone").and_then(|e| {
                        let other = e["other_set"].as_str()?.trim_start_matches("minecraft:");
                        Some((*set_index.get(other)?, e["chunk_count"].as_i64()? as i32))
                    }),
                    locate_offset: p
                        .get("locate_offset")
                        .and_then(|a| a.as_array())
                        .map(|a| {
                            [
                                a[0].as_i64().unwrap_or(0) as i32,
                                a[1].as_i64().unwrap_or(0) as i32,
                                a[2].as_i64().unwrap_or(0) as i32,
                            ]
                        })
                        .unwrap_or([0; 3]),
                }),
            };
            let entries = v["structures"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|e| {
                            let s = e["structure"].as_str()?.trim_start_matches("minecraft:");
                            Some((*by_name.get(s)?, e["weight"].as_i64().unwrap_or(1) as i32))
                        })
                        .collect()
                })
                .unwrap_or_default();
            sets.push(Set {
                name: name.clone(),
                placement,
                entries,
            });
        }
        let mut feature_ids = HashMap::new();
        fn scan(dir: &Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    scan(&p, out);
                    continue;
                }
                let Ok(t) = std::fs::read_to_string(&p) else {
                    continue;
                };
                let Ok(v) = serde_json::from_str::<Value>(&t) else {
                    continue;
                };
                for el in v["elements"].as_array().into_iter().flatten() {
                    if let Some(f) = el["element"]["feature"].as_str() {
                        out.push(f.to_string());
                    }
                }
            }
        }
        let mut names = Vec::new();
        scan(&wg.join("template_pool"), &mut names);
        for n in names {
            let key = n.trim_start_matches("minecraft:").to_string();
            if feature_ids.contains_key(&key) {
                continue;
            }
            let id = features.placed_ref(&Value::String(n));
            feature_ids.insert(key, id);
        }
        Ok(Structures {
            feature_ids,
            data: data.clone(),
            sets,
            defs,
            by_name,
            templates: Templates::new(data_dir),
            pools,
            processors: place::Processors::new(&wg),
            starts: Mutex::new(HashMap::new()),
            heights: Mutex::new(HashMap::new()),
            rings: Mutex::new(HashMap::new()),
            beard: std::env::var("BASALT_NO_BEARD").is_err(),
        })
    }

    pub fn set_of(&self, def: usize) -> Option<usize> {
        self.sets
            .iter()
            .position(|s| s.entries.iter().any(|e| e.0 == def))
    }
}

/// Is (cx, cz) the placement chunk of its region (geometry + frequency, no biome check)?
pub fn spread_chunk(seed: i64, sp: &Spread, cx: i32, cz: i32) -> bool {
    let (px, pz) = spread_potential(seed, sp, cx, cz);
    if px != cx || pz != cz {
        return false;
    }
    if sp.frequency < 1.0 && !frequency_ok(seed, sp, cx, cz) {
        return false;
    }
    true
}

/// The placement chunk of the spacing region that contains (cx, cz).
pub fn spread_potential(seed: i64, sp: &Spread, cx: i32, cz: i32) -> (i32, i32) {
    let rx = cx.div_euclid(sp.spacing);
    let rz = cz.div_euclid(sp.spacing);
    let mut r = JavaRandom::new(0);
    r.set_seed(
        (rx as i64)
            .wrapping_mul(341873128712)
            .wrapping_add((rz as i64).wrapping_mul(132897987541))
            .wrapping_add(seed)
            .wrapping_add(sp.salt as i64),
    );
    let range = sp.spacing - sp.separation;
    let (ox, oz) = if range <= 0 {
        (0, 0)
    } else if sp.triangular {
        let a = (r.next_int_bounded(range) + r.next_int_bounded(range)) / 2;
        let b = (r.next_int_bounded(range) + r.next_int_bounded(range)) / 2;
        (a, b)
    } else {
        let a = r.next_int_bounded(range);
        (a, r.next_int_bounded(range))
    };
    (rx * sp.spacing + ox, rz * sp.spacing + oz)
}

fn frequency_ok(seed: i64, sp: &Spread, cx: i32, cz: i32) -> bool {
    let mut r = JavaRandom::new(0);
    match sp.reduction {
        Reduction::Default => {
            r.set_seed(
                (cx as i64)
                    .wrapping_mul(341873128712)
                    .wrapping_add((cz as i64).wrapping_mul(132897987541))
                    .wrapping_add(seed)
                    .wrapping_add(sp.salt as i64),
            );
            r.next_float() < sp.frequency
        }
        Reduction::Legacy1 => {
            let i = cx >> 4;
            let j = cz >> 4;
            r.set_seed(((i ^ (j << 4)) as i64) ^ seed);
            r.next_int();
            r.next_int_bounded((1.0 / sp.frequency) as i32) == 0
        }
        Reduction::Legacy2 => {
            r.set_seed(
                (cx as i64)
                    .wrapping_mul(341873128712)
                    .wrapping_add((cz as i64).wrapping_mul(132897987541))
                    .wrapping_add(seed)
                    .wrapping_add(10387320),
            );
            r.next_float() < sp.frequency
        }
        Reduction::Legacy3 => {
            set_large_feature_seed(&mut r, seed, cx, cz);
            r.next_double() < sp.frequency as f64
        }
    }
}

/// Where a structure's start was found: position used for the biome check plus its pieces.
pub struct Stub {
    pub pos: [i32; 3],
    pub pieces: Vec<Piece>,
}

impl Generator {
    pub fn structures(&self) -> Option<&Structures> {
        self.structures.as_ref()
    }

    /// Noise-only surface height (cached): first free y at (x, z).
    pub fn first_free(&self, x: i32, z: i32, ocean_floor: bool) -> i32 {
        let s = self.structures.as_ref().expect("structures loaded");
        if let Some(h) = s.heights.lock().unwrap().get(&(x, z, ocean_floor)) {
            return *h;
        }
        let h = self.base_height(x, z, ocean_floor);
        let mut m = s.heights.lock().unwrap();
        if m.len() > 200_000 {
            m.clear();
        }
        m.insert((x, z, ocean_floor), h);
        h
    }

    /// Is (cx, cz) a structure chunk of `set` (placement geometry, frequency and exclusion zone)?
    pub fn is_structure_chunk(&self, set: usize, cx: i32, cz: i32) -> bool {
        let s = self.structures.as_ref().unwrap();
        match &s.sets[set].placement {
            Placement::Spread(sp) => {
                if !spread_chunk(self.seed, sp, cx, cz) {
                    return false;
                }
                if let Some((other, n)) = sp.exclusion {
                    if let Placement::Spread(o) = &s.sets[other].placement {
                        for x in cx - n..=cx + n {
                            for z in cz - n..=cz + n {
                                if spread_chunk(self.seed, o, x, z) {
                                    return false;
                                }
                            }
                        }
                    }
                }
                true
            }
            Placement::Rings(_) => self.ring_positions(set).contains(&(cx, cz)),
        }
    }

    /// Structure starts of a chunk (cached).
    pub fn structure_starts(&self, cx: i32, cz: i32) -> Arc<Vec<Start>> {
        let Some(s) = self.structures.as_ref() else {
            return Arc::new(Vec::new());
        };
        if let Some(v) = s.starts.lock().unwrap().get(&(cx, cz)) {
            return v.clone();
        }
        let mut out = Vec::new();
        for (si, set) in s.sets.iter().enumerate() {
            if set.entries.is_empty() || !self.is_structure_chunk(si, cx, cz) {
                continue;
            }
            if let Some(st) = self.select_start(si, cx, cz, true) {
                out.push(st);
            }
        }
        let v = Arc::new(out);
        let mut m = s.starts.lock().unwrap();
        if m.len() > 50_000 {
            m.clear();
        }
        m.insert((cx, cz), v.clone());
        v
    }

    /// Pick the structure of a set for a placement chunk (weighted tries) and generate it.
    pub fn select_start(&self, set: usize, cx: i32, cz: i32, full: bool) -> Option<Start> {
        let s = self.structures.as_ref().unwrap();
        let entries = &s.sets[set].entries;
        if entries.len() == 1 {
            return self.try_start(entries[0].0, cx, cz, full);
        }
        let mut list: Vec<(usize, i32)> = entries.clone();
        let mut total: i32 = list.iter().map(|e| e.1).sum();
        let mut r = JavaRandom::new(0);
        set_large_feature_seed(&mut r, self.seed, cx, cz);
        while !list.is_empty() {
            let mut k = r.next_int_bounded(total);
            let mut idx = 0;
            for (i, e) in list.iter().enumerate() {
                k -= e.1;
                if k < 0 {
                    idx = i;
                    break;
                }
            }
            let (d, w) = list[idx];
            if let Some(st) = self.try_start(d, cx, cz, full) {
                return Some(st);
            }
            list.remove(idx);
            total -= w;
        }
        None
    }

    /// Try to start structure `def` in chunk (cx, cz): generation point + biome check.
    pub fn try_start(&self, def: usize, cx: i32, cz: i32, full: bool) -> Option<Start> {
        let s = self.structures.as_ref().unwrap();
        let d = &s.defs[def];
        let mut r = JavaRandom::new(0);
        set_large_feature_seed(&mut r, self.seed, cx, cz);
        let stub = self.generation_point(def, cx, cz, &mut r, full)?;
        let b = self.biome_at(stub.pos[0], stub.pos[1], stub.pos[2]);
        if !d.biomes[b as usize] {
            return None;
        }
        Some(Start {
            def,
            cx,
            cz,
            pieces: stub.pieces,
        })
    }

    fn generation_point(
        &self,
        def: usize,
        cx: i32,
        cz: i32,
        r: &mut JavaRandom,
        full: bool,
    ) -> Option<Stub> {
        let s = self.structures.as_ref().unwrap();
        let d = &s.defs[def];
        let (mx, mz) = (cx * 16 + 8, cz * 16 + 8);
        match &d.kind {
            Kind::Jigsaw(j) => jigsaw::generate(self, s, j, cx, cz, r, full),
            Kind::BuriedTreasure => {
                let pos = [cx * 16 + 9, 90, cz * 16 + 9];
                Some(Stub {
                    pos,
                    pieces: vec![Piece {
                        bb: BBox::new(pos, pos),
                        kind: PieceKind::Chest {
                            pos,
                            loot: "minecraft:chests/buried_treasure".into(),
                        },
                    }],
                })
            }
            Kind::Shipwreck { beached } => place::shipwreck(self, s, *beached, cx, cz, r),
            Kind::Igloo => {
                let y = self.first_free(mx, mz, false) - 1;
                Some(Stub {
                    pos: [mx, y, mz],
                    pieces: place::igloo(self, s, cx, cz, r),
                })
            }
            Kind::OceanRuin {
                warm,
                large_p,
                cluster_p,
            } => {
                let y = self.first_free(mx, mz, true) - 1;
                Some(Stub {
                    pos: [mx, y, mz],
                    pieces: place::ocean_ruin(self, s, *warm, *large_p, *cluster_p, cx, cz, r),
                })
            }
            Kind::RuinedPortal(setups) => place::ruined_portal(self, s, setups, d, cx, cz, r),
            Kind::SwampHut => {
                let y = self.first_free(mx, mz, false) - 1;
                Some(Stub {
                    pos: [mx, y, mz],
                    pieces: vec![place::swamp_hut(self, cx, cz, r)],
                })
            }
            Kind::DesertPyramid | Kind::JunglePyramid => {
                // Rejected when a corner of the footprint (21x21 / 12x15 from the chunk corner)
                // has its surface below sea level (measured: /locate desert_pyramid, seed 12345).
                let (w, dd) = if matches!(d.kind, Kind::DesertPyramid) {
                    (21, 21)
                } else {
                    (12, 15)
                };
                let (x0, z0) = (cx * 16, cz * 16);
                let low = [
                    (x0, z0),
                    (x0 + w - 1, z0),
                    (x0, z0 + dd - 1),
                    (x0 + w - 1, z0 + dd - 1),
                ]
                .iter()
                .map(|c| self.first_free(c.0, c.1, false))
                .min()
                .unwrap_or(0);
                if low <= self.sea_level() {
                    return None;
                }
                let y = self.first_free(mx, mz, false) - 1;
                Some(Stub {
                    pos: [mx, y, mz],
                    pieces: Vec::new(),
                })
            }
            Kind::Mineshaft { .. } => {
                r.next_double();
                Some(Stub {
                    pos: [mx, 50, mz],
                    pieces: Vec::new(),
                })
            }
            Kind::Monument => Some(Stub {
                pos: [mx, self.sea_level(), mz],
                pieces: Vec::new(),
            }),
            Kind::Mansion | Kind::Stronghold | Kind::Other => None,
        }
    }

    /// Structure starts whose pieces may reach into chunk (cx, cz) (starts up to 8 chunks away).
    pub fn structure_refs(&self, cx: i32, cz: i32) -> Vec<Arc<Vec<Start>>> {
        self.structure_refs_reach(cx, cz, 0)
    }

    /// [`Generator::structure_refs`] for the chunk grown by `reach` blocks horizontally.
    pub fn structure_refs_reach(&self, cx: i32, cz: i32, reach: i32) -> Vec<Arc<Vec<Start>>> {
        let clip = BBox {
            min: [cx * 16 - reach, -64, cz * 16 - reach],
            max: [cx * 16 + 15 + reach, 319, cz * 16 + 15 + reach],
        };
        let mut out = Vec::new();
        for dx in -8..=8 {
            for dz in -8..=8 {
                let v = self.structure_starts(cx + dx, cz + dz);
                if v.iter()
                    .any(|st| !st.pieces.is_empty() && st.bbox().intersects(&clip))
                {
                    out.push(v);
                }
            }
        }
        out
    }

    /// Place the pieces of `refs` that belong to decoration `step` into chunk (cx, cz).
    #[allow(clippy::too_many_arguments)]
    pub fn place_structures(
        &self,
        refs: &[Arc<Vec<Start>>],
        step: usize,
        deco: i64,
        cx: i32,
        cz: i32,
        reg: &mut crate::region::Region,
        r: &mut crate::features::R,
    ) {
        let s = self.structures.as_ref().unwrap();
        // Structures of the step in registry order (file order); the index seeds the random.
        let mut idx = 0;
        for (di, d) in s.defs.iter().enumerate() {
            if d.step != step {
                continue;
            }
            let mut seeded = false;
            for v in refs {
                for st in v.iter().filter(|st| st.def == di && !st.pieces.is_empty()) {
                    if !seeded {
                        crate::random::set_feature_seed(r, deco, idx, step as i32);
                        seeded = true;
                    }
                    place::place_start(self, s, st, reg, cx, cz, r);
                }
            }
            idx += 1;
        }
    }

    /// Chunks of a concentric-rings set (strongholds), computed once.
    pub fn ring_positions(&self, set: usize) -> Arc<Vec<(i32, i32)>> {
        let s = self.structures.as_ref().unwrap();
        if let Some(v) = s.rings.lock().unwrap().get(&set) {
            return v.clone();
        }
        let v = Arc::new(Vec::new());
        s.rings.lock().unwrap().insert(set, v.clone());
        v
    }

    /// Nearest start of `structure` (id without namespace) to block (x, z), searching up to
    /// `radius` spacing regions: `(block x, block z)` of the locate position.
    pub fn locate(&self, structure: &str, x: i32, z: i32, radius: i32) -> Option<(i32, i32)> {
        self.locate_spec(structure, x, z, radius)
            .ok()
            .flatten()
            .map(|r| (r.1, r.2))
    }

    /// `/locate structure` semantics: `spec` is a structure id or `#tag`. Err(()) if it names no
    /// structure; Ok(None) if none was found. Ok(Some((found structure id, x, z))).
    pub fn locate_spec(
        &self,
        spec: &str,
        x: i32,
        z: i32,
        radius: i32,
    ) -> Result<Option<(String, i32, i32)>, ()> {
        let s = self.structures.as_ref().ok_or(())?;
        let mut want = vec![false; s.defs.len()];
        if let Some(tag) = spec.strip_prefix('#') {
            let rel = tag.trim_start_matches("minecraft:");
            let v = read_json(&s.data.join(format!("tags/worldgen/structure/{rel}.json")))
                .map_err(|_| ())?;
            fn add(s: &Structures, v: &Value, want: &mut [bool], depth: u32) {
                for e in v["values"].as_array().into_iter().flatten() {
                    let id = e.as_str().or_else(|| e["id"].as_str()).unwrap_or("");
                    if let Some(t) = id.strip_prefix('#') {
                        if depth < 8 {
                            if let Ok(tv) = read_json(&s.data.join(format!(
                                "tags/worldgen/structure/{}.json",
                                t.trim_start_matches("minecraft:")
                            ))) {
                                add(s, &tv, want, depth + 1);
                            }
                        }
                    } else if let Some(i) = s.by_name.get(id.trim_start_matches("minecraft:")) {
                        want[*i] = true;
                    }
                }
            }
            add(s, &v, &mut want, 0);
        } else {
            want[*s
                .by_name
                .get(spec.trim_start_matches("minecraft:"))
                .ok_or(())?] = true;
        }
        let mut best: Option<(i64, String, i32, i32)> = None;
        for (si, set) in s.sets.iter().enumerate() {
            if !set.entries.iter().any(|e| want[e.0]) {
                continue;
            }
            if let Some((d, def, bx, bz)) = self.locate_in(si, &want, x, z, radius) {
                if best.as_ref().map(|b| d < b.0).unwrap_or(true) {
                    best = Some((d, s.defs[def].name.clone(), bx, bz));
                }
            }
        }
        Ok(best.map(|b| (format!("minecraft:{}", b.1), b.2, b.3)))
    }

    fn locate_in(
        &self,
        set: usize,
        want: &[bool],
        x: i32,
        z: i32,
        radius: i32,
    ) -> Option<(i64, usize, i32, i32)> {
        let s = self.structures.as_ref()?;
        let Placement::Spread(sp) = &s.sets[set].placement else {
            return None;
        };
        let (scx, scz) = (x >> 4, z >> 4);
        for ring in 0..=radius {
            for dx in -ring..=ring {
                let edge_x = dx == -ring || dx == ring;
                for dz in -ring..=ring {
                    if !edge_x && dz != -ring && dz != ring {
                        continue;
                    }
                    let (pcx, pcz) = spread_potential(
                        self.seed,
                        sp,
                        scx + dx * sp.spacing,
                        scz + dz * sp.spacing,
                    );
                    if !self.is_structure_chunk(set, pcx, pcz) {
                        continue;
                    }
                    let Some(st) = self.select_start(set, pcx, pcz, false) else {
                        continue;
                    };
                    if !want[st.def] {
                        continue;
                    }
                    let bx = pcx * 16 + sp.locate_offset[0];
                    let bz = pcz * 16 + sp.locate_offset[2];
                    let d = (bx as i64 - x as i64).pow(2) + (bz as i64 - z as i64).pow(2);
                    // Measured: within a ring vanilla answers the first hit in x-then-z order,
                    // not the nearest (trail ruins seed 12345: [144, ~, 1392] over a closer one).
                    return Some((d, st.def, bx, bz));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gen() -> Option<Generator> {
        let d = crate::default_data_dir();
        if !d.join("data/minecraft/worldgen/structure_set").exists() {
            return None;
        }
        Some(Generator::with_data_dir(12345, d))
    }

    #[test]
    fn rotation_transforms_agree() {
        for rot in Rotation::ALL {
            for p in [[0, 0, 0], [3, 1, 7], [-2, 5, 4]] {
                assert_eq!(rot.apply(p), place::transform_pos(p, rot, false, [0; 3]));
            }
        }
        // rotating an east-facing stair a quarter turn clockwise faces it south
        let st = basalt_data::BlockState::parse("oak_stairs[facing=east]")
            .unwrap()
            .0 as u16;
        let out = basalt_data::BlockState(place::transform_state(st, Rotation::Cw90, false) as u32);
        assert_eq!(out.get("facing"), Some("south"));
    }

    #[test]
    fn spread_potential_is_in_region() {
        let sp = Spread {
            spacing: 34,
            separation: 8,
            salt: 10387312,
            triangular: false,
            frequency: 1.0,
            reduction: Reduction::Default,
            exclusion: None,
            locate_offset: [0; 3],
        };
        for (x, z) in [(0, 0), (-1, -1), (100, -77)] {
            let (px, pz) = spread_potential(12345, &sp, x, z);
            assert_eq!(px.div_euclid(34), x.div_euclid(34));
            assert_eq!(pz.div_euclid(34), z.div_euclid(34));
            assert!(px.rem_euclid(34) < 26 && pz.rem_euclid(34) < 26);
        }
    }

    /// Positions measured with vanilla's `/locate structure` from 0 0 0 (seed 12345, 26.2).
    #[test]
    fn locate_matches_vanilla() {
        let Some(g) = gen() else { return };
        if g.structures()
            .map(|s| !s.templates.available())
            .unwrap_or(true)
        {
            return;
        }
        assert_eq!(g.locate("village_plains", 0, 0, 100), Some((160, 880)));
        assert_eq!(g.locate("pillager_outpost", 0, 0, 100), Some((272, -960)));
        assert_eq!(g.locate("mineshaft", 0, 0, 100), Some((-64, 16)));
        assert_eq!(g.locate("buried_treasure", 0, 0, 100), Some((761, 409)));
    }
}

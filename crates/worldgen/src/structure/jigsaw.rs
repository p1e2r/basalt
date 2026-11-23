//! Jigsaw structures: template pools and the piece assembly (breadth-first, by placement
//! priority), checked piece by piece against the `Children` lists of vanilla structure starts.

use super::template::{BBox, Dir, Jigsaw, Rotation};
use super::{Junction, Piece, PieceKind, Structures, Stub};
use crate::random::{JavaRandom, Rng};
use crate::Generator;
use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub enum Element {
    Single {
        location: String,
        processors: Value,
        rigid: bool,
        legacy: bool,
    },
    List {
        elements: Vec<Arc<Element>>,
        rigid: bool,
    },
    Feature {
        feature: String,
        rigid: bool,
    },
    Empty,
}

impl Element {
    fn parse(v: &Value) -> Element {
        let rigid = v["projection"].as_str() != Some("terrain_matching");
        match v["element_type"]
            .as_str()
            .unwrap_or("")
            .trim_start_matches("minecraft:")
        {
            t @ ("single_pool_element" | "legacy_single_pool_element") => Element::Single {
                location: v["location"].as_str().unwrap_or("").to_string(),
                processors: v["processors"].clone(),
                rigid,
                legacy: t.starts_with("legacy"),
            },
            "list_pool_element" => Element::List {
                elements: v["elements"]
                    .as_array()
                    .map(|a| a.iter().map(|e| Arc::new(Element::parse(e))).collect())
                    .unwrap_or_default(),
                rigid,
            },
            "feature_pool_element" => Element::Feature {
                feature: v["feature"].as_str().unwrap_or("").to_string(),
                rigid,
            },
            _ => Element::Empty,
        }
    }

    pub fn rigid(&self) -> bool {
        match self {
            Element::Single { rigid, .. }
            | Element::List { rigid, .. }
            | Element::Feature { rigid, .. } => *rigid,
            Element::Empty => true,
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Element::Empty)
    }

    pub fn name(&self) -> String {
        match self {
            Element::Single { location, .. } => location.clone(),
            Element::List { elements, .. } => format!(
                "list[{}]",
                elements
                    .iter()
                    .map(|e| e.name())
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Element::Feature { feature, .. } => format!("feature {feature}"),
            Element::Empty => "empty".into(),
        }
    }

    pub fn bbox(&self, s: &Structures, pos: [i32; 3], rot: Rotation) -> Option<BBox> {
        match self {
            Element::Single { location, .. } => Some(s.templates.get(location)?.bbox(pos, rot)),
            Element::List { elements, .. } => {
                let mut b: Option<BBox> = None;
                for e in elements {
                    if let Some(x) = e.bbox(s, pos, rot) {
                        b = Some(b.map(|b| b.union(&x)).unwrap_or(x));
                    }
                }
                b
            }
            Element::Feature { .. } => Some(BBox::new(pos, pos)),
            Element::Empty => None,
        }
    }

    /// Jigsaw blocks in world coordinates for the element placed at `pos` with `rot`, in
    /// template order (before shuffling).
    pub fn jigsaws(&self, s: &Structures, pos: [i32; 3], rot: Rotation) -> Vec<Jigsaw> {
        match self {
            Element::Single { location, .. } => {
                let Some(t) = s.templates.get(location) else {
                    return Vec::new();
                };
                t.jigsaws
                    .iter()
                    .map(|j| {
                        let p = rot.apply(j.pos);
                        let mut j = j.clone();
                        j.pos = [p[0] + pos[0], p[1] + pos[1], p[2] + pos[2]];
                        j.front = j.front.rotate(rot);
                        j.top = j.top.rotate(rot);
                        j
                    })
                    .collect()
            }
            Element::List { elements, .. } => elements
                .first()
                .map(|e| e.jigsaws(s, pos, rot))
                .unwrap_or_default(),
            Element::Feature { .. } => vec![Jigsaw {
                pos,
                front: Dir::Down,
                top: Dir::South,
                name: "minecraft:bottom".into(),
                target: "minecraft:empty".into(),
                pool: "minecraft:empty".into(),
                final_state: "minecraft:air".into(),
                rollable: true,
                selection_priority: 0,
                placement_priority: 0,
            }],
            Element::Empty => Vec::new(),
        }
    }

    fn shuffled_jigsaws<R: Rng>(
        &self,
        s: &Structures,
        pos: [i32; 3],
        rot: Rotation,
        r: &mut R,
    ) -> Vec<Jigsaw> {
        let mut v = self.jigsaws(s, pos, rot);
        if matches!(self, Element::Feature { .. }) {
            return v;
        }
        shuffle(&mut v, r);
        // Stable sort, higher selection priority first.
        v.sort_by_key(|j| -j.selection_priority);
        v
    }
}

/// Fisher-Yates shuffle consuming the generator like vanilla's list shuffle.
pub fn shuffle<T, R: Rng>(v: &mut [T], r: &mut R) {
    let mut i = v.len();
    while i > 1 {
        let j = r.next_int_bounded(i as i32) as usize;
        v.swap(i - 1, j);
        i -= 1;
    }
}

pub struct Pool {
    /// Elements repeated by weight.
    pub list: Vec<Arc<Element>>,
    pub fallback: String,
    max_size: Mutex<Option<i32>>,
}

pub struct Pools {
    root: PathBuf,
    map: Mutex<HashMap<String, Option<Arc<Pool>>>>,
}

impl Pools {
    pub fn new(wg: &Path) -> Pools {
        Pools {
            root: wg.join("template_pool"),
            map: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, name: &str) -> Option<Arc<Pool>> {
        let key = name.trim_start_matches("minecraft:").to_string();
        if let Some(p) = self.map.lock().unwrap().get(&key) {
            return p.clone();
        }
        let p = std::fs::read_to_string(self.root.join(format!("{key}.json")))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .map(|v| {
                let mut list = Vec::new();
                for e in v["elements"].as_array().cloned().unwrap_or_default() {
                    let el = Arc::new(Element::parse(&e["element"]));
                    for _ in 0..e["weight"].as_i64().unwrap_or(1) {
                        list.push(el.clone());
                    }
                }
                Arc::new(Pool {
                    list,
                    fallback: v["fallback"]
                        .as_str()
                        .unwrap_or("minecraft:empty")
                        .to_string(),
                    max_size: Mutex::new(None),
                })
            });
        self.map.lock().unwrap().insert(key, p.clone());
        p
    }
}

impl Pool {
    fn max_size(&self, s: &Structures) -> i32 {
        if let Some(m) = *self.max_size.lock().unwrap() {
            return m;
        }
        let m = self
            .list
            .iter()
            .filter_map(|e| e.bbox(s, [0; 3], Rotation::None))
            .map(|b| b.span(1))
            .max()
            .unwrap_or(0);
        *self.max_size.lock().unwrap() = Some(m);
        m
    }
}

#[derive(Debug)]
pub enum StartHeight {
    Absolute(i32),
    Uniform(i32, i32),
}

#[derive(Debug)]
pub struct Alias {
    pub kind: AliasKind,
}

#[derive(Debug)]
pub enum AliasKind {
    Direct(String, String),
    /// alias → weighted targets
    Random(String, Vec<(String, i32)>),
    /// weighted groups of aliases
    Group(Vec<(Vec<Alias>, i32)>),
}

#[derive(Debug)]
pub struct JigsawDef {
    pub start_pool: String,
    pub start_jigsaw: Option<String>,
    pub size: i32,
    pub start_height: StartHeight,
    /// Heightmap the start is projected to (ocean floor?), if any.
    pub project: Option<bool>,
    pub max_dist: i32,
    pub expansion_hack: bool,
    pub padding: (i32, i32),
    pub aliases: Vec<Alias>,
    pub ignore_waterlogging: bool,
}

fn anchor(v: &Value) -> i32 {
    if let Some(a) = v.get("absolute") {
        return a.as_i64().unwrap_or(0) as i32;
    }
    if let Some(a) = v.get("above_bottom") {
        return -64 + a.as_i64().unwrap_or(0) as i32;
    }
    if let Some(a) = v.get("below_top") {
        return 319 - a.as_i64().unwrap_or(0) as i32;
    }
    0
}

fn parse_alias(v: &Value) -> Option<Alias> {
    let ty = v["type"].as_str()?.trim_start_matches("minecraft:");
    Some(Alias {
        kind: match ty {
            "direct" => AliasKind::Direct(
                v["alias"].as_str()?.to_string(),
                v["target"].as_str()?.to_string(),
            ),
            "random" => AliasKind::Random(
                v["alias"].as_str()?.to_string(),
                v["targets"]
                    .as_array()?
                    .iter()
                    .filter_map(|t| {
                        Some((
                            t["data"].as_str()?.to_string(),
                            t["weight"].as_i64()? as i32,
                        ))
                    })
                    .collect(),
            ),
            "random_group" => AliasKind::Group(
                v["groups"]
                    .as_array()?
                    .iter()
                    .filter_map(|g| {
                        Some((
                            g["data"]
                                .as_array()?
                                .iter()
                                .filter_map(parse_alias)
                                .collect(),
                            g["weight"].as_i64()? as i32,
                        ))
                    })
                    .collect(),
            ),
            _ => return None,
        },
    })
}

impl JigsawDef {
    pub fn parse(v: &Value, _pools: &mut Pools) -> Result<JigsawDef> {
        let sh = &v["start_height"];
        let start_height = match sh["type"]
            .as_str()
            .map(|t| t.trim_start_matches("minecraft:"))
        {
            Some("uniform") => {
                StartHeight::Uniform(anchor(&sh["min_inclusive"]), anchor(&sh["max_inclusive"]))
            }
            Some("constant") => StartHeight::Absolute(anchor(&sh["value"])),
            _ => StartHeight::Absolute(anchor(sh)),
        };
        let padding = match &v["dimension_padding"] {
            Value::Number(n) => (
                n.as_i64().unwrap_or(0) as i32,
                n.as_i64().unwrap_or(0) as i32,
            ),
            Value::Object(o) => (
                o.get("bottom").and_then(|x| x.as_i64()).unwrap_or(0) as i32,
                o.get("top").and_then(|x| x.as_i64()).unwrap_or(0) as i32,
            ),
            _ => (0, 0),
        };
        Ok(JigsawDef {
            start_pool: v["start_pool"].as_str().unwrap_or("").to_string(),
            start_jigsaw: v["start_jigsaw_name"].as_str().map(|s| s.to_string()),
            size: v["size"].as_i64().unwrap_or(1) as i32,
            start_height,
            project: v["project_start_to_heightmap"]
                .as_str()
                .map(|h| h.starts_with("OCEAN_FLOOR")),
            max_dist: v["max_distance_from_center"].as_i64().unwrap_or(80) as i32,
            expansion_hack: v["use_expansion_hack"].as_bool().unwrap_or(false),
            padding,
            aliases: v["pool_aliases"]
                .as_array()
                .map(|a| a.iter().filter_map(parse_alias).collect())
                .unwrap_or_default(),
            ignore_waterlogging: v["liquid_settings"].as_str() == Some("ignore_waterlogging"),
        })
    }
}

/// Resolve the pool aliases of a start (seeded by world seed and start position).
fn resolve_aliases(def: &JigsawDef, seed: i64, pos: [i32; 3]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if def.aliases.is_empty() {
        return out;
    }
    // Measured: a legacy (java.util.Random) positional generator of the world seed at the start origin.
    let mut r = crate::random::Positional::world(seed, true).at(pos[0], pos[1], pos[2]);
    fn pick<T, R: Rng>(v: &[(T, i32)], r: &mut R) -> Option<usize> {
        let total: i32 = v.iter().map(|x| x.1).sum();
        if total <= 0 {
            return None;
        }
        let mut k = r.next_int_bounded(total);
        for (i, x) in v.iter().enumerate() {
            k -= x.1;
            if k < 0 {
                return Some(i);
            }
        }
        None
    }
    fn walk<R: Rng>(a: &Alias, r: &mut R, out: &mut HashMap<String, String>) {
        match &a.kind {
            AliasKind::Direct(k, t) => {
                out.insert(k.clone(), t.clone());
            }
            AliasKind::Random(k, ts) => {
                if let Some(i) = pick(ts, r) {
                    out.insert(k.clone(), ts[i].0.clone());
                }
            }
            AliasKind::Group(gs) => {
                if let Some(i) = pick(gs, r) {
                    for a in &gs[i].0 {
                        walk(a, r, out);
                    }
                }
            }
        }
    }
    for a in &def.aliases {
        walk(a, &mut r, &mut out);
    }
    out
}

/// Free space: a container box minus the boxes placed in it.
struct Space {
    outer: BBox,
    taken: Vec<BBox>,
}

impl Space {
    fn fits(&self, b: &BBox) -> bool {
        b.inside(&self.outer) && !self.taken.iter().any(|t| t.intersects(b))
    }
}

struct Placing {
    piece: usize,
    space: usize,
    depth: i32,
}

/// Generate a jigsaw structure start for chunk (cx, cz). With `full` false only the start
/// piece is computed (enough for the biome check and /locate).
pub fn generate(
    g: &Generator,
    s: &Structures,
    def: &JigsawDef,
    cx: i32,
    cz: i32,
    r: &mut JavaRandom,
    full: bool,
) -> Option<Stub> {
    let start_y = match def.start_height {
        StartHeight::Absolute(y) => y,
        StartHeight::Uniform(a, b) => {
            if a > b {
                a
            } else {
                r.next_int_bounded(b - a + 1) + a
            }
        }
    };
    let origin = [cx * 16, start_y, cz * 16];
    let aliases = resolve_aliases(def, g.seed, origin);
    let alias = |p: &str| -> String { aliases.get(p).cloned().unwrap_or_else(|| p.to_string()) };
    let rot = Rotation::ALL[r.next_int_bounded(4) as usize];
    let pool = s.pools.get(&alias(&def.start_pool))?;
    if pool.list.is_empty() {
        return None;
    }
    let elem = pool.list[r.next_int_bounded(pool.list.len() as i32) as usize].clone();
    if elem.is_empty() {
        return None;
    }
    let mut pos = origin;
    if let Some(name) = &def.start_jigsaw {
        // The anchor is looked up in the shuffled jigsaw list (consumes the generator).
        let js = elem.shuffled_jigsaws(s, [0; 3], rot, r);
        let j = js.iter().find(|j| &j.name == name)?;
        pos = [pos[0] - j.pos[0], pos[1] - j.pos[1], pos[2] - j.pos[2]];
    }
    let bb = elem.bbox(s, pos, rot)?;
    let kx = (bb.max[0] + bb.min[0]) / 2;
    let kz = (bb.max[2] + bb.min[2]) / 2;
    let ground_delta = 1;
    // The start piece's floor (min y + ground delta) goes to the start y, lifted to the noise
    // surface at the box centre when projected (measured: ancient city min y = -27 - anchor y - 1).
    let surface = match def.project {
        Some(ocean) => g.first_free(kx, kz, ocean),
        None => 0,
    };
    let y = pos[1] + surface;
    let dy = y - (bb.min[1] + ground_delta);
    let (bb, pos) = (bb.moved([0, dy, 0]), [pos[0], pos[1] + dy, pos[2]]);
    let start = Piece {
        bb,
        kind: PieceKind::Pool {
            element: elem.clone(),
            pos,
            rot,
            ground_delta,
            junctions: Vec::new(),
        },
    };
    let stub_pos = [kx, start_y + surface, kz];
    if !full {
        return Some(Stub {
            pos: stub_pos,
            pieces: vec![start],
        });
    }
    let mut a = Assembler {
        g,
        s,
        r,
        pieces: vec![start],
        spaces: Vec::new(),
        queue: Vec::new(),
        seq: 0,
        max_depth: def.size,
        expansion: def.expansion_hack,
        alias: &alias,
        surface: HashMap::new(),
    };
    if def.size > 0 {
        let d = def.max_dist;
        let lo = (y - d).max(-64 + def.padding.0);
        let hi = (y + d).min(320 - 1 - def.padding.1);
        let outer = BBox {
            min: [kx - d, lo, kz - d],
            max: [kx + d, hi, kz + d],
        };
        a.spaces.push(Space {
            outer,
            taken: vec![bb],
        });
        a.children(0, 0, 0);
        while let Some(p) = a.pop() {
            a.children(p.piece, p.space, p.depth);
        }
    }
    let pieces = a.pieces;
    Some(Stub {
        pos: stub_pos,
        pieces,
    })
}

struct Assembler<'a, 'r> {
    g: &'a Generator,
    s: &'a Structures,
    r: &'r mut JavaRandom,
    pieces: Vec<Piece>,
    spaces: Vec<Space>,
    /// (priority, seq, item)
    queue: Vec<(i32, u64, Placing)>,
    seq: u64,
    max_depth: i32,
    expansion: bool,
    alias: &'a dyn Fn(&str) -> String,
    surface: HashMap<(i32, i32), i32>,
}

fn can_attach(a: &Jigsaw, b: &Jigsaw) -> bool {
    a.front == b.front.opposite() && (a.rollable || a.top == b.top) && a.target == b.name
}

impl Assembler<'_, '_> {
    fn pop(&mut self) -> Option<Placing> {
        if self.queue.is_empty() {
            return None;
        }
        let mut best = 0;
        for (i, q) in self.queue.iter().enumerate() {
            let b = &self.queue[best];
            if q.0 > b.0 || (q.0 == b.0 && q.1 < b.1) {
                best = i;
            }
        }
        Some(self.queue.remove(best).2)
    }

    fn surface(&mut self, x: i32, z: i32) -> i32 {
        if let Some(v) = self.surface.get(&(x, z)) {
            return *v;
        }
        let v = self.g.first_free(x, z, false);
        self.surface.insert((x, z), v);
        v
    }

    fn children(&mut self, pi: usize, outer_space: usize, depth: i32) {
        let (elem, pos, rot, bb, ground_delta) = match &self.pieces[pi].kind {
            PieceKind::Pool {
                element,
                pos,
                rot,
                ground_delta,
                ..
            } => (
                element.clone(),
                *pos,
                *rot,
                self.pieces[pi].bb,
                *ground_delta,
            ),
            _ => return,
        };
        let rigid = elem.rigid();
        let min_y = bb.min[1];
        let mut inner: Option<usize> = None;
        let jigsaws = elem.shuffled_jigsaws(self.s, pos, rot, self.r);
        'jig: for j in jigsaws {
            let step = j.front.step();
            let target = [j.pos[0] + step[0], j.pos[1] + step[1], j.pos[2] + step[2]];
            let jy = j.pos[1] - min_y;
            let mut surf: Option<i32> = None;
            let pool_name = (self.alias)(&j.pool);
            let Some(pool) = self.s.pools.get(&pool_name) else {
                continue;
            };
            if pool.list.is_empty() && pool_name.trim_start_matches("minecraft:") != "empty" {
                continue;
            }
            let Some(fallback) = self.s.pools.get(&pool.fallback) else {
                continue;
            };
            let space = if bb.contains(target) {
                match inner {
                    Some(i) => i,
                    None => {
                        self.spaces.push(Space {
                            outer: bb,
                            taken: Vec::new(),
                        });
                        inner = Some(self.spaces.len() - 1);
                        self.spaces.len() - 1
                    }
                }
            } else {
                outer_space
            };
            let mut cands: Vec<Arc<Element>> = Vec::new();
            if depth != self.max_depth {
                let mut l = pool.list.clone();
                shuffle(&mut l, self.r);
                cands.extend(l);
            }
            let mut l = fallback.list.clone();
            shuffle(&mut l, self.r);
            cands.extend(l);
            for cand in cands {
                if cand.is_empty() {
                    break;
                }
                let mut rots = Rotation::ALL;
                shuffle(&mut rots, self.r);
                for crot in rots {
                    let cjs = cand.shuffled_jigsaws(self.s, [0; 3], crot, self.r);
                    let Some(cbox0) = cand.bbox(self.s, [0; 3], crot) else {
                        continue;
                    };
                    let exp = if self.expansion && cbox0.span(1) <= 16 {
                        cjs.iter()
                            .map(|cj| {
                                let st = cj.front.step();
                                if !cbox0.contains([
                                    cj.pos[0] + st[0],
                                    cj.pos[1] + st[1],
                                    cj.pos[2] + st[2],
                                ]) {
                                    return 0;
                                }
                                let pn = (self.alias)(&cj.pool);
                                let p = self.s.pools.get(&pn);
                                let f = p.as_ref().and_then(|p| self.s.pools.get(&p.fallback));
                                p.map(|p| p.max_size(self.s))
                                    .unwrap_or(0)
                                    .max(f.map(|f| f.max_size(self.s)).unwrap_or(0))
                            })
                            .max()
                            .unwrap_or(0)
                    } else {
                        0
                    };
                    for cj in &cjs {
                        if !can_attach(&j, cj) {
                            continue;
                        }
                        let off = [
                            target[0] - cj.pos[0],
                            target[1] - cj.pos[1],
                            target[2] - cj.pos[2],
                        ];
                        let Some(cbox) = cand.bbox(self.s, off, crot) else {
                            continue;
                        };
                        let crigid = cand.rigid();
                        let cjy = cj.pos[1];
                        let t = jy - cjy + j.front.step()[1];
                        let place_y = if rigid && crigid {
                            min_y + t
                        } else {
                            let sv = match surf {
                                Some(v) => v,
                                None => {
                                    let v = self.surface(j.pos[0], j.pos[2]);
                                    surf = Some(v);
                                    v
                                }
                            };
                            sv - cjy
                        };
                        let dy = place_y - cbox.min[1];
                        let mut moved = cbox.moved([0, dy, 0]);
                        let cpos = [off[0], off[1] + dy, off[2]];
                        if exp > 0 {
                            let top = (exp + 1).max(moved.max[1] - moved.min[1]);
                            moved.encapsulate([moved.min[0], moved.min[1] + top, moved.min[2]]);
                        }
                        if !self.spaces[space].fits(&moved) {
                            continue;
                        }
                        self.spaces[space].taken.push(moved);
                        let cground = if crigid { ground_delta - t } else { 1 };
                        let junction_y = if rigid {
                            min_y + jy
                        } else if crigid {
                            place_y + cjy
                        } else {
                            surf.unwrap_or_else(|| self.surface(j.pos[0], j.pos[2])) + t / 2
                        };
                        if let PieceKind::Pool { junctions, .. } = &mut self.pieces[pi].kind {
                            junctions.push(Junction {
                                x: target[0],
                                y: junction_y - jy + ground_delta,
                                z: target[2],
                                dy: t,
                                rigid: crigid,
                            });
                        }
                        let np = Piece {
                            bb: moved,
                            kind: PieceKind::Pool {
                                element: cand.clone(),
                                pos: cpos,
                                rot: crot,
                                ground_delta: cground,
                                junctions: vec![Junction {
                                    x: j.pos[0],
                                    y: junction_y - cjy + cground,
                                    z: j.pos[2],
                                    dy: -t,
                                    rigid,
                                }],
                            },
                        };
                        self.pieces.push(np);
                        if depth < self.max_depth {
                            self.seq += 1;
                            self.queue.push((
                                j.placement_priority,
                                self.seq,
                                Placing {
                                    piece: self.pieces.len() - 1,
                                    space,
                                    depth: depth + 1,
                                },
                            ));
                        }
                        continue 'jig;
                    }
                }
            }
        }
    }
}

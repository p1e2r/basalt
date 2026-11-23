//! Placing structure pieces into chunks (templates + processors), and the generation points of
//! the template-based single structures.

use super::template::{BBox, Rotation, Template};
use super::{Def, Piece, PieceKind, PortalSetup, Start, Structures, Stub, TemplateOpts};
use crate::features::{block_set, Tags, R};
use crate::random::{pos_seed, JavaRandom, Rng};
use crate::region::{flag, Hm, Region, F_WATER};
use crate::Generator;
use basalt_data::BlockState;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

pub enum InPred {
    True,
    Block(u32),
    RandomBlock(u32, f32),
    State(u16),
    Tag(Arc<Vec<bool>>),
}

impl InPred {
    fn parse(tags: &Tags, v: &Value) -> InPred {
        let block = || {
            BlockState::from_name(
                v["block"]
                    .as_str()
                    .unwrap_or("")
                    .trim_start_matches("minecraft:"),
            )
            .map(|s| s.block_index() as u32)
            .unwrap_or(u32::MAX)
        };
        match v["predicate_type"]
            .as_str()
            .unwrap_or("")
            .trim_start_matches("minecraft:")
        {
            "block_match" => InPred::Block(block()),
            "random_block_match" => {
                InPred::RandomBlock(block(), v["probability"].as_f64().unwrap_or(1.0) as f32)
            }
            "blockstate_match" => {
                InPred::State(crate::state_from_json(&v["block_state"]).unwrap_or(u16::MAX))
            }
            "tag_match" => InPred::Tag(tags.get(v["tag"].as_str().unwrap_or(""))),
            "random_blockstate_match" => InPred::RandomBlock(u32::MAX, 0.0),
            _ => InPred::True,
        }
    }
    fn test(&self, s: u16, r: &mut JavaRandom) -> bool {
        match self {
            InPred::True => true,
            InPred::Block(b) => BlockState(s as u32).block_index() as u32 == *b,
            InPred::RandomBlock(b, p) => {
                BlockState(s as u32).block_index() as u32 == *b && r.next_float() < *p
            }
            InPred::State(x) => s == *x,
            InPred::Tag(t) => t[s as usize],
        }
    }
}

pub struct Rule {
    input: InPred,
    location: InPred,
    /// axis_aligned_linear_pos: (axis, min chance, max chance, min dist, max dist)
    position: Option<(usize, f32, f32, i32, i32)>,
    output: u16,
}

pub enum Proc {
    Rule(Vec<Rule>),
    BlockRot {
        integrity: f32,
        rottable: Option<Arc<Vec<bool>>>,
    },
    Protected(Arc<Vec<bool>>),
    /// Applied to a limited random subset of the template's blocks (not modelled: skipped).
    Capped,
    IgnoreAir,
    Gravity,
}

/// Processor lists (`worldgen/processor_list`), parsed lazily.
pub struct Processors {
    root: std::path::PathBuf,
    map: Mutex<HashMap<String, Arc<Vec<Proc>>>>,
    pub tags: Tags,
}

impl Processors {
    pub fn new(wg: &Path) -> Processors {
        Processors {
            root: wg.join("processor_list"),
            map: Mutex::new(HashMap::new()),
            tags: Tags::new(&wg.join("..")),
        }
    }

    /// Processors of a pool element / piece (a list name or an inline `{"processors": [..]}`).
    pub fn get(&self, v: &Value) -> Arc<Vec<Proc>> {
        if let Some(name) = v.as_str() {
            let key = name.trim_start_matches("minecraft:").to_string();
            if let Some(p) = self.map.lock().unwrap().get(&key) {
                return p.clone();
            }
            let pv = std::fs::read_to_string(self.root.join(format!("{key}.json")))
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .unwrap_or(Value::Null);
            let p = Arc::new(self.parse_list(&pv));
            self.map.lock().unwrap().insert(key, p.clone());
            return p;
        }
        Arc::new(self.parse_list(v))
    }

    fn parse_list(&self, v: &Value) -> Vec<Proc> {
        let list = v
            .get("processors")
            .and_then(|p| p.as_array())
            .cloned()
            .or_else(|| v.as_array().cloned())
            .unwrap_or_default();
        list.iter().filter_map(|p| self.parse(p)).collect()
    }

    fn parse(&self, p: &Value) -> Option<Proc> {
        Some(
            match p["processor_type"]
                .as_str()?
                .trim_start_matches("minecraft:")
            {
                "rule" => Proc::Rule(
                    p["rules"]
                        .as_array()?
                        .iter()
                        .map(|r| Rule {
                            input: InPred::parse(&self.tags, &r["input_predicate"]),
                            location: InPred::parse(&self.tags, &r["location_predicate"]),
                            position: r
                                .get("position_predicate")
                                .filter(|pp| {
                                    pp["predicate_type"]
                                        .as_str()
                                        .map(|t| t.ends_with("axis_aligned_linear_pos"))
                                        .unwrap_or(false)
                                })
                                .map(|pp| {
                                    (
                                        match pp["axis"].as_str().unwrap_or("y") {
                                            "x" => 0,
                                            "z" => 2,
                                            _ => 1,
                                        },
                                        pp["min_chance"].as_f64().unwrap_or(0.0) as f32,
                                        pp["max_chance"].as_f64().unwrap_or(0.0) as f32,
                                        pp["min_dist"].as_i64().unwrap_or(0) as i32,
                                        pp["max_dist"].as_i64().unwrap_or(0) as i32,
                                    )
                                }),
                            output: crate::state_from_json(&r["output_state"]).unwrap_or(0),
                        })
                        .collect(),
                ),
                "block_rot" => Proc::BlockRot {
                    integrity: p["integrity"].as_f64().unwrap_or(1.0) as f32,
                    rottable: p.get("rottable_blocks").map(|b| block_set(&self.tags, b)),
                },
                "protected_blocks" => {
                    Proc::Protected(self.tags.get(p["value"].as_str().unwrap_or("")))
                }
                "capped" => Proc::Capped,
                "block_ignore" => Proc::IgnoreAir,
                _ => return None,
            },
        )
    }
}

fn rot_dir_name(d: &str, rot: Rotation, mirror: bool) -> Option<&'static str> {
    let dir = super::template::Dir::parse(d)?;
    let dir = if mirror { dir.mirror_lr() } else { dir };
    Some(dir.rotate(rot).name())
}

/// Rotate (and mirror front-back: x → -x) a block state.
pub fn transform_state(s: u16, rot: Rotation, mirror: bool) -> u16 {
    if rot == Rotation::None && !mirror {
        return s;
    }
    let st = BlockState(s as u32);
    let props = st.props();
    if props.is_empty() {
        return s;
    }
    let mut out = st;
    let quarter = rot as u8 & 1 == 1;
    let mut dirs: Vec<(&str, &str)> = Vec::new();
    for (k, v) in &props {
        let nv: Option<String> = match *k {
            "facing" | "horizontal_facing" => rot_dir_name(v, rot, mirror).map(|x| x.to_string()),
            "axis" if quarter => Some(
                match *v {
                    "x" => "z",
                    "z" => "x",
                    o => o,
                }
                .to_string(),
            ),
            "rotation" => v.parse::<i32>().ok().map(|r| {
                let r = if mirror { (16 - r) & 15 } else { r };
                ((r + rot as i32 * 4) & 15).to_string()
            }),
            "orientation" => v.split_once('_').and_then(|(a, b)| {
                Some(format!(
                    "{}_{}",
                    rot_dir_name(a, rot, mirror)?,
                    rot_dir_name(b, rot, mirror)?
                ))
            }),
            "shape"
                if v.contains("north")
                    || v.contains("south")
                    || v.contains("east")
                    || v.contains("west") =>
            {
                // rails: rotate each direction token, keep vanilla's canonical names
                let parts: Vec<&str> = v.split('_').collect();
                let mapped: Vec<String> = parts
                    .iter()
                    .map(|p| {
                        rot_dir_name(p, rot, mirror)
                            .map(|x| x.to_string())
                            .unwrap_or(p.to_string())
                    })
                    .collect();
                let cand = mapped.join("_");
                let alt = if mapped.len() == 2 && mapped[0] != "ascending" {
                    format!("{}_{}", mapped[1], mapped[0])
                } else {
                    cand.clone()
                };
                if st.with(k, &cand).is_some() {
                    Some(cand)
                } else {
                    Some(alt)
                }
            }
            "shape" if mirror => Some(
                match *v {
                    "inner_left" => "inner_right",
                    "inner_right" => "inner_left",
                    "outer_left" => "outer_right",
                    "outer_right" => "outer_left",
                    o => o,
                }
                .to_string(),
            ),
            "type" if mirror && (*v == "left" || *v == "right") => {
                Some(if *v == "left" { "right" } else { "left" }.to_string())
            }
            "hinge" if mirror => Some(if *v == "left" { "right" } else { "left" }.to_string()),
            "north" | "east" | "south" | "west" => {
                dirs.push((k, v));
                None
            }
            _ => None,
        };
        if let Some(nv) = nv {
            if let Some(n) = out.with(k, &nv) {
                out = n;
            }
        }
    }
    for (k, v) in dirs {
        if let Some(nk) = rot_dir_name(k, rot, mirror) {
            if let Some(n) = out.with(nk, v) {
                out = n;
            }
        }
    }
    out.0 as u16
}

/// Transform a template-local position: mirror (front-back: x → -x) then rotate, around `pivot`.
pub fn transform_pos(p: [i32; 3], rot: Rotation, mirror: bool, pivot: [i32; 3]) -> [i32; 3] {
    let (mut x, y, mut z) = (p[0], p[1], p[2]);
    if mirror {
        x = -x;
    }
    let (px, pz) = (pivot[0], pivot[2]);
    match rot {
        Rotation::None => {}
        Rotation::Ccw90 => {
            let (nx, nz) = (px - pz + z, px + pz - x);
            x = nx;
            z = nz;
        }
        Rotation::Cw90 => {
            let (nx, nz) = (px + pz - z, pz - px + x);
            x = nx;
            z = nz;
        }
        Rotation::Cw180 => {
            x = px + px - x;
            z = pz + pz - z;
        }
    }
    [x, y, z]
}

pub fn template_bbox(
    t: &Template,
    pos: [i32; 3],
    rot: Rotation,
    mirror: bool,
    pivot: [i32; 3],
) -> BBox {
    let a = transform_pos([0, 0, 0], rot, mirror, pivot);
    let b = transform_pos(
        [t.size[0] - 1, t.size[1] - 1, t.size[2] - 1],
        rot,
        mirror,
        pivot,
    );
    BBox::new(a, b).moved(pos)
}

pub struct PlaceArgs<'a> {
    pub pos: [i32; 3],
    pub rot: Rotation,
    pub mirror: bool,
    pub pivot: [i32; 3],
    pub clip: BBox,
    pub procs: &'a [Proc],
    pub extra: &'a [Proc],
    pub ignore_air: bool,
    pub gravity: bool,
    pub waterlog: bool,
    pub palette: usize,
}

fn axis_chance(p: (usize, f32, f32, i32, i32), local: [i32; 3], r: &mut JavaRandom) -> bool {
    let (axis, lo, hi, dmin, dmax) = p;
    let d = local[axis].abs();
    let f = r.next_float();
    let t = if dmax <= dmin {
        1.0
    } else {
        ((d - dmin) as f32 / (dmax - dmin) as f32).clamp(0.0, 1.0)
    };
    f <= lo + (hi - lo) * t
}

/// Place a template into the region (only blocks inside `clip` and the region's write window).
pub fn place_template(s: &Structures, reg: &mut Region, t: &Template, a: &PlaceArgs) {
    let jigsaw = BlockState::from_name("jigsaw")
        .map(|b| b.block_index())
        .unwrap_or(usize::MAX);
    let void = BlockState::from_name("structure_void")
        .map(|b| b.block_index())
        .unwrap_or(usize::MAX);
    let sblock = BlockState::from_name("structure_block")
        .map(|b| b.block_index())
        .unwrap_or(usize::MAX);
    let pal = &t.palettes[a.palette.min(t.palettes.len() - 1)];
    let jig_states: HashMap<[i32; 3], &super::template::Jigsaw> =
        t.jigsaws.iter().map(|j| (j.pos, j)).collect();
    for b in &t.blocks {
        let mut st = pal.get(b.state as usize).copied().unwrap_or(0);
        let bi = BlockState(st as u32).block_index();
        if bi == void || bi == sblock {
            continue;
        }
        if a.ignore_air && st == 0 {
            continue;
        }
        if bi == jigsaw {
            let Some(j) = jig_states.get(&b.pos) else {
                continue;
            };
            st = BlockState::parse(j.final_state.trim_start_matches("minecraft:"))
                .map(|x| x.0 as u16)
                .unwrap_or(0);
            if BlockState(st as u32).block_index() == void {
                continue;
            }
        }
        let rel = transform_pos(b.pos, a.rot, a.mirror, a.pivot);
        let mut w = [rel[0] + a.pos[0], rel[1] + a.pos[1], rel[2] + a.pos[2]];
        let mut ok = true;
        for p in a.procs.iter().chain(a.extra.iter()) {
            match p {
                Proc::Rule(rules) => {
                    let mut r = JavaRandom::new(pos_seed(w[0], w[1], w[2]));
                    let world = reg.block(w[0], w[1], w[2]);
                    for rule in rules {
                        if rule.input.test(st, &mut r)
                            && rule.location.test(world, &mut r)
                            && rule
                                .position
                                .map(|pp| axis_chance(pp, b.pos, &mut r))
                                .unwrap_or(true)
                        {
                            st = rule.output;
                            break;
                        }
                    }
                }
                Proc::BlockRot {
                    integrity,
                    rottable,
                } => {
                    let mut r = JavaRandom::new(pos_seed(w[0], w[1], w[2]));
                    let keep = match rottable {
                        Some(set) if !set[st as usize] => true,
                        _ => r.next_float() <= *integrity,
                    };
                    if !keep {
                        ok = false;
                        break;
                    }
                }
                Proc::Protected(set) => {
                    if set[reg.block(w[0], w[1], w[2]) as usize] {
                        ok = false;
                        break;
                    }
                }
                Proc::IgnoreAir => {
                    if st == 0 {
                        ok = false;
                        break;
                    }
                }
                Proc::Capped => {}
                Proc::Gravity => {}
            }
        }
        if !ok {
            continue;
        }
        if a.gravity {
            w[1] = reg.height(Hm::WorldSurfaceWg, w[0], w[2]) - 1 + b.pos[1];
        }
        if !a.clip.contains(w) {
            continue;
        }
        let mut out = transform_state(st, a.rot, a.mirror);
        if a.waterlog {
            let old = reg.block(w[0], w[1], w[2]);
            if flag(old, F_WATER)
                && BlockState(old as u32).name() == "water"
                && BlockState(old as u32).get("level") == Some("0")
            {
                if let Some(n) = BlockState(out as u32).with("waterlogged", "true") {
                    out = n.0 as u16;
                }
            }
        }
        reg.set(w[0], w[1], w[2], out);
    }
    let _ = s;
}

/// Place the pieces of a start that intersect chunk (cx, cz).
pub fn place_start(
    g: &Generator,
    s: &Structures,
    st: &Start,
    reg: &mut Region,
    cx: i32,
    cz: i32,
    r: &mut R,
) {
    let clip = BBox {
        min: [cx * 16, -64, cz * 16],
        max: [cx * 16 + 15, 319, cz * 16 + 15],
    };
    for p in &st.pieces {
        if !p.bb.intersects(&clip) {
            continue;
        }
        place_piece(g, s, p, reg, clip, r);
    }
}

fn place_piece(g: &Generator, s: &Structures, p: &Piece, reg: &mut Region, clip: BBox, r: &mut R) {
    match &p.kind {
        PieceKind::Pool {
            element, pos, rot, ..
        } => place_element(g, s, element, *pos, *rot, reg, clip, r),
        PieceKind::Template {
            name,
            pos,
            rot,
            mirror,
            pivot,
            opts,
        } => {
            let Some(t) = s.templates.get(name) else {
                return;
            };
            let mut pos = *pos;
            if opts.floor_y {
                pos[1] = reg.height(Hm::OceanFloorWg, pos[0], pos[2]);
            }
            let pos = &pos;
            let procs = opts
                .processors
                .as_ref()
                .map(|n| s.processors.get(&Value::String(n.clone())))
                .unwrap_or_default();
            let extra: Vec<Proc> = if opts.integrity < 1.0 {
                vec![Proc::BlockRot {
                    integrity: opts.integrity,
                    rottable: None,
                }]
            } else {
                Vec::new()
            };
            place_template(
                s,
                reg,
                &t,
                &PlaceArgs {
                    pos: *pos,
                    rot: *rot,
                    mirror: *mirror,
                    pivot: *pivot,
                    clip,
                    procs: &procs,
                    extra: &extra,
                    ignore_air: opts.ignore_air,
                    gravity: false,
                    waterlog: true,
                    palette: 0,
                },
            );
            for (p, st) in &opts.set {
                if clip.contains(*p) {
                    reg.set(p[0], p[1], p[2], *st);
                }
            }
        }
        PieceKind::SwampHut { origin, o } => place_swamp_hut(reg, clip, *origin, *o),
        PieceKind::Mineshaft { piece, mesa } => {
            super::mineshaft::place(reg, clip, piece, *mesa, &s.mineshaft_blocking, r)
        }
        PieceKind::Stronghold { piece } => super::stronghold::place(reg, clip, piece, r),
        PieceKind::Temple { jungle, origin, o } => {
            super::temple::place_temple(reg, clip, *jungle, *origin, *o, r)
        }
        PieceKind::Chest { pos, .. } => {
            let chest = BlockState::parse("chest[facing=north,type=single,waterlogged=false]")
                .map(|b| b.0 as u16)
                .unwrap_or(0);
            if clip.contains(*pos) {
                reg.set(pos[0], pos[1], pos[2], chest);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn place_element(
    g: &Generator,
    s: &Structures,
    e: &super::jigsaw::Element,
    pos: [i32; 3],
    rot: Rotation,
    reg: &mut Region,
    clip: BBox,
    r: &mut R,
) {
    use super::jigsaw::Element;
    match e {
        Element::Single {
            location,
            processors,
            rigid,
            legacy,
        } => {
            let Some(t) = s.templates.get(location) else {
                return;
            };
            let procs = s.processors.get(processors);
            place_template(
                s,
                reg,
                &t,
                &PlaceArgs {
                    pos,
                    rot,
                    mirror: false,
                    pivot: [0; 3],
                    clip,
                    procs: &procs,
                    extra: &[],
                    ignore_air: *legacy,
                    gravity: !*rigid,
                    waterlog: true,
                    palette: 0,
                },
            );
        }
        Element::List { elements, .. } => {
            for x in elements {
                place_element(g, s, x, pos, rot, reg, clip, r);
            }
        }
        Element::Feature { feature, .. } => {
            if !clip.contains(pos) {
                return;
            }
            if let Some(pid) = s.feature_ids.get(feature.trim_start_matches("minecraft:")) {
                g.features
                    .place_placed(reg, r, *pid, None, pos[0], pos[1], pos[2]);
            }
        }
        Element::Empty => {}
    }
}

// ---- single-template structures ----------------------------------------------------------

/// Shipwreck templates in vanilla's selection order (measured: replaying the start random
/// against 16 ocean / 2 beached vanilla shipwrecks; the beached list is inferred from the
/// ocean list without the upside-down / sideways-degraded variants and matches both samples).
const SHIPWRECK_OCEAN: [&str; 20] = [
    "with_mast",
    "upsidedown_full",
    "upsidedown_fronthalf",
    "upsidedown_backhalf",
    "sideways_full",
    "sideways_fronthalf",
    "sideways_backhalf",
    "rightsideup_full",
    "rightsideup_fronthalf",
    "rightsideup_backhalf",
    "with_mast_degraded",
    "upsidedown_full_degraded",
    "upsidedown_fronthalf_degraded",
    "upsidedown_backhalf_degraded",
    "sideways_full_degraded",
    "sideways_fronthalf_degraded",
    "sideways_backhalf_degraded",
    "rightsideup_full_degraded",
    "rightsideup_fronthalf_degraded",
    "rightsideup_backhalf_degraded",
];
const SHIPWRECK_BEACHED: [&str; 11] = [
    "with_mast",
    "sideways_full",
    "sideways_fronthalf",
    "sideways_backhalf",
    "rightsideup_full",
    "rightsideup_fronthalf",
    "rightsideup_backhalf",
    "with_mast_degraded",
    "rightsideup_full_degraded",
    "rightsideup_fronthalf_degraded",
    "rightsideup_backhalf_degraded",
];

pub fn shipwreck(
    g: &Generator,
    s: &Structures,
    beached: bool,
    cx: i32,
    cz: i32,
    r: &mut JavaRandom,
) -> Option<Stub> {
    let (mx, mz) = (cx * 16 + 8, cz * 16 + 8);
    let y = g.first_free(mx, mz, !beached) - 1;
    let rot = Rotation::ALL[r.next_int_bounded(4) as usize];
    let list: &[&str] = if beached {
        &SHIPWRECK_BEACHED
    } else {
        &SHIPWRECK_OCEAN
    };
    let name = format!(
        "minecraft:shipwreck/{}",
        list[r.next_int_bounded(list.len() as i32) as usize]
    );
    let pivot = [4, 0, 15];
    let pos = [cx * 16, 90, cz * 16];
    let pieces = match s.templates.get(&name) {
        Some(t) => {
            // Height: over the template's unrotated footprint from the chunk corner, the mean
            // (ocean floor) or the minimum minus half the height minus 0..2 (beached) of the
            // worldgen heightmap (measured against vanilla's adjusted pieces).
            let (mut sum, mut min) = (0i64, i32::MAX);
            for dx in 0..t.size[0] {
                for dz in 0..t.size[2] {
                    let h = g.first_free(pos[0] + dx, pos[2] + dz, !beached);
                    sum += h as i64;
                    min = min.min(h);
                }
            }
            let n = (t.size[0] * t.size[2]).max(1) as i64;
            let y = if beached {
                min - t.size[1] / 2 - r.next_int_bounded(3)
            } else {
                (sum / n) as i32
            };
            let pos = [pos[0], y, pos[2]];
            vec![Piece {
                bb: template_bbox(&t, pos, rot, false, pivot),
                kind: PieceKind::Template {
                    name,
                    pos,
                    rot,
                    mirror: false,
                    pivot,
                    opts: TemplateOpts {
                        ignore_air: true,
                        integrity: 1.0,
                        processors: None,
                        floor_y: false,
                        set: Vec::new(),
                    },
                },
            }]
        }
        None => Vec::new(),
    };
    Some(Stub {
        pos: [mx, y, mz],
        pieces,
    })
}

/// Igloo (measured against 9 vanilla igloos of seed 12345, 2026-11-24): rotation `nextInt(4)`,
/// laboratory if `nextDouble() < 0.5`, then `nextInt(8) + 3` ladder pieces. Template origins from
/// the chunk corner: top (0, 0, 0) pivot (3, 0, 5); ladder i (2, -3 - 3i, 4) pivot (1, 0, 1); bottom
/// (0, -3n - 6, -2) pivot (3, 0, 7) (one shared world pivot). The top's floor y is the top solid
/// block (WORLD_SURFACE_WG - 1) at the top template's local (3, 0) after rotation.
pub fn igloo(g: &Generator, s: &Structures, cx: i32, cz: i32, r: &mut JavaRandom) -> Vec<Piece> {
    let rot = Rotation::ALL[r.next_int_bounded(4) as usize];
    let lab = r.next_double() < 0.5;
    let n = if lab { r.next_int_bounded(8) + 3 } else { 0 };
    let (x0, z0) = (cx * 16, cz * 16);
    let probe = transform_pos([3, 0, 0], rot, false, [3, 0, 5]);
    let y = g.first_free(x0 + probe[0], z0 + probe[2], false) - 1;
    // piece order as saved by vanilla: bottom, ladders top-down, top
    let mut parts: Vec<(&str, [i32; 3], [i32; 3])> = Vec::new();
    if lab {
        parts.push(("bottom", [0, -3 * n - 6, -2], [3, 0, 7]));
        for i in 0..n {
            parts.push(("middle", [2, -3 - 3 * i, 4], [1, 0, 1]));
        }
    }
    parts.push(("top", [0, 0, 0], [3, 0, 5]));
    // without a laboratory the top's trapdoor (at the pivot) is snow (measured, seed 12345 igloo 36,41)
    let snow = BlockState::from_name("snow_block")
        .map(|b| b.0 as u16)
        .unwrap_or(0);
    let mut out = Vec::new();
    for (name, off, pivot) in parts {
        let set = if name == "top" && !lab {
            vec![([x0 + 3, y, z0 + 5], snow)]
        } else {
            Vec::new()
        };
        let name = format!("minecraft:igloo/{name}");
        let Some(t) = s.templates.get(&name) else {
            return Vec::new();
        };
        let pos = [x0 + off[0], y + off[1], z0 + off[2]];
        out.push(Piece {
            bb: template_bbox(&t, pos, rot, false, pivot),
            kind: PieceKind::Template {
                name,
                pos,
                rot,
                mirror: false,
                pivot,
                opts: TemplateOpts {
                    ignore_air: false,
                    integrity: 1.0,
                    processors: None,
                    floor_y: false,
                    set,
                },
            },
        });
    }
    out
}
/// Ocean ruins (measured by replaying the start random against 26 vanilla cold ruins): rotation,
/// large (< large_probability), template index (cold: brick_1..8 / big_brick_1,2,3,8 drawn with
/// nextInt; each cold ruin is three overlaid templates brick/cracked/mossy at integrity 0.8/0.7/0.5,
/// large 0.9/0.7/0.5), origin = chunk corner, pivot = origin; y = OCEAN_FLOOR_WG height at the
/// template origin (after carvers: re-taken at placement). Clusters of small ruins around large
/// ones and the warm variants' integrity are not modelled yet.
#[allow(clippy::too_many_arguments)]
pub fn ocean_ruin(
    g: &Generator,
    s: &Structures,
    warm: bool,
    large_p: f32,
    _cp: f32,
    cx: i32,
    cz: i32,
    r: &mut JavaRandom,
) -> Vec<Piece> {
    let rot = Rotation::ALL[r.next_int_bounded(4) as usize];
    let large = r.next_float() <= large_p;
    let base = [cx * 16, 0, cz * 16];
    let y = g.first_free(base[0], base[2], true);
    let pos = [base[0], y, base[2]];
    let mut names: Vec<(String, f32)> = Vec::new();
    if warm {
        let n = if large {
            format!("big_warm_{}", 4 + r.next_int_bounded(4))
        } else {
            format!("warm_{}", 1 + r.next_int_bounded(8))
        };
        names.push((n, if large { 0.9 } else { 0.8 }));
    } else if large {
        let i = [1, 2, 3, 8][r.next_int_bounded(4) as usize];
        for (k, f) in [("big_brick", 0.9), ("big_cracked", 0.7), ("big_mossy", 0.5)] {
            names.push((format!("{k}_{i}"), f));
        }
    } else {
        let i = 1 + r.next_int_bounded(8);
        for (k, f) in [("brick", 0.8), ("cracked", 0.7), ("mossy", 0.5)] {
            names.push((format!("{k}_{i}"), f));
        }
    }
    let mut out = Vec::new();
    for (n, integrity) in names {
        let name = format!("minecraft:underwater_ruin/{n}");
        let Some(t) = s.templates.get(&name) else {
            continue;
        };
        out.push(Piece {
            bb: template_bbox(&t, pos, rot, false, [0; 3]),
            kind: PieceKind::Template {
                name,
                pos,
                rot,
                mirror: false,
                pivot: [0; 3],
                opts: TemplateOpts {
                    ignore_air: true,
                    integrity,
                    processors: None,
                    floor_y: true,
                    set: Vec::new(),
                },
            },
        });
    }
    out
}
/// Ruined portals. Draw order measured by replaying the start random against 14 vanilla portals:
/// setup (cumulative weights, only with several setups), air pocket (only when 0 < p < 1),
/// giant (< 0.05: giant_portal_1..3, else portal_1..10), rotation, mirror (< 0.5: none).
/// The piece y: scanning down, the first y where at least 3 of the 4 box corner columns are solid
/// (matches all on_land_surface / on_ocean_floor samples); other placements are approximations.
pub fn ruined_portal(
    g: &Generator,
    s: &Structures,
    setups: &[PortalSetup],
    _d: &Def,
    cx: i32,
    cz: i32,
    r: &mut JavaRandom,
) -> Option<Stub> {
    if setups.is_empty() {
        return None;
    }
    let setup = if setups.len() == 1 {
        &setups[0]
    } else {
        let total: f32 = setups.iter().map(|x| x.weight).sum();
        let mut f = r.next_float();
        let mut pick = &setups[setups.len() - 1];
        for x in setups {
            f -= x.weight / total;
            if f < 0.0 {
                pick = x;
                break;
            }
        }
        pick
    };
    let air = if setup.air_pocket >= 1.0 {
        true
    } else if setup.air_pocket <= 0.0 {
        false
    } else {
        r.next_float() < setup.air_pocket
    };
    let name = if r.next_float() < 0.05 {
        format!(
            "minecraft:ruined_portal/giant_portal_{}",
            r.next_int_bounded(3) + 1
        )
    } else {
        format!(
            "minecraft:ruined_portal/portal_{}",
            r.next_int_bounded(10) + 1
        )
    };
    let rot = Rotation::ALL[r.next_int_bounded(4) as usize];
    let mirror = r.next_float() >= 0.5;
    let t = s.templates.get(&name)?;
    let pivot = [t.size[0] / 2, 0, t.size[2] / 2];
    let base = [cx * 16, 0, cz * 16];
    let bb0 = template_bbox(&t, base, rot, mirror, pivot);
    let corners = [
        (bb0.min[0], bb0.min[2]),
        (bb0.max[0], bb0.min[2]),
        (bb0.min[0], bb0.max[2]),
        (bb0.max[0], bb0.max[2]),
    ];
    let hs: Vec<i32> = corners
        .iter()
        .map(|c| g.first_free(c.0, c.1, true))
        .collect();
    let mut sorted = hs.clone();
    sorted.sort();
    // highest y with >= 3 corners solid below it = second-lowest first-free height - 1
    let level = sorted[1] - 1;
    let span = bb0.span(1);
    let y = match setup.placement.as_str() {
        "underground" | "in_mountain" => (level - span - 8).max(-50),
        "partly_buried" => level - span / 2,
        _ => level,
    };
    let pos = [base[0], y, base[2]];
    let bb = bb0.moved([0, y, 0]);
    let _ = air;
    Some(Stub {
        pos: [base[0], y, base[2]],
        pieces: vec![Piece {
            bb,
            kind: PieceKind::Template {
                name,
                pos,
                rot,
                mirror,
                pivot,
                opts: TemplateOpts {
                    ignore_air: true,
                    integrity: 1.0,
                    processors: None,
                    floor_y: false,
                    set: Vec::new(),
                },
            },
        }],
    })
}

/// Swamp hut layout, measured from 6 vanilla huts of seed 42 (chunks -17,22 144,-51 -51,-23
/// 119,-126 197,333 558,-23; 2026-11-24, `structures blocks`), in the frame of orientation 1 (west):
/// layers y = 0..4 above the box floor, rows z = 0..6, columns x = 0..8; ' ' = not touched
/// (terrain / vegetation stay), '.' = air (only inside the hut). Palette below.
const HUT_LAYERS: [[&str; 7]; 5] = [
    [
        "         ",
        " b    b  ",
        "         ",
        "         ",
        "         ",
        " b    b  ",
        "         ",
    ],
    [
        "         ",
        " bccccbc ",
        " cccccccc",
        " cccccccc",
        " cccccccc",
        " bccccbc ",
        "         ",
    ],
    [
        "         ",
        " bccccbd ",
        " c ...c  ",
        " ce. .c  ",
        " cf.  .  ",
        " bccccbd ",
        "         ",
    ],
    [
        "         ",
        " bcg.cb  ",
        " c....h  ",
        " h....c  ",
        " c.....  ",
        " bc..cb  ",
        "         ",
    ],
    [
        "ijjjjjjk ",
        "lccccccm ",
        "lccccccm ",
        "lccccccm ",
        "lccccccm ",
        "lccccccm ",
        "noooooop ",
    ],
];
const HUT_PALETTE: [(char, &str); 16] = [
    ('.', "air"),
    ('b', "oak_log[axis=y]"),
    ('c', "spruce_planks"),
    (
        'd',
        "oak_fence[east=false,north=false,south=false,waterlogged=false,west=true]",
    ),
    ('e', "crafting_table"),
    ('f', "cauldron"),
    ('g', "potted_red_mushroom"),
    (
        'h',
        "oak_fence[east=false,north=true,south=true,waterlogged=false,west=false]",
    ),
    (
        'i',
        "spruce_stairs[facing=east,half=bottom,shape=outer_right,waterlogged=false]",
    ),
    (
        'j',
        "spruce_stairs[facing=south,half=bottom,shape=straight,waterlogged=false]",
    ),
    (
        'k',
        "spruce_stairs[facing=west,half=bottom,shape=outer_left,waterlogged=false]",
    ),
    (
        'l',
        "spruce_stairs[facing=east,half=bottom,shape=straight,waterlogged=false]",
    ),
    (
        'm',
        "spruce_stairs[facing=west,half=bottom,shape=straight,waterlogged=false]",
    ),
    (
        'n',
        "spruce_stairs[facing=east,half=bottom,shape=outer_left,waterlogged=false]",
    ),
    (
        'o',
        "spruce_stairs[facing=north,half=bottom,shape=straight,waterlogged=false]",
    ),
    (
        'p',
        "spruce_stairs[facing=west,half=bottom,shape=outer_right,waterlogged=false]",
    ),
];
/// Leg columns (frame of orientation 1): oak logs from y = -1 down while the block is air or water.
const HUT_LEGS: [(i32, i32); 4] = [(1, 1), (6, 1), (1, 5), (6, 5)];

/// Swamp hut start (measured): orientation `nextInt(4)` over (north, east, south, west) -> vanilla
/// `O` (2, 3, 0, 1); box at the chunk corner, 9 x 7 (west/east) or 7 x 9 (north/south), 7 high;
/// floor y = floor(mean WORLD_SURFACE_WG over the box footprint) (6/6 vanilla huts).
pub fn swamp_hut(g: &Generator, cx: i32, cz: i32, r: &mut JavaRandom) -> Piece {
    let o = [2u8, 3, 0, 1][r.next_int_bounded(4) as usize];
    let (w, d) = if o == 1 || o == 3 { (9, 7) } else { (7, 9) };
    let (x0, z0) = (cx * 16, cz * 16);
    let mut sum = 0i64;
    for x in 0..w {
        for z in 0..d {
            sum += g.first_free(x0 + x, z0 + z, false) as i64;
        }
    }
    let y = sum.div_euclid((w * d) as i64) as i32;
    Piece {
        bb: BBox::new([x0, y, z0], [x0 + w - 1, y + 6, z0 + d - 1]),
        kind: PieceKind::SwampHut {
            origin: [x0, y, z0],
            o,
        },
    }
}

/// Map a layout position (frame of orientation 1) into the box of orientation `o`, with the block
/// state transform. East mirrors x, north transposes; south (no vanilla sample yet) is assumed to be
/// the transpose with z flipped.
fn hut_transform(o: u8, x: i32, z: i32) -> (i32, i32) {
    match o {
        3 => (8 - x, z),
        2 => (z, x),
        0 => (z, 8 - x),
        _ => (x, z),
    }
}

fn hut_state(o: u8, s: u16) -> u16 {
    match o {
        3 => transform_state(s, Rotation::None, true),
        2 => transform_state(s, Rotation::Ccw90, true),
        0 => transform_state(s, Rotation::Ccw90, false),
        _ => s,
    }
}

fn place_swamp_hut(reg: &mut Region, clip: BBox, origin: [i32; 3], o: u8) {
    let pal: HashMap<char, u16> = HUT_PALETTE
        .iter()
        .map(|(c, n)| (*c, BlockState::parse(n).map(|b| b.0 as u16).unwrap_or(0)))
        .collect();
    for (y, layer) in HUT_LAYERS.iter().enumerate() {
        for (z, row) in layer.iter().enumerate() {
            for (x, c) in row.chars().enumerate() {
                let Some(st) = pal.get(&c) else { continue };
                let (bx, bz) = hut_transform(o, x as i32, z as i32);
                let p = [origin[0] + bx, origin[1] + y as i32, origin[2] + bz];
                if clip.contains(p) {
                    reg.set(p[0], p[1], p[2], hut_state(o, *st));
                }
            }
        }
    }
    let log = pal[&'b'];
    for (x, z) in HUT_LEGS {
        let (bx, bz) = hut_transform(o, x, z);
        let (px, pz) = (origin[0] + bx, origin[2] + bz);
        let mut y = origin[1] - 1;
        while y > origin[1] - 64 && clip.contains([px, y, pz]) {
            let Some(cur) = reg.get(px, y, pz) else { break };
            let name = BlockState(cur as u32).name();
            if !(BlockState(cur as u32).is_air() || name == "water") {
                break;
            }
            reg.set(px, y, pz, log);
            y -= 1;
        }
    }
}

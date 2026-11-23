//! Mineshafts (procedural in vanilla). The piece generator was fitted against the pieces saved by
//! the official 26.2 server (2026-11-24): 53 mineshafts of seeds 12345 / 42 / 1 (64 x 64 chunks
//! around the origin) plus 10 mesa and 2 more normal ones (seeds 12345 / 42 / 2) are reproduced
//! exactly (65 starts, 8274 / 8274 pieces: type, box, orientation, sections, rails, spider flag,
//! two floors, depth, in saved order). The draw order below is the fitted one; NOTES.md "Mineshafts" lists the rules.
//!
//! Block placement (carving, supports, rails, cobwebs, bridges) was measured from the same worlds
//! (per-cell statistics in the piece frames); the per-block random elements are drawn from the
//! placement random but in an order that is not vanilla's, so they match only statistically.

use super::template::BBox;
use super::{Piece, PieceKind};
use crate::random::{JavaRandom, Rng};
use crate::region::Region;
use basalt_data::BlockState;
use std::sync::OnceLock;

/// Directions as vanilla's saved `O`: 0 south, 1 west, 2 north, 3 east.
const S: u8 = 0;
const W: u8 = 1;
const N: u8 = 2;
const E: u8 = 3;

#[derive(Clone, Debug)]
pub enum MsKind {
    /// Start room; entrance openings (boxes) towards its children.
    Room(Vec<BBox>),
    Corridor {
        sections: i32,
        rails: bool,
        spider: bool,
    },
    Crossing {
        two_floors: bool,
    },
    Stairs,
}

#[derive(Clone, Debug)]
pub struct MsPiece {
    pub kind: MsKind,
    pub bb: BBox,
    /// Direction (crossings: the direction they were entered with, vanilla `D`).
    pub dir: u8,
    pub depth: i32,
}

struct Gen<'a> {
    r: &'a mut JavaRandom,
    pieces: Vec<MsPiece>,
    sx: i32,
    sz: i32,
}

/// A box `len` long in direction `o` from (x, y, z), `w` wide starting `side` blocks to the side.
fn dir_box(x: i32, y: i32, z: i32, o: u8, len: i32, h: i32, w: i32, side: i32) -> BBox {
    let (a, b) = match o {
        N => ([x + side, y, z - len + 1], [x + side + w - 1, y + h - 1, z]),
        S => ([x + side, y, z], [x + side + w - 1, y + h - 1, z + len - 1]),
        W => ([x - len + 1, y, z + side], [x, y + h - 1, z + side + w - 1]),
        _ => ([x, y, z + side], [x + len - 1, y + h - 1, z + side + w - 1]),
    };
    BBox::new(a, b)
}

impl Gen<'_> {
    fn collides(&self, b: &BBox) -> bool {
        self.pieces.iter().any(|p| p.bb.intersects(b))
    }

    fn room(&mut self, cx: i32, cz: i32) {
        let w = 8 + self.r.next_int_bounded(6);
        let h = 5 + self.r.next_int_bounded(6);
        let d = 8 + self.r.next_int_bounded(6);
        let (x0, z0) = (cx * 16 + 2, cz * 16 + 2);
        let bb = BBox::new([x0, 50, z0], [x0 + w - 1, 50 + h - 1, z0 + d - 1]);
        self.sx = x0;
        self.sz = z0;
        self.pieces.push(MsPiece {
            kind: MsKind::Room(Vec::new()),
            bb,
            dir: 0,
            depth: 0,
        });
        let (b0, b1) = (bb.min, bb.max);
        let hy = h - 4;
        for side in [N, S, W, E] {
            let len = if side == N || side == S { w } else { d };
            let mut k = 0;
            while k < len {
                k += self.r.next_int_bounded(len);
                if k + 3 > len {
                    break;
                }
                let y = b0[1]
                    + if hy > 0 {
                        self.r.next_int_bounded(hy)
                    } else {
                        0
                    }
                    + 1;
                let (x, z) = match side {
                    N => (b0[0] + k, b0[2] - 1),
                    S => (b0[0] + k, b1[2] + 1),
                    W => (b0[0] - 1, b0[2] + k),
                    _ => (b1[0] + 1, b0[2] + k),
                };
                if let Some(c) = self.child(x, y, z, side, 1) {
                    let (c0, c1) = (c.min, c.max);
                    let e = match side {
                        N => BBox::new([c0[0], c0[1], b0[2]], [c1[0], c1[1], b0[2] + 1]),
                        S => BBox::new([c0[0], c0[1], b1[2] - 1], [c1[0], c1[1], b1[2]]),
                        W => BBox::new([b0[0], c0[1], c0[2]], [b0[0] + 1, c1[1], c1[2]]),
                        _ => BBox::new([b1[0] - 1, c0[1], c0[2]], [b1[0], c1[1], c1[2]]),
                    };
                    if let MsKind::Room(es) = &mut self.pieces[0].kind {
                        es.push(e);
                    }
                }
                k += 4;
            }
        }
    }

    /// Create (and recurse into) a child piece; returns its box.
    fn child(&mut self, x: i32, y: i32, z: i32, o: u8, depth: i32) -> Option<BBox> {
        if depth > 9 || (x - self.sx).abs() > 80 || (z - self.sz).abs() > 80 {
            return None;
        }
        let t = self.r.next_int_bounded(100);
        let p = if t >= 80 {
            let two = self.r.next_int_bounded(4) == 0;
            let bb = dir_box(x, y, z, o, 5, if two { 7 } else { 3 }, 5, -1);
            if self.collides(&bb) {
                return None;
            }
            MsPiece {
                kind: MsKind::Crossing { two_floors: two },
                bb,
                dir: o,
                depth,
            }
        } else if t >= 70 {
            let bb = dir_box(x, y - 5, z, o, 9, 8, 3, 0);
            if self.collides(&bb) {
                return None;
            }
            MsPiece {
                kind: MsKind::Stairs,
                bb,
                dir: o,
                depth,
            }
        } else {
            let mut n = self.r.next_int_bounded(3) + 2;
            let mut bb = dir_box(x, y, z, o, n * 5, 3, 3, 0);
            while n > 0 {
                bb = dir_box(x, y, z, o, n * 5, 3, 3, 0);
                if !self.collides(&bb) {
                    break;
                }
                n -= 1;
            }
            if n == 0 {
                return None;
            }
            let rails = self.r.next_int_bounded(3) == 0;
            let spider = !rails && self.r.next_int_bounded(23) == 0;
            MsPiece {
                kind: MsKind::Corridor {
                    sections: n,
                    rails,
                    spider,
                },
                bb,
                dir: o,
                depth,
            }
        };
        let bb = p.bb;
        let kind = p.kind.clone();
        self.pieces.push(p);
        match kind {
            MsKind::Corridor { .. } => self.corridor_children(bb, o, depth),
            MsKind::Crossing { two_floors } => self.crossing_children(bb, o, depth, two_floors),
            MsKind::Stairs => self.stairs_children(bb, o, depth),
            MsKind::Room(_) => {}
        }
        Some(bb)
    }

    fn corridor_children(&mut self, bb: BBox, o: u8, gd: i32) {
        let ([x0, y0, z0], [x1, _, z1]) = (bb.min, bb.max);
        let j = self.r.next_int_bounded(4);
        let y = y0 + self.r.next_int_bounded(3) - 1;
        let d = gd + 1;
        match (o, j) {
            (N, 0 | 1) => self.child(x0, y, z0 - 1, N, d),
            (N, 2) => self.child(x0 - 1, y, z0, W, d),
            (N, _) => self.child(x1 + 1, y, z0, E, d),
            (S, 0 | 1) => self.child(x0, y, z1 + 1, S, d),
            (S, 2) => self.child(x0 - 1, y, z1 - 3, W, d),
            (S, _) => self.child(x1 + 1, y, z1 - 3, E, d),
            (W, 0 | 1) => self.child(x0 - 1, y, z0, W, d),
            (W, 2) => self.child(x0, y, z0 - 1, N, d),
            (W, _) => self.child(x0, y, z1 + 1, S, d),
            (_, 0 | 1) => self.child(x1 + 1, y, z0, E, d),
            (_, 2) => self.child(x1 - 3, y, z0 - 1, N, d),
            (_, _) => self.child(x1 - 3, y, z1 + 1, S, d),
        };
        if gd < 8 {
            // side branches every 5 blocks from the far / near end, depth + 2
            let ns = o == N || o == S;
            let (lo, hi) = if ns { (z0, z1) } else { (x0, x1) };
            let mut k = lo + 3;
            while k <= hi - 4 {
                let a = self.r.next_int_bounded(5);
                if a == 0 {
                    if ns {
                        self.child(x0 - 1, y0, k, W, gd + 2);
                    } else {
                        self.child(k, y0, z0 - 1, N, gd + 2);
                    }
                } else if a == 1 {
                    if ns {
                        self.child(x1 + 1, y0, k, E, gd + 2);
                    } else {
                        self.child(k, y0, z1 + 1, S, gd + 2);
                    }
                }
                k += 5;
            }
        }
    }

    fn crossing_children(&mut self, bb: BBox, o: u8, gd: i32, two: bool) {
        let ([x0, y0, z0], [x1, _, z1]) = (bb.min, bb.max);
        let n = (x0 + 1, z0 - 1, N);
        let s = (x0 + 1, z1 + 1, S);
        let w = (x0 - 1, z0 + 1, W);
        let e = (x1 + 1, z0 + 1, E);
        // all four directions in the order N, S, W, E without the one it was entered from
        let lower = match o {
            N => [n, w, e],
            S => [s, w, e],
            W => [n, s, w],
            _ => [n, s, e],
        };
        for (x, z, d) in lower {
            self.child(x, y0, z, d, gd + 1);
        }
        if two {
            // upper floor: N, W, E, S (the entry side too), each with a coin flip
            for (x, z, d) in [n, w, e, s] {
                if self.r.next_bool() {
                    self.child(x, y0 + 4, z, d, gd + 1);
                }
            }
        }
    }

    fn stairs_children(&mut self, bb: BBox, o: u8, gd: i32) {
        let ([x0, y0, z0], [x1, _, z1]) = (bb.min, bb.max);
        match o {
            N => self.child(x0, y0, z0 - 1, N, gd + 1),
            S => self.child(x0, y0, z1 + 1, S, gd + 1),
            W => self.child(x0 - 1, y0, z0, W, gd + 1),
            _ => self.child(x1 + 1, y0, z0, E, gd + 1),
        };
    }
}

/// Generate the pieces of a mineshaft starting in chunk (cx, cz) (start random after the
/// frequency draw). Normal mineshafts: the whole structure is moved down so that its top lands at
/// a random y between (its height - 62) and 52 (measured 53/53: top = h + minY + 1, plus
/// `nextInt(sea - 10 - that)` when below). Mesa mineshafts: see [`mesa_shift`].
pub fn mineshaft(
    g: &crate::Generator,
    mesa: bool,
    cx: i32,
    cz: i32,
    r: &mut JavaRandom,
) -> Vec<Piece> {
    let mut gen = Gen {
        r,
        pieces: Vec::new(),
        sx: 0,
        sz: 0,
    };
    gen.room(cx, cz);
    let ymin = gen.pieces.iter().map(|p| p.bb.min[1]).min().unwrap_or(50);
    let ymax = gen.pieces.iter().map(|p| p.bb.max[1]).max().unwrap_or(50);
    let dy = if mesa {
        mesa_shift(g, ymin, ymax, gen.r)
    } else {
        let top = g.sea_level() - 10;
        let mut j = ymax - ymin + 2 + crate::MIN_Y;
        if j < top {
            j += gen.r.next_int_bounded(top - j);
        }
        j - ymax
    };
    gen.pieces
        .into_iter()
        .map(|mut p| {
            p.bb = p.bb.moved([0, dy, 0]);
            if let MsKind::Room(es) = &mut p.kind {
                for e in es.iter_mut() {
                    *e = e.moved([0, dy, 0]);
                }
            }
            Piece {
                bb: p.bb,
                kind: PieceKind::Mineshaft { piece: p, mesa },
            }
        })
        .collect()
}

/// Mesa mineshafts sit higher: the 10 measured vanilla ones (seeds 12345 / 42 / 2) have their
/// bottom at y 55..87 and top at 79..113; no rule on the start random or the surface height
/// reproduced them, so the bottom is drawn uniformly from that range (statistical only).
fn mesa_shift(g: &crate::Generator, ymin: i32, _ymax: i32, r: &mut JavaRandom) -> i32 {
    g.sea_level() - 8 + r.next_int_bounded(33) - ymin
}

struct States {
    cave_air: u16,
    air: u16,
    planks: [u16; 2],
    fence: [u16; 2],
    rail_ns: u16,
    rail_ew: u16,
    cobweb: u16,
    spawner: u16,
}

fn states() -> &'static States {
    static S: OnceLock<States> = OnceLock::new();
    S.get_or_init(|| {
        let p = |n: &str| BlockState::parse(n).map(|b| b.0 as u16).unwrap_or(0);
        States {
            cave_air: p("cave_air"),
            air: p("air"),
            planks: [p("oak_planks"), p("dark_oak_planks")],
            fence: [
                p("oak_fence[east=false,north=false,south=false,waterlogged=false,west=false]"),
                p("dark_oak_fence[east=false,north=false,south=false,waterlogged=false,west=false]"),
            ],
            rail_ns: p("rail[shape=north_south,waterlogged=false]"),
            rail_ew: p("rail[shape=east_west,waterlogged=false]"),
            cobweb: p("cobweb"),
            spawner: p("spawner"),
        }
    })
}

fn is_liquid(s: u16) -> bool {
    let n = BlockState(s as u32).name();
    n == "water" || n == "lava"
}

fn is_airish(s: u16) -> bool {
    BlockState(s as u32).is_air()
}

/// Place one mineshaft piece into the part of the region inside `clip`.
pub fn place(
    reg: &mut Region,
    clip: BBox,
    p: &MsPiece,
    mesa: bool,
    blocking: &[bool],
    r: &mut crate::features::R,
) {
    let st = states();
    let wood = mesa as usize;
    // Measured: a piece is left out of a chunk when its box holds liquid inside that chunk, or
    // when the biome there is in #mineshaft_blocking (deep dark: whole areas of a vanilla
    // mineshaft are missing there).
    let Some(c) = intersect(&p.bb, &clip) else {
        return;
    };
    let mode = std::env::var("BASALT_MS_BIOME").unwrap_or_default();
    let probe = if mode == "full" { p.bb } else { c };
    let b = reg.biome(
        (probe.min[0] + probe.max[0]) >> 1,
        (probe.min[1] + probe.max[1]) >> 1,
        (probe.min[2] + probe.max[2]) >> 1,
    );
    if mode != "off" && blocking.get(b as usize).copied().unwrap_or(false) {
        return;
    }
    for x in c.min[0]..=c.max[0] {
        for z in c.min[2]..=c.max[2] {
            for y in c.min[1]..=c.max[1] {
                if reg.get(x, y, z).map(is_liquid).unwrap_or(false) {
                    return;
                }
            }
        }
    }
    let carve = |reg: &mut Region, x: i32, y: i32, z: i32| {
        if clip.contains([x, y, z]) {
            if let Some(s) = reg.get(x, y, z) {
                if s != st.cave_air {
                    reg.set(x, y, z, st.cave_air);
                }
            }
        }
    };
    let set = |reg: &mut Region, x: i32, y: i32, z: i32, s: u16| {
        if clip.contains([x, y, z]) {
            reg.set(x, y, z, s);
        }
    };
    let ([x0, y0, z0], [x1, y1, z1]) = (p.bb.min, p.bb.max);
    match &p.kind {
        MsKind::Room(entrances) => {
            for x in x0..=x1 {
                for z in z0..=z1 {
                    for y in y0 + 1..=(y0 + 3).min(y1) {
                        carve(reg, x, y, z);
                    }
                }
            }
            // dome: ellipsoid over the box above y0 + 3 (fitted, 87 % of the dome cells)
            let (cx, cz) = ((x0 + x1) as f32 / 2.0, (z0 + z1) as f32 / 2.0);
            let (rx, rz) = ((x1 - x0 + 1) as f32 / 2.0, (z1 - z0 + 1) as f32 / 2.0);
            let ry = (y1 - y0 - 3) as f32;
            if ry > 0.0 {
                for x in x0..=x1 {
                    for z in z0..=z1 {
                        for y in y0 + 4..=y1 {
                            let f = ((x as f32 - cx) / rx).powi(2)
                                + ((y - y0 - 4) as f32 / ry).powi(2)
                                + ((z as f32 - cz) / rz).powi(2);
                            if f <= 1.0 {
                                carve(reg, x, y, z);
                            }
                        }
                    }
                }
            }
            for e in entrances {
                for x in e.min[0]..=e.max[0] {
                    for y in e.min[1]..=e.max[1] {
                        for z in e.min[2]..=e.max[2] {
                            carve(reg, x, y, z);
                        }
                    }
                }
            }
        }
        MsKind::Corridor {
            sections,
            rails,
            spider,
        } => {
            let len = sections * 5;
            // local (a along from the start, l across) -> world (x, z)
            let at = |a: i32, l: i32| -> (i32, i32) {
                match p.dir {
                    S => (x0 + l, z0 + a),
                    N => (x1 - l, z1 - a),
                    E => (x0 + a, z1 - l),
                    _ => (x1 - a, z0 + l),
                }
            };
            for a in 0..len {
                for l in 0..3 {
                    let (x, z) = at(a, l);
                    for y in y0..=y0 + 2 {
                        carve(reg, x, y, z);
                    }
                    // bridge over gaps (measured 11 % of floor cells: where the floor is air)
                    if reg.get(x, y0 - 1, z).map(is_airish).unwrap_or(false) {
                        set(reg, x, y0 - 1, z, st.planks[wood]);
                    }
                }
            }
            for i in 0..*sections {
                let a = 2 + 5 * i;
                // supports (fences + planks) where the ceiling above both sides is solid
                // (measured: 2139 + 74 of 2233 vanilla supports; 16 % of such spots have none)
                let open = [0, 2].iter().any(|&l| {
                    let (x, z) = at(a, l);
                    reg.get(x, y0 + 3, z).map(is_airish).unwrap_or(true)
                });
                if open {
                    continue;
                }
                for l in [0, 2] {
                    let (x, z) = at(a, l);
                    // fences connect to the solid wall beside them (vanilla's saved states)
                    let (ox, oz) = at(a, if l == 0 { -1 } else { 3 });
                    let side = match (ox - x, oz - z) {
                        (1, _) => "east",
                        (-1, _) => "west",
                        (_, 1) => "south",
                        _ => "north",
                    };
                    for y in [y0, y0 + 1] {
                        let solid = reg
                            .get(ox, y, oz)
                            .map(|s| !is_airish(s) && !is_liquid(s))
                            .unwrap_or(true);
                        let f = if solid {
                            BlockState(st.fence[wood] as u32)
                                .with(side, "true")
                                .map(|b| b.0 as u16)
                                .unwrap_or(st.fence[wood])
                        } else {
                            st.fence[wood]
                        };
                        set(reg, x, y, z, f);
                    }
                }
                for l in 0..3 {
                    let (x, z) = at(a, l);
                    if l != 1 || r.next_float() < 0.75 {
                        set(reg, x, y0 + 2, z, st.planks[wood]);
                    }
                }
            }
            for a in 0..len {
                let support = a % 5 == 2;
                for l in 0..3 {
                    let (x, z) = at(a, l);
                    if *spider {
                        for y in y0..=y0 + 1 {
                            if !(support && l != 1) && r.next_float() < 0.55 {
                                set(reg, x, y, z, st.cobweb);
                            }
                        }
                    }
                    // cobwebs under the ceiling at the sides (measured 4-9 % per cell)
                    if !support && l != 1 {
                        let pr = if a % 5 == 1 || a % 5 == 3 { 0.09 } else { 0.04 };
                        if r.next_float() < pr {
                            set(reg, x, y0 + 2, z, st.cobweb);
                        }
                    }
                }
                if *rails {
                    let (x, z) = at(a, 1);
                    let floor = reg
                        .get(x, y0 - 1, z)
                        .map(|s| !is_airish(s))
                        .unwrap_or(false);
                    if floor && r.next_float() < 0.7 {
                        let rail = if p.dir == N || p.dir == S {
                            st.rail_ns
                        } else {
                            st.rail_ew
                        };
                        set(reg, x, y0, z, rail);
                    }
                }
            }
            if *spider {
                let (x, z) = at(len / 2, 1);
                set(reg, x, y0, z, st.spawner);
            }
        }
        MsKind::Crossing { two_floors } => {
            let plus = |dx: i32, dz: i32| (1..=3).contains(&dx) || (1..=3).contains(&dz);
            for dx in 0..5 {
                for dz in 0..5 {
                    let (x, z) = (x0 + dx, z0 + dz);
                    let pillar = (dx == 1 || dx == 3) && (dz == 1 || dz == 3);
                    if plus(dx, dz) && !pillar {
                        for y in y0..=y0 + 2 {
                            carve(reg, x, y, z);
                        }
                        if *two_floors {
                            for y in y0 + 4..=y1 {
                                carve(reg, x, y, z);
                            }
                            if (1..=3).contains(&dx) && (1..=3).contains(&dz) {
                                carve(reg, x, y0 + 3, z);
                            }
                        }
                        if reg.get(x, y0 - 1, z).map(is_airish).unwrap_or(false) {
                            set(reg, x, y0 - 1, z, st.planks[wood]);
                        }
                    }
                    if pillar {
                        for y in y0..=y1 {
                            set(reg, x, y, z, st.planks[wood]);
                        }
                    }
                }
            }
        }
        MsKind::Stairs => {
            let at = |a: i32, l: i32| -> (i32, i32) {
                match p.dir {
                    S => (x0 + l, z0 + a),
                    N => (x1 - l, z1 - a),
                    E => (x0 + a, z1 - l),
                    _ => (x1 - a, z0 + l),
                }
            };
            for a in 0..9 {
                let (lo, hi) = match a {
                    0 | 1 => (5, 7),
                    7 | 8 => (0, 2),
                    _ => {
                        let i = a - 2;
                        (5 - i - if i < 4 { 1 } else { 0 }, 7 - i)
                    }
                };
                for l in 0..3 {
                    let (x, z) = at(a, l);
                    for y in y0 + lo..=y0 + hi {
                        carve(reg, x, y, z);
                    }
                }
            }
        }
    }
    let _ = st.air;
}

fn intersect(a: &BBox, b: &BBox) -> Option<BBox> {
    let min = [
        a.min[0].max(b.min[0]),
        a.min[1].max(b.min[1]),
        a.min[2].max(b.min[2]),
    ];
    let max = [
        a.max[0].min(b.max[0]),
        a.max[1].min(b.max[1]),
        a.max[2].min(b.max[2]),
    ];
    if (0..3).all(|i| min[i] <= max[i]) {
        Some(BBox { min, max })
    } else {
        None
    }
}

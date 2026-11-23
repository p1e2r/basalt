//! Strongholds (procedural in vanilla). The piece generator was fitted against the pieces saved by
//! the official 26.2 server (NOTES.md "Strongholds"): piece geometry, exits, weights, limits and the
//! draw order were derived from the saved piece lists (type, box, `O`, `GD`, per-piece flags, saved
//! order = creation order) of vanilla strongholds at the ring positions.

use super::template::BBox;
use super::{Piece, PieceKind};
use crate::random::{JavaRandom, Rng};

/// Directions as vanilla's saved `O`: 0 south, 1 west, 2 north, 3 east.
const S: u8 = 0;
const W: u8 = 1;
const N: u8 = 2;
const E: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Door {
    Opening,
    Wood,
    Grates,
    Iron,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ShKind {
    /// The start: a spiral staircase down (vanilla `shstart`).
    Start,
    Straight {
        left: bool,
        right: bool,
    },
    ChestCorridor,
    LeftTurn,
    RightTurn,
    RoomCrossing {
        ty: i32,
    },
    StraightStairsDown,
    StairsDown,
    FiveCrossing {
        ll: bool,
        lh: bool,
        rl: bool,
        rh: bool,
    },
    PrisonHall,
    Library {
        tall: bool,
    },
    PortalRoom,
    Filler {
        steps: i32,
    },
}

impl ShKind {
    pub fn id(&self) -> &'static str {
        match self {
            ShKind::Start => "shstart",
            ShKind::Straight { .. } => "shs",
            ShKind::ChestCorridor => "shcc",
            ShKind::LeftTurn => "shlt",
            ShKind::RightTurn => "shrt",
            ShKind::RoomCrossing { .. } => "shrc",
            ShKind::StraightStairsDown => "shssd",
            ShKind::StairsDown => "shsd",
            ShKind::FiveCrossing { .. } => "sh5c",
            ShKind::PrisonHall => "shph",
            ShKind::Library { .. } => "shli",
            ShKind::PortalRoom => "shpr",
            ShKind::Filler { .. } => "shfc",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ShPiece {
    pub kind: ShKind,
    pub bb: BBox,
    pub o: u8,
    pub depth: i32,
    pub door: Door,
}

/// Weighted piece types: (type index, weight, limit (0 = none), minimum depth).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum T {
    Straight,
    PrisonHall,
    LeftTurn,
    RightTurn,
    RoomCrossing,
    StraightStairsDown,
    StairsDown,
    FiveCrossing,
    ChestCorridor,
    Library,
    PortalRoom,
}

struct Weight {
    t: T,
    weight: i32,
    limit: i32,
    count: i32,
}

impl Weight {
    fn can_place(&self, depth: i32) -> bool {
        let lim = self.limit == 0 || self.count < self.limit;
        match self.t {
            T::Library => lim && depth > 4,
            T::PortalRoom => lim && depth > 5,
            _ => lim,
        }
    }
    fn valid(&self) -> bool {
        self.limit == 0 || self.count < self.limit
    }
}

fn weights() -> Vec<Weight> {
    let w = |t, weight, limit| Weight {
        t,
        weight,
        limit,
        count: 0,
    };
    vec![
        w(T::Straight, 40, 0),
        w(T::PrisonHall, 5, 5),
        w(T::LeftTurn, 20, 0),
        w(T::RightTurn, 20, 0),
        w(T::RoomCrossing, 10, 6),
        w(T::StraightStairsDown, 5, 5),
        w(T::StairsDown, 5, 5),
        w(T::FiveCrossing, 5, 4),
        w(T::ChestCorridor, 5, 4),
        w(T::Library, 10, 2),
        w(T::PortalRoom, 20, 1),
    ]
}

/// Box of a piece entered at (x, y, z) going `o`: `w` wide, `h` high, `d` deep, offset by
/// (ox, oy, oz) in the piece frame (x across, z along the direction).
#[allow(clippy::too_many_arguments)]
fn orient_box(p: [i32; 3], ox: i32, oy: i32, oz: i32, w: i32, h: i32, d: i32, o: u8) -> BBox {
    let [x, y, z] = p;
    let (y0, y1) = (y + oy, y + oy + h - 1);
    match o {
        N => BBox::new([x + ox, y0, z - d + 1 + oz], [x + ox + w - 1, y1, z + oz]),
        S => BBox::new([x + ox, y0, z + oz], [x + ox + w - 1, y1, z + oz + d - 1]),
        W => BBox::new([x - d + 1 + oz, y0, z + ox], [x + oz, y1, z + ox + w - 1]),
        _ => BBox::new([x + oz, y0, z + ox], [x + oz + d - 1, y1, z + ox + w - 1]),
    }
}

struct Gen<'a> {
    r: &'a mut JavaRandom,
    pieces: Vec<ShPiece>,
    pending: Vec<usize>,
    weights: Vec<Weight>,
    total: i32,
    imposed: Option<T>,
    previous: Option<T>,
    sx: i32,
    sz: i32,
    portal: bool,
}

impl Gen<'_> {
    fn collides(&self, b: &BBox) -> Option<usize> {
        self.pieces.iter().position(|p| p.bb.intersects(b))
    }

    fn door(&mut self) -> Door {
        match self.r.next_int_bounded(5) {
            2 => Door::Wood,
            3 => Door::Grates,
            4 => Door::Iron,
            _ => Door::Opening,
        }
    }

    fn ok(&self, b: &BBox) -> bool {
        b.min[1] > 10 && self.collides(b).is_none()
    }

    /// Try to create a piece of type `t` at entrance `p` going `o`.
    fn make(&mut self, t: T, p: [i32; 3], o: u8, depth: i32) -> Option<ShPiece> {
        let (kind, bb) = match t {
            T::Straight => {
                let bb = orient_box(p, -1, -1, 0, 5, 5, 7, o);
                if !self.ok(&bb) {
                    return None;
                }
                let door = self.door();
                let left = self.r.next_int_bounded(2) == 0;
                let right = self.r.next_int_bounded(2) == 0;
                return Some(self.piece(ShKind::Straight { left, right }, bb, o, depth, door));
            }
            T::PrisonHall => (ShKind::PrisonHall, orient_box(p, -1, -1, 0, 9, 5, 11, o)),
            T::LeftTurn => (ShKind::LeftTurn, orient_box(p, -1, -1, 0, 5, 5, 5, o)),
            T::RightTurn => (ShKind::RightTurn, orient_box(p, -1, -1, 0, 5, 5, 5, o)),
            T::RoomCrossing => {
                let bb = orient_box(p, -4, -1, 0, 11, 7, 11, o);
                if !self.ok(&bb) {
                    return None;
                }
                let door = self.door();
                let ty = self.r.next_int_bounded(5);
                return Some(self.piece(ShKind::RoomCrossing { ty }, bb, o, depth, door));
            }
            T::StraightStairsDown => (
                ShKind::StraightStairsDown,
                orient_box(p, -1, -7, 0, 5, 11, 8, o),
            ),
            T::StairsDown => (ShKind::StairsDown, orient_box(p, -1, -7, 0, 5, 11, 5, o)),
            T::FiveCrossing => {
                let bb = orient_box(p, -4, -3, 0, 10, 9, 11, o);
                if !self.ok(&bb) {
                    return None;
                }
                let door = self.door();
                let ll = self.r.next_bool();
                let lh = self.r.next_bool();
                let rl = self.r.next_bool();
                let rh = self.r.next_int_bounded(3) > 0;
                return Some(self.piece(
                    ShKind::FiveCrossing { ll, lh, rl, rh },
                    bb,
                    o,
                    depth,
                    door,
                ));
            }
            T::ChestCorridor => (ShKind::ChestCorridor, orient_box(p, -1, -1, 0, 5, 5, 7, o)),
            T::Library => {
                let mut bb = orient_box(p, -4, -1, 0, 14, 11, 15, o);
                let mut tall = true;
                if !self.ok(&bb) {
                    bb = orient_box(p, -4, -1, 0, 14, 6, 15, o);
                    tall = false;
                    if !self.ok(&bb) {
                        return None;
                    }
                }
                let door = self.door();
                return Some(self.piece(ShKind::Library { tall }, bb, o, depth, door));
            }
            T::PortalRoom => {
                let bb = orient_box(p, -4, -1, 0, 11, 8, 16, o);
                if !self.ok(&bb) {
                    return None;
                }
                return Some(self.piece(ShKind::PortalRoom, bb, o, depth, Door::Opening));
            }
        };
        if !self.ok(&bb) {
            return None;
        }
        let door = self.door();
        Some(self.piece(kind, bb, o, depth, door))
    }

    fn piece(&self, kind: ShKind, bb: BBox, o: u8, depth: i32, door: Door) -> ShPiece {
        ShPiece {
            kind,
            bb,
            o,
            depth,
            door,
        }
    }

    fn from_weights(&mut self, p: [i32; 3], o: u8, depth: i32) -> Option<ShPiece> {
        if !self
            .weights
            .iter()
            .any(|w| w.limit > 0 && w.count < w.limit)
        {
            return None;
        }
        if let Some(t) = self.imposed.take() {
            if let Some(pc) = self.make(t, p, o, depth) {
                return Some(pc);
            }
        }
        for _ in 0..5 {
            let mut k = self.r.next_int_bounded(self.total);
            for i in 0..self.weights.len() {
                k -= self.weights[i].weight;
                if k < 0 {
                    let t = self.weights[i].t;
                    if !self.weights[i].can_place(depth) || Some(t) == self.previous {
                        break;
                    }
                    if let Some(pc) = self.make(t, p, o, depth) {
                        self.weights[i].count += 1;
                        self.previous = Some(t);
                        if t == T::PortalRoom {
                            self.portal = true;
                        }
                        if !self.weights[i].valid() {
                            self.total -= self.weights[i].weight;
                            self.weights.remove(i);
                        }
                        return Some(pc);
                    }
                }
            }
        }
        // filler corridor: up to 3 long, shortened until it no longer reaches the piece in the way
        let bb = orient_box(p, -1, -1, 0, 5, 5, 4, o);
        let hit = self.collides(&bb)?;
        if self.pieces[hit].bb.min[1] != bb.min[1] {
            return None;
        }
        // measured: the filler is one longer than the longest box (< 4) clear of that piece
        for len in (1..=2).rev() {
            let b = orient_box(p, -1, -1, 0, 5, 5, len, o);
            if !self.pieces[hit].bb.intersects(&b) {
                if b.min[1] > 1 {
                    return Some(self.piece(
                        ShKind::Filler { steps: len + 1 },
                        orient_box(p, -1, -1, 0, 5, 5, len + 1, o),
                        o,
                        depth,
                        Door::Opening,
                    ));
                }
                return None;
            }
        }
        None
    }

    fn child(&mut self, p: [i32; 3], o: u8, depth: i32) {
        if depth > 50 || (p[0] - self.sx).abs() > 112 || (p[2] - self.sz).abs() > 112 {
            return;
        }
        if let Some(pc) = self.from_weights(p, o, depth + 1) {
            self.pieces.push(pc);
            self.pending.push(self.pieces.len() - 1);
        }
    }

    fn fwd(&mut self, i: usize, ox: i32, oy: i32) {
        let (b, o, d) = (self.pieces[i].bb, self.pieces[i].o, self.pieces[i].depth);
        let p = match o {
            N => [b.min[0] + ox, b.min[1] + oy, b.min[2] - 1],
            S => [b.min[0] + ox, b.min[1] + oy, b.max[2] + 1],
            W => [b.min[0] - 1, b.min[1] + oy, b.min[2] + ox],
            _ => [b.max[0] + 1, b.min[1] + oy, b.min[2] + ox],
        };
        self.child(p, o, d);
    }

    fn left(&mut self, i: usize, oy: i32, oz: i32) {
        let (b, o, d) = (self.pieces[i].bb, self.pieces[i].o, self.pieces[i].depth);
        let (p, no) = match o {
            N | S => ([b.min[0] - 1, b.min[1] + oy, b.min[2] + oz], W),
            _ => ([b.min[0] + oz, b.min[1] + oy, b.min[2] - 1], N),
        };
        self.child(p, no, d);
    }

    fn right(&mut self, i: usize, oy: i32, oz: i32) {
        let (b, o, d) = (self.pieces[i].bb, self.pieces[i].o, self.pieces[i].depth);
        let (p, no) = match o {
            N | S => ([b.max[0] + 1, b.min[1] + oy, b.min[2] + oz], E),
            _ => ([b.min[0] + oz, b.min[1] + oy, b.max[2] + 1], S),
        };
        self.child(p, no, d);
    }

    fn children(&mut self, i: usize) {
        let o = self.pieces[i].o;
        match self.pieces[i].kind.clone() {
            ShKind::Start => {
                self.imposed = Some(T::FiveCrossing);
                self.fwd(i, 1, 1);
            }
            ShKind::StairsDown
            | ShKind::StraightStairsDown
            | ShKind::ChestCorridor
            | ShKind::PrisonHall => self.fwd(i, 1, 1),
            ShKind::Straight { left, right } => {
                self.fwd(i, 1, 1);
                if left {
                    self.left(i, 1, 2);
                }
                if right {
                    self.right(i, 1, 2);
                }
            }
            ShKind::LeftTurn => {
                if o == N || o == E {
                    self.left(i, 1, 1);
                } else {
                    self.right(i, 1, 1);
                }
            }
            ShKind::RightTurn => {
                if o == N || o == E {
                    self.right(i, 1, 1);
                } else {
                    self.left(i, 1, 1);
                }
            }
            ShKind::RoomCrossing { .. } => {
                self.fwd(i, 4, 1);
                self.left(i, 1, 4);
                self.right(i, 1, 4);
            }
            ShKind::FiveCrossing { ll, lh, rl, rh } => {
                let (lo, hi) = if o == W || o == N { (5, 3) } else { (3, 5) };
                self.fwd(i, 5, 1);
                if ll {
                    self.left(i, lo, 1);
                }
                if lh {
                    self.left(i, hi, 7);
                }
                if rl {
                    self.right(i, lo, 1);
                }
                if rh {
                    self.right(i, hi, 7);
                }
            }
            ShKind::Library { .. } | ShKind::PortalRoom | ShKind::Filler { .. } => {}
        }
    }
}

const DIRS: [u8; 4] = [N, E, S, W];

/// Generate the pieces of a stronghold starting in chunk (cx, cz) (start random).
pub fn stronghold(g: &crate::Generator, cx: i32, cz: i32, r: &mut JavaRandom) -> Vec<Piece> {
    let (x0, z0) = (cx * 16 + 2, cz * 16 + 2);
    let mut gen = Gen {
        r,
        pieces: Vec::new(),
        pending: Vec::new(),
        weights: weights(),
        total: 0,
        imposed: None,
        previous: None,
        sx: x0,
        sz: z0,
        portal: false,
    };
    // measured: a stronghold without a portal room is generated again with the start random
    // re-seeded from (world seed + attempt) (1 of 48 vanilla strongholds of seed 12345)
    for attempt in 0..64 {
        if attempt > 0 {
            crate::random::set_large_feature_seed(gen.r, g.seed + attempt as i64, cx, cz);
        }
        gen.pieces.clear();
        gen.pending.clear();
        gen.weights = weights();
        gen.total = gen.weights.iter().map(|w| w.weight).sum();
        gen.imposed = None;
        gen.previous = None;
        gen.portal = false;
        let o = DIRS[gen.r.next_int_bounded(4) as usize];
        let bb = BBox::new([x0, 64, z0], [x0 + 4, 74, z0 + 4]);
        gen.pieces.push(ShPiece {
            kind: ShKind::Start,
            bb,
            o,
            depth: 0,
            door: Door::Opening,
        });
        gen.children(0);
        while !gen.pending.is_empty() {
            let k = gen.r.next_int_bounded(gen.pending.len() as i32) as usize;
            let i = gen.pending.remove(k);
            gen.children(i);
        }
        if gen.portal {
            break;
        }
    }
    let ymin = gen.pieces.iter().map(|p| p.bb.min[1]).min().unwrap_or(0);
    let ymax = gen.pieces.iter().map(|p| p.bb.max[1]).max().unwrap_or(0);
    let top = g.sea_level() - 10;
    let mut j = ymax - ymin + 2 + crate::MIN_Y;
    if j < top {
        j += gen.r.next_int_bounded(top - j);
    }
    let dy = j - ymax;
    gen.pieces
        .into_iter()
        .map(|mut p| {
            p.bb = p.bb.moved([0, dy, 0]);
            Piece {
                bb: p.bb,
                kind: PieceKind::Stronghold { piece: p },
            }
        })
        .collect()
}

/// Place a stronghold piece into the region (clipped to `clip`).
pub fn place(
    _reg: &mut crate::region::Region,
    _clip: BBox,
    _p: &ShPiece,
    _r: &mut crate::features::R,
) {
}

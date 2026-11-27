//! Fluid ticking (M6): water and lava flow as scheduled block ticks.
//!
//! Model (behaviour measured on vanilla 26.2 with scenario m6_fluids, 2026-11-26):
//! * a fluid cell is a source (level 0), flowing (level 1..7, amount 8 - level) or falling
//!   (level 8, amount 8). Waterlogged blocks count as water sources.
//! * any block change schedules a fluid tick for the changed cell and its 6 neighbours that hold
//!   fluid: water after 5 ticks, lava after 30 (10 in the nether).
//! * on its tick a non-source cell is recomputed: falling if the same fluid is above, else the
//!   largest horizontal neighbour amount minus the drop-off (water 1, lava 2 / nether 1); water
//!   with 2+ horizontal source neighbours over a solid block or a source becomes a source; amount
//!   0 -> air.
//! * then it spreads: down into an empty / washable cell as falling fluid (and sideways too only
//!   with 3+ source neighbours), otherwise sideways at amount - drop-off, only towards the
//!   directions with the shortest path (up to 4 cells for water, 2 for lava / 4 in the nether)
//!   to a cell it could fall from, or every direction if none is that close.
//! * blocks without collision (torches, flowers, grass) are washed away with their drops; lava
//!   touching water turns into obsidian (source) or cobblestone (flowing) at once; lava flowing
//!   down into water makes stone.
use super::*;
use basalt_data::BlockState;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Fluid {
    Water,
    Lava,
}

/// (fluid, amount 1..=8, source, falling) of a cell.
#[derive(Clone, Copy, Debug)]
struct Cell {
    f: Fluid,
    amount: i32,
    source: bool,
    falling: bool,
}

fn cell_of(s: BlockState) -> Option<Cell> {
    let f = match s.name() {
        "water" | "bubble_column" => Fluid::Water,
        "lava" => Fluid::Lava,
        _ => {
            if s.get("waterlogged") == Some("true") {
                return Some(Cell {
                    f: Fluid::Water,
                    amount: 8,
                    source: true,
                    falling: false,
                });
            }
            return None;
        }
    };
    if s.name() == "bubble_column" {
        return Some(Cell {
            f,
            amount: 8,
            source: true,
            falling: false,
        });
    }
    let level: i32 = s.get("level").and_then(|v| v.parse().ok()).unwrap_or(0);
    Some(if level == 0 {
        Cell {
            f,
            amount: 8,
            source: true,
            falling: false,
        }
    } else if level >= 8 {
        Cell {
            f,
            amount: 8,
            source: false,
            falling: true,
        }
    } else {
        Cell {
            f,
            amount: 8 - level,
            source: false,
            falling: false,
        }
    })
}

fn state_for(f: Fluid, amount: i32, falling: bool) -> u32 {
    let name = match f {
        Fluid::Water => "water",
        Fluid::Lava => "lava",
    };
    let level = if falling { 8 } else { 8 - amount.clamp(1, 8) };
    BlockState::from_name(name)
        .and_then(|s| s.with("level", &level.to_string()))
        .map(|s| s.0)
        .unwrap_or(0)
}

const HORIZ: [usize; 4] = [2, 3, 4, 5];

fn opposite(d: usize) -> usize {
    d ^ 1
}

impl Game {
    fn nether(&self) -> bool {
        self.cur == dim::NETHER
    }

    fn delay(&self, f: Fluid) -> u64 {
        match f {
            Fluid::Water => 5,
            Fluid::Lava if self.nether() => 10,
            Fluid::Lava => 30,
        }
    }

    fn drop_off(&self, f: Fluid) -> i32 {
        match f {
            Fluid::Lava if !self.nether() => 2,
            _ => 1,
        }
    }

    fn slope_dist(&self, f: Fluid) -> i32 {
        match f {
            Fluid::Lava if !self.nether() => 2,
            _ => 4,
        }
    }

    /// Fluid can pass between `p` and its neighbour in direction `d` unless either block's shape covers
    /// the shared face (measured: waterlogged stairs facing north keep the water in on the north side).
    fn face_open(&self, p: [i32; 3], d: usize) -> bool {
        let q = step(p, d);
        let (a, b) = (
            self.world.get_block(p[0], p[1], p[2]),
            self.world.get_block(q[0], q[1], q[2]),
        );
        let shaped = |s: u32| BlockState(s).get("waterlogged").is_some();
        !(shaped(a) && face_full(a, d) || shaped(b) && face_full(b, opposite(d)))
    }

    fn fcell(&self, p: [i32; 3]) -> Option<Cell> {
        cell_of(BlockState(self.world.get_block(p[0], p[1], p[2])))
    }

    /// A block changed at `p` (old -> new): schedule fluid ticks around it, convert lava touching
    /// water. Called from set_block.
    pub(super) fn fluid_block_changed(&mut self, p: [i32; 3]) {
        // bubble columns: the cell itself and the one above may start / stop a column
        self.update_bubble_column(p);
        self.update_bubble_column(step(p, 1));
        for q in std::iter::once(p).chain((0..6).map(|d| step(p, d))) {
            if let Some(c) = self.fcell(q) {
                self.schedule_fluid(q, c.f);
                if c.f == Fluid::Lava {
                    self.lava_meets_water(q, c);
                }
            }
        }
    }

    /// What a water source / bubble column at `p` should be given the block below: soul sand -> column
    /// pushing up (drag=false), magma -> pulling down (drag=true), another column -> the same, else water.
    /// The whole contiguous run of water sources / columns above is converted at once (measured:
    /// m6_fluids bubble_soul_sand / bubble_magma / bubble_removed).
    fn update_bubble_column(&mut self, start: [i32; 3]) {
        let water_src = BlockState::from_name("water").map(|s| s.0).unwrap_or(0);
        let col = |drag: bool| {
            BlockState::from_name("bubble_column")
                .and_then(|s| s.with("drag", if drag { "true" } else { "false" }))
                .map(|s| s.0)
                .unwrap_or(0)
        };
        let mut p = start;
        for _ in 0..512 {
            if !self.world.layout.contains_y(p[1]) {
                return;
            }
            let here = BlockState(self.world.get_block(p[0], p[1], p[2]));
            let is_src = here.name() == "water" && here.get("level") == Some("0");
            if !is_src && here.name() != "bubble_column" {
                return;
            }
            let b = step(p, 0);
            let below = BlockState(self.world.get_block(b[0], b[1], b[2]));
            let want = match below.name() {
                "soul_sand" => col(false),
                "magma_block" => col(true),
                "bubble_column" => col(below.get("drag") == Some("true")),
                _ => water_src,
            };
            if want == here.0 {
                return;
            }
            // write directly (set_block would recurse into this walk); viewers get the update
            self.world.set_block(p[0], p[1], p[2], want);
            let (x, y, z) = (p[0], p[1], p[2]);
            self.broadcast_chunk((x >> 4, z >> 4), None, || {
                let mut w = Writer::with_id(cb::BLOCK_UPDATE);
                w.position(x, y, z).varint(want as i32);
                w
            });
            p = step(p, 1);
        }
    }

    fn schedule_fluid(&mut self, p: [i32; 3], f: Fluid) {
        if self.fluid_pending.insert(p) {
            let due = self.tick + self.delay(f);
            self.fluid_ticks.push((due, p));
        }
    }

    /// Lava with water beside or above it hardens: source -> obsidian, flowing -> cobblestone.
    fn lava_meets_water(&mut self, p: [i32; 3], c: Cell) {
        // basalt generator: lava over soul soil touching blue ice (measured: m6_fluids lava_basalt, the
        // flowing cell becomes basalt[axis=y] on the tick it fills)
        let below = step(p, 0);
        if BlockState(self.world.get_block(below[0], below[1], below[2])).name() == "soul_soil"
            && (1..6).any(|d| {
                let q = step(p, d);
                BlockState(self.world.get_block(q[0], q[1], q[2])).name() == "blue_ice"
            })
        {
            // the default basalt state is axis=y
            let Some(st) = BlockState::from_name("basalt").map(|s| s.0) else {
                return;
            };
            self.set_block(p[0], p[1], p[2], st);
            self.level_event_data(1501, p, 0, None);
            return;
        }
        let touching = (1..6).any(|d| self.fcell(step(p, d)).is_some_and(|n| n.f == Fluid::Water));
        if !touching {
            return;
        }
        let to = if c.source { "obsidian" } else { "cobblestone" };
        let st = BlockState::from_name(to).map(|s| s.0).unwrap_or(0);
        self.set_block(p[0], p[1], p[2], st);
        self.level_event_data(1501, p, 0, None);
    }

    pub(super) fn tick_fluids(&mut self) {
        let now = self.tick;
        if !self.fluid_ticks.iter().any(|(t, _)| *t <= now) {
            return;
        }
        let mut due = Vec::new();
        self.fluid_ticks.retain(|(t, p)| {
            if *t <= now {
                due.push(*p);
                false
            } else {
                true
            }
        });
        for p in &due {
            self.fluid_pending.remove(p);
        }
        for p in due {
            self.fluid_tick(p);
        }
    }

    fn fluid_tick(&mut self, p: [i32; 3]) {
        let Some(mut c) = self.fcell(p) else {
            return;
        };
        if !c.source {
            match self.new_cell(p, c.f) {
                None => {
                    self.set_block(p[0], p[1], p[2], 0);
                    return;
                }
                Some(n) => {
                    if n.amount != c.amount || n.falling != c.falling || n.source != c.source {
                        // a source is amount 8, not falling: level 0
                        let st = state_for(n.f, n.amount, n.falling && !n.source);
                        self.set_block(p[0], p[1], p[2], st);
                        c = n;
                    }
                }
            }
        }
        self.spread(p, c);
    }

    /// What a non-source cell of fluid `f` at `p` becomes (None = dries up).
    fn new_cell(&self, p: [i32; 3], f: Fluid) -> Option<Cell> {
        let mut best = 0;
        let mut sources = 0;
        for d in HORIZ {
            if !self.face_open(p, d) {
                continue;
            }
            if let Some(n) = self.fcell(step(p, d)) {
                if n.f == f {
                    if n.source {
                        sources += 1;
                    }
                    best = best.max(n.amount);
                }
            }
        }
        if f == Fluid::Water && sources >= 2 {
            let below = step(p, 0);
            let bs = self.world.get_block(below[0], below[1], below[2]);
            let solid = basalt_world::props::props(bs).blocks_motion();
            if solid || self.fcell(below).is_some_and(|b| b.f == f && b.source) {
                return Some(Cell {
                    f,
                    amount: 8,
                    source: true,
                    falling: false,
                });
            }
        }
        if self.face_open(p, 1) && self.fcell(step(p, 1)).is_some_and(|a| a.f == f) {
            return Some(Cell {
                f,
                amount: 8,
                source: false,
                falling: true,
            });
        }
        let a = best - self.drop_off(f);
        if a <= 0 {
            return None;
        }
        Some(Cell {
            f,
            amount: a,
            source: false,
            falling: false,
        })
    }

    /// Can fluid `f` flow into `q` (amount `a`)? Air, washable blocks, the same fluid with less.
    fn accepts(&self, q: [i32; 3], f: Fluid, a: i32, falling: bool) -> bool {
        if !self.world.layout.contains_y(q[1]) {
            return false;
        }
        let s = BlockState(self.world.get_block(q[0], q[1], q[2]));
        if s.is_air() {
            return true;
        }
        if let Some(c) = cell_of(s) {
            if c.f != f {
                // lava falling into water makes stone
                return f == Fluid::Lava && falling && c.f == Fluid::Water;
            }
            return !c.source && (falling && !c.falling || c.amount < a);
        }
        washable(s)
    }

    /// Could fluid fall from `q` (the cell below it accepts it)?
    fn hole_below(&self, q: [i32; 3], f: Fluid) -> bool {
        let b = step(q, 0);
        if self.accepts(b, f, 8, true) {
            return true;
        }
        self.fcell(b).is_some_and(|c| c.f == f)
    }

    fn spread(&mut self, p: [i32; 3], c: Cell) {
        let f = c.f;
        let below = step(p, 0);
        if self.face_open(p, 0) && self.accepts(below, f, 8, true) {
            self.flow_into(below, f, 8, true);
            let srcs = HORIZ
                .iter()
                .filter(|&&d| self.fcell(step(p, d)).is_some_and(|n| n.f == f && n.source))
                .count();
            if !(c.source && srcs >= 3) {
                return;
            }
        } else if !c.source && self.hole_below(p, f) {
            // flowing over the same fluid (a filled hole): no sideways spread (water_hole, water_pillar)
            return;
        }
        let a = c.amount - self.drop_off(f);
        if a <= 0 {
            return;
        }
        let limit = self.slope_dist(f);
        let mut best = i32::MAX;
        let mut dirs: Vec<(usize, i32)> = Vec::new();
        for d in HORIZ {
            if !self.face_open(p, d) {
                continue;
            }
            let q = step(p, d);
            // directions: cells it can flow into, or already holding the same flowing fluid (a
            // filled path towards a hole keeps the other directions dry: water_hole)
            let same = self.fcell(q).is_some_and(|n| n.f == f && !n.source);
            if !self.accepts(q, f, a, false) && !same {
                continue;
            }
            let dist = if self.hole_below(q, f) {
                0
            } else {
                self.slope(q, f, 1, opposite(d), limit)
            };
            best = best.min(dist);
            dirs.push((d, dist));
        }
        for (d, dist) in dirs {
            if dist == best && self.accepts(step(p, d), f, a, false) {
                self.flow_into(step(p, d), f, a, false);
            }
        }
    }

    /// Shortest number of steps from `q` (depth `depth`) to a cell with a hole below, within `limit`;
    /// 1000 when none.
    fn slope(&self, q: [i32; 3], f: Fluid, depth: i32, from: usize, limit: i32) -> i32 {
        let mut best = 1000;
        for d in HORIZ {
            if d == from {
                continue;
            }
            let n = step(q, d);
            if !self.accepts(n, f, 1, false) && self.fcell(n).is_none_or(|c| c.f != f || c.source) {
                continue;
            }
            if self.hole_below(n, f) {
                return depth;
            }
            if depth < limit {
                best = best.min(self.slope(n, f, depth + 1, opposite(d), limit));
            }
        }
        best
    }

    fn flow_into(&mut self, q: [i32; 3], f: Fluid, a: i32, falling: bool) {
        let s = BlockState(self.world.get_block(q[0], q[1], q[2]));
        if let Some(c) = cell_of(s) {
            if c.f != f {
                // lava into water
                let st = BlockState::from_name("stone").map(|s| s.0).unwrap_or(0);
                self.set_block(q[0], q[1], q[2], st);
                self.level_event_data(1501, q, 0, None);
                return;
            }
        } else if !s.is_air() {
            let drops = self.tool_less_drops(s, q);
            self.set_block(q[0], q[1], q[2], 0);
            self.drop_at_block(q, drops);
        }
        self.set_block(q[0], q[1], q[2], state_for(f, a, falling));
    }
}

/// Does the collision shape of `state` fully cover its face towards direction `d` (0 down 1 up 2 north
/// 3 south 4 west 5 east)? Sampled on an 8x8 grid (shapes here sit on 1/4 boundaries).
fn face_full(state: u32, d: usize) -> bool {
    if state == 0 {
        return false;
    }
    let mut boxes = Vec::new();
    super::mob::block_boxes(state, &mut boxes);
    if boxes.is_empty() {
        return false;
    }
    let (axis, at_max) = match d {
        0 => (1, false),
        1 => (1, true),
        2 => (2, false),
        3 => (2, true),
        4 => (0, false),
        _ => (0, true),
    };
    let (u, v) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    let touches = |b: &[f64; 6]| {
        if at_max {
            b[axis + 3] >= 1.0 - 1e-6
        } else {
            b[axis] <= 1e-6
        }
    };
    (0..8).all(|i| {
        (0..8).all(|j| {
            let (pu, pv) = ((i as f64 + 0.5) / 8.0, (j as f64 + 0.5) / 8.0);
            boxes
                .iter()
                .any(|b| touches(b) && b[u] <= pu && pu <= b[u + 3] && b[v] <= pv && pv <= b[v + 3])
        })
    })
}

fn step(p: [i32; 3], d: usize) -> [i32; 3] {
    [p[0] + FACE[d][0], p[1] + FACE[d][1], p[2] + FACE[d][2]]
}

/// Blocks a fluid washes away (no collision, not a door/sign/ladder/portal/waterloggable).
fn washable(s: BlockState) -> bool {
    if basalt_world::props::props(s.0).blocks_motion() || s.get("waterlogged").is_some() {
        return false;
    }
    let n = s.name();
    !(n.ends_with("_door")
        || n.ends_with("_sign")
        || n.ends_with("_banner")
        || matches!(
            n,
            "ladder"
                | "sugar_cane"
                | "nether_portal"
                | "end_portal"
                | "end_gateway"
                | "structure_void"
                | "bubble_column"
                | "kelp"
                | "kelp_plant"
                | "seagrass"
                | "tall_seagrass"
                | "light"
                | "barrier"
                | "powder_snow"
                | "cobweb"
        ))
}

impl Game {
    /// Horizontal flow direction of the fluid in cell `p` (unit vector or zero): towards lower
    /// neighbours (air beside counts as empty), like the current that pushes entities.
    pub(super) fn flow_at(&self, p: [i32; 3]) -> [f64; 3] {
        let Some(c) = self.fcell(p) else {
            return [0.0; 3];
        };
        let h = |c: &Cell| c.amount as f64 / 9.0;
        let own = h(&c);
        let mut v = [0.0f64; 3];
        for d in HORIZ {
            let q = step(p, d);
            let nh = match self.fcell(q) {
                Some(n) if n.f == c.f => h(&n),
                Some(_) => continue,
                None => {
                    let s = self.world.get_block(q[0], q[1], q[2]);
                    if basalt_world::props::props(s).blocks_motion() {
                        continue;
                    }
                    0.0
                }
            };
            let diff = own - nh;
            v[0] += FACE[d][0] as f64 * diff;
            v[2] += FACE[d][2] as f64 * diff;
        }
        let l = (v[0] * v[0] + v[2] * v[2]).sqrt();
        if l < 1e-9 {
            return [0.0; 3];
        }
        [v[0] / l, 0.0, v[2] / l]
    }
}

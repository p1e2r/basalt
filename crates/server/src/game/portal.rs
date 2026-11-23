//! Nether portals: frame detection, lighting, breaking, travel (8:1), finding or building the
//! exit portal.
//!
//! Rules (vanilla 26.2 behaviour, checked with the m8_* scenarios against the real server):
//! * frame = obsidian; the inside may hold air / fire / nether_portal; inside width 2..=21,
//!   height 3..=21. Fire placed inside an empty frame (flint and steel, `/setblock`, ...) in
//!   the overworld or nether fills it with `nether_portal[axis=..]`, trying axis x first.
//! * a portal block whose in-plane neighbour changes turns to air unless its frame is still a
//!   complete portal (breaking any frame block removes the portal).
//! * standing in a portal for 80 ticks (creative: 1) moves a player to the other dimension at
//!   x/8, z/8 (nether -> overworld x*8); the exit is the closest portal block within 16 (to the
//!   nether) / 128 (to the overworld) blocks horizontally, else a new 2x3 portal (with an
//!   obsidian platform if no spot fits) found by vanilla's spiral search.
//! * the relative position inside the portal is kept; a player on portal cooldown that stays
//!   inside a portal keeps renewing the cooldown (no bounce-back until they step out).
use super::dim::{DIMS, NETHER, OVERWORLD};
use super::*;

/// Ticks a non-creative player must stand in a portal (gamerule players_nether_portal_default_delay).
pub(super) const PORTAL_DELAY: i32 = 80;
/// Creative players (players_nether_portal_creative_delay 0: they travel on the first tick inside;
/// the counter is compared before it is incremented, so survival travels on the 81st tick).
pub(super) const PORTAL_DELAY_CREATIVE: i32 = 0;
/// Player portal cooldown after a dimension change (ticks).
pub(super) const PORTAL_COOLDOWN: i32 = 10;

/// A portal frame candidate (vanilla's PortalShape).
#[derive(Clone, Copy, Debug)]
pub(super) struct Shape {
    pub axis_x: bool,
    pub bottom_left: [i32; 3],
    pub width: i32,
    pub height: i32,
    pub portal_blocks: i32,
}

impl Shape {
    pub fn valid(&self) -> bool {
        (2..=21).contains(&self.width) && (3..=21).contains(&self.height)
    }
    pub fn complete(&self) -> bool {
        self.valid() && self.portal_blocks == self.width * self.height
    }
    /// Direction along the portal from the bottom-left corner (x axis: west, z axis: south).
    fn right(&self) -> [i32; 3] {
        right_dir(self.axis_x)
    }
}

fn right_dir(axis_x: bool) -> [i32; 3] {
    if axis_x {
        [-1, 0, 0]
    } else {
        [0, 0, 1]
    }
}
fn add(a: [i32; 3], d: [i32; 3], k: i32) -> [i32; 3] {
    [a[0] + d[0] * k, a[1] + d[1] * k, a[2] + d[2] * k]
}

fn is_empty(s: BlockState) -> bool {
    s.is_air() || matches!(s.name(), "fire" | "soul_fire" | "nether_portal")
}
fn is_frame(s: BlockState) -> bool {
    s.name() == "obsidian"
}

/// Rectangle of a portal: min corner, width along the axis, height.
#[derive(Clone, Copy, Debug)]
pub(super) struct Rect {
    pub min: [i32; 3],
    pub width: i32,
    pub height: i32,
    pub axis_x: bool,
}

impl Game {
    fn pbs(&self, p: [i32; 3]) -> BlockState {
        BlockState(self.world.get_block(p[0], p[1], p[2]))
    }

    /// Shape of the frame around `pos` for one axis (always returned; check `valid()`).
    pub(super) fn portal_shape(&self, pos: [i32; 3], axis_x: bool) -> Shape {
        let right = right_dir(axis_x);
        let left = [-right[0], -right[1], -right[2]];
        let mut shape = Shape {
            axis_x,
            bottom_left: pos,
            width: 0,
            height: 0,
            portal_blocks: 0,
        };
        // bottom-left corner
        let lo = self.world.layout.min_y().max(pos[1] - 21);
        let mut p = pos;
        while p[1] > lo && is_empty(self.pbs([p[0], p[1] - 1, p[2]])) {
            p[1] -= 1;
        }
        let j = self.edge_distance(p, left) - 1;
        if j < 0 {
            return shape;
        }
        let bl = add(p, left, j);
        shape.bottom_left = bl;
        let w = self.edge_distance(bl, right);
        if !(2..=21).contains(&w) {
            return shape;
        }
        shape.width = w;
        let mut n = 0;
        let h = self.distance_to_top(bl, right, w, &mut n);
        shape.portal_blocks = n;
        let top_ok = (0..w).all(|i| is_frame(self.pbs(add([bl[0], bl[1] + h, bl[2]], right, i))));
        shape.height = if (3..=21).contains(&h) && top_ok {
            h
        } else {
            0
        };
        shape
    }

    /// Steps from `p` along `dir` over empty blocks standing on frame blocks until a frame block.
    fn edge_distance(&self, p: [i32; 3], dir: [i32; 3]) -> i32 {
        for i in 0..=21 {
            let q = add(p, dir, i);
            let s = self.pbs(q);
            if !is_empty(s) {
                return if is_frame(s) { i } else { 0 };
            }
            if !is_frame(self.pbs([q[0], q[1] - 1, q[2]])) {
                break;
            }
        }
        0
    }

    fn distance_to_top(&self, bl: [i32; 3], right: [i32; 3], width: i32, n: &mut i32) -> i32 {
        for h in 0..21 {
            let row = [bl[0], bl[1] + h, bl[2]];
            if !is_frame(self.pbs(add(row, right, -1)))
                || !is_frame(self.pbs(add(row, right, width)))
            {
                return h;
            }
            for i in 0..width {
                let s = self.pbs(add(row, right, i));
                if !is_empty(s) {
                    return h;
                }
                if s.name() == "nether_portal" {
                    *n += 1;
                }
            }
        }
        21
    }

    /// An empty valid frame around `pos` (axis x first, then z).
    pub(super) fn find_empty_portal(&self, pos: [i32; 3]) -> Option<Shape> {
        [true, false]
            .into_iter()
            .map(|ax| self.portal_shape(pos, ax))
            .find(|s| s.valid() && s.portal_blocks == 0)
    }

    /// Fill a frame with portal blocks (no neighbour updates, like vanilla's flag 18).
    pub(super) fn create_portal_blocks(&mut self, s: Shape) {
        let st = block_state_str(if s.axis_x {
            "nether_portal[axis=x]"
        } else {
            "nether_portal[axis=z]"
        });
        let right = s.right();
        self.portal_quiet = true;
        for h in 0..s.height {
            for i in 0..s.width {
                let q = add(
                    [s.bottom_left[0], s.bottom_left[1] + h, s.bottom_left[2]],
                    right,
                    i,
                );
                self.set_block(q[0], q[1], q[2], st);
            }
        }
        self.portal_quiet = false;
    }

    /// Hook from `set_block`: fire lights frames, changed blocks break portals.
    pub(super) fn portal_block_changed(&mut self, p: [i32; 3], old: u32, new: u32) {
        if self.portal_quiet {
            return;
        }
        let (on, nn) = (BlockState(old).name(), BlockState(new).name());
        if self.portals_enabled() && matches!(nn, "fire" | "soul_fire") && on != nn {
            if let Some(s) = self.find_empty_portal(p) {
                self.create_portal_blocks(s);
                return;
            }
        }
        // in-plane neighbours of a changed block: portal blocks check their frame
        for d in FACE {
            let q = add(p, d, 1);
            let s = self.pbs(q);
            if s.name() != "nether_portal" {
                continue;
            }
            let axis_x = s.get("axis") == Some("x");
            // a change across the portal's thin side doesn't matter
            if (axis_x && d[2] != 0) || (!axis_x && d[0] != 0) {
                continue;
            }
            if nn == "nether_portal" && d[1] == 0 {
                continue;
            }
            if !self.portal_shape(q, axis_x).complete() {
                self.remove_portal(q, axis_x);
            }
        }
    }

    /// The overworld and the nether have portals (vanilla: not the end).
    fn portals_enabled(&self) -> bool {
        self.cur == OVERWORLD || self.cur == NETHER
    }

    /// Remove every connected portal block of one portal (vanilla does it through neighbour
    /// updates one block at a time; the end result is the same).
    fn remove_portal(&mut self, start: [i32; 3], axis_x: bool) {
        let right = right_dir(axis_x);
        let mut todo = vec![start];
        let mut seen = HashSet::new();
        self.portal_quiet = true;
        while let Some(q) = todo.pop() {
            if !seen.insert(q) || seen.len() > 21 * 21 {
                continue;
            }
            let s = self.pbs(q);
            if s.name() != "nether_portal" || (s.get("axis") == Some("x")) != axis_x {
                continue;
            }
            self.set_block(q[0], q[1], q[2], 0);
            for d in [[0, 1, 0], [0, -1, 0], right, [-right[0], 0, -right[2]]] {
                todo.push(add(q, d, 1));
            }
        }
        self.portal_quiet = false;
    }

    /// Flint and steel used on a block face: fire on the adjacent block (lighting a frame).
    pub(super) fn use_flint_and_steel(&mut self, eid: i32, b: [i32; 3], face: usize) {
        let t = add(b, FACE[face], 1);
        if !self.world.layout.contains_y(t[1]) {
            return;
        }
        if !self.pbs(t).is_air() {
            return;
        }
        let below = self.pbs([t[0], t[1] - 1, t[2]]);
        let soul = matches!(below.name(), "soul_sand" | "soul_soil");
        let supported = basalt_world::props::props(below.0).blocks_motion();
        let in_frame = self.portals_enabled() && self.find_empty_portal(t).is_some();
        if !supported && !in_frame {
            return;
        }
        let fire = block_state_str(if soul { "soul_fire" } else { "fire" });
        self.set_block(t[0], t[1], t[2], fire);
        // vanilla plays flintandsteel.use and wears the tool by 1 (survival)
        if self
            .players
            .get(&eid)
            .map(|p| p.gamemode != 1)
            .unwrap_or(false)
        {
            self.wear_held(eid, 1);
        }
    }

    /// The portal block the player's box touches (the thin slab of the block, 6/16..10/16 across).
    fn portal_touched(&self, p: &Player) -> Option<[i32; 3]> {
        let (lo, hi) = (
            [p.pos[0] - 0.3, p.pos[1], p.pos[2] - 0.3],
            [p.pos[0] + 0.3, p.pos[1] + 1.8, p.pos[2] + 0.3],
        );
        for x in (lo[0] + 1e-7).floor() as i32..=(hi[0] - 1e-7).floor() as i32 {
            for y in (lo[1] + 1e-7).floor() as i32..=(hi[1] - 1e-7).floor() as i32 {
                for z in (lo[2] + 1e-7).floor() as i32..=(hi[2] - 1e-7).floor() as i32 {
                    let s = self.pbs([x, y, z]);
                    if s.name() != "nether_portal" {
                        continue;
                    }
                    let (a, b2) = if s.get("axis") == Some("x") {
                        (z as f64 + 0.375, z as f64 + 0.625)
                    } else {
                        (x as f64 + 0.375, x as f64 + 0.625)
                    };
                    let (l, h) = if s.get("axis") == Some("x") {
                        (lo[2], hi[2])
                    } else {
                        (lo[0], hi[0])
                    };
                    if l < b2 && h > a {
                        return Some([x, y, z]);
                    }
                }
            }
        }
        None
    }

    /// Per tick: portal timers of the current dimension's players; travel when due.
    pub(super) fn tick_portals(&mut self) {
        if !self.portals_enabled() {
            return;
        }
        let ids: Vec<i32> = self.players.keys().copied().collect();
        let mut travel = Vec::new();
        for eid in ids {
            let touched = {
                let p = &self.players[&eid];
                if p.dead || p.gamemode == 3 {
                    None
                } else {
                    self.portal_touched(p)
                }
            };
            let p = self.players.get_mut(&eid).unwrap();
            if p.portal_cooldown > 0 {
                p.portal_cooldown -= 1;
            }
            match touched {
                Some(_) if p.portal_cooldown > 0 => {
                    p.portal_cooldown = PORTAL_COOLDOWN;
                    p.portal_ticks = 0;
                }
                Some(b) => {
                    p.portal_entry = Some(b);
                    let delay = if p.gamemode == 1 {
                        PORTAL_DELAY_CREATIVE
                    } else {
                        PORTAL_DELAY
                    };
                    let t = p.portal_ticks;
                    p.portal_ticks += 1;
                    if t >= delay {
                        p.portal_ticks = 0;
                        travel.push((eid, b));
                    }
                }
                None => {
                    p.portal_ticks = (p.portal_ticks - 4).max(0);
                }
            }
        }
        for (eid, b) in travel {
            self.portal_travel(eid, b);
        }
    }

    /// Bounding rectangle of the connected portal blocks around `p` (same axis).
    fn portal_rect(&self, p: [i32; 3], axis_x: bool) -> Rect {
        let right = right_dir(axis_x);
        let same = |g: &Game, q: [i32; 3]| {
            let s = g.pbs(q);
            s.name() == "nether_portal" && (s.get("axis") == Some("x")) == axis_x
        };
        let mut lo = p;
        while lo[1] > p[1] - 21 && same(self, [lo[0], lo[1] - 1, lo[2]]) {
            lo[1] -= 1;
        }
        // along the axis toward the smaller coordinate
        let neg = if axis_x { [-1, 0, 0] } else { [0, 0, -1] };
        while (if axis_x { p[0] - lo[0] } else { p[2] - lo[2] }) < 21 && same(self, add(lo, neg, 1))
        {
            lo = add(lo, neg, 1);
        }
        let pos_dir = [-neg[0], 0, -neg[2]];
        let mut w = 1;
        while w < 21 && same(self, add(lo, pos_dir, w)) {
            w += 1;
        }
        let mut h = 1;
        while h < 21 && same(self, [lo[0], lo[1] + h, lo[2]]) {
            h += 1;
        }
        let _ = right;
        Rect {
            min: lo,
            width: w,
            height: h,
            axis_x,
        }
    }

    /// Closest portal block to `target` within `radius` (square) in the current dimension's
    /// portal registry; ties: lower y.
    fn closest_portal(&self, target: [i32; 3], radius: i32) -> Option<[i32; 3]> {
        self.portals
            .iter()
            .copied()
            .filter(|p| (p[0] - target[0]).abs() <= radius && (p[2] - target[2]).abs() <= radius)
            .min_by_key(|p| {
                let d = [
                    (p[0] - target[0]) as i64,
                    (p[1] - target[1]) as i64,
                    (p[2] - target[2]) as i64,
                ];
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2], p[1])
            })
    }

    fn can_portal_replace(&self, q: [i32; 3]) -> bool {
        let s = self.pbs(q);
        (s.is_air() || self.replaceable_for(s)) && !basalt_world::props::props(s.0).has_fluid()
    }

    /// The 4x5 frame (with `offset` sideways) at `origin` fits: solid below, replaceable above.
    fn can_host_frame(&self, origin: [i32; 3], dir: [i32; 3], offset: i32) -> bool {
        let cw = [-dir[2], 0, dir[0]]; // clockwise of dir
        for i in -1..3 {
            for j in -1..4 {
                let q = [
                    origin[0] + dir[0] * i + cw[0] * offset,
                    origin[1] + j,
                    origin[2] + dir[2] * i + cw[2] * offset,
                ];
                if j < 0
                    && !basalt_world::props::props(self.world.get_block(q[0], q[1], q[2]))
                        .blocks_motion()
                {
                    return false;
                }
                if j >= 0 && !self.can_portal_replace(q) {
                    return false;
                }
            }
        }
        true
    }

    /// Build a new 2x3 portal near `target` in the current dimension (vanilla's PortalForcer).
    fn build_exit_portal(&mut self, target: [i32; 3], axis_x: bool) -> Option<Rect> {
        let dir = if axis_x { [1, 0, 0] } else { [0, 0, 1] };
        let l = self.world.layout;
        let logical = if self.cur == NETHER {
            128
        } else {
            l.max_y() - l.min_y()
        };
        let top = (l.max_y() - 1).min(l.min_y() + logical - 1);
        for cx in ((target[0] - 17) >> 4)..=((target[0] + 17) >> 4) {
            for cz in ((target[2] - 17) >> 4)..=((target[2] + 17) >> 4) {
                self.ensure_chunk(cx, cz);
            }
        }
        let (mut best, mut best_pos) = (-1f64, None::<[i32; 3]>);
        let (mut best2, mut best_pos2) = (-1f64, None::<[i32; 3]>);
        for (x, z) in spiral(target[0], target[2], 16) {
            let hm = self
                .world
                .chunk(x >> 4, z >> 4)
                .map(|c| {
                    c.heightmaps.motion_blocking[((z & 15) * 16 + (x & 15)) as usize] as i32
                        + basalt_world::MIN_Y
                })
                .unwrap_or(l.min_y());
            let j = top.min(hm);
            let (bx, bz) = (x - dir[0], z - dir[2]);
            let mut k = j;
            while k >= l.min_y() {
                if !self.can_portal_replace([bx, k, bz]) {
                    k -= 1;
                    continue;
                }
                let l1 = k;
                while k > l.min_y() && self.can_portal_replace([bx, k - 1, bz]) {
                    k -= 1;
                }
                if k + 4 > top {
                    k -= 1;
                    continue;
                }
                let m1 = l1 - k;
                if m1 > 0 && m1 < 3 {
                    k -= 1;
                    continue;
                }
                let o = [bx, k, bz];
                if self.can_host_frame(o, dir, 0) {
                    let d = {
                        let a = [
                            (o[0] - target[0]) as f64,
                            (o[1] - target[1]) as f64,
                            (o[2] - target[2]) as f64,
                        ];
                        a[0] * a[0] + a[1] * a[1] + a[2] * a[2]
                    };
                    if self.can_host_frame(o, dir, -1)
                        && self.can_host_frame(o, dir, 1)
                        && (best == -1.0 || best > d)
                    {
                        best = d;
                        best_pos = Some(o);
                    }
                    if best == -1.0 && (best2 == -1.0 || best2 > d) {
                        best2 = d;
                        best_pos2 = Some(o);
                    }
                }
                k -= 1;
            }
        }
        if best == -1.0 && best2 != -1.0 {
            best_pos = best_pos2;
            best = best2;
        }
        let cw = [-dir[2], 0, dir[0]];
        let obsidian = block_state_str("obsidian");
        let origin = if best == -1.0 {
            let lo = (l.min_y() + 1).max(70);
            let hi = top - 9;
            if hi < lo {
                return None;
            }
            let o = [target[0], target[1].clamp(lo, hi), target[2]];
            // obsidian platform with air above
            for i1 in -1..2 {
                for j1 in 0..2 {
                    for k1 in -1..3 {
                        let q = [
                            o[0] + j1 * dir[0] + i1 * cw[0],
                            o[1] + k1,
                            o[2] + j1 * dir[2] + i1 * cw[2],
                        ];
                        self.set_block(q[0], q[1], q[2], if k1 < 0 { obsidian } else { 0 });
                    }
                }
            }
            o
        } else {
            best_pos?
        };
        self.portal_quiet = true;
        for l1 in -1..3 {
            for i2 in -1..4 {
                if l1 == -1 || l1 == 2 || i2 == -1 || i2 == 3 {
                    let q = [
                        origin[0] + dir[0] * l1,
                        origin[1] + i2,
                        origin[2] + dir[2] * l1,
                    ];
                    self.set_block(q[0], q[1], q[2], obsidian);
                }
            }
        }
        let st = block_state_str(if axis_x {
            "nether_portal[axis=x]"
        } else {
            "nether_portal[axis=z]"
        });
        for j2 in 0..2 {
            for k2 in 0..3 {
                let q = [
                    origin[0] + dir[0] * j2,
                    origin[1] + k2,
                    origin[2] + dir[2] * j2,
                ];
                self.set_block(q[0], q[1], q[2], st);
            }
        }
        self.portal_quiet = false;
        Some(Rect {
            min: origin,
            width: 2,
            height: 3,
            axis_x,
        })
    }

    /// Move a player through the portal block `entry`.
    pub(super) fn portal_travel(&mut self, eid: i32, entry: [i32; 3]) {
        let from = self.cur;
        let to = if from == NETHER { OVERWORLD } else { NETHER };
        let Some(p) = self.players.get(&eid) else {
            return;
        };
        let (pos, yaw, pitch) = (p.pos, p.yaw, p.pitch);
        let axis_x = self.pbs(entry).get("axis") != Some("z");
        let src = self.portal_rect(entry, axis_x);
        // relative position inside the source portal (vanilla's getRelativePosition, player 0.6 x 1.8)
        let (pw, ph) = (0.6, 1.8);
        let along = |v: [f64; 3], ax: bool| if ax { v[0] } else { v[2] };
        let across = |v: [f64; 3], ax: bool| if ax { v[2] } else { v[0] };
        let minf = [src.min[0] as f64, src.min[1] as f64, src.min[2] as f64];
        let w = src.width as f64 - pw;
        let h = src.height as f64 - ph;
        let rx = if w > 0.0 {
            ((along(pos, axis_x) - (along(minf, axis_x) + pw / 2.0)) / w).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let ry = if h > 0.0 {
            ((pos[1] - minf[1]) / h).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let rz = across(pos, axis_x) - (across(minf, axis_x) + 0.5);
        let scale = DIMS[from].scale / DIMS[to].scale;
        let target = [
            (pos[0] * scale).floor() as i32,
            pos[1].floor() as i32,
            (pos[2] * scale).floor() as i32,
        ];
        self.enter(to);
        let radius = if to == NETHER { 16 } else { 128 };
        let rect = match self.closest_portal(target, radius) {
            Some(b) => {
                self.ensure_chunk(b[0] >> 4, b[2] >> 4);
                let ax = self.pbs(b).get("axis") != Some("z");
                Some(self.portal_rect(b, ax))
            }
            None => self.build_exit_portal(target, axis_x),
        };
        let Some(rect) = rect else {
            self.enter(from);
            return;
        };
        let rot = if rect.axis_x == axis_x { 0.0 } else { 90.0 };
        let d2 = pw / 2.0 + (rect.width as f64 - pw) * rx;
        let d3 = (rect.height as f64 - ph) * ry;
        // measured (m8_portal_travel vs vanilla): the exit is centred across the portal
        let _ = rz;
        let d4 = 0.5;
        let m = rect.min;
        let exit = if rect.axis_x {
            [m[0] as f64 + d2, m[1] as f64 + d3, m[2] as f64 + d4]
        } else {
            [m[0] as f64 + d4, m[1] as f64 + d3, m[2] as f64 + d2]
        };
        self.enter(from);
        self.change_dimension(eid, to, exit, yaw + rot, pitch);
        if let Some(p) = self.players.get_mut(&eid) {
            p.portal_cooldown = PORTAL_COOLDOWN;
            p.portal_ticks = 0;
            // level event 1032: the portal travel sound (vanilla sends it to the travelling player)
            let mut w = Writer::with_id(cb::LEVEL_EVENT);
            w.i32(1032).position(0, 0, 0).i32(0).bool(false);
            p.send(w);
        }
    }

    /// Keep the current dimension's portal registry in sync with a block change.
    pub(super) fn portal_registry(&mut self, p: [i32; 3], new: u32) {
        if BlockState(new).name() == "nether_portal" {
            self.portals.insert(p);
        } else {
            self.portals.remove(&p);
        }
    }

    pub(super) fn save_portals(&self) {
        let Some(d) = &self.dir else { return };
        let path = d.join(DIMS[self.cur].portals);
        if let Some(p) = path.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        let v: Vec<_> = self.portals.iter().collect();
        let _ = std::fs::write(path, serde_json::json!(v).to_string());
    }

    pub(super) fn load_portals(&mut self) {
        let Some(d) = &self.dir else { return };
        let Ok(s) = std::fs::read_to_string(d.join(DIMS[self.cur].portals)) else {
            return;
        };
        if let Ok(v) = serde_json::from_str::<Vec<[i32; 3]>>(&s) {
            self.portals = v.into_iter().collect();
        }
    }
}

/// Columns around (x, z) in vanilla's spiral order (BlockPos.spiralAround with east, south):
/// the centre, then legs east 1, south 1, west 2, north 2, east 3, ... up to `radius`.
pub(super) fn spiral(x: i32, z: i32, radius: i32) -> Vec<(i32, i32)> {
    let dirs = [(1, 0), (0, 1), (-1, 0), (0, -1)];
    let mut out = vec![(x, z)];
    let (mut cx, mut cz) = (x, z);
    for leg in 0..4 * radius {
        let (dx, dz) = dirs[(leg % 4) as usize];
        for _ in 0..leg / 2 + 1 {
            cx += dx;
            cz += dz;
            out.push((cx, cz));
        }
    }
    out
}

/// Block state id from `name[prop=value,...]`.
fn block_state_str(s: &str) -> u32 {
    BlockState::parse(s).map(|b| b.0).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spiral_order() {
        let s = spiral(0, 0, 2);
        assert_eq!(&s[..6], &[(0, 0), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0)]);
        assert_eq!(
            s.len(),
            1 + (1..=8).map(|l| (l - 1) / 2 + 1).sum::<i32>() as usize
        );
    }
}

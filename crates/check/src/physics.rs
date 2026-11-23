//! Player movement good enough for a vanilla server's movement checks: gravity 0.08 / drag 0.98,
//! ground friction, swept AABB collision against block boxes, step-up, sneaking edge stop,
//! simple water physics. Same structure as crates/client/src/player.rs but GPU/asset-free.
use crate::world::World;
use basalt_data::BlockState;

pub const WIDTH: f64 = 0.6;
pub const HEIGHT: f64 = 1.8;
type Aabb = [f64; 6];

#[derive(Default, Clone, Copy, Debug)]
pub struct Input { pub forward: f32, pub strafe: f32, pub jump: bool, pub sneak: bool, pub sprint: bool }

#[derive(Clone, Debug)]
pub struct Body {
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub h_collide: bool,
    pub flying: bool,
    pub in_water: bool,
    pub sneaking: bool,
}

impl Default for Body {
    fn default() -> Self { Body { pos: [0.0; 3], vel: [0.0; 3], yaw: 0.0, pitch: 0.0, on_ground: false, h_collide: false, flying: false, in_water: false, sneaking: false } }
}

/// Collision boxes of a block state in block-local coordinates.
pub fn shape(state: u32) -> Vec<Aabb> {
    if state == 0 { return Vec::new(); }
    let st = BlockState(state);
    let (name, kind) = (st.name(), st.block().kind);
    if !basalt_world::props::props(state).blocks_motion() || matches!(name, "powder_snow" | "cobweb" | "water" | "lava" | "bubble_column") { return Vec::new(); }
    let full = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    let h = |y: f64| vec![[0.0, 0.0, 0.0, 1.0, y, 1.0]];
    match name {
        "cactus" => return vec![[1.0 / 16.0, 0.0, 1.0 / 16.0, 15.0 / 16.0, 15.0 / 16.0, 15.0 / 16.0]],
        "farmland" | "dirt_path" => return h(15.0 / 16.0),
        "soul_sand" | "mud" => return h(14.0 / 16.0),
        "honey_block" => return vec![[1.0 / 16.0, 0.0, 1.0 / 16.0, 15.0 / 16.0, 15.0 / 16.0, 15.0 / 16.0]],
        "enchanting_table" => return h(0.75),
        "snow" => { let l: f64 = st.get("layers").and_then(|v| v.parse().ok()).unwrap_or(1.0); return if l <= 1.0 { Vec::new() } else { h((l - 1.0) / 8.0) }; }
        _ => {}
    }
    match kind {
        "slab" => match st.get("type") { Some("top") => vec![[0.0, 0.5, 0.0, 1.0, 1.0, 1.0]], Some("double") => vec![full], _ => h(0.5) },
        "carpet" | "wool_carpet" => h(1.0 / 16.0),
        "bed" => h(9.0 / 16.0),
        "chest" | "ender_chest" | "trapped_chest" => vec![[1.0 / 16.0, 0.0, 1.0 / 16.0, 15.0 / 16.0, 14.0 / 16.0, 15.0 / 16.0]],
        "fence" | "wall" => h(1.5),
        "fence_gate" => if st.get("open") == Some("true") { Vec::new() } else { h(1.5) },
        _ => vec![full],
    }
}

fn collect(w: &World, bb: &Aabb, out: &mut Vec<Aabb>) {
    out.clear();
    for x in (bb[0].floor() as i32 - 1)..=(bb[3].floor() as i32 + 1) {
        for y in (bb[1].floor() as i32 - 1)..=(bb[4].floor() as i32 + 1) {
            for z in (bb[2].floor() as i32 - 1)..=(bb[5].floor() as i32 + 1) {
                for b in shape(w.get(x, y, z)) {
                    out.push([x as f64 + b[0], y as f64 + b[1], z as f64 + b[2], x as f64 + b[3], y as f64 + b[4], z as f64 + b[5]]);
                }
            }
        }
    }
}

fn overlaps(a: &Aabb, b: &Aabb) -> bool { (0..3).all(|i| a[i] < b[i + 3] - 1e-7 && a[i + 3] > b[i] + 1e-7) }

fn clip_axis(bb: &Aabb, boxes: &[Aabb], axis: usize, mut d: f64) -> f64 {
    let (a1, a2) = match axis { 0 => (1, 2), 1 => (0, 2), _ => (0, 1) };
    for b in boxes {
        if bb[a1 + 3] <= b[a1] + 1e-9 || bb[a1] >= b[a1 + 3] - 1e-9 || bb[a2 + 3] <= b[a2] + 1e-9 || bb[a2] >= b[a2 + 3] - 1e-9 { continue; }
        if d > 0.0 && bb[axis + 3] <= b[axis] + 1e-9 { d = d.min(b[axis] - bb[axis + 3]); }
        if d < 0.0 && bb[axis] >= b[axis + 3] - 1e-9 { d = d.max(b[axis + 3] - bb[axis]); }
    }
    d
}

fn sweep(mut bb: Aabb, boxes: &[Aabb], d: [f64; 3]) -> ([f64; 3], Aabb) {
    let mut out = [0.0; 3];
    for axis in [1usize, 0, 2] {
        let m = clip_axis(&bb, boxes, axis, d[axis]);
        bb[axis] += m; bb[axis + 3] += m; out[axis] = m;
    }
    (out, bb)
}

impl Body {
    pub fn aabb(&self) -> Aabb { let h = WIDTH / 2.0; [self.pos[0] - h, self.pos[1], self.pos[2] - h, self.pos[0] + h, self.pos[1] + HEIGHT, self.pos[2] + h] }
    pub fn eye(&self) -> [f64; 3] { [self.pos[0], self.pos[1] + if self.sneaking { 1.27 } else { 1.62 }, self.pos[2]] }

    fn do_move(&mut self, w: &World, d: [f64; 3]) {
        let bb = self.aabb();
        let swept = [bb[0] + d[0].min(0.0), bb[1] + d[1].min(0.0) - 0.7, bb[2] + d[2].min(0.0), bb[3] + d[0].max(0.0), bb[4] + d[1].max(0.0) + 0.7, bb[5] + d[2].max(0.0)];
        let mut boxes = Vec::new(); collect(w, &swept, &mut boxes);
        // like vanilla, boxes we already overlap (e.g. teleported into a block) don't stop us
        boxes.retain(|b| !overlaps(&bb, b));
        let (mut m, _) = sweep(bb, &boxes, d);
        let hit_h = m[0] != d[0] || m[2] != d[2];
        let grounded = self.on_ground || (d[1] < 0.0 && m[1] != d[1]);
        if hit_h && grounded {
            let (up, bb1) = sweep(bb, &boxes, [0.0, 0.6, 0.0]);
            let (hm, bb2) = sweep(bb1, &boxes, [d[0], 0.0, d[2]]);
            let (down, _) = sweep(bb2, &boxes, [0.0, -up[1] + d[1].min(0.0), 0.0]);
            if hm[0] * hm[0] + hm[2] * hm[2] > m[0] * m[0] + m[2] * m[2] + 1e-9 { m = [hm[0], up[1] + down[1], hm[2]]; }
        }
        for i in 0..3 { self.pos[i] += m[i]; }
        self.h_collide = m[0] != d[0] || m[2] != d[2];
        if m[0] != d[0] { self.vel[0] = 0.0; }
        if m[2] != d[2] { self.vel[2] = 0.0; }
        self.on_ground = d[1] < 0.0 && (m[1] - d[1]).abs() > 1e-9 || (d[1] <= 0.0 && m[1] > d[1] + 1e-9);
        if (m[1] - d[1]).abs() > 1e-9 && !(hit_h && m[1] > 0.0 && d[1] <= 0.0) { self.vel[1] = 0.0; }
        if hit_h && m[1] > d[1] + 1e-9 && d[1] <= 0.0 { self.on_ground = true; self.vel[1] = 0.0; }
    }

    /// One 50 ms tick. Returns false (and doesn't move) when the chunk underfoot isn't loaded.
    pub fn tick(&mut self, w: &World, input: Input, spectator: bool) -> bool {
        self.sneaking = input.sneak && !self.flying;
        if !w.loaded(self.pos[0].floor() as i32, self.pos[2].floor() as i32) { self.vel = [0.0; 3]; return false; }
        let feet = w.get(self.pos[0].floor() as i32, (self.pos[1] + 0.4).floor() as i32, self.pos[2].floor() as i32);
        self.in_water = basalt_world::props::props(feet).has_fluid() && BlockState(feet).name() != "lava";
        let in_lava = BlockState(feet).name() == "lava";
        let (mut f, mut s) = (input.forward as f64, input.strafe as f64);
        let len = (f * f + s * s).sqrt(); if len > 1.0 { f /= len; s /= len; }
        f *= 0.98; s *= 0.98;
        if self.sneaking { f *= 0.3; s *= 0.3; }
        let sprint = input.sprint && input.forward > 0.0 && !self.sneaking;
        let (sin, cos) = (self.yaw.to_radians() as f64).sin_cos();
        let accel = |acc: f64, vel: &mut [f64; 3]| { vel[0] += (s * cos - f * sin) * acc; vel[2] += (f * cos + s * sin) * acc; };
        if self.flying || spectator {
            accel(if sprint { 0.1 } else { 0.05 }, &mut self.vel);
            self.vel[1] += (input.jump as i32 - input.sneak as i32) as f64 * 0.15;
            let v = self.vel;
            if spectator { for i in 0..3 { self.pos[i] += v[i]; } } else { self.do_move(w, v); }
            self.vel[0] *= 0.91; self.vel[2] *= 0.91; self.vel[1] *= 0.6;
            return true;
        }
        if self.in_water || in_lava {
            if input.jump { self.vel[1] += 0.04; }
            accel(0.02, &mut self.vel);
            let v = self.vel; self.do_move(w, v);
            let k = if in_lava { 0.5 } else { 0.8 };
            self.vel[0] *= k; self.vel[2] *= k; self.vel[1] = self.vel[1] * k - 0.02;
            if self.h_collide && input.forward > 0.0 { self.vel[1] = 0.3; }
            return true;
        }
        if input.jump && self.on_ground {
            self.vel[1] = 0.42;
            if sprint { self.vel[0] -= sin * 0.2; self.vel[2] += cos * 0.2; }
        }
        let below = w.get(self.pos[0].floor() as i32, (self.pos[1] - 0.5).floor() as i32, self.pos[2].floor() as i32);
        let slip = match BlockState(below).name() { "ice" | "packed_ice" | "frosted_ice" => 0.98, "blue_ice" => 0.989, "slime_block" => 0.8, _ => 0.6 };
        let friction = if self.on_ground { slip * 0.91 } else { 0.91 };
        let speed = 0.1 * if sprint { 1.3 } else { 1.0 };
        let acc = if self.on_ground { speed * (0.21600002 / (friction * friction * friction)) } else if sprint { 0.026 } else { 0.02 };
        accel(acc, &mut self.vel);
        let mut v = self.vel;
        // blocks that slow movement while inside them (vanilla "stuck in block")
        let inside = BlockState(feet).name();
        let stuck = match inside { "sweet_berry_bush" => Some([0.8, 0.75, 0.8]), "cobweb" => Some([0.25, 0.05, 0.25]), "powder_snow" => Some([0.9, 1.5, 0.9]), _ => None };
        if let Some(k) = stuck { for i in 0..3 { v[i] *= k[i]; } }
        if self.sneaking && self.on_ground {
            let bb = self.aabb(); let mut boxes = Vec::new();
            let mut probe = |dx: f64, dz: f64| -> bool {
                let t = [bb[0] + dx, bb[1] - 0.6, bb[2] + dz, bb[3] + dx, bb[1], bb[5] + dz];
                collect(w, &t, &mut boxes);
                boxes.iter().any(|b| overlaps(&t, b))
            };
            let step = 0.05;
            while v[0] != 0.0 && !probe(v[0], 0.0) { if v[0].abs() < step { v[0] = 0.0 } else { v[0] -= step * v[0].signum() } }
            while v[2] != 0.0 && !probe(0.0, v[2]) { if v[2].abs() < step { v[2] = 0.0 } else { v[2] -= step * v[2].signum() } }
            while v[0] != 0.0 && v[2] != 0.0 && !probe(v[0], v[2]) {
                if v[0].abs() < step { v[0] = 0.0 } else { v[0] -= step * v[0].signum() }
                if v[2].abs() < step { v[2] = 0.0 } else { v[2] -= step * v[2].signum() }
            }
        }
        self.do_move(w, v);
        if stuck.is_some() { self.vel = [0.0; 3]; }
        self.vel[1] = (self.vel[1] - 0.08) * 0.98;
        self.vel[0] *= friction; self.vel[2] *= friction;
        for i in [0, 2] { if self.vel[i].abs() < 0.003 { self.vel[i] = 0.0; } }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Column, World};
    use basalt_world::PalettedContainer;

    fn flat(floor_y: i32) -> World {
        let stone = BlockState::from_name("stone").unwrap().0;
        let mut w = World::default();
        for cx in -1..=1 { for cz in -1..=1 {
            let blocks = (0..24).map(|_| PalettedContainer::blocks_filled(0)).collect();
            let biomes = (0..24).map(|_| PalettedContainer::biomes_filled(0)).collect();
            w.chunks.insert((cx, cz), Column { blocks, biomes });
        } }
        for x in -16..32 { for z in -16..32 { w.set(x, floor_y, z, stone); } }
        w
    }

    #[test]
    fn falls_and_lands() {
        let w = flat(64);
        let mut b = Body { pos: [0.5, 75.0, 0.5], ..Default::default() };
        for _ in 0..100 { b.tick(&w, Input::default(), false); }
        assert!(b.on_ground);
        assert!((b.pos[1] - 65.0).abs() < 1e-9, "{}", b.pos[1]);
    }

    #[test]
    fn walks_about_4_3_blocks_per_second() {
        let w = flat(64);
        let mut b = Body { pos: [0.5, 65.0, 0.5], on_ground: true, ..Default::default() };
        let i = Input { forward: 1.0, ..Default::default() };
        for _ in 0..20 { b.tick(&w, i, false); }
        let d = b.pos[2] - 0.5;
        assert!(d > 3.8 && d < 4.6, "{d}");
    }
}

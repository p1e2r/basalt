//! Local player: vanilla-like movement physics with AABB collision.
use crate::assets::Assets;
use crate::world::World;

#[derive(Default, Clone, Copy)]
pub struct Input { pub forward: f32, pub strafe: f32, pub jump: bool, pub sneak: bool, pub sprint: bool }

pub struct Player {
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub h_collide: bool,
    pub flying: bool,
    pub gamemode: u8,
    pub in_water: bool,
    pub prev_pos: [f64; 3],
    pub sneaking: bool,
}

pub const WIDTH: f64 = 0.6;
pub const HEIGHT: f64 = 1.8;

type Aabb = [f64; 6];

/// Collision boxes of the block at (x,y,z), in world coordinates.
pub fn block_boxes(a: &Assets, w: &World, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) {
    let s = w.get(x, y, z);
    if s == 0 { return; }
    let info = a.info(s);
    if !info.collides { return; }
    let m = a.model(s);
    let b = basalt_data::BlockState(s as u32).block();
    let (fx, fy, fz) = (x as f64, y as f64, z as f64);
    if m.boxes.is_empty() {
        let h = match b.kind { "bed" => 0.5625, "chest" | "ender_chest" | "trapped_chest" | "copper_chest" | "weathering_copper_chest" => 0.875, _ => 1.0 };
        let inset = if h == 0.875 { 0.0625 } else { 0.0 };
        out.push([fx + inset, fy, fz + inset, fx + 1.0 - inset, fy + h, fz + 1.0 - inset]);
        return;
    }
    let tall = matches!(b.kind, "fence" | "wall" | "fence_gate");
    for bx in &m.boxes {
        let mut top = bx[4] as f64;
        if tall { top = 1.5; }
        out.push([fx + bx[0] as f64, fy + bx[1] as f64, fz + bx[2] as f64, fx + bx[3] as f64, fy + top, fz + bx[5] as f64]);
    }
}

fn collect(a: &Assets, w: &World, bb: &Aabb, out: &mut Vec<Aabb>) {
    out.clear();
    for x in (bb[0].floor() as i32 - 1)..=(bb[3].floor() as i32 + 1) { for y in (bb[1].floor() as i32 - 1)..=(bb[4].floor() as i32 + 1) { for z in (bb[2].floor() as i32 - 1)..=(bb[5].floor() as i32 + 1) {
        block_boxes(a, w, x, y, z, out);
    } } }
}

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

impl Player {
    pub fn new() -> Self { Player { pos: [0.0, 100.0, 0.0], vel: [0.0; 3], yaw: 0.0, pitch: 0.0, on_ground: false, h_collide: false, flying: false, gamemode: 0, in_water: false, prev_pos: [0.0, 100.0, 0.0], sneaking: false } }
    pub fn eye_height(&self) -> f64 { if self.sneaking { 1.27 } else { 1.62 } }
    pub fn aabb(&self) -> Aabb { let h = WIDTH / 2.0; [self.pos[0] - h, self.pos[1], self.pos[2] - h, self.pos[0] + h, self.pos[1] + HEIGHT, self.pos[2] + h] }

    fn do_move(&mut self, a: &Assets, w: &World, d: [f64; 3]) {
        if self.gamemode == 3 { for i in 0..3 { self.pos[i] += d[i]; } self.on_ground = false; return; }
        let bb = self.aabb();
        let swept = [bb[0] + d[0].min(0.0), bb[1] + d[1].min(0.0) - 0.7, bb[2] + d[2].min(0.0), bb[3] + d[0].max(0.0), bb[4] + d[1].max(0.0) + 0.7, bb[5] + d[2].max(0.0)];
        let mut boxes = Vec::new(); collect(a, w, &swept, &mut boxes);
        let (mut m, _) = sweep(bb, &boxes, d);
        let hit_h = m[0] != d[0] || m[2] != d[2];
        let grounded = self.on_ground || (d[1] < 0.0 && m[1] != d[1]);
        if hit_h && grounded {
            // try stepping up
            let step = 0.6;
            let (up, bb1) = sweep(bb, &boxes, [0.0, step, 0.0]);
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

    /// One 50 ms physics tick.
    pub fn tick(&mut self, a: &Assets, w: &World, input: Input) {
        self.prev_pos = self.pos;
        self.sneaking = input.sneak && !self.flying;
        // don't fall into unloaded chunks
        if !w.loaded(self.pos[0].floor() as i32, self.pos[2].floor() as i32) { self.vel = [0.0; 3]; return; }
        let feet = w.get(self.pos[0].floor() as i32, (self.pos[1] + 0.4).floor() as i32, self.pos[2].floor() as i32);
        self.in_water = a.info(feet).fluid != 0;
        let (mut f, mut s) = (input.forward as f64, input.strafe as f64);
        let len = (f * f + s * s).sqrt(); if len > 1.0 { f /= len; s /= len; }
        f *= 0.98; s *= 0.98;
        if self.sneaking { f *= 0.3; s *= 0.3; }
        let sprint = input.sprint && input.forward > 0.0 && !self.sneaking;
        let (sin, cos) = (self.yaw.to_radians() as f64).sin_cos();
        let accel_dir = |acc: f64, vel: &mut [f64; 3]| { vel[0] += (s * cos - f * sin) * acc; vel[2] += (f * cos + s * sin) * acc; };
        if self.flying || self.gamemode == 3 {
            let sp = if sprint { 0.1 } else { 0.05 };
            accel_dir(sp, &mut self.vel);
            self.vel[1] += (input.jump as i32 - input.sneak as i32) as f64 * 0.15;
            let v = self.vel; self.do_move(a, w, v);
            self.vel[0] *= 0.91; self.vel[2] *= 0.91; self.vel[1] *= 0.6;
            if self.on_ground && self.gamemode != 3 && self.gamemode != 1 { self.flying = false; }
            return;
        }
        if self.in_water {
            if input.jump { self.vel[1] += 0.04; }
            accel_dir(0.02, &mut self.vel);
            let v = self.vel; self.do_move(a, w, v);
            self.vel[0] *= 0.8; self.vel[2] *= 0.8; self.vel[1] = self.vel[1] * 0.8 - 0.02;
            if self.h_collide && input.forward > 0.0 { self.vel[1] = 0.3; }
            return;
        }
        if input.jump && self.on_ground {
            self.vel[1] = 0.42;
            if sprint { self.vel[0] -= sin * 0.2; self.vel[2] += cos * 0.2; }
        }
        let below = w.get(self.pos[0].floor() as i32, (self.pos[1] - 0.5).floor() as i32, self.pos[2].floor() as i32);
        let slip = match basalt_data::BlockState(below as u32).name() { "ice" | "packed_ice" | "frosted_ice" => 0.98, "blue_ice" => 0.989, "slime_block" => 0.8, _ => 0.6 };
        let friction = if self.on_ground { slip * 0.91 } else { 0.91 };
        let speed = 0.1 * if sprint { 1.3 } else { 1.0 };
        let acc = if self.on_ground { ground_accel(speed, slip) } else if sprint { 0.026 } else { 0.02 };
        accel_dir(acc, &mut self.vel);
        // climbing
        let fs = basalt_data::BlockState(feet as u32);
        let climbing = matches!(fs.block().kind, "ladder" | "vine" | "scaffolding" | "weeping_vines" | "weeping_vines_plant" | "twisting_vines" | "twisting_vines_plant" | "cave_vines" | "cave_vines_plant");
        if climbing {
            self.vel[0] = self.vel[0].clamp(-0.15, 0.15); self.vel[2] = self.vel[2].clamp(-0.15, 0.15);
            if self.vel[1] < -0.15 { self.vel[1] = -0.15; }
            if input.sneak && self.vel[1] < 0.0 { self.vel[1] = 0.0; }
        }
        let mut v = self.vel;
        // sneaking: don't walk off edges
        if self.sneaking && self.on_ground {
            let bb = self.aabb(); let mut boxes = Vec::new();
            let probe = |dx: f64, dz: f64, boxes: &mut Vec<Aabb>| -> bool {
                let t = [bb[0] + dx, bb[1] - 0.6, bb[2] + dz, bb[3] + dx, bb[1], bb[5] + dz];
                collect(a, w, &t, boxes);
                boxes.iter().any(|b| t[0] < b[3] && t[3] > b[0] && t[1] < b[4] && t[4] > b[1] && t[2] < b[5] && t[5] > b[2])
            };
            let step = 0.05;
            while v[0] != 0.0 && !probe(v[0], 0.0, &mut boxes) { if v[0].abs() < step { v[0] = 0.0 } else { v[0] -= step * v[0].signum() } }
            while v[2] != 0.0 && !probe(0.0, v[2], &mut boxes) { if v[2].abs() < step { v[2] = 0.0 } else { v[2] -= step * v[2].signum() } }
            while v[0] != 0.0 && v[2] != 0.0 && !probe(v[0], v[2], &mut boxes) {
                if v[0].abs() < step { v[0] = 0.0 } else { v[0] -= step * v[0].signum() }
                if v[2].abs() < step { v[2] = 0.0 } else { v[2] -= step * v[2].signum() }
            }
        }
        self.do_move(a, w, v);
        if climbing && (self.h_collide || input.jump) { self.vel[1] = 0.2; }
        self.vel[1] = (self.vel[1] - 0.08) * 0.98;
        self.vel[0] *= friction; self.vel[2] *= friction;
        for i in [0, 2] { if self.vel[i].abs() < 0.003 { self.vel[i] = 0.0; } }
    }
}

/// Per-tick acceleration on the ground: movement speed scaled by the block's slipperiness cubed
/// (0.6³ = 0.216 on normal blocks, so plain walking accelerates by exactly the movement speed).
/// The extra 0.91 air-drag factor belongs only to the velocity damping, not here (it made walking 36% too fast).
pub fn ground_accel(speed: f64, slip: f64) -> f64 { speed * (0.21600002 / (slip * slip * slip)) }

pub struct Hit { pub block: [i32; 3], pub face: u8, pub point: [f64; 3], pub boxes: Vec<[f64; 6]> }

/// Ray-cast against block selection shapes.
pub fn raycast(a: &Assets, w: &World, origin: [f64; 3], dir: [f64; 3], reach: f64) -> Option<Hit> {
    let mut best: Option<(f64, Hit)> = None;
    let steps = (reach * 8.0) as i32;
    let mut visited = std::collections::HashSet::new();
    let mut boxes = Vec::new();
    for i in 0..=steps {
        let t = i as f64 / 8.0;
        let p = [origin[0] + dir[0] * t, origin[1] + dir[1] * t, origin[2] + dir[2] * t];
        let c = [p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32];
        for dx in -1..=1 { for dy in -1..=1 { for dz in -1..=1 {
            let b = [c[0] + dx, c[1] + dy, c[2] + dz];
            if !visited.insert(b) { continue; }
            let s = w.get(b[0], b[1], b[2]);
            if s == 0 { continue; }
            let info = a.info(s);
            let m = a.model(s);
            boxes.clear();
            if m.boxes.is_empty() { if info.fluid != 0 && !info.collides { continue; } boxes.push([b[0] as f64, b[1] as f64, b[2] as f64, b[0] as f64 + 1.0, b[1] as f64 + 1.0, b[2] as f64 + 1.0]); }
            else { for bx in &m.boxes { boxes.push([b[0] as f64 + bx[0] as f64, b[1] as f64 + bx[1] as f64, b[2] as f64 + bx[2] as f64, b[0] as f64 + bx[3] as f64, b[1] as f64 + bx[4] as f64, b[2] as f64 + bx[5] as f64]); } }
            for bx in &boxes {
                // slab test
                let (mut tmin, mut tmax, mut face) = (0.0f64, reach, 0u8);
                let mut ok = true;
                for ax in 0..3 {
                    if dir[ax].abs() < 1e-12 { if origin[ax] < bx[ax] || origin[ax] > bx[ax + 3] { ok = false; break; } continue; }
                    let (mut t1, mut t2) = ((bx[ax] - origin[ax]) / dir[ax], (bx[ax + 3] - origin[ax]) / dir[ax]);
                    let mut f = match ax { 0 => 4u8, 1 => 0, _ => 2 };
                    if t1 > t2 { std::mem::swap(&mut t1, &mut t2); f += 1; }
                    if t1 > tmin { tmin = t1; face = f; }
                    tmax = tmax.min(t2);
                    if tmin > tmax { ok = false; break; }
                }
                if ok && best.as_ref().map(|(bt, _)| tmin < *bt).unwrap_or(true) {
                    best = Some((tmin, Hit { block: b, face, point: [origin[0] + dir[0] * tmin, origin[1] + dir[1] * tmin, origin[2] + dir[2] * tmin], boxes: boxes.clone() }));
                }
            }
        } } }
        if let Some((bt, _)) = &best { if *bt < t - 1.8 { break; } }
    }
    best.map(|b| b.1)
}

#[cfg(test)]
mod tests {
    use super::ground_accel;
    #[test]
    fn walking_accelerates_by_the_movement_speed() {
        assert!((ground_accel(0.1, 0.6) - 0.1).abs() < 1e-6);
        // terminal walking speed: v = a / (1 - slip*0.91) → 4.3 blocks/s on normal ground
        let v = ground_accel(0.1, 0.6) / (1.0 - 0.6 * 0.91) * 20.0;
        assert!((v - 4.4).abs() < 0.2, "walk speed {v}");
        assert!(ground_accel(0.1, 0.98) < ground_accel(0.1, 0.6), "ice accelerates more slowly");
    }
}

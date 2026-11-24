//! Block particles: the chips that burst out of a broken block and spray off the face being dug.
//! Small camera-facing squares cut from a random corner of the block's particle texture; they fall,
//! settle on the ground and vanish. Counts, speeds and lifetimes tuned by eye against a vanilla
//! 26.2 client recording (2026-11-23).
use crate::assets::Assets;
use crate::mesh::{MeshData, Vertex};
use crate::world::World;
use glam::Vec3;

pub struct Particle {
    pos: [f64; 3],
    prev: [f64; 3],
    vel: [f64; 3],
    age: u32,
    life: u32,
    size: f32,
    uv: [f32; 4],
    tint: [u8; 3],
    on_ground: bool,
}

#[derive(Default)]
pub struct Particles {
    list: Vec<Particle>,
    seed: u64,
}

/// Upper bound like the vanilla client keeps (old particles make way for new ones).
const MAX: usize = 4000;

impl Particles {
    fn rand(&mut self) -> f64 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        (self.seed >> 11) as f64 / (1u64 << 53) as f64
    }

    fn spawn(&mut self, a: &Assets, state: u16, pos: [f64; 3], vel: [f64; 3], tint: [u8; 3]) {
        let Some(name) = a.particle.get(state as usize).and_then(|n| n.as_ref()) else {
            return;
        };
        let Some(t) = a.tex_uv(name) else { return };
        if self.seed == 0 {
            self.seed = 0x9E37_79B9_7F4A_7C15;
        }
        // a random quarter-size square of the sprite
        let (fu, fv) = (self.rand() as f32 * 0.75, self.rand() as f32 * 0.75);
        let (du, dv) = (t[2] - t[0], t[3] - t[1]);
        let uv = [
            t[0] + fu * du,
            t[1] + fv * dv,
            t[0] + (fu + 0.25) * du,
            t[1] + (fv + 0.25) * dv,
        ];
        let life = (4.0 / (self.rand() * 0.9 + 0.1)) as u32;
        let size = (0.1 * (self.rand() * 0.5 + 0.5)) as f32;
        if self.list.len() >= MAX {
            self.list.remove(0);
        }
        self.list.push(Particle {
            pos,
            prev: pos,
            vel,
            age: 0,
            life,
            size,
            uv,
            tint,
            on_ground: false,
        });
    }

    /// A broken block: a 4x4x4 burst spread through the block, flying outwards from its centre.
    pub fn block_break(&mut self, a: &Assets, state: u16, b: [i32; 3], tint: [u8; 3]) {
        if state == 0 {
            return;
        }
        for i in 0..4 {
            for j in 0..4 {
                for k in 0..4 {
                    let o = [
                        (i as f64 + 0.5) / 4.0,
                        (j as f64 + 0.5) / 4.0,
                        (k as f64 + 0.5) / 4.0,
                    ];
                    let mut v = [o[0] - 0.5, o[1] - 0.5, o[2] - 0.5];
                    for c in v.iter_mut() {
                        *c = *c * 0.3 + (self.rand() - 0.5) * 0.12;
                    }
                    v[1] += 0.06;
                    self.spawn(
                        a,
                        state,
                        [b[0] as f64 + o[0], b[1] as f64 + o[1], b[2] as f64 + o[2]],
                        v,
                        tint,
                    );
                }
            }
        }
    }

    /// Landing dust: `n` pieces of the landed-on block from around the feet, thrown up and out.
    pub fn block_land(&mut self, a: &Assets, state: u16, feet: [f64; 3], n: usize, tint: [u8; 3]) {
        if state == 0 {
            return;
        }
        for _ in 0..n {
            let ang = self.rand() * std::f64::consts::TAU;
            let r = self.rand() * 0.3;
            let pos = [
                feet[0] + ang.cos() * r,
                feet[1] + 0.05,
                feet[2] + ang.sin() * r,
            ];
            let v = [
                ang.cos() * (0.05 + self.rand() * 0.1),
                0.1 + self.rand() * 0.15,
                ang.sin() * (0.05 + self.rand() * 0.1),
            ];
            self.spawn(a, state, pos, v, tint);
        }
    }

    /// One chip off the face being dug (`face` = protocol direction 0 down .. 5 east).
    pub fn block_hit(&mut self, a: &Assets, state: u16, b: [i32; 3], face: u8, tint: [u8; 3]) {
        if state == 0 {
            return;
        }
        let mut p = [
            b[0] as f64 + 0.1 + self.rand() * 0.8,
            b[1] as f64 + 0.1 + self.rand() * 0.8,
            b[2] as f64 + 0.1 + self.rand() * 0.8,
        ];
        let (axis, out) = match face {
            0 => (1, -0.1),
            1 => (1, 1.1),
            2 => (2, -0.1),
            3 => (2, 1.1),
            4 => (0, -0.1),
            _ => (0, 1.1),
        };
        p[axis] = b[axis] as f64 + out;
        let v = [
            (self.rand() - 0.5) * 0.08,
            self.rand() * 0.06,
            (self.rand() - 0.5) * 0.08,
        ];
        self.spawn(a, state, p, v, tint);
    }

    pub fn tick(&mut self, a: &Assets, world: &World) {
        let solid = |p: [f64; 3]| {
            let s = world.get(
                p[0].floor() as i32,
                p[1].floor() as i32,
                p[2].floor() as i32,
            );
            s != 0 && a.info(s).collides
        };
        self.list.retain_mut(|q| {
            q.age += 1;
            if q.age >= q.life {
                return false;
            }
            q.prev = q.pos;
            q.vel[1] -= 0.04;
            let next = [
                q.pos[0] + q.vel[0],
                q.pos[1] + q.vel[1],
                q.pos[2] + q.vel[2],
            ];
            // stop at blocks: land on top, slide along walls
            if solid([q.pos[0], next[1] - q.size as f64 * 0.5, q.pos[2]]) {
                if q.vel[1] < 0.0 {
                    q.on_ground = true;
                    q.pos[1] = (next[1] - q.size as f64 * 0.5).floor() + 1.0 + q.size as f64 * 0.5;
                }
                q.vel[1] = 0.0;
            } else {
                q.pos[1] = next[1];
                q.on_ground = false;
            }
            if solid([next[0], q.pos[1], q.pos[2]]) {
                q.vel[0] = 0.0;
            } else {
                q.pos[0] = next[0];
            }
            if solid([q.pos[0], q.pos[1], next[2]]) {
                q.vel[2] = 0.0;
            } else {
                q.pos[2] = next[2];
            }
            let drag = if q.on_ground { 0.7 } else { 0.98 };
            q.vel[0] *= drag;
            q.vel[1] *= 0.98;
            q.vel[2] *= drag;
            true
        });
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// Camera-facing quads (block atlas, cutout) at interpolated positions.
    pub fn build(&self, world: &World, yaw: f32, pitch: f32, alpha: f32, out: &mut MeshData) {
        let (y, p) = (yaw.to_radians(), pitch.to_radians());
        // camera right / up in world space (yaw 0 faces +Z)
        let right = Vec3::new(-y.cos(), 0.0, -y.sin());
        let fwd = Vec3::new(-y.sin() * p.cos(), -p.sin(), y.cos() * p.cos());
        let up = right.cross(fwd).normalize();
        for q in &self.list {
            let c = Vec3::new(
                (q.prev[0] + (q.pos[0] - q.prev[0]) * alpha as f64) as f32,
                (q.prev[1] + (q.pos[1] - q.prev[1]) * alpha as f64) as f32,
                (q.prev[2] + (q.pos[2] - q.prev[2]) * alpha as f64) as f32,
            );
            let (sl, bl) = world.light(c.x.floor() as i32, c.y.floor() as i32, c.z.floor() as i32);
            let (r, u) = (right * q.size * 0.5, up * q.size * 0.5);
            let pts = [c - r + u, c - r - u, c + r - u, c + r + u];
            let uvs = [
                [q.uv[0], q.uv[1]],
                [q.uv[0], q.uv[3]],
                [q.uv[2], q.uv[3]],
                [q.uv[2], q.uv[1]],
            ];
            let b = out.verts.len() as u32;
            for i in 0..4 {
                out.verts.push(Vertex {
                    pos: pts[i].to_array(),
                    uv: uvs[i],
                    color: [q.tint[0], q.tint[1], q.tint[2], 255],
                    light: [sl * 17, bl * 17, 0, 0],
                });
            }
            out.idx.extend_from_slice(&[
                b,
                b + 1,
                b + 2,
                b,
                b + 2,
                b + 3,
                b,
                b + 2,
                b + 1,
                b,
                b + 3,
                b + 2,
            ]);
        }
    }
}

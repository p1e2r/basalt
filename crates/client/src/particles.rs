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
    /// 0 block chip, 1 water bubble, 2 water splash drop
    kind: u8,
}

#[derive(Default)]
pub struct Particles {
    list: Vec<Particle>,
    seed: u64,
}

/// Upper bound like the vanilla client keeps (old particles make way for new ones).
const MAX: usize = 4000;

/// One tick of a water bubble (buoyancy +0.002, drag 0.85) or splash drop (gravity 0.04, drag 0.98):
/// accelerate, move by the new velocity, then apply the drag (checked against the real client's
/// per-tick particle positions, oracle fx log 2026-11-24 — see the test).
fn water_step(bubble: bool, pos: &mut [f64; 3], vel: &mut [f64; 3]) {
    vel[1] += if bubble { 0.002 } else { -0.04 };
    for i in 0..3 {
        pos[i] += vel[i];
    }
    let drag = if bubble { 0.85 } else { 0.98 };
    for v in vel.iter_mut() {
        *v *= drag;
    }
}

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
            kind: 0,
        });
    }

    fn spawn_uv(
        &mut self,
        uv: [f32; 4],
        pos: [f64; 3],
        vel: [f64; 3],
        tint: [u8; 3],
        life: u32,
        size: f32,
    ) {
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
            kind: 0,
        });
    }

    /// Item break burst (a snowball or egg hitting something): small chips of the item sprite.
    pub fn item_break(&mut self, a: &Assets, item: i32, pos: [f64; 3], n: usize) {
        let Some(t) = a.icon_uv(item) else { return };
        if self.seed == 0 {
            self.seed = 0x9E37_79B9_7F4A_7C15;
        }
        for _ in 0..n {
            let (fu, fv) = (self.rand() as f32 * 0.75, self.rand() as f32 * 0.75);
            let (du, dv) = (t[2] - t[0], t[3] - t[1]);
            let uv = [
                t[0] + fu * du,
                t[1] + fv * dv,
                t[0] + (fu + 0.25) * du,
                t[1] + (fv + 0.25) * dv,
            ];
            let vel = [
                (self.rand() - 0.5) * 0.2,
                self.rand() * 0.2,
                (self.rand() - 0.5) * 0.2,
            ];
            let life = (4.0 / (self.rand() * 0.9 + 0.1)) as u32;
            let size = (0.1 * (self.rand() * 0.5 + 0.5)) as f32;
            self.spawn_uv(uv, pos, vel, [255, 255, 255], life, size);
        }
    }

    /// Potion / experience bottle shatter (level event 2002 / 2007): coloured specks rising from the impact.
    pub fn splash(&mut self, a: &Assets, argb: u32, b: [i32; 3]) {
        let Some(t) = a.tex_uv("block/white_concrete") else {
            return;
        };
        if self.seed == 0 {
            self.seed = 0x9E37_79B9_7F4A_7C15;
        }
        let tint = [
            (argb >> 16 & 255) as u8,
            (argb >> 8 & 255) as u8,
            (argb & 255) as u8,
        ];
        let c = [b[0] as f64 + 0.5, b[1] as f64 + 0.3, b[2] as f64 + 0.5];
        let uv = [
            t[0],
            t[1],
            t[0] + (t[2] - t[0]) * 0.25,
            t[1] + (t[3] - t[1]) * 0.25,
        ];
        for _ in 0..40 {
            let ang = self.rand() * std::f64::consts::TAU;
            let sp = self.rand() * 0.15;
            let vel = [ang.cos() * sp, 0.05 + self.rand() * 0.15, ang.sin() * sp];
            let life = 10 + (self.rand() * 20.0) as u32;
            self.spawn_uv(uv, c, vel, tint, life, 0.12);
        }
    }

    pub fn rr(&mut self, lo: f64, hi: f64) -> f64 {
        if self.seed == 0 {
            self.seed = 0x9E37_79B9_7F4A_7C15;
        }
        lo + (hi - lo) * self.rand()
    }

    /// The splash a body makes entering water at `speed` (blocks/tick, vertical): 13 splash drops and
    /// 13 bubbles from a spot on the surface (floor(y)+1) within 0.5 blocks of the body's axis.
    /// Fitted to the real 26.2 client's local particles (oracle fx log, client-water cases, 2026-11-24):
    /// bubbles start with vy = -(0.2 * speed) - U(-0.004..0.056), vx/vz +-0.02, quad half-size
    /// 0.027..0.137; drops vy U(0.105..0.295) vx/vz +-0.04, half-size 0.10..0.196, one of 4 sprites;
    /// both live 8 / (0.2 + 0.8 U) ticks.
    pub fn water_splash(&mut self, a: &Assets, pos: [f64; 3], speed: f64) {
        let y = pos[1].floor() + 1.0;
        for _ in 0..13 {
            let at = [pos[0] + self.rr(-0.5, 0.5), y, pos[2] + self.rr(-0.5, 0.5)];
            let v = [
                self.rr(-0.02, 0.02),
                -(0.2 * speed) - self.rr(-0.004, 0.056),
                self.rr(-0.02, 0.02),
            ];
            let (life, size) = (self.life8(), self.rr(0.027, 0.137) as f32);
            self.spawn_sprite(a, "particle/bubble", 1, at, v, life, size);
        }
        for _ in 0..13 {
            let at = [pos[0] + self.rr(-0.5, 0.5), y, pos[2] + self.rr(-0.5, 0.5)];
            let v = [
                self.rr(-0.04, 0.04),
                self.rr(0.105, 0.295),
                self.rr(-0.04, 0.04),
            ];
            let (life, size) = (self.life8(), self.rr(0.10, 0.196) as f32);
            let n = (self.rand() * 4.0) as u32;
            self.spawn_sprite(a, &format!("particle/splash_{n}"), 2, at, v, life, size);
        }
    }

    fn life8(&mut self) -> u32 {
        (8.0 / self.rr(0.2, 1.0)) as u32
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_sprite(
        &mut self,
        a: &Assets,
        name: &str,
        kind: u8,
        pos: [f64; 3],
        vel: [f64; 3],
        life: u32,
        half: f32,
    ) {
        let Some(uv) = a.tex_uv(name) else { return };
        crate::script::log_fx(
            "particle",
            if kind == 1 {
                "BubbleParticle"
            } else {
                "SplashParticle"
            },
            &[
                ("x", pos[0]),
                ("y", pos[1]),
                ("z", pos[2]),
                ("vx", vel[0]),
                ("vy", vel[1]),
                ("vz", vel[2]),
                ("life", life as f64),
                ("size", half as f64),
            ],
        );
        if self.list.len() >= MAX {
            self.list.remove(0);
        }
        self.list.push(Particle {
            pos,
            prev: pos,
            vel,
            age: 0,
            life,
            size: half,
            uv,
            tint: [255; 3],
            on_ground: false,
            kind,
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
        // surface height of the water at a point (None when it isn't in water): still water is 8/9 of
        // its block, water with water above fills it
        let water_top = |p: [f64; 3]| -> Option<f64> {
            let (x, y, z) = (
                p[0].floor() as i32,
                p[1].floor() as i32,
                p[2].floor() as i32,
            );
            let st = world.get(x, y, z);
            if st == 0 || a.info(st).fluid != 1 {
                return None;
            }
            if a.info(world.get(x, y + 1, z)).fluid != 0 {
                return Some(y as f64 + 1.0);
            }
            let lvl = basalt_data::BlockState(st as u32)
                .get("level")
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(0);
            Some(y as f64 + (8 - if lvl >= 8 { 0 } else { lvl }) as f64 / 9.0)
        };
        self.list.retain_mut(|q| {
            if q.kind != 0 {
                // water bubble / splash drop: vanilla order — expire, accelerate, move, drag, then die
                // outside water (bubbles) or below the surface (drops); positions fitted to the real client
                if q.age >= q.life {
                    return false;
                }
                q.age += 1;
                q.prev = q.pos;
                water_step(q.kind == 1, &mut q.pos, &mut q.vel);
                return match water_top(q.pos) {
                    Some(top) => q.kind == 1 || q.pos[1] >= top,
                    None => q.kind != 1 && !solid(q.pos),
                };
            }
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
            let k = if q.kind == 0 { 0.5 } else { 1.0 };
            let (r, u) = (right * q.size * k, up * q.size * k);
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

#[cfg(test)]
mod tests {
    use super::water_step;

    /// Positions of two particles recorded from the real client (client-water fall_10, tick by tick).
    #[test]
    fn bubble_and_drop_follow_the_recorded_paths() {
        let (mut p, mut v) = ([2005.0792, 200.0, 2005.4943], [-0.0171, -0.2531, 0.0043]);
        let want = [
            199.7489, 199.5374, 199.3597, 199.2106, 199.0859, 198.9819, 198.8955, 198.8241,
        ];
        for w in want {
            water_step(true, &mut p, &mut v);
            assert!((p[1] - w).abs() < 2e-4, "bubble y {} vs {w}", p[1]);
        }
        let (mut p, mut v) = ([2004.98, 200.0, 2005.838], [0.0244, 0.1345, -0.0079]);
        for w in [200.0945, 200.1471, 200.1586, 200.1299, 200.0618, 199.9551] {
            water_step(false, &mut p, &mut v);
            assert!((p[1] - w).abs() < 2e-4, "drop y {} vs {w}", p[1]);
        }
    }
}

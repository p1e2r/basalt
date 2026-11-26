//! Fishing rods: use_item casts a fishing_bobber (add_entity data = the angler), a second use reels
//! it in. In water the bobber floats just under the surface and waits for a bite; reeling in while
//! it bites (entity data 9 = true) pulls a `gameplay/fishing` loot item to the angler and drops 1-6
//! experience. Measured on vanilla 26.2 with scenario fishing (2026-11-26): spawn at eye height,
//! speed ~1.1, throw / retrieve sounds, floats ~0.15 under the cell top, reeling in empty or from
//! the ground costs no durability, a catch costs 1; Lure III: bite after ~6 s.
//! Not measured (vanilla-like guesses): the wait (100-600 ticks minus 100 per Lure level), the fish
//! approach (20-80 ticks) and bite window (20-40 ticks); hooking entities is not implemented.
use super::mob::{block_boxes, write_lpvec3};
use super::proj::{PKind, Proj};
use super::*;
use rand::Rng;

#[derive(Clone, Copy, Debug, Default)]
pub struct Fish {
    pub lure: u8,
    pub in_ground: bool,
    pub in_water: bool,
    pub wait: i32,
    pub approach: i32,
    pub bite: i32,
}

impl Game {
    fn bobber_of(&self, eid: i32) -> Option<i32> {
        self.projs
            .values()
            .find(|p| p.kind == PKind::Bobber && p.owner == eid)
            .map(|p| p.eid)
    }

    fn new_wait(&mut self, lure: u8) -> i32 {
        (self.rng.gen_range(100..=600) - lure as i32 * 100).max(0)
    }

    /// use_item with a fishing rod. True when handled.
    pub(super) fn try_fish(&mut self, eid: i32, hand: i32, yaw: f32, pitch: f32) -> bool {
        let Some(p) = self.players.get_mut(&eid) else {
            return false;
        };
        let slot = if hand == 0 { 36 + p.held } else { 45 };
        let s = p.inv[slot];
        if s.count <= 0 || reg::ITEM.get(s.item as usize).copied() != Some("fishing_rod") {
            return false;
        }
        if p.dead || p.gamemode == 3 {
            return true;
        }
        p.yaw = yaw;
        p.pitch = pitch;
        let (feet, sneaking) = (p.pos, p.sneaking);
        if let Some(b) = self.bobber_of(eid) {
            self.reel_in(eid, slot, b);
            return true;
        }
        let lure = self
            .ench_id("lure")
            .map(|id| s.ench.level_of(id))
            .unwrap_or(0);
        let eye = feet[1] + if sneaking { 1.27 } else { 1.62 };
        let (yr, xr) = ((yaw as f64).to_radians(), (pitch as f64).to_radians());
        let dir = [-yr.sin() * xr.cos(), -xr.sin(), yr.cos() * xr.cos()];
        let speed = self.rng.gen_range(1.0..1.2);
        let vel = [dir[0] * speed, dir[1] * speed + 0.1, dir[2] * speed];
        let pos = [feet[0] + dir[0] * 0.3, eye, feet[2] + dir[2] * 0.3];
        let peid = self
            .sh
            .next_eid
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut pr = Proj::new_cloud(peid, pos, eid, Stack::default());
        pr.kind = PKind::Bobber;
        pr.vel = vel;
        pr.fish = Some(Fish {
            lure,
            ..Fish::default()
        });
        self.projs.insert(peid, pr);
        self.sync_viewers_for(peid);
        let pitch = 0.4 / (self.rng.gen::<f32>() * 0.4 + 0.8);
        self.play_sound("entity.fishing_bobber.throw", 6, feet, 0.5, pitch);
        true
    }

    fn ench_id(&self, name: &str) -> Option<u16> {
        self.sh
            .registries
            .entries("minecraft:enchantment")
            .iter()
            .position(|e| e.trim_start_matches("minecraft:") == name)
            .map(|i| i as u16)
    }

    fn reel_in(&mut self, eid: i32, slot: usize, b: i32) {
        let Some(pr) = self.projs.remove(&b) else {
            return;
        };
        self.untrack(b);
        let feet = self.players[&eid].pos;
        let fish = pr.fish.unwrap_or_default();
        let pitch = 0.4 / (self.rng.gen::<f32>() * 0.4 + 0.8);
        self.play_sound("entity.fishing_bobber.retrieve", 6, feet, 1.0, pitch);
        if fish.bite > 0 && fish.in_water {
            let ctx = basalt_game::LootContext::default();
            let loot = self.data.loot("gameplay/fishing", &ctx, &mut self.rng);
            let d = [
                feet[0] - pr.pos[0],
                feet[1] - pr.pos[1],
                feet[2] - pr.pos[2],
            ];
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            let vel = [d[0] * 0.1, d[1] * 0.1 + len.sqrt() * 0.08, d[2] * 0.1];
            for (item, count) in loot {
                self.spawn_item(
                    pr.pos,
                    vel,
                    Stack {
                        item,
                        count,
                        ..Stack::default()
                    },
                    0,
                );
            }
            let xp = self.rng.gen_range(1..=6);
            self.spawn_xp([feet[0], feet[1] + 0.5, feet[2] + 0.5], xp);
            if self.players[&eid].gamemode != 1 && slot >= 36 {
                self.wear_held(eid, 1);
            }
        }
    }

    fn set_biting(&mut self, id: i32, on: bool) {
        let mut m = Writer::with_id(cb::SET_ENTITY_DATA);
        m.varint(id).u8(9).varint(8).bool(on).u8(0xff);
        self.send_to_viewers(id, &m);
    }

    pub(super) fn tick_bobber(&mut self, mut a: Proj) {
        let id = a.eid;
        let mut f = a.fish.unwrap_or_default();
        a.age += 1;
        // the angler must stay close, alive and keep the rod in hand
        let keep = self.players.get(&a.owner).is_some_and(|p| {
            let s = p.inv[36 + p.held];
            let off = p.inv[45];
            let rod = |s: Stack| {
                reg::ITEM.get(s.item as usize).copied() == Some("fishing_rod") && s.count > 0
            };
            let d2: f64 = (0..3).map(|i| (p.pos[i] - a.pos[i]).powi(2)).sum();
            !p.dead && (rod(s) || rod(off)) && d2 < 32.0 * 32.0
        });
        if !keep || a.age > 6000 {
            self.untrack(id);
            return;
        }
        let c = [
            a.pos[0].floor() as i32,
            a.pos[1].floor() as i32,
            a.pos[2].floor() as i32,
        ];
        let st = BlockState(self.world.get_block(c[0], c[1], c[2]));
        // top of the water column the bobber is in (it floats relative to the top cell)
        let surface = if st.name() == "water" {
            let mut top = c[1];
            while BlockState(self.world.get_block(c[0], top + 1, c[2])).name() == "water"
                && top < c[1] + 16
            {
                top += 1;
            }
            Some(top as f64 + 8.0 / 9.0)
        } else {
            None
        };
        if let Some(h) = surface {
            if !f.in_water {
                f.in_water = true;
                f.wait = self.new_wait(f.lure);
                f.approach = self.rng.gen_range(20..=80);
            }
            // buoyancy towards ~0.15 under the cell top (measured float height), damped
            let target = h + 1.0 / 9.0 - 0.15;
            a.vel[1] = (a.vel[1] * 0.6 + (target - a.pos[1]) * 0.15).clamp(-0.4, 0.4);
            a.vel[0] *= 0.8;
            a.vel[2] *= 0.8;
            if f.wait > 0 {
                f.wait -= 1;
            } else if f.approach > 0 {
                f.approach -= 1;
                if f.approach == 0 {
                    f.bite = self.rng.gen_range(20..=40);
                    self.set_biting(id, true);
                    let pitch = 1.0 + (self.rng.gen::<f32>() - self.rng.gen::<f32>()) * 0.4;
                    self.play_sound("entity.fishing_bobber.splash", 6, a.pos, 0.25, pitch);
                }
            } else if f.bite > 0 {
                f.bite -= 1;
                if f.bite == 0 {
                    self.set_biting(id, false);
                    f.wait = self.new_wait(f.lure);
                    f.approach = self.rng.gen_range(20..=80);
                }
            }
        } else if !f.in_ground {
            a.vel[1] -= 0.03;
            for v in a.vel.iter_mut() {
                *v *= 0.92;
            }
        }
        if !f.in_ground {
            // move, stopping on the first block box along the way
            let (p, d) = (a.pos, a.vel);
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            let steps = (len / 0.1).ceil().max(1.0) as usize;
            let mut boxes = Vec::new();
            let mut hit: Option<f64> = None;
            'outer: for i in 1..=steps {
                let t = i as f64 / steps as f64;
                let q = [p[0] + d[0] * t, p[1] + d[1] * t, p[2] + d[2] * t];
                let b = [
                    q[0].floor() as i32,
                    q[1].floor() as i32,
                    q[2].floor() as i32,
                ];
                block_boxes(self.world.get_block(b[0], b[1], b[2]), &mut boxes);
                for bx in &boxes {
                    let w = [
                        b[0] as f64 + bx[0],
                        b[1] as f64 + bx[1],
                        b[2] as f64 + bx[2],
                        b[0] as f64 + bx[3],
                        b[1] as f64 + bx[4],
                        b[2] as f64 + bx[5],
                    ];
                    if let Some(tt) = super::arrow::seg_aabb(p, d, w, 0.0) {
                        hit = Some(tt);
                        break 'outer;
                    }
                }
            }
            let t = hit.unwrap_or(1.0);
            for k in 0..3 {
                a.pos[k] = p[k] + d[k] * t;
            }
            if hit.is_some() {
                f.in_ground = surface.is_none();
                a.vel = [0.0; 3];
            }
        }
        a.fish = Some(f);
        if a.age.is_multiple_of(5) {
            let mut w = Writer::with_id(cb::ENTITY_POSITION_SYNC);
            w.varint(id)
                .f64(a.pos[0])
                .f64(a.pos[1])
                .f64(a.pos[2])
                .f64(a.vel[0])
                .f64(a.vel[1])
                .f64(a.vel[2])
                .f32(0.0)
                .f32(0.0)
                .bool(false);
            self.send_to_viewers(id, &w);
            let mut m = Writer::with_id(cb::SET_ENTITY_MOTION);
            m.varint(id);
            write_lpvec3(&mut m, a.vel);
            self.send_to_viewers(id, &m);
        }
        self.projs.insert(id, a);
    }
}

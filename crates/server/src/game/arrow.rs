//! Arrows: flight physics (gravity 0.05, drag 0.99 / 0.6 in water), block and entity hits,
//! sticking in blocks. add_entity's data field carries the shooter's entity id (measured).
use super::health::DamageSource;
use super::mob::{angle, is_water, write_lpvec3};
use super::Game;
use basalt_data::{registries as reg, registry_id};
use basalt_protocol::{ids::play::clientbound as cb, Writer};
use rand::Rng;
use uuid::Uuid;

pub struct Arrow {
    pub eid: i32,
    pub uuid: Uuid,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub owner: Option<i32>,
    pub base_damage: f64,
    pub in_ground: Option<[i32; 3]>,
    pub life: u32,
    pub crit: bool,
    /// entity type: "arrow" or "trident"
    pub kind: &'static str,
    /// what picking it up gives (players' arrows and tridents); None = not collectable
    pub item: Option<super::Stack>,
    /// shot from a crossbow (hit sound item.crossbow.hit)
    pub crossbow: bool,
    /// Flame: burning (shared flag 0x01), sets the target on fire for 5 s
    pub fire: bool,
    /// Punch level: extra knockback 0.6 per level along the flight direction (measured: Punch II 0.4 -> 1.6)
    pub punch: u8,
    /// tipped arrow: effects (applied at 1/8 duration) and the potion colour (entity data 11)
    pub fx: Vec<(&'static str, u8, i32)>,
    pub color: Option<i32>,
    /// Piercing level (entity data 9) and the entities already passed through
    pub pierce: u8,
    pub pierced: Vec<i32>,
}

impl Arrow {
    pub fn rot(&self) -> (f32, f32) {
        let h = (self.vel[0] * self.vel[0] + self.vel[2] * self.vel[2]).sqrt();
        (
            self.vel[0].atan2(self.vel[2]).to_degrees() as f32,
            self.vel[1].atan2(h).to_degrees() as f32,
        )
    }
    pub fn spawn_packets(&self) -> Vec<Writer> {
        let (yaw, pitch) = self.rot();
        let mut a = Writer::with_id(cb::ADD_ENTITY);
        a.varint(self.eid)
            .uuid(self.uuid)
            .varint(registry_id(reg::ENTITY_TYPE, self.kind).unwrap_or(0))
            .f64(self.pos[0])
            .f64(self.pos[1])
            .f64(self.pos[2]);
        write_lpvec3(
            &mut a,
            if self.in_ground.is_some() {
                [0.0; 3]
            } else {
                self.vel
            },
        );
        a.u8(angle(pitch))
            .u8(angle(yaw))
            .u8(0)
            .varint(self.owner.unwrap_or(0));
        let mut v = vec![Writer::with_id(cb::BUNDLE_DELIMITER), a];
        if self.in_ground.is_some()
            || self.crit
            || self.fire
            || self.color.is_some()
            || self.pierce > 0
        {
            let mut d = Writer::with_id(cb::SET_ENTITY_DATA);
            d.varint(self.eid);
            if self.fire {
                d.u8(0).varint(0).u8(0x01);
            }
            if self.crit {
                d.u8(8).varint(0).u8(1);
            }
            if self.pierce > 0 && self.in_ground.is_none() {
                d.u8(9).varint(0).u8(self.pierce);
            }
            if let Some(c) = self.color {
                d.u8(11).varint(1).varint(c);
            }
            if self.in_ground.is_some() {
                d.u8(10).varint(8).bool(true);
            }
            d.u8(0xff);
            v.push(d);
        }
        v.push(Writer::with_id(cb::BUNDLE_DELIMITER));
        v
    }
}

/// Segment p→p+d against an AABB (inflated by `grow`): entry fraction in [0,1].
pub fn seg_aabb(p: [f64; 3], d: [f64; 3], bb: [f64; 6], grow: f64) -> Option<f64> {
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for a in 0..3 {
        let (lo, hi) = (bb[a] - grow, bb[a + 3] + grow);
        if d[a].abs() < 1e-12 {
            if p[a] < lo || p[a] > hi {
                return None;
            }
            continue;
        }
        let (mut a0, mut a1) = ((lo - p[a]) / d[a], (hi - p[a]) / d[a]);
        if a0 > a1 {
            std::mem::swap(&mut a0, &mut a1);
        }
        t0 = t0.max(a0);
        t1 = t1.min(a1);
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}

impl Game {
    /// Shoot an arrow from `from` towards `dir` (normalised inside) at `speed` blocks/tick with
    /// gaussian `inaccuracy` like a bow.
    pub(super) fn shoot_arrow(
        &mut self,
        owner: i32,
        from: [f64; 3],
        dir: [f64; 3],
        speed: f64,
        inaccuracy: f64,
        damage: f64,
    ) {
        self.shoot_projectile(
            owner, from, dir, speed, inaccuracy, damage, "arrow", false, None,
        );
    }

    /// Arrow-like projectile (arrow / trident) fired by `owner`; returns its entity id.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn shoot_projectile(
        &mut self,
        owner: i32,
        from: [f64; 3],
        dir: [f64; 3],
        speed: f64,
        inaccuracy: f64,
        damage: f64,
        kind: &'static str,
        crit: bool,
        item: Option<super::Stack>,
    ) -> i32 {
        self.shoot_projectile_with(
            owner,
            from,
            dir,
            speed,
            inaccuracy,
            damage,
            kind,
            crit,
            item,
            |_| {},
        )
    }

    /// shoot_projectile with a hook to set more fields before the spawn is sent.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn shoot_projectile_with(
        &mut self,
        owner: i32,
        from: [f64; 3],
        dir: [f64; 3],
        speed: f64,
        inaccuracy: f64,
        damage: f64,
        kind: &'static str,
        crit: bool,
        item: Option<super::Stack>,
        cfg: impl FnOnce(&mut Arrow),
    ) -> i32 {
        let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2])
            .sqrt()
            .max(1e-9);
        let mut v = [dir[0] / len, dir[1] / len, dir[2] / len];
        for c in v.iter_mut() {
            let g: f64 = self.rng.sample(rand_distr_normal());
            *c += g * 0.0075 * inaccuracy;
        }
        let v = [v[0] * speed, v[1] * speed, v[2] * speed];
        let eid = self
            .sh
            .next_eid
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut arrow = Arrow {
            eid,
            uuid: Uuid::new_v4(),
            pos: from,
            vel: v,
            owner: Some(owner),
            base_damage: damage,
            in_ground: None,
            life: 0,
            crit,
            kind,
            item,
            crossbow: false,
            fire: false,
            punch: 0,
            fx: Vec::new(),
            color: None,
            pierce: 0,
            pierced: Vec::new(),
        };
        cfg(&mut arrow);
        self.arrows.insert(eid, arrow);
        self.sync_viewers_for(eid);
        eid
    }

    pub(super) fn tick_arrows(&mut self) {
        let ids: Vec<i32> = self.arrows.keys().copied().collect();
        for id in ids {
            let Some(mut a) = self.arrows.remove(&id) else {
                continue;
            };
            if self
                .world
                .chunk(
                    (a.pos[0].floor() as i32) >> 4,
                    (a.pos[2].floor() as i32) >> 4,
                )
                .is_none()
            {
                self.arrows.insert(id, a);
                continue;
            }
            a.life += 1;
            if let Some(b) = a.in_ground {
                // stuck: fall out if the block is gone, despawn after a minute
                if !basalt_world::props::props(self.world.get_block(b[0], b[1], b[2]))
                    .blocks_motion()
                {
                    a.in_ground = None;
                    a.vel = [0.0; 3];
                    a.life = 0;
                }
                if a.life > 1200 {
                    self.untrack(id);
                    continue;
                }
                if let Some(item) = a.item {
                    let near = self
                        .players
                        .values()
                        .find(|pl| {
                            !pl.dead
                                && pl.gamemode != 3
                                && (pl.pos[0] - a.pos[0]).abs() < 1.0
                                && (pl.pos[2] - a.pos[2]).abs() < 1.0
                                && a.pos[1] > pl.pos[1] - 0.5
                                && a.pos[1] < pl.pos[1] + 2.3
                        })
                        .map(|pl| (pl.eid, pl.gamemode));
                    if let Some((pe, gm)) = near {
                        let ok = gm == 1 || self.give_stack(pe, item) == 0;
                        if ok {
                            let mut w = Writer::with_id(cb::TAKE_ITEM_ENTITY);
                            w.varint(id).varint(pe).varint(1);
                            self.broadcast(None, || Writer { buf: w.buf.clone() });
                            let pv = 1.0 + self.rng.gen_range(-0.2..0.2);
                            self.play_sound("entity.item.pickup", 7, a.pos, 0.2, pv);
                            self.untrack(id);
                            continue;
                        }
                    }
                }
                self.arrows.insert(id, a);
                continue;
            }
            if a.life > 1200 || a.pos[1] < basalt_world::MIN_Y as f64 - 64.0 {
                self.untrack(id);
                continue;
            }
            let (p, d) = (a.pos, a.vel);
            // first block hit along the segment (sampled every 0.1 block)
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            let steps = (len / 0.1).ceil().max(1.0) as usize;
            let mut block_hit: Option<(f64, [i32; 3])> = None;
            let mut boxes = Vec::new();
            'outer: for i in 1..=steps {
                let t = i as f64 / steps as f64;
                let q = [p[0] + d[0] * t, p[1] + d[1] * t, p[2] + d[2] * t];
                let b = [
                    q[0].floor() as i32,
                    q[1].floor() as i32,
                    q[2].floor() as i32,
                ];
                super::mob::block_boxes(self.world.get_block(b[0], b[1], b[2]), &mut boxes);
                for bx in &boxes {
                    let w = [
                        b[0] as f64 + bx[0],
                        b[1] as f64 + bx[1],
                        b[2] as f64 + bx[2],
                        b[0] as f64 + bx[3],
                        b[1] as f64 + bx[4],
                        b[2] as f64 + bx[5],
                    ];
                    if let Some(tt) = seg_aabb(p, d, w, 0.0) {
                        block_hit = Some((tt, b));
                        break 'outer;
                    }
                }
            }
            let limit = block_hit.map(|h| h.0).unwrap_or(1.0);
            // entities (skip the shooter for the first few ticks)
            let mut ent_hit: Option<(f64, i32, bool)> = None;
            for m in self.mobs.values() {
                if !m.alive()
                    || (Some(m.eid) == a.owner && a.life < 5)
                    || a.pierced.contains(&m.eid)
                {
                    continue;
                }
                if let Some(t) = seg_aabb(p, d, m.aabb(), 0.3) {
                    if t <= limit && ent_hit.map(|h| t < h.0).unwrap_or(true) {
                        ent_hit = Some((t, m.eid, false));
                    }
                }
            }
            for pl in self.players.values() {
                if pl.dead
                    || pl.gamemode == 3
                    || Some(pl.eid) == a.owner
                    || a.pierced.contains(&pl.eid)
                {
                    continue;
                }
                let bb = [
                    pl.pos[0] - 0.3,
                    pl.pos[1],
                    pl.pos[2] - 0.3,
                    pl.pos[0] + 0.3,
                    pl.pos[1] + 1.8,
                    pl.pos[2] + 0.3,
                ];
                if let Some(t) = seg_aabb(p, d, bb, 0.3) {
                    if t <= limit && ent_hit.map(|h| t < h.0).unwrap_or(true) {
                        ent_hit = Some((t, pl.eid, true));
                    }
                }
            }
            if let Some((_, target, is_player)) = ent_hit {
                let mut dmg = if a.kind == "trident" {
                    8.0
                } else {
                    (len * a.base_damage).ceil()
                };
                if a.crit {
                    dmg += self.rng.gen_range(0.0..dmg / 2.0 + 2.0).floor();
                }
                let owner_name = a.owner.and_then(|o| {
                    self.mobs
                        .get(&o)
                        .map(|m| m.kind.display().to_string())
                        .or_else(|| self.players.get(&o).map(|p| p.name.clone()))
                });
                let push = {
                    let h = (d[0] * d[0] + d[2] * d[2]).sqrt().max(1e-6);
                    [d[0] / h * 0.6, d[2] / h * 0.6]
                };
                if is_player {
                    let yaw = (-push[0]).atan2(-push[1]).to_degrees() as f32;
                    // arrows shot by mobs scale with difficulty (easy: min(d/2+1, d)); a player's own arrows don't
                    let from_mob = a.owner.map(|o| self.mobs.contains_key(&o)).unwrap_or(false);
                    let dmg = if from_mob {
                        basalt_game::difficulty_scaled(dmg as f32, self.difficulty) as f64
                    } else {
                        dmg
                    };
                    self.hurt_by(
                        target,
                        dmg as f32,
                        a.kind,
                        DamageSource {
                            cause: a.owner,
                            direct: Some(id),
                            killer: owner_name,
                            hurt_yaw: Some(yaw),
                        },
                    );
                    self.knock_player(target, [push[0], 0.1, push[1]]);
                } else {
                    self.damage_mob(
                        target,
                        dmg as f32,
                        a.kind,
                        a.owner,
                        Some(id),
                        Some([-push[0], -push[1]]),
                    );
                }
                self.arrow_hit_effects(&a, target, is_player, d);
                if a.kind == "trident" {
                    // bounces off the target and falls (measured: it ends up lying on the ground, collectable)
                    self.play_sound("item.trident.hit", 7, p, 1.0, 1.0);
                    a.vel = [d[0] * -0.01, d[1] * -0.1, d[2] * -0.01];
                    self.arrows.insert(id, a);
                    continue;
                }
                self.play_sound("entity.arrow.hit", 7, p, 1.0, 1.1);
                if a.pierce > 0 {
                    // Piercing: keep flying through (measured: Piercing II goes through a cow into a second one)
                    a.pierced.push(target);
                    if (a.pierced.len() as u8) <= a.pierce {
                        a.pos = [a.pos[0] + d[0], a.pos[1] + d[1], a.pos[2] + d[2]];
                        a.vel[1] -= 0.05;
                        self.arrows.insert(id, a);
                        continue;
                    }
                }
                self.untrack(id);
                continue;
            }
            if let Some((t, b)) = block_hit {
                for k in 0..3 {
                    a.pos[k] = p[k] + d[k] * t - d[k] / len.max(1e-6) * 0.05;
                }
                a.in_ground = Some(b);
                a.life = 0;
                let (yaw, pitch) = a.rot();
                let mut w = Writer::with_id(cb::ENTITY_POSITION_SYNC);
                w.varint(id)
                    .f64(a.pos[0])
                    .f64(a.pos[1])
                    .f64(a.pos[2])
                    .f64(0.0)
                    .f64(0.0)
                    .f64(0.0)
                    .f32(yaw)
                    .f32(pitch)
                    .bool(false);
                self.send_to_viewers(id, &w);
                let mut m = Writer::with_id(cb::SET_ENTITY_DATA);
                m.varint(id);
                if a.crit || a.crossbow {
                    m.u8(8).varint(0).u8(0);
                }
                if a.pierce > 0 {
                    // vanilla clears the pierce level when the arrow sticks (measured)
                    m.u8(9).varint(0).u8(0);
                    a.pierce = 0;
                }
                m.u8(10).varint(8).bool(true).u8(0xff);
                a.crit = false;
                self.send_to_viewers(id, &m);
                a.vel = [0.0; 3];
                if a.crossbow {
                    // measured (crossbow scenario): category 6, pitch 1.18 / 1.32 -> 1.2 / (0.9 + 0.2 r)
                    let pitch = 1.2 / (self.rng.gen::<f32>() * 0.2 + 0.9);
                    self.play_sound("item.crossbow.hit", 6, a.pos, 1.0, pitch);
                } else {
                    self.play_sound("entity.arrow.hit", 7, a.pos, 1.0, 1.2);
                }
                self.arrows.insert(id, a);
                continue;
            }
            for k in 0..3 {
                a.pos[k] += d[k];
            }
            let drag = if is_water(&self.world, a.pos[0], a.pos[1], a.pos[2]) {
                0.6
            } else {
                0.99
            };
            for k in 0..3 {
                a.vel[k] *= drag;
            }
            a.vel[1] -= 0.05;
            if a.life % 10 == 0 {
                let (yaw, pitch) = a.rot();
                let mut w = Writer::with_id(cb::ENTITY_POSITION_SYNC);
                w.varint(id)
                    .f64(a.pos[0])
                    .f64(a.pos[1])
                    .f64(a.pos[2])
                    .f64(a.vel[0])
                    .f64(a.vel[1])
                    .f64(a.vel[2])
                    .f32(yaw)
                    .f32(pitch)
                    .bool(false);
                self.send_to_viewers(id, &w);
            }
            self.arrows.insert(id, a);
        }
    }
}

impl Game {
    /// Flame, Punch, spectral glowing and tipped-arrow effects on the entity an arrow just hit.
    fn arrow_hit_effects(&mut self, a: &Arrow, target: i32, is_player: bool, d: [f64; 3]) {
        if a.fire {
            if is_player {
                if let Some(p) = self.players.get_mut(&target) {
                    p.fire_ticks = p.fire_ticks.max(100);
                }
            } else if let Some(m) = self.mobs.get_mut(&target) {
                m.fire_ticks = m.fire_ticks.max(100);
            }
        }
        if a.punch > 0 {
            let h = (d[0] * d[0] + d[2] * d[2]).sqrt().max(1e-6);
            let k = a.punch as f64 * 0.6;
            let push = [d[0] / h * k, 0.1, d[2] / h * k];
            if is_player {
                self.knock_player(target, push);
            } else if let Some(m) = self.mobs.get_mut(&target) {
                if m.alive() {
                    m.vel = [m.vel[0] + push[0], m.vel[1] + push[1], m.vel[2] + push[2]];
                    let v = m.vel;
                    let mut w = Writer::with_id(cb::SET_ENTITY_MOTION);
                    w.varint(target);
                    write_lpvec3(&mut w, v);
                    self.send_to_viewers(target, &w);
                }
            }
        }
        let mut fx: Vec<(&'static str, u8, i32)> = a.fx.clone();
        if a.kind == "spectral_arrow" {
            fx.push(("glowing", 0, 200 * 8));
        }
        for (name, amp, ticks) in fx {
            match name {
                "instant_health" | "instant_damage" => {
                    let harm = name == "instant_damage";
                    if is_player {
                        if harm {
                            let src = DamageSource {
                                cause: a.owner,
                                direct: Some(a.eid),
                                killer: a
                                    .owner
                                    .and_then(|o| self.players.get(&o).map(|p| p.name.clone())),
                                hurt_yaw: None,
                            };
                            self.hurt_by(target, (6u32 << amp) as f32, "indirect_magic", src);
                        } else if let Some(p) = self.players.get_mut(&target) {
                            p.health = (p.health + (4u32 << amp) as f32).min(20.0);
                            self.send_health(target);
                        }
                    } else {
                        self.instant_on_mob(target, harm, amp, 1.0, a.owner, Some(a.eid));
                    }
                }
                _ => {
                    let t = (ticks / 8).max(1);
                    if is_player {
                        self.add_effect(target, name, amp, t, true);
                    } else {
                        self.add_mob_effect(target, name, amp, t);
                    }
                }
            }
        }
    }
}

/// Standard normal sampler via Box-Muller (avoids a rand_distr dependency).
pub fn rand_distr_normal() -> Normal {
    Normal
}
pub struct Normal;
impl rand::distributions::Distribution<f64> for Normal {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> f64 {
        let u1: f64 = rng.gen_range(1e-12..1.0);
        let u2: f64 = rng.gen();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
}

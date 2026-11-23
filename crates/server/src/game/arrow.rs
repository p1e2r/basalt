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
}

impl Arrow {
    pub fn rot(&self) -> (f32, f32) {
        let h = (self.vel[0] * self.vel[0] + self.vel[2] * self.vel[2]).sqrt();
        (self.vel[0].atan2(self.vel[2]).to_degrees() as f32, self.vel[1].atan2(h).to_degrees() as f32)
    }
    pub fn spawn_packets(&self) -> Vec<Writer> {
        let (yaw, pitch) = self.rot();
        let mut a = Writer::with_id(cb::ADD_ENTITY);
        a.varint(self.eid).uuid(self.uuid).varint(registry_id(reg::ENTITY_TYPE, "arrow").unwrap_or(0)).f64(self.pos[0]).f64(self.pos[1]).f64(self.pos[2]);
        write_lpvec3(&mut a, if self.in_ground.is_some() { [0.0; 3] } else { self.vel });
        a.u8(angle(pitch)).u8(angle(yaw)).u8(0).varint(self.owner.unwrap_or(0));
        let mut v = vec![Writer::with_id(cb::BUNDLE_DELIMITER), a];
        if self.in_ground.is_some() || self.crit {
            let mut d = Writer::with_id(cb::SET_ENTITY_DATA); d.varint(self.eid);
            if self.crit { d.u8(8).varint(0).u8(1); }
            if self.in_ground.is_some() { d.u8(10).varint(8).bool(true); }
            d.u8(0xff); v.push(d);
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
        if d[a].abs() < 1e-12 { if p[a] < lo || p[a] > hi { return None; } continue; }
        let (mut a0, mut a1) = ((lo - p[a]) / d[a], (hi - p[a]) / d[a]);
        if a0 > a1 { std::mem::swap(&mut a0, &mut a1); }
        t0 = t0.max(a0); t1 = t1.min(a1);
        if t0 > t1 { return None; }
    }
    Some(t0)
}

impl Game {
    /// Shoot an arrow from `from` towards `dir` (normalised inside) at `speed` blocks/tick with
    /// gaussian `inaccuracy` like a bow.
    pub(super) fn shoot_arrow(&mut self, owner: i32, from: [f64; 3], dir: [f64; 3], speed: f64, inaccuracy: f64, damage: f64) {
        let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt().max(1e-9);
        let mut v = [dir[0] / len, dir[1] / len, dir[2] / len];
        for c in v.iter_mut() { let g: f64 = self.rng.sample(rand_distr_normal()); *c += g * 0.0075 * inaccuracy; }
        let v = [v[0] * speed, v[1] * speed, v[2] * speed];
        let eid = self.sh.next_eid.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.arrows.insert(eid, Arrow { eid, uuid: Uuid::new_v4(), pos: from, vel: v, owner: Some(owner), base_damage: damage, in_ground: None, life: 0, crit: false });
        self.sync_viewers_for(eid);
    }

    pub(super) fn tick_arrows(&mut self) {
        let ids: Vec<i32> = self.arrows.keys().copied().collect();
        for id in ids {
            let Some(mut a) = self.arrows.remove(&id) else { continue };
            if self.world.chunk((a.pos[0].floor() as i32) >> 4, (a.pos[2].floor() as i32) >> 4).is_none() { self.arrows.insert(id, a); continue; }
            a.life += 1;
            if let Some(b) = a.in_ground {
                // stuck: fall out if the block is gone, despawn after a minute
                if !basalt_world::props::props(self.world.get_block(b[0], b[1], b[2])).blocks_motion() { a.in_ground = None; a.vel = [0.0; 3]; a.life = 0; }
                if a.life > 1200 { self.untrack(id); continue; }
                self.arrows.insert(id, a); continue;
            }
            if a.life > 1200 || a.pos[1] < basalt_world::MIN_Y as f64 - 64.0 { self.untrack(id); continue; }
            let (p, d) = (a.pos, a.vel);
            // first block hit along the segment (sampled every 0.1 block)
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            let steps = (len / 0.1).ceil().max(1.0) as usize;
            let mut block_hit: Option<(f64, [i32; 3])> = None;
            let mut boxes = Vec::new();
            'outer: for i in 1..=steps {
                let t = i as f64 / steps as f64;
                let q = [p[0] + d[0] * t, p[1] + d[1] * t, p[2] + d[2] * t];
                let b = [q[0].floor() as i32, q[1].floor() as i32, q[2].floor() as i32];
                super::mob::block_boxes(self.world.get_block(b[0], b[1], b[2]), &mut boxes);
                for bx in &boxes {
                    let w = [b[0] as f64 + bx[0], b[1] as f64 + bx[1], b[2] as f64 + bx[2], b[0] as f64 + bx[3], b[1] as f64 + bx[4], b[2] as f64 + bx[5]];
                    if let Some(tt) = seg_aabb(p, d, w, 0.0) { block_hit = Some((tt, b)); break 'outer; }
                }
            }
            let limit = block_hit.map(|h| h.0).unwrap_or(1.0);
            // entities (skip the shooter for the first few ticks)
            let mut ent_hit: Option<(f64, i32, bool)> = None;
            for m in self.mobs.values() {
                if !m.alive() || (Some(m.eid) == a.owner && a.life < 5) { continue; }
                if let Some(t) = seg_aabb(p, d, m.aabb(), 0.3) { if t <= limit && ent_hit.map(|h| t < h.0).unwrap_or(true) { ent_hit = Some((t, m.eid, false)); } }
            }
            for pl in self.players.values() {
                if pl.dead || pl.gamemode == 3 || Some(pl.eid) == a.owner { continue; }
                let bb = [pl.pos[0] - 0.3, pl.pos[1], pl.pos[2] - 0.3, pl.pos[0] + 0.3, pl.pos[1] + 1.8, pl.pos[2] + 0.3];
                if let Some(t) = seg_aabb(p, d, bb, 0.3) { if t <= limit && ent_hit.map(|h| t < h.0).unwrap_or(true) { ent_hit = Some((t, pl.eid, true)); } }
            }
            if let Some((_, target, is_player)) = ent_hit {
                let mut dmg = (len * a.base_damage).ceil();
                if a.crit { dmg += self.rng.gen_range(0.0..dmg / 2.0 + 2.0).floor(); }
                let owner_name = a.owner.and_then(|o| self.mobs.get(&o).map(|m| m.kind.display().to_string()).or_else(|| self.players.get(&o).map(|p| p.name.clone())));
                let push = { let h = (d[0] * d[0] + d[2] * d[2]).sqrt().max(1e-6); [d[0] / h * 0.6, d[2] / h * 0.6] };
                if is_player {
                    let yaw = (-push[0]).atan2(-push[1]).to_degrees() as f32;
                    self.hurt_by(target, dmg as f32, "arrow", DamageSource { cause: a.owner, direct: Some(id), killer: owner_name, hurt_yaw: Some(yaw) });
                    self.knock_player(target, [push[0], 0.1, push[1]]);
                } else {
                    self.damage_mob(target, dmg as f32, "arrow", a.owner, Some(id), Some([-push[0], -push[1]]));
                }
                self.play_sound("entity.arrow.hit", 7, p, 1.0, 1.1);
                self.untrack(id);
                continue;
            }
            if let Some((t, b)) = block_hit {
                for k in 0..3 { a.pos[k] = p[k] + d[k] * t - d[k] / len.max(1e-6) * 0.05; }
                a.in_ground = Some(b); a.life = 0;
                let (yaw, pitch) = a.rot();
                let mut w = Writer::with_id(cb::ENTITY_POSITION_SYNC);
                w.varint(id).f64(a.pos[0]).f64(a.pos[1]).f64(a.pos[2]).f64(0.0).f64(0.0).f64(0.0).f32(yaw).f32(pitch).bool(false);
                self.send_to_viewers(id, &w);
                let mut m = Writer::with_id(cb::SET_ENTITY_DATA); m.varint(id).u8(10).varint(8).bool(true).u8(0xff);
                self.send_to_viewers(id, &m);
                a.vel = [0.0; 3];
                self.play_sound("entity.arrow.hit", 7, a.pos, 1.0, 1.2);
                self.arrows.insert(id, a);
                continue;
            }
            for k in 0..3 { a.pos[k] += d[k]; }
            let drag = if is_water(&self.world, a.pos[0], a.pos[1], a.pos[2]) { 0.6 } else { 0.99 };
            for k in 0..3 { a.vel[k] *= drag; }
            a.vel[1] -= 0.05;
            if a.life % 10 == 0 {
                let (yaw, pitch) = a.rot();
                let mut w = Writer::with_id(cb::ENTITY_POSITION_SYNC);
                w.varint(id).f64(a.pos[0]).f64(a.pos[1]).f64(a.pos[2]).f64(a.vel[0]).f64(a.vel[1]).f64(a.vel[2]).f32(yaw).f32(pitch).bool(false);
                self.send_to_viewers(id, &w);
            }
            self.arrows.insert(id, a);
        }
    }
}

/// Standard normal sampler via Box-Muller (avoids a rand_distr dependency).
pub fn rand_distr_normal() -> Normal { Normal }
pub struct Normal;
impl rand::distributions::Distribution<f64> for Normal {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> f64 {
        let u1: f64 = rng.gen_range(1e-12..1.0); let u2: f64 = rng.gen();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
}

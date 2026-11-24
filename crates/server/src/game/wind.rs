//! Wind charge burst. Measured on vanilla 26.2 (2026-11-24, thrower standing on a floor, charge thrown at it):
//! explode packet centred 0.25 above the floor (hit position + face normal * 0.25), radius 1.2, particle
//! gust_emitter_small, sound entity.wind_charge.wind_burst; the knockback sent to a player points from the
//! centre to the player's eyes with magnitude (1 - |feet - centre| / 2.4) * 1.22 (measured 0.7192 against 0.7189
//! from the formula); no block damage. Mobs get the same velocity change.
use super::proj::Proj;
use super::Game;
use basalt_data::{registries as reg, registry_id};
use basalt_protocol::{ids::play::clientbound as cb, Writer};

const RADIUS: f64 = 1.2;
const KNOCKBACK: f64 = 1.22;

impl Game {
    pub(super) fn wind_burst(&mut self, a: &Proj) {
        let c = [
            a.hit_pos[0] + a.hit_normal[0] * 0.25,
            a.hit_pos[1] + a.hit_normal[1] * 0.25,
            a.hit_pos[2] + a.hit_normal[2] * 0.25,
        ];
        let kb_for = |feet: [f64; 3], eye: [f64; 3]| -> Option<[f64; 3]> {
            let d =
                ((feet[0] - c[0]).powi(2) + (feet[1] - c[1]).powi(2) + (feet[2] - c[2]).powi(2))
                    .sqrt();
            let f = 1.0 - d / (2.0 * RADIUS);
            if f <= 0.0 {
                return None;
            }
            let v = [eye[0] - c[0], eye[1] - c[1], eye[2] - c[2]];
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-9);
            let k = f * KNOCKBACK / l;
            Some([v[0] * k, v[1] * k, v[2] * k])
        };
        let mut player_kb: Vec<(i32, [f64; 3])> = Vec::new();
        for p in self.players.values() {
            if p.dead || p.gamemode == 3 || (p.gamemode == 1 && p.flying) {
                continue;
            }
            let eye = [
                p.pos[0],
                p.pos[1] + if p.sneaking { 1.27 } else { 1.62 },
                p.pos[2],
            ];
            if let Some(k) = kb_for(p.pos, eye) {
                player_kb.push((p.eid, k));
            }
        }
        for (eid, _k) in &player_kb {
            if let Some(p) = self.players.get_mut(eid) {
                if p.gamemode == 1 {
                    continue;
                }
                p.fall_start = None;
            }
        }
        let mut mob_kb: Vec<(i32, [f64; 3])> = Vec::new();
        for m in self.mobs.values() {
            if !m.alive() {
                continue;
            }
            if let Some(k) = kb_for(m.pos, [m.pos[0], m.eye_y(), m.pos[2]]) {
                mob_kb.push((m.eid, k));
            }
        }
        for (eid, k) in mob_kb {
            if let Some(m) = self.mobs.get_mut(&eid) {
                for i in 0..3 {
                    m.vel[i] += k[i];
                }
                m.on_ground = false;
            }
            let vel = self.mobs.get(&eid).map(|m| m.vel).unwrap_or(k);
            let mut w = Writer::with_id(cb::SET_ENTITY_MOTION);
            w.varint(eid);
            super::mob::write_lpvec3(&mut w, vel);
            self.send_to_viewers(eid, &w);
        }
        let particle = registry_id(reg::PARTICLE_TYPE, "gust_emitter_small").unwrap_or(0);
        let sound = Self::sound_id("entity.wind_charge.wind_burst");
        let chunk = ((c[0].floor() as i32) >> 4, (c[2].floor() as i32) >> 4);
        for p in self.players.values() {
            if !p.sees_chunk(chunk) {
                continue;
            }
            let mut w = Writer::with_id(cb::EXPLODE);
            w.f64(c[0]).f64(c[1]).f64(c[2]).f32(RADIUS as f32).i32(0);
            match player_kb.iter().find(|k| k.0 == p.eid) {
                Some((_, v)) => {
                    w.bool(true).f64(v[0]).f64(v[1]).f64(v[2]);
                }
                None => {
                    w.bool(false);
                }
            }
            w.varint(particle).varint(sound).varint(0);
            p.send(w);
        }
    }
}

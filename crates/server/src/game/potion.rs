//! Splash and lingering potions landing, and the area effect cloud they leave. Measured on vanilla 26.2
//! (2026-11-24, probe over the wire):
//! * landing: level_event 2002 (2007 when every effect is instant) with the potion colour at the hit block;
//!   a thrown potion's direct hit gets full strength; others within 4 (2 vertically) get `1 - dist / 4`;
//!   durations scale by strength, instant healing / harming (`4 << amp` / `6 << amp`, damage type
//!   indirect_magic) likewise
//! * lingering: an area_effect_cloud entity at the hit position (metadata 10 = entity_effect particle +
//!   colour), radius 3, waits 10 ticks, then shrinks 0.005 per tick (radius metadata 8 sent every tick);
//!   every 5 ticks entities inside get the effects (a quarter of the duration), each entity again only after
//!   20 ticks, and every application round shrinks the radius by 0.5; gone below 0.5
use super::potions::potion;
use super::proj::{PKind, Proj};
use super::{Game, Stack};
use basalt_data::{registries as reg, registry_id};
use basalt_protocol::{ids::play::clientbound as cb, Writer};
use std::collections::HashMap;

pub struct Cloud {
    pub radius: f32,
    pub color: u32,
    pub effects: Vec<(&'static str, u8, i32)>,
    pub instant: bool,
    pub reapply: HashMap<i32, u32>,
}

/// Name of the potion held by a potion item stack ("water" without a potion component).
pub fn potion_name(s: &Stack) -> &'static str {
    if s.ench.potion == 0 {
        "water"
    } else {
        reg::POTION
            .get(s.ench.potion as usize - 1)
            .copied()
            .unwrap_or("water")
    }
}

impl Cloud {
    pub fn for_potion(s: &Stack) -> Cloud {
        let (color, effects, instant) = potion(potion_name(s)).unwrap_or((0xff385dc6, &[], false));
        Cloud {
            radius: 3.0,
            color,
            effects: effects.to_vec(),
            instant,
            reapply: HashMap::new(),
        }
    }
    pub fn spawn_data(&self, eid: i32) -> Writer {
        let mut d = Writer::with_id(cb::SET_ENTITY_DATA);
        d.varint(eid).u8(10).varint(16);
        d.varint(registry_id(reg::PARTICLE_TYPE, "entity_effect").unwrap_or(0));
        d.i32(self.color as i32);
        d.u8(0xff);
        d
    }
}

impl Game {
    /// Entities (players, then mobs) near `pos` as (eid, is_player, feet position, aabb).
    fn near_entities(&self, pos: [f64; 3], rx: f64, ry: f64) -> Vec<(i32, bool, [f64; 3])> {
        let mut v = Vec::new();
        for p in self.players.values() {
            if p.dead || p.gamemode == 3 {
                continue;
            }
            let c = [p.pos[0], p.pos[1] + 0.9, p.pos[2]];
            if (c[0] - pos[0]).abs() <= rx + 0.3
                && (c[2] - pos[2]).abs() <= rx + 0.3
                && (c[1] - pos[1]).abs() <= ry + 0.9
            {
                v.push((p.eid, true, p.pos));
            }
        }
        for m in self.mobs.values() {
            if !m.alive() {
                continue;
            }
            let c = [m.pos[0], m.pos[1] + m.height() / 2.0, m.pos[2]];
            if (c[0] - pos[0]).abs() <= rx + m.width() / 2.0
                && (c[2] - pos[2]).abs() <= rx + m.width() / 2.0
                && (c[1] - pos[1]).abs() <= ry + m.height() / 2.0
            {
                v.push((m.eid, false, m.pos));
            }
        }
        v
    }

    /// Apply a potion's effects to one entity at `strength` (1.0 = full) with durations scaled by `dur_scale`.
    #[allow(clippy::too_many_arguments)]
    fn apply_potion(
        &mut self,
        eid: i32,
        is_player: bool,
        effects: &[(&'static str, u8, i32)],
        strength: f64,
        dur_scale: f64,
        owner: i32,
        direct: i32,
    ) {
        for &(name, amp, ticks) in effects {
            match name {
                "instant_health" | "instant_damage" if !is_player => {
                    let harm = name == "instant_damage";
                    self.instant_on_mob(eid, harm, amp, strength, Some(owner), Some(direct));
                }
                "instant_health" => {
                    let heal = (strength * (4u32 << amp) as f64 + 0.5) as i32 as f32;
                    if let Some(p) = self.players.get_mut(&eid) {
                        if !p.dead {
                            p.health = (p.health + heal).min(20.0);
                        }
                    }
                    self.send_health(eid);
                }
                "instant_damage" => {
                    let dmg = (strength * (6u32 << amp) as f64 + 0.5) as i32 as f32;
                    let src = super::health::DamageSource {
                        cause: Some(owner),
                        direct: Some(direct),
                        killer: self.players.get(&owner).map(|p| p.name.clone()),
                        hurt_yaw: None,
                    };
                    self.hurt_by(eid, dmg, "indirect_magic", src);
                }
                _ => {
                    let t = (strength * ticks as f64 * dur_scale + 0.5) as i32;
                    if t > 20 {
                        if is_player {
                            self.add_effect(eid, name, amp, t, true);
                        } else {
                            self.add_mob_effect(eid, name, amp, t);
                        }
                    }
                }
            }
        }
    }

    /// A splash or lingering potion landed (block or entity hit) at `a.hit_pos`.
    pub(super) fn potion_burst(&mut self, a: &Proj, hit: Option<(i32, bool)>) {
        let pos = a.hit_pos;
        let cloud = Cloud::for_potion(&a.item);
        let event = if cloud.instant { 2007 } else { 2002 };
        let mut w = Writer::with_id(cb::LEVEL_EVENT);
        w.i32(event)
            .position(
                pos[0].floor() as i32,
                pos[1].floor() as i32,
                pos[2].floor() as i32,
            )
            .i32(cloud.color as i32)
            .bool(false);
        self.broadcast_chunk(
            ((pos[0].floor() as i32) >> 4, (pos[2].floor() as i32) >> 4),
            None,
            || Writer { buf: w.buf.clone() },
        );
        if a.kind == PKind::Lingering {
            let eid = self
                .sh
                .next_eid
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut c = Proj::new_cloud(eid, pos, a.owner, a.item);
            c.cloud = Some(cloud);
            self.projs.insert(eid, c);
            self.sync_viewers_for(eid);
            return;
        }
        for (eid, is_player, epos) in self.near_entities(pos, 4.0, 2.0) {
            let d2 = (epos[0] - pos[0]).powi(2)
                + (epos[1] - pos[1]).powi(2)
                + (epos[2] - pos[2]).powi(2);
            let strength = if hit.map(|h| h.0) == Some(eid) {
                1.0
            } else if d2 < 16.0 {
                // measured: vanilla's effective distance is ~0.58 shorter than feet-to-impact (near 3251/3600, far 411/900)
                1.0 - (d2.sqrt() - 0.58).max(0.0) / 4.0
            } else {
                continue;
            };
            self.apply_potion(
                eid,
                is_player,
                &cloud.effects,
                strength,
                1.0,
                a.owner,
                a.eid,
            );
        }
    }

    /// One tick of an area effect cloud.
    pub(super) fn tick_cloud(&mut self, mut a: Proj) {
        let id = a.eid;
        a.age += 1;
        let Some(mut cloud) = a.cloud.take() else {
            self.untrack(id);
            return;
        };
        if a.age >= 10 {
            cloud.radius -= 0.005;
            if a.age % 5 == 0 {
                let mut used = false;
                for (eid, is_player, epos) in self.near_entities(a.pos, cloud.radius as f64, 1.0) {
                    let (dx, dz) = (epos[0] - a.pos[0], epos[2] - a.pos[2]);
                    if dx * dx + dz * dz > (cloud.radius as f64).powi(2) {
                        continue;
                    }
                    if cloud.reapply.get(&eid).map(|t| *t > a.age).unwrap_or(false) {
                        continue;
                    }
                    cloud.reapply.insert(eid, a.age + 20);
                    let fx = cloud.effects.clone();
                    self.apply_potion(eid, is_player, &fx, 1.0, 0.25, a.owner, id);
                    used = true;
                }
                if used {
                    cloud.radius -= 0.5;
                }
            }
            let mut w = Writer::with_id(cb::SET_ENTITY_DATA);
            w.varint(id).u8(8).varint(3).f32(cloud.radius).u8(0xff);
            self.send_to_viewers(id, &w);
        }
        if cloud.radius < 0.5 || a.age > 6000 {
            self.untrack(id);
            return;
        }
        a.cloud = Some(cloud);
        self.projs.insert(id, a);
    }
}

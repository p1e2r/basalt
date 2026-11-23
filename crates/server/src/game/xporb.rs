//! Experience orbs: dropped by mobs killed by a player and by ores mined with a tool that can
//! harvest them; drift to the nearest player within 8 blocks and are collected on contact
//! (set_experience). add_entity data 0, the value in entity data index 8 (VarInt), spawn velocity
//! ±0.2 sideways / up to 0.2 up — all measured on vanilla 26.2 with scenario m5_xp_orbs.
use super::Game;
use basalt_data::{registries as reg, registry_id, BlockState};
use basalt_protocol::{ids::play::clientbound as cb, Writer};
use basalt_world::props;
use rand::Rng;
use uuid::Uuid;

pub struct Orb {
    pub eid: i32,
    pub uuid: Uuid,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub value: i32,
    pub age: u32,
}

/// Orb sizes vanilla splits a total into (largest first).
const SIZES: [i32; 11] = [2477, 1237, 617, 307, 149, 73, 37, 17, 7, 3, 1];

impl Orb {
    pub fn spawn_packets(&self) -> Vec<Writer> {
        let mut a = Writer::with_id(cb::ADD_ENTITY);
        a.varint(self.eid)
            .uuid(self.uuid)
            .varint(registry_id(reg::ENTITY_TYPE, "experience_orb").unwrap_or(0))
            .f64(self.pos[0])
            .f64(self.pos[1])
            .f64(self.pos[2]);
        super::mob::write_lpvec3(&mut a, self.vel);
        a.u8(0).u8(0).u8(0).varint(0);
        let mut d = Writer::with_id(cb::SET_ENTITY_DATA);
        d.varint(self.eid)
            .u8(8)
            .varint(1)
            .varint(self.value)
            .u8(0xff);
        vec![
            Writer::with_id(cb::BUNDLE_DELIMITER),
            a,
            d,
            Writer::with_id(cb::BUNDLE_DELIMITER),
        ]
    }
}

/// XP for a mob killed by a player.
pub fn mob_xp(kind: &str, rng: &mut impl Rng) -> i32 {
    match kind {
        "cow" | "pig" | "sheep" | "chicken" => rng.gen_range(1..=3),
        "zombie" | "skeleton" | "creeper" | "spider" => 5,
        _ => 0,
    }
}

/// XP for an ore mined with a tool that harvests it (no silk touch).
pub fn ore_xp(state: u32, rng: &mut impl Rng) -> i32 {
    let n = BlockState(state).name().trim_start_matches("deepslate_");
    match n {
        "coal_ore" => rng.gen_range(0..=2),
        "diamond_ore" | "emerald_ore" => rng.gen_range(3..=7),
        "lapis_ore" | "nether_quartz_ore" => rng.gen_range(2..=5),
        "redstone_ore" => rng.gen_range(1..=5),
        "nether_gold_ore" => rng.gen_range(0..=1),
        _ => 0,
    }
}

impl Game {
    /// Split `total` into orbs at `pos`.
    pub(super) fn spawn_xp(&mut self, pos: [f64; 3], mut total: i32) {
        while total > 0 {
            let v = *SIZES.iter().find(|s| **s <= total).unwrap_or(&1);
            total -= v;
            let vel = [
                (self.rng.gen::<f64>() - 0.5) * 0.4,
                self.rng.gen::<f64>() * 0.2,
                (self.rng.gen::<f64>() - 0.5) * 0.4,
            ];
            let eid = self
                .sh
                .next_eid
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.orbs.insert(
                eid,
                Orb {
                    eid,
                    uuid: Uuid::new_v4(),
                    pos,
                    vel,
                    value: v,
                    age: 0,
                },
            );
            self.sync_viewers_for(eid);
        }
    }

    pub(super) fn tick_orbs(&mut self) {
        let ids: Vec<i32> = self.orbs.keys().copied().collect();
        for id in ids {
            let Some(mut o) = self.orbs.remove(&id) else {
                continue;
            };
            o.age += 1;
            if o.age > 6000 {
                self.untrack_orb(id);
                continue;
            }
            // drift towards the nearest player within 8 blocks
            let target = self
                .players
                .values()
                .filter(|p| !p.dead)
                .map(|p| (p.eid, [p.pos[0], p.pos[1] + 0.5, p.pos[2]]))
                .min_by(|a, b| {
                    let d = |q: [f64; 3]| {
                        (q[0] - o.pos[0]).powi(2)
                            + (q[1] - o.pos[1]).powi(2)
                            + (q[2] - o.pos[2]).powi(2)
                    };
                    d(a.1).partial_cmp(&d(b.1)).unwrap()
                });
            o.vel[1] -= 0.03;
            if let Some((_, t)) = target {
                let d = [t[0] - o.pos[0], t[1] - o.pos[1], t[2] - o.pos[2]];
                let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                if len < 8.0 && len > 1e-6 {
                    let pull = (1.0 - len / 8.0).powi(2) * 0.1;
                    for (v, di) in o.vel.iter_mut().zip(d) {
                        *v += di / len * pull;
                    }
                }
            }
            let ny = o.pos[1] + o.vel[1];
            let below = self.world.get_block(
                o.pos[0].floor() as i32,
                ny.floor() as i32,
                o.pos[2].floor() as i32,
            );
            if o.vel[1] < 0.0 && props::props(below).blocks_motion() {
                o.pos[1] = ny.floor() + 1.0;
                o.vel[1] = 0.0;
                o.vel[0] *= 0.6;
                o.vel[2] *= 0.6;
            } else {
                o.pos[1] = ny;
            }
            o.pos[0] += o.vel[0];
            o.pos[2] += o.vel[2];
            for i in 0..3 {
                o.vel[i] *= 0.98;
            }
            // collect on contact (one orb per player per tick)
            let hit = self
                .players
                .values()
                .find(|p| {
                    !p.dead
                        && (p.pos[0] - o.pos[0]).abs() < 0.8
                        && (p.pos[2] - o.pos[2]).abs() < 0.8
                        && o.pos[1] > p.pos[1] - 0.5
                        && o.pos[1] < p.pos[1] + 2.3
                })
                .map(|p| p.eid);
            if let Some(pe) = hit {
                let mut w = Writer::with_id(cb::TAKE_ITEM_ENTITY);
                w.varint(id).varint(pe).varint(1);
                for p in self.players.values().filter(|p| p.seen.contains(&id)) {
                    p.send(Writer { buf: w.buf.clone() });
                }
                self.untrack_orb(id);
                self.give_xp(pe, o.value);
                continue;
            }
            if o.age % 20 == 0 {
                for p in self.players.values().filter(|p| p.seen.contains(&id)) {
                    let mut w = Writer::with_id(cb::ENTITY_POSITION_SYNC);
                    w.varint(id)
                        .f64(o.pos[0])
                        .f64(o.pos[1])
                        .f64(o.pos[2])
                        .f64(o.vel[0])
                        .f64(o.vel[1])
                        .f64(o.vel[2])
                        .f32(0.0)
                        .f32(0.0)
                        .bool(false);
                    p.send(w);
                }
            }
            self.orbs.insert(id, o);
        }
    }

    fn untrack_orb(&mut self, eid: i32) {
        for p in self.players.values_mut() {
            if p.seen.remove(&eid) {
                let mut w = Writer::with_id(cb::REMOVE_ENTITIES);
                w.varint(1).varint(eid);
                p.send(w);
            }
        }
    }
}

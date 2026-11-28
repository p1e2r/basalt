//! Explosions (creepers): the ray-marching block destruction described on the Minecraft Wiki
//! "Explosion" page, entity damage/knockback by exposure, loot with explosion decay, and the
//! `explode` packet (format measured from vanilla 26.2, see NOTES.md).
use super::health::DamageSource;
use super::mob::write_lpvec3;
use super::Game;
use basalt_data::{registries as reg, registry_id, BlockState};
use basalt_protocol::{ids::play::clientbound as cb, Writer};
use rand::Rng;
use std::collections::HashSet;

impl Game {
    /// Fraction of sample points in `bb` with a clear line to `c`.
    fn exposure(&self, c: [f64; 3], bb: [f64; 6]) -> f64 {
        let (mut seen, mut total) = (0, 0);
        for i in 0..=2 {
            for j in 0..=2 {
                for k in 0..=2 {
                    let p = [
                        bb[0] + (bb[3] - bb[0]) * i as f64 / 2.0,
                        bb[1] + (bb[4] - bb[1]) * j as f64 / 2.0,
                        bb[2] + (bb[5] - bb[2]) * k as f64 / 2.0,
                    ];
                    total += 1;
                    let d = [p[0] - c[0], p[1] - c[1], p[2] - c[2]];
                    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    let n = (len / 0.25).ceil().max(1.0) as i32;
                    let blocked = (1..n).any(|s| {
                        let t = s as f64 / n as f64;
                        basalt_world::props::props(self.world.get_block(
                            (c[0] + d[0] * t).floor() as i32,
                            (c[1] + d[1] * t).floor() as i32,
                            (c[2] + d[2] * t).floor() as i32,
                        ))
                        .blocks_motion()
                    });
                    if !blocked {
                        seen += 1;
                    }
                }
            }
        }
        seen as f64 / total as f64
    }

    /// Explode at `c` with `radius`. `source` = (entity id, display name) for damage attribution.
    pub(super) fn explode(
        &mut self,
        c: [f64; 3],
        radius: f32,
        source: Option<(i32, String)>,
        destroy_blocks: bool,
    ) {
        let r = radius as f64;
        self.crystals_in_blast(c, r);
        // ---- blocks: 16×16×16 rays from the cube surface
        let mut touched: HashSet<[i32; 3]> = HashSet::new();
        let mut destroyed: HashSet<[i32; 3]> = HashSet::new();
        for i in 0..16 {
            for j in 0..16 {
                for k in 0..16 {
                    if !(i == 0 || i == 15 || j == 0 || j == 15 || k == 0 || k == 15) {
                        continue;
                    }
                    let mut d = [
                        i as f64 / 15.0 * 2.0 - 1.0,
                        j as f64 / 15.0 * 2.0 - 1.0,
                        k as f64 / 15.0 * 2.0 - 1.0,
                    ];
                    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    for v in d.iter_mut() {
                        *v /= l;
                    }
                    let mut power = r * (0.7 + self.rng.gen::<f64>() * 0.6);
                    let mut p = c;
                    while power > 0.0 {
                        let b = [
                            p[0].floor() as i32,
                            p[1].floor() as i32,
                            p[2].floor() as i32,
                        ];
                        let st = self.world.get_block(b[0], b[1], b[2]);
                        if b[1] < basalt_world::MIN_Y
                            || b[1] >= basalt_world::MIN_Y + basalt_world::HEIGHT
                        {
                            break;
                        }
                        touched.insert(b);
                        if st != 0 {
                            let res = basalt_game::blast_resistance(BlockState(st).name()) as f64;
                            power -= (res + 0.3) * 0.3;
                            if power > 0.0 && destroy_blocks && !basalt_world::props::is_air(st) {
                                destroyed.insert(b);
                            }
                        }
                        for a in 0..3 {
                            p[a] += d[a] * 0.3;
                        }
                        power -= 0.22500001;
                    }
                }
            }
        }
        // ---- entities
        let range = r * 2.0;
        let src_eid = source.as_ref().map(|s| s.0);
        let kind = if src_eid.is_some() {
            "player_explosion"
        } else {
            "explosion"
        };
        let mut player_kb: Vec<(i32, [f64; 3])> = Vec::new();
        let victims: Vec<(i32, bool, [f64; 3], [f64; 6])> = self
            .players
            .values()
            .filter(|p| !p.dead && p.gamemode != 3)
            .map(|p| {
                (
                    p.eid,
                    true,
                    p.pos,
                    [
                        p.pos[0] - 0.3,
                        p.pos[1],
                        p.pos[2] - 0.3,
                        p.pos[0] + 0.3,
                        p.pos[1] + 1.8,
                        p.pos[2] + 0.3,
                    ],
                )
            })
            .chain(
                self.mobs
                    .values()
                    .filter(|m| m.alive())
                    .map(|m| (m.eid, false, m.pos, m.aabb())),
            )
            .collect();
        for (eid, is_player, pos, bb) in victims {
            // measured on vanilla: impact uses the distance to the entity's feet position, the
            // push direction points at its eyes
            let eye = if is_player {
                pos[1] + 1.62
            } else {
                pos[1] + (bb[4] - bb[1]) * 0.85
            };
            let feet = [pos[0] - c[0], pos[1] - c[1], pos[2] - c[2]];
            let dist = (feet[0] * feet[0] + feet[1] * feet[1] + feet[2] * feet[2]).sqrt();
            if dist / range > 1.0 {
                continue;
            }
            let d = [pos[0] - c[0], eye - c[1], pos[2] - c[2]];
            let dl = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            if dl < 1e-6 {
                continue;
            }
            let (dmg, impact) = basalt_game::explosion_damage(dist, r, self.exposure(c, bb));
            let kb = [d[0] / dl * impact, d[1] / dl * impact, d[2] / dl * impact];
            if is_player {
                let creative = self
                    .players
                    .get(&eid)
                    .map(|p| matches!(p.gamemode, 1 | 3))
                    .unwrap_or(true);
                let dmg = basalt_game::difficulty_scaled(dmg, self.difficulty);
                self.hurt_by(
                    eid,
                    dmg,
                    kind,
                    DamageSource {
                        cause: src_eid,
                        direct: src_eid,
                        killer: source.as_ref().map(|s| s.1.clone()),
                        hurt_yaw: None,
                    },
                );
                if !creative {
                    player_kb.push((eid, kb));
                }
            } else {
                self.damage_mob_kb(eid, dmg, kind, src_eid, src_eid, None, 0.0);
                if let Some(m) = self.mobs.get_mut(&eid) {
                    for a in 0..3 {
                        m.vel[a] += kb[a];
                    }
                    let v = m.vel;
                    let mut w = Writer::with_id(cb::SET_ENTITY_MOTION);
                    w.varint(eid);
                    write_lpvec3(&mut w, v);
                    self.send_to_viewers(eid, &w);
                }
            }
        }
        // ---- blocks removed, drops
        let mut drops = Vec::new();
        let mut changed_chunks: HashSet<(i32, i32)> = HashSet::new();
        for b in &destroyed {
            let st = self.world.get_block(b[0], b[1], b[2]);
            if st == 0 {
                continue;
            }
            let d = self.data.explosion_drops(st, radius, &mut self.rng);
            drops.push((*b, d));
            self.container_broken(*b, st);
            for ch in self.world.set_block(b[0], b[1], b[2], 0) {
                changed_chunks.insert(ch);
            }
            changed_chunks.insert((b[0] >> 4, b[2] >> 4));
            let (x, y, z) = (b[0], b[1], b[2]);
            self.broadcast_chunk((x >> 4, z >> 4), None, || {
                let mut w = Writer::with_id(cb::BLOCK_UPDATE);
                w.position(x, y, z).varint(0);
                w
            });
        }
        for (cx, cz) in changed_chunks {
            let Some(ch) = self.world.chunk(cx, cz) else {
                continue;
            };
            let mut body = Vec::new();
            ch.write_light(&mut body);
            self.broadcast_chunk((cx, cz), None, || {
                let mut w = Writer::with_id(cb::LIGHT_UPDATE);
                w.varint(cx).varint(cz).bytes(&body);
                w
            });
        }
        for (b, d) in drops {
            self.drop_at_block(b, d);
        }
        // ---- packet
        let particle = registry_id(
            reg::PARTICLE_TYPE,
            if radius >= 2.0 && destroy_blocks {
                "explosion_emitter"
            } else {
                "explosion"
            },
        )
        .unwrap_or(0);
        let sound = Self::sound_id("entity.generic.explode");
        let (poof, smoke) = (
            registry_id(reg::PARTICLE_TYPE, "poof").unwrap_or(0),
            registry_id(reg::PARTICLE_TYPE, "smoke").unwrap_or(0),
        );
        let n = touched.len() as i32;
        let chunk = ((c[0].floor() as i32) >> 4, (c[2].floor() as i32) >> 4);
        for p in self.players.values() {
            if !p.sees_chunk(chunk) {
                continue;
            }
            let mut w = Writer::with_id(cb::EXPLODE);
            w.f64(c[0]).f64(c[1]).f64(c[2]).f32(radius).i32(n);
            match player_kb.iter().find(|k| k.0 == p.eid) {
                Some((_, v)) => {
                    w.bool(true).f64(v[0]).f64(v[1]).f64(v[2]);
                }
                None => {
                    w.bool(false);
                }
            }
            w.varint(particle).varint(sound);
            w.varint(2)
                .varint(poof)
                .f32(0.5)
                .f32(1.0)
                .varint(1)
                .varint(smoke)
                .f32(1.0)
                .f32(1.0)
                .varint(1);
            p.send(w);
        }
    }
}

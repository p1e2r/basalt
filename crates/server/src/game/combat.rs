//! Combat: players hitting mobs (and players), mobs taking damage, knockback, death and drops,
//! and mob interactions (shearing, milking, igniting creepers).
use super::health::DamageSource;
use super::mob::{MobKind, SHEEP_COLORS};
use super::{Game, Stack, NO_ENCH};
use basalt_data::{registries as reg, registry_id};
use basalt_protocol::{ids::play::clientbound as cb, Writer};
use rand::Rng;

impl Game {
    /// Push a player: the client applies set_entity_motion for its own entity.
    pub(super) fn knock_player(&mut self, eid: i32, v: [f64; 3]) {
        let Some(p) = self.players.get(&eid) else {
            return;
        };
        if p.dead || matches!(p.gamemode, 1 | 3) {
            return;
        }
        let mut w = Writer::with_id(cb::SET_ENTITY_MOTION);
        w.varint(eid);
        super::mob::write_lpvec3(&mut w, v);
        self.broadcast(None, || Writer { buf: w.buf.clone() });
    }

    /// Damage a mob. `toward` is the horizontal direction from the mob towards the attacker
    /// (knockback pushes the other way). Returns true if it died.
    pub(super) fn damage_mob(
        &mut self,
        eid: i32,
        amount: f32,
        kind: &str,
        cause: Option<i32>,
        direct: Option<i32>,
        toward: Option<[f64; 2]>,
    ) -> bool {
        self.damage_mob_kb(eid, amount, kind, cause, direct, toward, 0.4)
    }

    pub(super) fn damage_mob_kb(
        &mut self,
        eid: i32,
        amount: f32,
        kind: &str,
        cause: Option<i32>,
        direct: Option<i32>,
        toward: Option<[f64; 2]>,
        kb: f64,
    ) -> bool {
        let dt = self.damage_type(kind);
        let tick = self.tick;
        let by_player = cause.filter(|c| self.players.contains_key(c));
        let Some(m) = self.mobs.get_mut(&eid) else {
            return false;
        };
        if !m.alive() || amount <= 0.0 {
            return false;
        }
        // during the hurt cooldown only a bigger hit lands, dealing the difference, without the hurt
        // animation, knockback or sound (measured: a tipped harming arrow = arrow 3 + harming 6 -> 6 in total)
        let mut toward = toward;
        let mut quiet = false;
        if m.hurt_cooldown > 0 && !matches!(kind, "on_fire" | "out_of_world" | "generic_kill") {
            if amount <= m.last_hurt {
                return false;
            }
            let extra = amount - m.last_hurt;
            m.last_hurt = amount;
            m.health = (m.health - extra).max(0.0);
            toward = None;
            quiet = true;
        } else {
            m.hurt_cooldown = 10;
            m.last_hurt = amount;
            m.health = (m.health - amount).max(0.0);
        }
        if let Some(p) = by_player {
            m.last_player_hit = Some((p, tick));
        }
        if !m.kind.hostile() {
            m.ai.panic_ticks = 80 + (tick % 40) as u32;
            m.ai.path.clear();
        } else if let Some(p) = by_player {
            m.ai.target = Some(p);
        }
        if let Some(t) = toward {
            let l = (t[0] * t[0] + t[1] * t[1]).sqrt();
            if l > 1e-6 && kb > 0.0 {
                let (dx, dz) = (t[0] / l * kb, t[1] / l * kb);
                m.vel[0] = m.vel[0] / 2.0 - dx;
                m.vel[2] = m.vel[2] / 2.0 - dz;
                if m.on_ground {
                    m.vel[1] = (m.vel[1] / 2.0 + kb).min(0.4);
                }
            }
        }
        let (pos, health, vel, kindm) = (m.pos, m.health, m.vel, m.kind);
        let c = cause.map(|e| e + 1).unwrap_or(0);
        let d = direct.or(cause).map(|e| e + 1).unwrap_or(0);
        if !quiet {
            let mut w = Writer::with_id(cb::DAMAGE_EVENT);
            w.varint(eid).varint(dt).varint(c).varint(d).bool(false);
            self.send_to_viewers(eid, &w);
        }
        self.send_mob_data(eid, |w| {
            w.u8(9).varint(3).f32(health);
        });
        if toward.is_some() {
            let mut w = Writer::with_id(cb::SET_ENTITY_MOTION);
            w.varint(eid);
            super::mob::write_lpvec3(&mut w, vel);
            self.send_to_viewers(eid, &w);
        }
        let pitch = 1.0 + self.rng.gen_range(-0.2..0.2);
        if health <= 0.0 {
            self.kill_mob(eid, kind);
            true
        } else if !quiet {
            self.play_sound(
                &kindm.sound("hurt"),
                kindm.sound_category(),
                pos,
                1.0,
                pitch,
            );
            false
        } else {
            false
        }
    }

    /// Start the death animation; drop loot now (vanilla drops at the moment of death).
    pub(super) fn kill_mob(&mut self, eid: i32, _kind: &str) {
        let tick = self.tick;
        let Some(m) = self.mobs.get_mut(&eid) else {
            return;
        };
        if !m.alive() {
            return;
        }
        m.dead = Some(0);
        m.health = 0.0;
        m.vel = [0.0, m.vel[1].min(0.0), 0.0];
        let (pos, kind, baby) = (m.pos, m.kind, m.baby);
        let by_player = m
            .last_player_hit
            .filter(|(_, t)| tick.saturating_sub(*t) <= 100)
            .map(|(p, _)| p);
        let props = basalt_game::EntityProps {
            on_fire: m.fire_ticks > 0,
            sheep_color: Some(SHEEP_COLORS[m.sheep_color as usize & 15].to_string()),
            sheared: m.sheared,
            baby,
        };
        self.send_mob_data(eid, |w| {
            w.u8(9).varint(3).f32(0.0).u8(6).varint(20).varint(7);
        });
        let mut w = Writer::with_id(cb::ENTITY_EVENT);
        w.i32(eid).u8(3);
        self.send_to_viewers(eid, &w);
        self.play_sound(&kind.sound("death"), kind.sound_category(), pos, 1.0, 1.0);
        if baby {
            return;
        }
        let tool = by_player
            .and_then(|p| self.players.get(&p))
            .map(|p| p.inv[36 + p.held])
            .filter(|s| s.count > 0)
            .map(|s| s.item);
        let ctx = basalt_game::LootContext {
            killed_by_player: by_player.is_some(),
            tool,
            this_entity: Some(props),
            ..Default::default()
        };
        let drops = self
            .data
            .loot(&format!("entities/{}", kind.name()), &ctx, &mut self.rng);
        for (item, count) in drops {
            let vel = [
                self.rng.gen_range(-0.1..0.1),
                0.2,
                self.rng.gen_range(-0.1..0.1),
            ];
            self.spawn_item(
                [pos[0], pos[1] + 0.3, pos[2]],
                vel,
                Stack {
                    item,
                    count,
                    damage: 0,
                    ench: NO_ENCH,
                },
                10,
            );
        }
        if by_player.is_some() {
            let xp = super::xporb::mob_xp(kind.name(), &mut self.rng);
            if xp > 0 {
                self.spawn_xp(pos, xp);
            }
        }
    }

    /// Serverbound attack (or interact-attack) from a player.
    pub(super) fn player_attack(&mut self, eid: i32, target: i32) {
        let Some(p) = self.players.get(&eid) else {
            return;
        };
        if p.dead || p.gamemode == 3 || target == eid {
            return;
        }
        if self.hit_crystal(eid, target) {
            return;
        }
        let p = &self.players[&eid];
        let eye = [p.pos[0], p.pos[1] + 1.62, p.pos[2]];
        let tpos = if let Some(m) = self.mobs.get(&target) {
            if !m.alive() {
                return;
            }
            m.pos
        } else if let Some(o) = self.players.get(&target) {
            if o.dead {
                return;
            }
            o.pos
        } else {
            return;
        };
        // reach: vanilla validates ~ 3 blocks to the hitbox; allow slack for latency and box size
        let d2 = (tpos[0] - eye[0]).powi(2)
            + (tpos[1] + 0.9 - eye[1]).powi(2)
            + (tpos[2] - eye[2]).powi(2);
        if d2 > 7.0 * 7.0 {
            log::debug!("{} attack out of reach ({:.1})", p.name, d2.sqrt());
            return;
        }
        let held = p.inv[36 + p.held];
        let info = (held.count > 0)
            .then(|| self.data.item(held.item))
            .flatten();
        let base = info
            .map(|i| i.attack_damage)
            .filter(|d| *d > 0.0)
            .unwrap_or(1.0);
        let speed = info
            .map(|i| i.attack_speed)
            .filter(|s| *s > 0.0)
            .unwrap_or(4.0);
        let period = 20.0 / speed as f64;
        let since = self.tick.saturating_sub(p.last_attack_tick) as f64;
        let progress = ((since + 0.5) / period).clamp(0.0, 1.0) as f32;
        let mut dmg = base * (0.2 + progress * progress * 0.8);
        let falling = p.fall_start.is_some() && !p.on_ground;
        let crit = progress > 0.9 && falling && !p.sprinting;
        if crit {
            dmg *= 1.5;
        }
        let kb = 0.4
            + if p.sprinting && progress > 0.9 {
                0.5
            } else {
                0.0
            };
        let (name, is_sword) = (
            p.name.clone(),
            info.map(|i| {
                self.data.tags.item_has("minecraft:swords", i.id) || i.name.ends_with("_sword")
            })
            .unwrap_or(false),
        );
        let is_tool = info
            .map(|i| {
                i.max_damage > 0
                    && (i.tool.is_some()
                        || i.name.ends_with("_axe")
                        || i.name.ends_with("_hoe")
                        || i.name.ends_with("_shovel")
                        || i.name.ends_with("_pickaxe"))
            })
            .unwrap_or(false);
        let yaw = p.yaw;
        {
            let p = self.players.get_mut(&eid).unwrap();
            p.last_attack_tick = self.tick;
            p.exhaustion += 0.1;
            if p.sprinting && kb > 0.4 {
                p.sprinting = false;
            }
        }
        let toward = [eye[0] - tpos[0], eye[2] - tpos[2]];
        if crit {
            let mut w = Writer::with_id(cb::ANIMATE);
            w.varint(target).u8(4);
            self.broadcast(None, || Writer { buf: w.buf.clone() });
        }
        let hit = if self.mobs.contains_key(&target) {
            let before = self.mobs[&target].hurt_cooldown;
            self.damage_mob_kb(
                target,
                dmg,
                "player_attack",
                Some(eid),
                None,
                Some(toward),
                kb,
            );
            before == 0
        } else {
            let hit = self
                .players
                .get(&target)
                .map(|o| o.hurt_cooldown == 0 && !matches!(o.gamemode, 1 | 3))
                .unwrap_or(false);
            let yaw_hurt = (toward[0]).atan2(toward[1]).to_degrees() as f32;
            self.hurt_by(
                target,
                dmg,
                "player_attack",
                DamageSource {
                    cause: Some(eid),
                    direct: None,
                    killer: Some(name.clone()),
                    hurt_yaw: Some(yaw_hurt),
                },
            );
            if hit {
                let r = (yaw as f64).to_radians();
                self.knock_player(target, [-r.sin() * kb, 0.4f64.min(kb), r.cos() * kb]);
            }
            hit
        };
        log::debug!(
            "{name} hit {target} for {dmg:.2} (progress {progress:.2}{})",
            if crit { ", crit" } else { "" }
        );
        if hit && (is_sword || is_tool) {
            self.wear_held_exact(eid, if is_sword { 1 } else { 2 });
        }
        let snd = if crit {
            "entity.player.attack.crit"
        } else if progress > 0.9 {
            "entity.player.attack.strong"
        } else {
            "entity.player.attack.weak"
        };
        self.play_sound(snd, 7, tpos, 1.0, 1.0);
    }

    /// Right-click on a mob: shears on sheep, bucket on cow, flint and steel on creepers.
    pub(super) fn player_interact(&mut self, eid: i32, target: i32, hand: i32) {
        let Some(p) = self.players.get(&eid) else {
            return;
        };
        if p.dead || p.gamemode == 3 {
            return;
        }
        let slot = if hand == 0 { 36 + p.held } else { 45 };
        let held = p.inv[slot];
        let creative = p.gamemode == 1;
        let Some(m) = self.mobs.get(&target) else {
            return;
        };
        if !m.alive() {
            return;
        }
        let item = reg::ITEM.get(held.item as usize).copied().unwrap_or("air");
        if held.count <= 0 {
            return;
        }
        if self.try_feed(eid, target, slot) {
            return;
        }
        let Some(m) = self.mobs.get(&target) else {
            return;
        };
        match (m.kind, item) {
            (MobKind::Sheep, "shears") if !m.sheared && !m.baby => {
                let (pos, col) = (m.pos, m.sheep_color);
                let n = self.rng.gen_range(1..=3);
                self.mobs.get_mut(&target).unwrap().sheared = true;
                self.send_mob_data(target, |w| {
                    w.u8(18).varint(0).u8(col & 15 | 0x10);
                });
                if let Some(wool) = registry_id(
                    reg::ITEM,
                    &format!("{}_wool", SHEEP_COLORS[col as usize & 15]),
                ) {
                    for _ in 0..n {
                        let v = [
                            self.rng.gen_range(-0.1..0.1),
                            0.2 + self.rng.gen_range(0.0..0.05),
                            self.rng.gen_range(-0.1..0.1),
                        ];
                        self.spawn_item(
                            [pos[0], pos[1] + 1.0, pos[2]],
                            v,
                            Stack {
                                item: wool,
                                count: 1,
                                damage: 0,
                                ench: NO_ENCH,
                            },
                            10,
                        );
                    }
                }
                self.play_sound("entity.sheep.shear", 7, pos, 1.0, 1.0);
                if !creative {
                    self.wear_held_exact(eid, 1);
                }
            }
            (MobKind::Cow, "bucket") if !m.baby => {
                let pos = m.pos;
                if let Some(milk) = registry_id(reg::ITEM, "milk_bucket") {
                    if !creative {
                        let p = self.players.get_mut(&eid).unwrap();
                        p.inv[slot].count -= 1;
                        if p.inv[slot].count <= 0 {
                            p.inv[slot] = Stack::default();
                        }
                        Self::send_window(p);
                    }
                    let left = self.give_stack(
                        eid,
                        Stack {
                            item: milk,
                            count: 1,
                            damage: 0,
                            ench: NO_ENCH,
                        },
                    );
                    if left > 0 {
                        self.throw_item(
                            eid,
                            Stack {
                                item: milk,
                                count: left,
                                damage: 0,
                                ench: NO_ENCH,
                            },
                            false,
                        );
                    }
                }
                self.play_sound("entity.cow.milk", 7, pos, 1.0, 1.0);
            }
            (MobKind::Creeper, "flint_and_steel") => {
                let pos = m.pos;
                let mm = self.mobs.get_mut(&target).unwrap();
                mm.ai.ignited = true;
                self.send_mob_data(target, |w| {
                    w.u8(18).varint(8).bool(true);
                });
                self.play_sound("item.flintandsteel.use", 7, pos, 1.0, 1.0);
                if !creative {
                    self.wear_held_exact(eid, 1);
                }
            }
            _ => {}
        }
    }

    /// Mob melee hit on a player.
    pub(super) fn mob_melee(&mut self, mob: &super::mob::Mob, target: i32) {
        let dmg = basalt_game::difficulty_scaled(mob.kind.stats().attack, self.difficulty);
        let Some(p) = self.players.get(&target) else {
            return;
        };
        let (dx, dz) = (p.pos[0] - mob.pos[0], p.pos[2] - mob.pos[2]);
        let l = (dx * dx + dz * dz).sqrt().max(1e-6);
        let yaw = (-dx).atan2(-dz).to_degrees() as f32;
        let mut w = Writer::with_id(cb::ANIMATE);
        w.varint(mob.eid).u8(0);
        self.send_to_viewers(mob.eid, &w);
        let landed = p.hurt_cooldown == 0;
        self.hurt_by(
            target,
            dmg,
            "mob_attack",
            DamageSource {
                cause: Some(mob.eid),
                direct: None,
                killer: Some(mob.kind.display().to_string()),
                hurt_yaw: Some(yaw),
            },
        );
        if landed {
            self.knock_player(target, [dx / l * 0.4, 0.4, dz / l * 0.4]);
        }
    }
}

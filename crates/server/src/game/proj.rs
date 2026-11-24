//! Thrown projectiles: snowball, egg, ender pearl, experience bottle, splash / lingering potion,
//! wind charge (all launched by use_item). Rules measured on vanilla 26.2 over the wire (2026-11-24,
//! `basalt-check --probe ADDR NAME raw:... use dumpraw`, a glass platform at y=250):
//! * spawn at eye height - 0.1 (wind charge: eye height), direction from yaw/pitch (+ a pitch offset of
//!   -20 degrees for bottles/potions, applied to the vertical component only, then normalised), plus
//!   gaussian noise 0.0075 per component, times the speed (1.5 snowball/egg/pearl/wind charge, 0.7 xp bottle,
//!   0.5 potions), plus the thrower's horizontal velocity (vertical only while airborne);
//!   add_entity.data = the thrower's entity id; metadata index 8 = the item stack (type 7)
//! * per tick: v.y = (v.y - gravity) * 0.99 (water 0.8), v.xz *= 0.99, then the segment pos -> pos + v is
//!   traced; gravity 0.03 snowball/egg/pearl, 0.07 xp bottle, 0.05 potions, 0 wind charge
//! * viewers get entity_position_sync (+ set_entity_motion when the velocity changed) every 10 ticks
//! * on a hit the projectile acts at its position BEFORE the colliding move (pearl teleport y = 250.0465 above
//!   a platform at 250); snowball/egg: entity_event 3 then remove; xp bottle: level_event 2002 colour
//!   0xFF385DC6 + orbs; pearl: teleport + 5 "ender_pearl" damage + entity.player.teleport (cat 7) at the target
//! * throw sounds (volume 0.5, pitch 0.4 / (0.8 + 0.4 r)): snowball 6, egg 7, ender pearl 6, xp bottle 6,
//!   splash potion 7, lingering potion 6, wind charge 6 (categories measured)
//! * cooldown packets (item group, ticks): ender pearl 20, wind charge 10; sent again with 0 when over
//! * egg hit: 1/8 chance of a baby chicken (1/32 of those: four) at the egg's position
//! * snowball/egg hitting a mob: 0 damage ("thrown" damage event, cause = thrower, direct = projectile),
//!   knockback 0.4 away from the flight direction (vertical 0), hurt sound
use super::arrow::{rand_distr_normal, seg_aabb};
use super::health::DamageSource;
use super::mob::{angle, block_boxes, is_water, write_lpvec3, MobKind};
use super::{write_slot, Game, Player, Stack};
use basalt_data::{registries as reg, registry_id};
use basalt_protocol::{ids::play::clientbound as cb, Writer};
use rand::Rng;
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PKind {
    Snowball,
    Egg,
    Pearl,
    XpBottle,
    Splash,
    Lingering,
    Wind,
}

impl PKind {
    pub fn from_item(name: &str) -> Option<PKind> {
        Some(match name {
            "snowball" => PKind::Snowball,
            "egg" | "blue_egg" | "brown_egg" => PKind::Egg,
            "ender_pearl" => PKind::Pearl,
            "experience_bottle" => PKind::XpBottle,
            "splash_potion" => PKind::Splash,
            "lingering_potion" => PKind::Lingering,
            "wind_charge" => PKind::Wind,
            _ => return None,
        })
    }
    pub fn entity(self) -> &'static str {
        match self {
            PKind::Snowball => "snowball",
            PKind::Egg => "egg",
            PKind::Pearl => "ender_pearl",
            PKind::XpBottle => "experience_bottle",
            PKind::Splash => "splash_potion",
            PKind::Lingering => "lingering_potion",
            PKind::Wind => "wind_charge",
        }
    }
    fn speed(self) -> f64 {
        match self {
            PKind::XpBottle => 0.7,
            PKind::Splash | PKind::Lingering => 0.5,
            _ => 1.5,
        }
    }
    /// vertical angle offset in degrees
    fn offset(self) -> f64 {
        match self {
            PKind::XpBottle | PKind::Splash | PKind::Lingering => -20.0,
            _ => 0.0,
        }
    }
    fn gravity(self) -> f64 {
        match self {
            PKind::XpBottle => 0.07,
            PKind::Splash | PKind::Lingering => 0.05,
            PKind::Wind => 0.0,
            _ => 0.03,
        }
    }
    fn throw_sound(self) -> (&'static str, i32) {
        match self {
            PKind::Snowball => ("entity.snowball.throw", 6),
            PKind::Egg => ("entity.egg.throw", 7),
            PKind::Pearl => ("entity.ender_pearl.throw", 6),
            PKind::XpBottle => ("entity.experience_bottle.throw", 6),
            PKind::Splash => ("entity.splash_potion.throw", 7),
            PKind::Lingering => ("entity.lingering_potion.throw", 6),
            PKind::Wind => ("entity.wind_charge.throw", 6),
        }
    }
    /// (cooldown group item, ticks)
    fn cooldown(self) -> Option<(&'static str, u64)> {
        match self {
            PKind::Pearl => Some(("ender_pearl", 20)),
            PKind::Wind => Some(("wind_charge", 10)),
            _ => None,
        }
    }
}

pub struct Proj {
    pub eid: i32,
    pub uuid: Uuid,
    pub kind: PKind,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub owner: i32,
    pub item: Stack,
    pub age: u32,
    sent_vel: [f64; 3],
}

impl Proj {
    pub fn rot(&self) -> (f32, f32) {
        let h = (self.vel[0] * self.vel[0] + self.vel[2] * self.vel[2]).sqrt();
        (
            self.vel[0].atan2(self.vel[2]).to_degrees() as f32,
            self.vel[1].atan2(h).to_degrees() as f32,
        )
    }
    pub fn spawn_packets(&self) -> Vec<Writer> {
        let (yaw, pitch) = self.rot();
        let (yaw, pitch) = if self.kind == PKind::Wind {
            (0.0, 0.0)
        } else {
            (yaw, pitch)
        };
        let mut a = Writer::with_id(cb::ADD_ENTITY);
        a.varint(self.eid)
            .uuid(self.uuid)
            .varint(registry_id(reg::ENTITY_TYPE, self.kind.entity()).unwrap_or(0))
            .f64(self.pos[0])
            .f64(self.pos[1])
            .f64(self.pos[2]);
        write_lpvec3(&mut a, self.vel);
        a.u8(angle(pitch)).u8(angle(yaw)).u8(0).varint(self.owner);
        let mut v = vec![Writer::with_id(cb::BUNDLE_DELIMITER), a];
        if self.kind != PKind::Wind {
            let mut d = Writer::with_id(cb::SET_ENTITY_DATA);
            d.varint(self.eid).u8(8).varint(7);
            write_slot(
                &mut d,
                Stack {
                    count: 1,
                    ..self.item
                },
            );
            d.u8(0xff);
            v.push(d);
        }
        v.push(Writer::with_id(cb::BUNDLE_DELIMITER));
        v
    }
}

impl Game {
    /// use_item with a throwable in hand. Returns true when the item was handled (thrown or refused by a cooldown).
    pub(super) fn try_throw(&mut self, eid: i32, hand: i32, yaw: f32, pitch: f32) -> bool {
        let Some(p) = self.players.get_mut(&eid) else {
            return false;
        };
        if p.dead || p.gamemode == 3 {
            return false;
        }
        p.yaw = yaw;
        p.pitch = pitch;
        let slot = if hand == 0 { 36 + p.held } else { 45 };
        let s = p.inv[slot];
        if s.count <= 0 {
            return false;
        }
        let name = reg::ITEM.get(s.item as usize).copied().unwrap_or("air");
        let Some(kind) = PKind::from_item(name) else {
            return false;
        };
        if let Some((group, _)) = kind.cooldown() {
            if p.cooldowns
                .get(group)
                .map(|e| *e > self.tick)
                .unwrap_or(false)
            {
                return true;
            }
        }
        let creative = p.gamemode == 1;
        let feet = p.pos;
        let eye = p.pos[1] + if p.sneaking { 1.27 } else { 1.62 };
        let (pos, ppos, on_ground) = (
            [
                p.pos[0],
                eye - if kind == PKind::Wind { 0.0 } else { 0.1 },
                p.pos[2],
            ],
            p.prev_tick_pos,
            p.on_ground,
        );
        let owner_v = [
            p.pos[0] - ppos[0],
            if on_ground { 0.0 } else { p.pos[1] - ppos[1] },
            p.pos[2] - ppos[2],
        ];
        let owner_v = if p.prev_tick_pos == [0.0; 3] {
            [0.0; 3]
        } else {
            owner_v
        };
        if !creative {
            p.inv[slot].count -= 1;
            if p.inv[slot].count <= 0 {
                p.inv[slot] = Stack::default();
            }
            Self::send_window(p);
        }
        if let Some((group, ticks)) = kind.cooldown() {
            p.cooldowns.insert(group.to_string(), self.tick + ticks);
            let mut w = Writer::with_id(cb::COOLDOWN);
            w.str(&format!("minecraft:{group}")).varint(ticks as i32);
            p.send(w);
        }
        // direction
        let (yr, xr) = ((yaw as f64).to_radians(), (pitch as f64).to_radians());
        let mut d = [
            -yr.sin() * xr.cos(),
            -((pitch as f64 + kind.offset()).to_radians()).sin(),
            yr.cos() * xr.cos(),
        ];
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-9);
        for c in d.iter_mut() {
            let g: f64 = self.rng.sample(rand_distr_normal());
            *c = *c / l + g * 0.0075;
        }
        let sp = kind.speed();
        let vel = [
            d[0] * sp + owner_v[0],
            d[1] * sp + owner_v[1],
            d[2] * sp + owner_v[2],
        ];
        let mut item = s;
        item.count = 1;
        let peid = self
            .sh
            .next_eid
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.projs.insert(
            peid,
            Proj {
                eid: peid,
                uuid: Uuid::new_v4(),
                kind,
                pos,
                vel,
                owner: eid,
                item,
                age: 0,
                sent_vel: vel,
            },
        );
        self.sync_viewers_for(peid);
        let (sound, cat) = kind.throw_sound();
        let pitch = 0.4 / (self.rng.gen::<f32>() * 0.4 + 0.8);
        self.play_sound(sound, cat, feet, 0.5, pitch);
        true
    }

    /// Cooldown expiry notifications (the end of a cooldown is announced with 0 ticks).
    pub(super) fn tick_cooldowns(&mut self) {
        let tick = self.tick;
        for p in self.players.values_mut() {
            let done: Vec<String> = p
                .cooldowns
                .iter()
                .filter(|(_, e)| **e <= tick)
                .map(|(k, _)| k.clone())
                .collect();
            for g in done {
                p.cooldowns.remove(&g);
                let mut w = Writer::with_id(cb::COOLDOWN);
                w.str(&format!("minecraft:{g}")).varint(0);
                p.send(w);
            }
        }
    }

    pub(super) fn tick_projs(&mut self) {
        let ids: Vec<i32> = self.projs.keys().copied().collect();
        for id in ids {
            let Some(mut a) = self.projs.remove(&id) else {
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
                self.projs.insert(id, a);
                continue;
            }
            a.age += 1;
            if a.age > 1200 || a.pos[1] < basalt_world::MIN_Y as f64 - 64.0 {
                self.untrack(id);
                continue;
            }
            let wet = is_water(&self.world, a.pos[0], a.pos[1], a.pos[2]);
            let drag = if wet { 0.8 } else { 0.99 };
            a.vel[1] = (a.vel[1] - a.kind.gravity()) * drag;
            a.vel[0] *= drag;
            a.vel[2] *= drag;
            let (p, d) = (a.pos, a.vel);
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            let steps = (len / 0.1).ceil().max(1.0) as usize;
            let mut block_hit: Option<f64> = None;
            let mut boxes = Vec::new();
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
                    if let Some(tt) = seg_aabb(p, d, w, 0.0) {
                        block_hit = Some(tt);
                        break 'outer;
                    }
                }
            }
            let limit = block_hit.unwrap_or(1.0);
            let mut ent_hit: Option<(f64, i32, bool)> = None;
            for m in self.mobs.values() {
                if !m.alive() {
                    continue;
                }
                if let Some(t) = seg_aabb(p, d, m.aabb(), 0.3) {
                    if t <= limit && ent_hit.map(|h| t < h.0).unwrap_or(true) {
                        ent_hit = Some((t, m.eid, false));
                    }
                }
            }
            for pl in self.players.values() {
                if pl.dead || pl.gamemode == 3 || (pl.eid == a.owner && a.age < 5) {
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
            if ent_hit.is_some() || block_hit.is_some() {
                let hit_ent = ent_hit.map(|h| (h.1, h.2));
                self.projectile_hit(a, hit_ent);
                continue;
            }
            for k in 0..3 {
                a.pos[k] += d[k];
            }
            if a.age % 10 == 0 {
                let (yaw, pitch) = a.rot();
                if a.vel != a.sent_vel {
                    let mut m = Writer::with_id(cb::SET_ENTITY_MOTION);
                    m.varint(id);
                    write_lpvec3(&mut m, a.vel);
                    self.send_to_viewers(id, &m);
                    a.sent_vel = a.vel;
                }
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
            self.projs.insert(id, a);
        }
    }

    /// A projectile hit a block or an entity (`hit` = (eid, is_player)); it acts at its current position and is removed.
    fn projectile_hit(&mut self, a: Proj, hit: Option<(i32, bool)>) {
        let id = a.eid;
        let pos = a.pos;
        let owner = a.owner;
        let dir = {
            let h = (a.vel[0] * a.vel[0] + a.vel[2] * a.vel[2]).sqrt().max(1e-6);
            [a.vel[0] / h, a.vel[2] / h]
        };
        let owner_name = self.players.get(&owner).map(|p| p.name.clone());
        match a.kind {
            PKind::Snowball | PKind::Egg => {
                if let Some((t, is_player)) = hit {
                    if is_player {
                        let src = DamageSource {
                            cause: Some(owner),
                            direct: Some(id),
                            killer: owner_name,
                            hurt_yaw: None,
                        };
                        self.hurt_by(t, 0.0, "thrown", src);
                        self.knock_player(t, [dir[0] * 0.4, 0.0, dir[1] * 0.4]);
                    } else {
                        self.hit_mob_zero(t, owner, id, [-dir[0], -dir[1]]);
                    }
                }
                let mut w = Writer::with_id(cb::ENTITY_EVENT);
                w.i32(id).u8(3);
                self.send_to_viewers(id, &w);
                self.untrack(id);
                if a.kind == PKind::Egg && self.rng.gen_range(0..8) == 0 {
                    let n = if self.rng.gen_range(0..32) == 0 { 4 } else { 1 };
                    let (yaw, _) = a.rot();
                    for _ in 0..n {
                        self.spawn_mob(MobKind::Chicken, pos, yaw, |m| {
                            m.baby = true;
                            m.grow = 0;
                        });
                    }
                }
            }
            PKind::Pearl => {
                self.untrack(id);
                let Some(p) = self.players.get_mut(&owner) else {
                    return;
                };
                if p.dead {
                    return;
                }
                p.pos = pos;
                p.fall_start = None;
                p.on_ground = false;
                Self::teleport(p);
                let sync = Self::sync_entity(p);
                self.broadcast(Some(owner), || Writer {
                    buf: sync.buf.clone(),
                });
                self.play_sound("entity.player.teleport", 7, pos, 1.0, 1.0);
                self.hurt(owner, 5.0, "ender_pearl");
            }
            PKind::XpBottle => {
                self.untrack(id);
                let mut w = Writer::with_id(cb::LEVEL_EVENT);
                w.i32(2002)
                    .position(
                        pos[0].floor() as i32,
                        pos[1].floor() as i32,
                        pos[2].floor() as i32,
                    )
                    .i32(0xFF385DC6u32 as i32)
                    .bool(false);
                self.broadcast_chunk(
                    ((pos[0].floor() as i32) >> 4, (pos[2].floor() as i32) >> 4),
                    None,
                    || Writer { buf: w.buf.clone() },
                );
                let xp = 3 + self.rng.gen_range(0..5) + self.rng.gen_range(0..5);
                self.spawn_xp(pos, xp);
            }
            PKind::Splash | PKind::Lingering => {
                self.untrack(id);
                self.potion_burst(&a, hit);
            }
            PKind::Wind => {
                self.untrack(id);
            }
        }
    }

    /// Zero-damage projectile hit on a mob: damage event, knockback, hurt sound (vertical push 0).
    fn hit_mob_zero(&mut self, target: i32, owner: i32, direct: i32, toward: [f64; 2]) {
        let dt = self.damage_type("thrown");
        let tick = self.tick;
        let Some(m) = self.mobs.get_mut(&target) else {
            return;
        };
        if !m.alive() || m.hurt_cooldown > 0 {
            return;
        }
        m.hurt_cooldown = 10;
        if self.players.contains_key(&owner) {
            m.last_player_hit = Some((owner, tick));
        }
        if !m.kind.hostile() {
            m.ai.panic_ticks = 80 + (tick % 40) as u32;
            m.ai.path.clear();
        } else if self.players.contains_key(&owner) {
            m.ai.target = Some(owner);
        }
        let l = (toward[0] * toward[0] + toward[1] * toward[1])
            .sqrt()
            .max(1e-6);
        // `toward` points from the mob to the thrower; the push goes the other way
        m.vel[0] = m.vel[0] / 2.0 - toward[0] / l * 0.4;
        m.vel[2] = m.vel[2] / 2.0 - toward[1] / l * 0.4;
        let (pos, vel, kind) = (m.pos, m.vel, m.kind);
        let mut w = Writer::with_id(cb::DAMAGE_EVENT);
        w.varint(target)
            .varint(dt)
            .varint(owner + 1)
            .varint(direct + 1)
            .bool(false);
        self.send_to_viewers(target, &w);
        let mut w = Writer::with_id(cb::SET_ENTITY_MOTION);
        w.varint(target);
        write_lpvec3(&mut w, vel);
        self.send_to_viewers(target, &w);
        let pitch = 1.0 + self.rng.gen_range(-0.2..0.2);
        self.play_sound(&kind.sound("hurt"), kind.sound_category(), pos, 1.0, pitch);
    }

    /// Splash / lingering potion landing (effects are added in a later step).
    fn potion_burst(&mut self, _a: &Proj, _hit: Option<(i32, bool)>) {}
}

impl Player {
    pub(super) fn cooldown_active(&self, group: &str, tick: u64) -> bool {
        self.cooldowns
            .get(group)
            .map(|e| *e > tick)
            .unwrap_or(false)
    }
}

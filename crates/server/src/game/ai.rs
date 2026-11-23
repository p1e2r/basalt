//! Mob AI and per-tick update: goals (wander, look at player, panic, tempt, target + melee,
//! skeleton archery, creeper swelling, spider leaps), A* path following, daylight burning,
//! fire, despawning, ambient/step sounds and the death animation.
use super::mob::{Mob, MobKind};
use super::path::{self, Cell, Grid};
use super::{Game, Stack, NO_ENCH};
use basalt_data::{registries as reg, registry_id, BlockState};
use basalt_protocol::{ids::play::clientbound as cb, Writer};
use basalt_world::{props, World};
use rand::Rng;

/// Path searches allowed per tick across all mobs.
const PATH_BUDGET: u32 = 6;

pub struct WorldGrid<'a>(pub &'a World);
impl Grid for WorldGrid<'_> {
    fn cell(&self, x: i32, y: i32, z: i32) -> Cell {
        if !self.0.is_loaded(x >> 4, z >> 4) { return Cell::Solid; }
        let s = self.0.get_block(x, y, z);
        let n = BlockState(s).name();
        match n {
            "lava" | "fire" | "soul_fire" | "cactus" | "magma_block" | "sweet_berry_bush" | "campfire" | "soul_campfire" | "wither_rose" | "powder_snow" => return Cell::Danger,
            "water" | "bubble_column" => return Cell::Water,
            _ => {}
        }
        let p = props::props(s);
        if p.blocks_motion() { Cell::Solid } else if p.has_fluid() { Cell::Water } else { Cell::Open }
    }
}

fn yaw_to(dx: f64, dz: f64) -> f32 { (-dx).atan2(dz).to_degrees() as f32 }
fn approach_angle(cur: f32, want: f32, max: f32) -> f32 { let d = (want - cur + 540.0).rem_euclid(360.0) - 180.0; cur + d.clamp(-max, max) }

impl Game {
    pub(super) fn is_day(&self) -> bool { let t = self.day_time.rem_euclid(24000); !(12542..23460).contains(&t) }
    /// Sky darkening subtracted from sky light (0 by day, up to 11 at night).
    pub(super) fn sky_darken(&self) -> u8 {
        let t = self.day_time.rem_euclid(24000);
        if (13800..22200).contains(&t) { 11 } else if (12000..13800).contains(&t) { ((t - 12000) * 11 / 1800) as u8 } else if (22200..24000).contains(&t) { ((24000 - t) * 11 / 1800) as u8 } else { 0 }
    }

    fn nearest_player(&self, pos: [f64; 3], range: f64, survival_only: bool) -> Option<(i32, f64)> {
        self.players.values().filter(|p| !p.dead && p.gamemode != 3 && (!survival_only || matches!(p.gamemode, 0 | 2)))
            .map(|p| (p.eid, ((p.pos[0] - pos[0]).powi(2) + (p.pos[1] - pos[1]).powi(2) + (p.pos[2] - pos[2]).powi(2)).sqrt()))
            .filter(|(_, d)| *d <= range).min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
    }

    fn line_of_sight(&self, a: [f64; 3], b: [f64; 3]) -> bool {
        let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let n = (len / 0.3).ceil().max(1.0) as i32;
        (1..n).all(|s| { let t = s as f64 / n as f64; !props::props(self.world.get_block((a[0] + d[0] * t).floor() as i32, (a[1] + d[1] * t).floor() as i32, (a[2] + d[2] * t).floor() as i32)).blocks_motion() })
    }

    fn holding_any(&self, pid: i32, items: &[&str]) -> bool {
        let Some(p) = self.players.get(&pid) else { return false };
        [p.inv[36 + p.held], p.inv[45]].iter().any(|s| s.count > 0 && reg::ITEM.get(s.item as usize).map(|n| items.contains(n)).unwrap_or(false))
    }

    /// Pick a random standable spot within `h` blocks horizontally of the mob.
    fn random_spot(&mut self, m: &Mob, h: i32) -> Option<[i32; 3]> {
        let pr = path::Params { height: m.height().ceil() as i32, ..Default::default() };
        for _ in 0..10 {
            let p = [m.pos[0].floor() as i32 + self.rng.gen_range(-h..=h), m.pos[1].floor() as i32 + self.rng.gen_range(-3..=3), m.pos[2].floor() as i32 + self.rng.gen_range(-h..=h)];
            let g = WorldGrid(&self.world);
            for dy in 0..6 { let q = [p[0], p[1] - dy, p[2]]; if path::standable(&g, q, &pr) { return Some(q); } }
        }
        None
    }

    pub(super) fn tick_mobs(&mut self) {
        self.path_budget = PATH_BUDGET;
        let ids: Vec<i32> = self.mobs.keys().copied().collect();
        for id in ids {
            let Some(mut m) = self.mobs.remove(&id) else { continue };
            let keep = self.tick_mob(&mut m);
            if keep { self.mobs.insert(id, m); } else { self.untrack(id); }
        }
    }

    /// One tick of one mob (removed from `self.mobs` while it runs). Returns false to despawn it.
    fn tick_mob(&mut self, m: &mut Mob) -> bool {
        let (cx, cz) = m.chunk();
        if !self.world.is_loaded(cx, cz) { return true; }
        m.age += 1;
        if m.hurt_cooldown > 0 { m.hurt_cooldown -= 1; }
        if m.ai.attack_cooldown > 0 { m.ai.attack_cooldown -= 1; }
        if let Some(t) = m.dead.as_mut() {
            *t += 1;
            let t = *t;
            self.mob_physics(m, [0.0, 0.0], false);
            if t >= 20 {
                let mut w = Writer::with_id(cb::ENTITY_EVENT); w.i32(m.eid).u8(60); self.send_to_viewers(m.eid, &w);
                return false;
            }
            return true;
        }
        if m.pos[1] < basalt_world::MIN_Y as f64 - 64.0 { return false; }
        // ---- despawning (hostiles only; animals and named/persistent mobs stay)
        if m.kind.hostile() && !m.persistent {
            let near = self.nearest_player(m.pos, f64::MAX, false).map(|n| n.1).unwrap_or(f64::MAX);
            if near > 128.0 { return false; }
            if near > 32.0 && self.rng.gen_range(0..800) == 0 { return false; }
        }
        // ---- environment: daylight, fire, lava
        let eye = [m.pos[0], m.eye_y(), m.pos[2]];
        if m.kind.undead() && self.is_day() && !m.in_water {
            let (sky, _) = self.world.light(eye[0].floor() as i32, eye[1].floor() as i32, eye[2].floor() as i32);
            if sky >= 15 && self.rng.gen_range(0..30) < 8 { m.fire_ticks = m.fire_ticks.max(160); }
        }
        if super::mob::is_lava(&self.world, m.pos[0], m.pos[1] + 0.1, m.pos[2]) { m.fire_ticks = m.fire_ticks.max(300); self.hurt_held_mob(m, 4.0, "lava"); if !m.alive() { return true; } }
        if m.in_water && m.fire_ticks > 0 { m.fire_ticks = 0; }
        if m.fire_ticks > 0 {
            m.fire_ticks -= 1;
            if m.fire_ticks % 20 == 0 { self.hurt_held_mob(m, 1.0, "on_fire"); if !m.alive() { return true; } }
        }
        let fire = m.fire_ticks > 0;
        if fire != m.sent_fire { m.sent_fire = fire; let eid = m.eid; self.send_mob_data(eid, |w| { w.u8(0).varint(0).u8(fire as u8); }); }
        // ---- AI
        let (mut goal, mut speed_mod, mut jump) = (None::<[f64; 3]>, 1.0f64, false);
        let mut look: Option<[f64; 3]> = None;
        if !m.no_ai { self.think(m, &mut goal, &mut speed_mod, &mut jump, &mut look); if !m.alive() { return false; } }
        // ---- steering along the path
        let mut accel = [0.0, 0.0];
        if let Some(g) = goal {
            let need_path = m.ai.path.is_empty() || m.ai.path_goal.map(|pg| (pg[0] as f64 + 0.5 - g[0]).abs() + (pg[2] as f64 + 0.5 - g[2]).abs() > 1.5).unwrap_or(true);
            if m.ai.repath_in > 0 { m.ai.repath_in -= 1; }
            if need_path && m.ai.repath_in == 0 && self.path_budget > 0 && m.on_ground || (need_path && m.in_water && m.ai.repath_in == 0 && self.path_budget > 0) {
                self.path_budget -= 1;
                let pr = path::Params { height: m.height().ceil() as i32, max_nodes: 500, ..Default::default() };
                let start = [m.pos[0].floor() as i32, (m.pos[1] + 0.01).floor() as i32, m.pos[2].floor() as i32];
                let gb = [g[0].floor() as i32, g[1].floor() as i32, g[2].floor() as i32];
                m.ai.path = path::find(&WorldGrid(&self.world), start, gb, 1.0, &pr).unwrap_or_default();
                m.ai.path_goal = Some(gb);
                m.ai.repath_in = 10 + self.rng.gen_range(0..10);
            }
            while let Some(n) = m.ai.path.first().copied() {
                let (dx, dz) = (n[0] as f64 + 0.5 - m.pos[0], n[2] as f64 + 0.5 - m.pos[2]);
                if dx * dx + dz * dz < (m.width() / 2.0).max(0.35).powi(2) && (n[1] as f64 - m.pos[1]).abs() < 1.2 { m.ai.path.remove(0); } else { break; }
            }
            let target = m.ai.path.first().map(|n| [n[0] as f64 + 0.5, n[1] as f64, n[2] as f64 + 0.5])
                .or_else(|| { let d2 = (g[0] - m.pos[0]).powi(2) + (g[2] - m.pos[2]).powi(2); (d2 > 0.3 && d2 < 4.0).then_some(g) });
            if let Some(t) = target {
                let (dx, dz) = (t[0] - m.pos[0], t[2] - m.pos[2]);
                let l = (dx * dx + dz * dz).sqrt();
                if l > 1e-3 {
                    let s = m.speed() * m.speed() * speed_mod * if m.on_ground || m.in_water { 1.0 } else { 0.2 };
                    accel = [dx / l * s, dz / l * s];
                    m.yaw = approach_angle(m.yaw, yaw_to(dx, dz), 30.0);
                    if look.is_none() { m.head_yaw = m.yaw; }
                }
                if (t[1] > m.pos[1] + 0.5 && l < 1.5) || (m.h_collision && m.on_ground) { jump = true; }
            }
            // stuck: drop the path
            if m.age % 40 == 0 {
                let moved = (m.pos[0] - m.ai.last_pos[0]).abs() + (m.pos[2] - m.ai.last_pos[2]).abs();
                if moved < 0.1 && !m.ai.path.is_empty() { m.ai.path.clear(); m.ai.repath_in = 20; }
                m.ai.last_pos = m.pos;
            }
        } else { m.ai.path.clear(); }
        if let Some(l) = look {
            let (dx, dy, dz) = (l[0] - m.pos[0], l[1] - m.eye_y(), l[2] - m.pos[2]);
            m.head_yaw = approach_angle(m.head_yaw, yaw_to(dx, dz), 20.0);
            m.pitch = (-(dy.atan2((dx * dx + dz * dz).sqrt()).to_degrees()) as f32).clamp(-40.0, 40.0);
            if goal.is_none() { m.yaw = approach_angle(m.yaw, m.head_yaw, 10.0); }
        } else { m.pitch *= 0.8; }
        if m.in_water && goal.is_some() { jump = true; }
        // ---- physics
        let fall = self.mob_physics(m, accel, jump);
        if fall > 0.0 { self.hurt_held_mob(m, fall, "fall"); }
        // ---- sounds
        m.ambient_timer += 1;
        if m.alive() && self.rng.gen_range(0..1000) < m.ambient_timer {
            m.ambient_timer = -80;
            let pitch = if m.baby { 1.5 } else { 1.0 } + self.rng.gen_range(-0.2..0.2);
            if m.kind != MobKind::Creeper { self.play_sound(&m.kind.sound("ambient"), m.kind.sound_category(), m.pos, 1.0, pitch); }
        }
        if m.step_dist > 1.6 && m.on_ground { m.step_dist = 0.0; if m.kind != MobKind::Creeper { self.play_sound(&m.kind.sound("step"), m.kind.sound_category(), m.pos, 0.15, 1.0); } }
        // chickens lay eggs
        if m.kind == MobKind::Chicken && !m.baby {
            if m.egg_timer > 0 { m.egg_timer -= 1; } else {
                m.egg_timer = self.rng.gen_range(6000..12000);
                if let Some(egg) = registry_id(reg::ITEM, "egg") { let p = m.pos; self.spawn_item(p, [0.0, 0.1, 0.0], Stack { item: egg, count: 1, damage: 0, ench: NO_ENCH }, 10); }
                self.play_sound("entity.chicken.egg", 6, m.pos, 1.0, 1.0);
            }
        }
        true
    }

    /// Damage the mob currently being ticked (it's out of the map): reinsert, damage, take back.
    fn hurt_held_mob(&mut self, m: &mut Mob, amount: f32, kind: &str) {
        let eid = m.eid;
        let placeholder = Mob::new(eid, m.kind, m.pos, 0.0);
        self.mobs.insert(eid, std::mem::replace(m, placeholder));
        self.damage_mob(eid, amount, kind, None, None, None);
        if let Some(back) = self.mobs.remove(&eid) { *m = back; }
    }

    /// Goal selection. Writes the movement goal, speed modifier, jump and look target.
    fn think(&mut self, m: &mut Mob, goal: &mut Option<[f64; 3]>, speed: &mut f64, jump: &mut bool, look: &mut Option<[f64; 3]>) {
        let st = m.kind.stats();
        let _ = jump;
        if m.kind.hostile() {
            // (re)acquire target every 10 ticks
            if m.age % 10 == (m.eid as u64 % 10) {
                let valid = m.ai.target.and_then(|t| self.players.get(&t)).map(|p| !p.dead && matches!(p.gamemode, 0 | 2)
                    && ((p.pos[0] - m.pos[0]).powi(2) + (p.pos[2] - m.pos[2]).powi(2)).sqrt() <= st.follow_range * 1.2).unwrap_or(false);
                if !valid { m.ai.target = None; }
                if m.ai.target.is_none() {
                    let spider_ok = m.kind != MobKind::Spider || {
                        let (sky, bl) = self.world.light(m.pos[0].floor() as i32, (m.pos[1] + 0.5).floor() as i32, m.pos[2].floor() as i32);
                        sky.saturating_sub(self.sky_darken()).max(bl) <= 7
                    };
                    if spider_ok {
                        if let Some((pid, _)) = self.nearest_player(m.pos, st.follow_range, true) {
                            let p = &self.players[&pid];
                            let sees = self.line_of_sight([m.pos[0], m.eye_y(), m.pos[2]], [p.pos[0], p.pos[1] + 1.62, p.pos[2]]);
                            if sees || m.kind == MobKind::Zombie { m.ai.target = Some(pid); }
                        }
                    }
                }
                let aggressive = m.ai.target.is_some() && matches!(m.kind, MobKind::Zombie | MobKind::Skeleton);
                if aggressive != m.ai.aggressive {
                    m.ai.aggressive = aggressive;
                    let f = m.no_ai as u8 | (aggressive as u8) << 2;
                    self.send_mob_data(m.eid, |w| { w.u8(15).varint(0).u8(f); });
                }
            }
            if m.kind == MobKind::Creeper { self.creeper_tick(m); if !m.alive() { return; } }
            if let Some(t) = m.ai.target {
                let Some(p) = self.players.get(&t) else { return };
                let tp = p.pos;
                let (dx, dy, dz) = (tp[0] - m.pos[0], tp[1] - m.pos[1], tp[2] - m.pos[2]);
                let hd2 = dx * dx + dz * dz;
                *look = Some([tp[0], tp[1] + 1.62, tp[2]]);
                match m.kind {
                    MobKind::Skeleton => {
                        let sees = self.line_of_sight([m.pos[0], m.eye_y(), m.pos[2]], [tp[0], tp[1] + 1.5, tp[2]]);
                        if !(sees && hd2 < 12.0 * 12.0) { *goal = Some(tp); }
                        if sees && hd2 < 15.0 * 15.0 {
                            match m.ai.draw {
                                None if m.ai.attack_cooldown == 0 => { m.ai.draw = Some(0); self.send_mob_data(m.eid, |w| { w.u8(8).varint(0).u8(1); }); }
                                Some(d) if d >= 20 => {
                                    m.ai.draw = None; m.ai.attack_cooldown = 20;
                                    self.send_mob_data(m.eid, |w| { w.u8(8).varint(0).u8(0); });
                                    let from = [m.pos[0], m.eye_y() - 0.1, m.pos[2]];
                                    let dist = hd2.sqrt();
                                    let dir = [tp[0] - from[0], tp[1] + 1.8 / 3.0 - from[1] + dist * 0.2, tp[2] - from[2]];
                                    let inacc = (14 - super::combat::DIFFICULTY as i32 * 4) as f64;
                                    self.shoot_arrow(m.eid, from, dir, 1.6, inacc, 2.0);
                                    let pitch = 1.0 / (self.rng.gen_range(0.0..0.4) + 0.8);
                                    self.play_sound("entity.skeleton.shoot", 5, m.pos, 1.0, pitch);
                                }
                                Some(ref mut d) => *d += 1,
                                None => {}
                            }
                        } else if m.ai.draw.take().is_some() { self.send_mob_data(m.eid, |w| { w.u8(8).varint(0).u8(0); }); }
                    }
                    MobKind::Creeper => { if hd2 > 2.0 * 2.0 || m.ai.swell_dir < 0 { *goal = Some(tp); } }
                    _ => {
                        *goal = Some(tp);
                        let reach = (st.width * m.scale() * 2.0).powi(2) + 0.6;
                        if hd2 <= reach && dy.abs() < 2.0 && m.ai.attack_cooldown == 0 {
                            m.ai.attack_cooldown = 20;
                            self.mob_melee(m, t);
                        }
                        if m.kind == MobKind::Spider && m.on_ground && hd2 > 4.0 && hd2 < 16.0 && m.ai.leap_cooldown == 0 && self.rng.gen_range(0..5) == 0 {
                            let l = hd2.sqrt();
                            m.vel[0] += dx / l * 0.4; m.vel[2] += dz / l * 0.4; m.vel[1] = 0.4; m.ai.leap_cooldown = 20;
                        }
                    }
                }
                if m.ai.leap_cooldown > 0 { m.ai.leap_cooldown -= 1; }
                return;
            }
        } else {
            // panic
            if m.ai.panic_ticks > 0 {
                m.ai.panic_ticks -= 1;
                *speed = match m.kind { MobKind::Cow => 2.0, MobKind::Chicken => 1.4, _ => 1.25 } * 1.5;
                if m.ai.path_goal.map(|g| (g[0] as f64 - m.pos[0]).abs() + (g[2] as f64 - m.pos[2]).abs() < 1.5).unwrap_or(true) || m.ai.path.is_empty() {
                    if let Some(s) = self.random_spot(m, 5) { m.ai.path_goal = None; m.ai.path.clear(); m.ai.idle_ticks = 0; *goal = Some([s[0] as f64 + 0.5, s[1] as f64, s[2] as f64 + 0.5]); m.ai.panic_target = Some(s); }
                } else if let Some(s) = m.ai.panic_target { *goal = Some([s[0] as f64 + 0.5, s[1] as f64, s[2] as f64 + 0.5]); }
                return;
            }
            // tempt
            if m.age % 10 == 0 {
                let items = m.kind.tempt_items();
                m.ai.tempted_by = self.players.values().filter(|p| !p.dead && p.gamemode != 3)
                    .filter(|p| ((p.pos[0] - m.pos[0]).powi(2) + (p.pos[1] - m.pos[1]).powi(2) + (p.pos[2] - m.pos[2]).powi(2)) < 100.0)
                    .map(|p| p.eid).find(|&pid| self.holding_any(pid, items));
            }
            if let Some(t) = m.ai.tempted_by.and_then(|t| self.players.get(&t)) {
                let tp = t.pos;
                *look = Some([tp[0], tp[1] + 1.62, tp[2]]);
                let d2 = (tp[0] - m.pos[0]).powi(2) + (tp[2] - m.pos[2]).powi(2);
                if d2 > 2.5 * 2.5 { *goal = Some(tp); *speed = match m.kind { MobKind::Cow => 1.25, MobKind::Pig => 1.2, MobKind::Sheep => 1.1, _ => 1.0 }; }
                return;
            }
        }
        // look at a nearby player now and then
        if m.ai.look_ticks > 0 {
            m.ai.look_ticks -= 1;
            if let Some(p) = m.ai.look_at.and_then(|l| self.players.get(&l)) { *look = Some([p.pos[0], p.pos[1] + 1.62, p.pos[2]]); }
        } else if self.rng.gen_range(0..50) == 0 {
            if let Some((pid, _)) = self.nearest_player(m.pos, if m.kind.hostile() { 8.0 } else { 6.0 }, false) { m.ai.look_at = Some(pid); m.ai.look_ticks = 40 + self.rng.gen_range(0..40); }
        }
        // wander
        if let Some(g) = m.ai.wander {
            let d2 = (g[0] as f64 + 0.5 - m.pos[0]).powi(2) + (g[2] as f64 + 0.5 - m.pos[2]).powi(2);
            m.ai.idle_ticks += 1;
            if d2 < 1.0 || m.ai.idle_ticks > 200 { m.ai.wander = None; m.ai.idle_ticks = 0; } else { *goal = Some([g[0] as f64 + 0.5, g[1] as f64, g[2] as f64 + 0.5]); }
        } else if self.rng.gen_range(0..120) == 0 {
            m.ai.wander = self.random_spot(m, 10);
            m.ai.idle_ticks = 0;
        }
    }

    /// Creeper: swell towards an explosion when the target is within 3 blocks (or ignited),
    /// shrink back when it leaves 7 blocks. Fuse 30 ticks, radius 3.
    fn creeper_tick(&mut self, m: &mut Mob) {
        let dist = m.ai.target.and_then(|t| self.players.get(&t)).map(|p| ((p.pos[0] - m.pos[0]).powi(2) + (p.pos[1] - m.pos[1]).powi(2) + (p.pos[2] - m.pos[2]).powi(2)).sqrt());
        let want = if m.ai.ignited { 1 } else { match dist { Some(d) if d < 3.0 => 1, Some(d) if d < 7.0 && m.ai.swell_dir > 0 => 1, _ => -1 } };
        if want != m.ai.swell_dir {
            m.ai.swell_dir = want;
            self.send_mob_data(m.eid, |w| { w.u8(16).varint(1).varint(want); });
            if want > 0 { self.play_sound("entity.creeper.primed", 5, m.pos, 1.0, 0.5); }
        }
        m.ai.fuse = (m.ai.fuse + m.ai.swell_dir).max(0);
        if m.ai.fuse >= 30 {
            m.dead = Some(100); // gone without the death animation
            let pos = [m.pos[0], m.pos[1], m.pos[2]];
            self.explode(pos, 3.0, Some((m.eid, "Creeper".into())), true);
        }
    }
}

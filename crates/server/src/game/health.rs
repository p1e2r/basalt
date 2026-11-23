//! Health, hunger, fall damage, eating, death and respawn (survival/adventure only).
use super::{Game, Stack};
use basalt_protocol::{ids::play::clientbound as cb, text, Writer};

const MAX_HEALTH: f32 = 20.0;

/// Who caused damage: `cause`/`direct` entity ids (damage_event sends id+1), the killer's name for
/// death messages, and the direction for hurt_animation.
#[derive(Default, Clone)]
pub struct DamageSource { pub cause: Option<i32>, pub direct: Option<i32>, pub killer: Option<String>, pub hurt_yaw: Option<f32> }

impl Game {
    pub(super) fn damage_type(&self, name: &str) -> i32 { self.sh.registries.id("minecraft:damage_type", &format!("minecraft:{name}")).unwrap_or(0) }

    pub(super) fn send_health(&self, eid: i32) {
        if let Some(p) = self.players.get(&eid) { let mut w = Writer::with_id(cb::SET_HEALTH); w.f32(p.health).varint(p.food).f32(p.saturation); p.send(w); }
    }

    /// Apply damage of a vanilla damage type ("fall", "starve", "generic", …). Returns true if it killed.
    pub(super) fn hurt(&mut self, eid: i32, amount: f32, kind: &str) -> bool { self.hurt_inner(eid, amount, kind, false, DamageSource::default()) }
    /// Damage that ignores invulnerability frames (void, /kill).
    pub(super) fn hurt_bypass(&mut self, eid: i32, amount: f32, kind: &str) -> bool { self.hurt_inner(eid, amount, kind, true, DamageSource::default()) }
    /// Damage dealt by an entity (`cause` = attacker, `direct` = projectile or the attacker itself).
    pub(super) fn hurt_by(&mut self, eid: i32, amount: f32, kind: &str, src: DamageSource) -> bool { self.hurt_inner(eid, amount, kind, false, src) }

    /// Whether a damage type is in #minecraft:bypasses_armor (from the synced tag data).
    pub(super) fn bypasses_armor(&self, kind: &str) -> bool {
        let id = self.damage_type(kind);
        self.sh.registries.tags.iter().find(|(r, _)| r == "minecraft:damage_type")
            .and_then(|(_, tags)| tags.iter().find(|(t, _)| t == "minecraft:bypasses_armor" || t == "bypasses_armor")).map(|(_, ids)| ids.contains(&id)).unwrap_or(false)
    }

    /// Armor points and toughness worn in inventory slots 5..=8.
    pub(super) fn armor_of(&self, eid: i32) -> (f32, f32) {
        let Some(p) = self.players.get(&eid) else { return (0.0, 0.0) };
        (5..9).filter(|&i| p.inv[i].count > 0).filter_map(|i| self.data.item(p.inv[i].item)).fold((0.0, 0.0), |a, it| (a.0 + it.armor, a.1 + it.toughness))
    }

    /// Armor durability: each worn piece loses max(1, floor(damage / 4)).
    fn wear_armor(&mut self, eid: i32, damage: f32) {
        let loss = ((damage / 4.0).floor() as i32).max(1);
        let maxes: Vec<(usize, i32)> = { let Some(p) = self.players.get(&eid) else { return }; (5..9).filter(|&i| p.inv[i].count > 0).filter_map(|i| self.data.item(p.inv[i].item).filter(|it| it.max_damage > 0 && it.armor > 0.0).map(|it| (i, it.max_damage))).collect() };
        if maxes.is_empty() { return; }
        let p = self.players.get_mut(&eid).unwrap();
        for (i, max) in maxes { p.inv[i].damage += loss; if p.inv[i].damage >= max { p.inv[i] = Stack::default(); } }
        Self::send_window(p);
    }

    fn hurt_inner(&mut self, eid: i32, amount: f32, kind: &str, bypass: bool, src: DamageSource) -> bool {
        let dt = self.damage_type(kind);
        let Some(p) = self.players.get(&eid) else { return false };
        if p.dead || amount <= 0.0 { return false; }
        // creative / spectator players are invulnerable except to the void and /kill
        if matches!(p.gamemode, 1 | 3) && !matches!(kind, "out_of_world" | "generic_kill") { return false; }
        if p.hurt_cooldown > 0 && !bypass && kind != "starve" { return false; }
        let mut amount = amount;
        if !self.bypasses_armor(kind) {
            let (armor, tough) = self.armor_of(eid);
            if armor > 0.0 { self.wear_armor(eid, amount); amount = basalt_game::armor_reduced(amount, armor, tough); }
        }
        let p = self.players.get_mut(&eid).unwrap();
        p.hurt_cooldown = 10;
        p.health = (p.health - amount).max(0.0);
        log::debug!("{} took {amount} {kind} damage -> {}", p.name, p.health);
        p.exhaustion += 0.1;
        let died = p.health <= 0.0;
        let (c, d) = (src.cause.map(|e| e + 1).unwrap_or(0), src.direct.or(src.cause).map(|e| e + 1).unwrap_or(0));
        self.broadcast(None, || { let mut w = Writer::with_id(cb::DAMAGE_EVENT); w.varint(eid).varint(dt).varint(c).varint(d).bool(false); w });
        if let Some(yaw) = src.hurt_yaw { let mut w = Writer::with_id(cb::HURT_ANIMATION); w.varint(eid).f32(yaw); self.players[&eid].send(w); }
        self.send_health(eid);
        if died { self.die(eid, kind, src.killer.as_deref()); }
        died
    }

    fn die(&mut self, eid: i32, kind: &str, killer: Option<&str>) {
        let Some(p) = self.players.get_mut(&eid) else { return };
        p.dead = true;
        p.using = None;
        let name = p.name.clone();
        let msg = match kind {
            "fall" => format!("{name} hit the ground too hard"), "starve" => format!("{name} starved to death"), "out_of_world" => format!("{name} fell out of the world"),
            "drown" => format!("{name} drowned"), "in_wall" => format!("{name} suffocated in a wall"), "lava" => format!("{name} tried to swim in lava"),
            "in_fire" | "campfire" => format!("{name} went up in flames"), "on_fire" => format!("{name} burned to death"), "hot_floor" => format!("{name} discovered the floor was lava"),
            "mob_attack" | "mob_attack_no_aggro" | "player_attack" => format!("{name} was slain by {}", killer.unwrap_or("a mob")),
            "arrow" | "mob_projectile" => format!("{name} was shot by {}", killer.unwrap_or("an arrow")),
            "explosion" | "player_explosion" => match killer { Some(k) => format!("{name} was blown up by {k}"), None => format!("{name} blew up") },
            "cactus" => format!("{name} was pricked to death"), "sweet_berry_bush" => format!("{name} was poked to death by a sweet berry bush"), "generic_kill" => format!("{name} was killed"), "freeze" => format!("{name} froze to death"),
            _ => format!("{name} died"),
        };
        // `/kill` sends vanilla's translatable death message (the other causes keep their English text for now)
        let msg_tag = if kind == "generic_kill" { text::translate("death.attack.genericKill", vec![text::plain(&name)]) } else { text::plain(&msg) };
        let mut w = Writer::with_id(cb::PLAYER_COMBAT_KILL); w.varint(eid).nbt(&msg_tag); p.send(w);
        // death animation for everyone else
        self.broadcast(Some(eid), || { let mut w = Writer::with_id(cb::ENTITY_EVENT); w.i32(eid).u8(3); w });
        self.system_chat(msg_tag);
        self.force_close(eid); // an open container / crafting window closes on death
        // drop the whole inventory (keepInventory off)
        let stacks: Vec<Stack> = { let p = self.players.get_mut(&eid).unwrap(); let v = p.inv.iter().skip(1).copied().filter(|s| s.count > 0).chain(std::iter::once(p.cursor).filter(|s| s.count > 0)).collect(); p.inv = [Stack::default(); 46]; p.cursor = Stack::default(); v };
        for s in stacks { self.throw_item(eid, s, true); }
        if let Some(p) = self.players.get_mut(&eid) { Self::send_window(p); }
    }

    /// client_command 0: perform respawn.
    pub(super) fn respawn(&mut self, eid: i32) {
        // players always respawn at the world spawn in the overworld (no beds / anchors yet)
        // a /spawnpoint wins (Basalt has no beds / anchors yet), else the overworld's world spawn
        // (the respawn packet below is overworld-only, so other dimensions' points are ignored)
        let point = self.players.get(&eid).and_then(|p| p.spawn_point).filter(|(_, d)| *d == super::dim::OVERWORLD);
        let home = super::dim::OVERWORLD;
        if self.cur != home && self.players.get(&eid).map(|p| p.dead).unwrap_or(false) {
            let Some(p) = self.players.remove(&eid) else { return };
            for o in self.players.values() { let mut w = Writer::with_id(cb::REMOVE_ENTITIES); w.varint(1).varint(eid); o.send(w); }
            self.enter(home);
            self.players.insert(eid, p);
        }
        let spawn = point.map(|(b, _)| [b[0] as f64 + 0.5, b[1] as f64, b[2] as f64 + 0.5]).unwrap_or(self.spawn);
        let dims = self.sh.registries.entries("minecraft:dimension_type");
        let dim_type = dims.iter().position(|d| d == "minecraft:overworld").unwrap_or(0) as i32;
        let seed_hash = self.hashed_seed();
        // flat worlds use the min build height as their horizon (so the sky isn't darkened below y=63)
        let flat = self.sh.cfg.flat;
        let Some(p) = self.players.get_mut(&eid) else { return };
        if !p.dead { return; }
        p.dead = false; p.health = MAX_HEALTH; p.food = 20; p.saturation = 5.0; p.exhaustion = 0.0; p.fall_start = None;
        p.air = super::env::MAX_AIR; p.fire_ticks = 0; p.ticks_frozen = 0;
        let effects: Vec<i32> = p.effects.keys().copied().collect();
        p.effects.clear();
        p.pos = spawn; p.yaw = 0.0; p.pitch = 0.0;
        let mut w = Writer::with_id(cb::RESPAWN);
        w.varint(dim_type).str("minecraft:overworld").i64(seed_hash).u8(p.gamemode).i8(-1).bool(false).bool(flat).bool(false).varint(0).varint(63).u8(0);
        p.send(w);
        // the client starts a fresh level: resend chunks around the new position
        p.loaded.clear(); p.seen_items.clear(); p.seen.clear(); p.center = (i32::MAX, i32::MAX); p.batch_outstanding = false;
        Self::send_abilities(p);
        Self::teleport(p);
        let mut w = Writer::with_id(cb::GAME_EVENT); w.u8(13).f32(0.0); p.send(w);
        Self::send_window(p);
        let w = self.set_time_packet(); self.players[&eid].send(w);
        for id in effects { let mut w = Writer::with_id(cb::REMOVE_MOB_EFFECT); w.varint(eid).varint(id); self.players[&eid].send(w); }
        self.send_health(eid);
        // re-add for others (they saw the death animation)
        let add = self.add_entity(&self.players[&eid]);
        for o in self.players.values() { if o.eid != eid { o.send(Writer::with_id(cb::REMOVE_ENTITIES).tap(|w| { w.varint(1).varint(eid); })); o.send(Writer { buf: add.buf.clone() }); } }
        // the respawned player sees the others again
        let others: Vec<Writer> = self.players.values().filter(|o| o.eid != eid).map(|o| self.add_entity(o)).collect();
        for w in others { self.players[&eid].send(w); }
        self.stream_chunks(eid);
    }

    /// Fall tracking from movement packets; called after the player's position updated.
    pub(super) fn track_fall(&mut self, eid: i32, old_y: f64) {
        let Some(p) = self.players.get_mut(&eid) else { return };
        if p.dead || matches!(p.gamemode, 1 | 3) || p.flying { p.fall_start = None; return; }
        let feet = self.world.get_block(p.pos[0].floor() as i32, p.pos[1].floor() as i32, p.pos[2].floor() as i32);
        let in_fluid = basalt_world::props::props(feet).has_fluid();
        let climbing = matches!(basalt_data::BlockState(feet).block().kind, "ladder" | "vine" | "scaffolding");
        if in_fluid || climbing { p.fall_start = None; return; }
        if p.on_ground {
            if let Some(top) = p.fall_start.take() {
                let dist = top - p.pos[1];
                let dmg = (dist - 3.0).ceil() as f32;
                // landing on hay / slime / beds etc. reduces damage
                let below = basalt_data::BlockState(self.world.get_block(p.pos[0].floor() as i32, (p.pos[1] - 0.2).floor() as i32, p.pos[2].floor() as i32)).name();
                let k = match below { "hay_block" => 0.2, "slime_block" => 0.0, "honey_block" => 0.2, n if n.ends_with("_bed") => 0.5, "pointed_dripstone" => 2.0, _ => 1.0 };
                if dmg * k >= 1.0 { self.hurt(eid, (dmg * k).floor(), "fall"); }
            }
        } else {
            let top = p.fall_start.get_or_insert(old_y.max(p.pos[1]));
            if p.pos[1] > *top { *top = p.pos[1]; }
        }
    }

    /// Hunger, regeneration, starvation, eating. Once per tick per player.
    pub(super) fn tick_survival(&mut self) {
        let difficulty = self.difficulty;
        let ids: Vec<i32> = self.players.keys().copied().collect();
        for eid in ids {
            let mut changed = false;
            let mut starve = false;
            let mut finished_eating: Option<usize> = None;
            {
                let p = self.players.get_mut(&eid).unwrap();
                if p.hurt_cooldown > 0 { p.hurt_cooldown -= 1; }
                if p.dead || matches!(p.gamemode, 1 | 3) { continue; }
                // exhaustion → saturation → food
                if p.exhaustion >= 4.0 {
                    p.exhaustion -= 4.0;
                    if p.saturation > 0.0 { p.saturation = (p.saturation - 1.0).max(0.0); } else { p.food = (p.food - 1).max(0); }
                    changed = true;
                }
                p.regen_timer += 1;
                if p.food >= 20 && p.saturation > 0.0 && p.health < MAX_HEALTH {
                    if p.regen_timer >= 10 { let heal = p.saturation.min(6.0) / 6.0; p.health = (p.health + heal).min(MAX_HEALTH); p.exhaustion += heal * 6.0; p.regen_timer = 0; changed = true; }
                } else if p.food >= 18 && p.health < MAX_HEALTH {
                    if p.regen_timer >= 80 { p.health = (p.health + 1.0).min(MAX_HEALTH); p.exhaustion += 6.0; p.regen_timer = 0; changed = true; }
                } else if p.food == 0 {
                    // starving: easy stops at 5 hearts, normal at half a heart, hard kills
                    if p.regen_timer >= 80 { p.regen_timer = 0; if p.health > [20.0, 10.0, 1.0, 0.0][difficulty as usize] { starve = true; } }
                } else { p.regen_timer = 0; }
                if let Some((slot, left)) = p.using.as_mut() {
                    if *left > 0 { *left -= 1; }
                    if *left == 0 { finished_eating = Some(*slot); }
                }
            }
            if let Some(slot) = finished_eating { self.finish_eating(eid, slot); changed = true; }
            if starve { self.hurt(eid, 1.0, "starve"); }
            if changed { self.send_health(eid); }
        }
    }

    /// use_item with food in hand: start eating if allowed.
    pub(super) fn start_using(&mut self, eid: i32, hand: i32) {
        let Some(p) = self.players.get(&eid) else { return };
        let slot = if hand == 0 { 36 + p.held } else { 45 };
        let s = p.inv[slot];
        if s.count <= 0 || p.dead { return; }
        let Some(info) = self.data.item(s.item) else { return };
        let Some(food) = &info.food else { return };
        let creative = p.gamemode == 1;
        if p.food >= 20 && !food.can_always_eat && !creative { return; }
        let ticks = if info.consume_ticks > 0 { info.consume_ticks } else { 32 };
        self.players.get_mut(&eid).unwrap().using = Some((slot, ticks));
    }

    fn finish_eating(&mut self, eid: i32, slot: usize) {
        let Some(p) = self.players.get(&eid) else { return };
        let s = p.inv[slot];
        let food = self.data.item(s.item).and_then(|i| i.food.as_ref()).map(|f| (f.nutrition, f.saturation));
        let p = self.players.get_mut(&eid).unwrap();
        p.using = None;
        let Some((n, sat)) = food else { return };
        if s.count <= 0 { return; }
        p.food = (p.food + n).min(20);
        p.saturation = (p.saturation + sat).min(p.food as f32);
        if p.gamemode != 1 { p.inv[slot].count -= 1; if p.inv[slot].count <= 0 { p.inv[slot] = Stack::default(); } }
        let mut w = Writer::with_id(cb::ENTITY_EVENT); w.i32(eid).u8(9); p.send(w);
        Self::send_window(p);
    }

    /// Exhaustion from movement (called with the horizontal distance moved this packet).
    pub(super) fn add_move_exhaustion(&mut self, eid: i32, dist: f64, jumped: bool) {
        let Some(p) = self.players.get_mut(&eid) else { return };
        if matches!(p.gamemode, 1 | 3) { return; }
        if p.sprinting { p.exhaustion += 0.1 * dist as f32; }
        if jumped { p.exhaustion += if p.sprinting { 0.2 } else { 0.05 }; }
    }
}


trait Tap { fn tap(self, f: impl FnOnce(&mut Self)) -> Self; }
impl Tap for Writer { fn tap(mut self, f: impl FnOnce(&mut Self)) -> Self { f(&mut self); self } }

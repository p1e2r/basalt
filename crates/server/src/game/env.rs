//! Environmental hazards and status effects: air supply / drowning, suffocation, lava, fire and
//! burning, magma, cactus, sweet berry bushes, the void; water breathing, conduit power,
//! fire resistance; Respiration on helmets.
use super::{Game, Player};
use basalt_data::BlockState;
use basalt_protocol::{ids::play::clientbound as cb, Writer};
use basalt_world::props;
use rand::Rng;

pub const MAX_AIR: i32 = 300;
/// Ticks in powder snow until fully frozen.
pub const FREEZE_TICKS: i32 = 140;
const FIRE_FLAG: u8 = 0x01;

pub fn effect_id(name: &str) -> Option<i32> { basalt_data::registry_id(basalt_data::registries::MOB_EFFECT, name) }

/// Vanilla conduit frame: the outer ring of the three axis planes through the conduit in a 5×5×5 cube.
fn conduit_frame() -> impl Iterator<Item = [i32; 3]> {
    (-2..=2).flat_map(|x| (-2..=2).flat_map(move |y| (-2..=2).map(move |z| [x, y, z]))).filter(|p| {
        let zeros = p.iter().filter(|v| **v == 0).count();
        let ring = |a: i32, b: i32| a.abs().max(b.abs()) == 2;
        zeros >= 1 && ((p[0] == 0 && ring(p[1], p[2])) || (p[1] == 0 && ring(p[0], p[2])) || (p[2] == 0 && ring(p[0], p[1])))
    })
}

fn is_water(state: u32) -> bool {
    let st = BlockState(state);
    matches!(st.name(), "water" | "bubble_column") || st.get("waterlogged") == Some("true") || matches!(st.name(), "kelp" | "kelp_plant" | "seagrass" | "tall_seagrass")
}

impl Game {
    pub(super) fn has_effect(p: &Player, name: &str) -> bool { effect_id(name).map(|id| p.effects.contains_key(&id)).unwrap_or(false) }

    pub(super) fn add_effect(&mut self, eid: i32, name: &str, amplifier: u8, ticks: i32, show: bool) {
        let Some(id) = effect_id(name) else { return };
        let Some(p) = self.players.get_mut(&eid) else { return };
        let e = p.effects.entry(id).or_insert((amplifier, ticks));
        // keep the stronger / longer one, like vanilla
        if amplifier > e.0 || (amplifier == e.0 && (ticks < 0 || (e.1 >= 0 && ticks > e.1))) { *e = (amplifier, ticks); }
        let (a, t) = *e;
        let flags = if show { 0x0e } else { 0x08 };
        let mut w = Writer::with_id(cb::UPDATE_MOB_EFFECT); w.varint(eid).varint(id).varint(a as i32).varint(t).u8(flags); p.send(w);
    }

    pub(super) fn clear_effect(&mut self, eid: i32, id: i32) {
        let Some(p) = self.players.get_mut(&eid) else { return };
        if p.effects.remove(&id).is_some() { let mut w = Writer::with_id(cb::REMOVE_MOB_EFFECT); w.varint(eid).varint(id); p.send(w); }
    }

    /// Re-send active effects (join / respawn).
    pub(super) fn send_effects(&self, eid: i32) {
        let Some(p) = self.players.get(&eid) else { return };
        for (id, (a, t)) in &p.effects { let mut w = Writer::with_id(cb::UPDATE_MOB_EFFECT); w.varint(eid).varint(*id).varint(*a as i32).varint(*t).u8(0x0e); p.send(w); }
    }

    fn respiration_level(&self, p: &Player) -> u8 {
        let Some(id) = self.sh.registries.id("minecraft:enchantment", "minecraft:respiration") else { return 0 };
        p.inv[5].ench.iter().find(|(e, l)| *l > 0 && *e as i32 == id).map(|(_, l)| *l).unwrap_or(0)
    }

    /// Whether an entity type is in an entity_type tag (from the synced tag data).
    fn entity_tag_has(&self, tag: &str, entity: &str) -> bool {
        let Some(id) = basalt_data::registry_id(basalt_data::registries::ENTITY_TYPE, entity) else { return false };
        self.sh.registries.tags.iter().find(|(r, _)| r == "minecraft:entity_type")
            .and_then(|(_, tags)| tags.iter().find(|(t, _)| t == tag)).map(|(_, ids)| ids.contains(&id)).unwrap_or(false)
    }

    /// Any worn armor piece in #freeze_immune_wearables (leather) stops freezing entirely.
    fn freeze_immune_gear(&self, p: &Player) -> bool {
        (5..9).any(|i| p.inv[i].count > 0 && self.data.tags.item_has("minecraft:freeze_immune_wearables", p.inv[i].item))
    }

    fn send_frozen(&self, eid: i32) {
        let Some(p) = self.players.get(&eid) else { return };
        let v = p.ticks_frozen;
        // index 7 = ticks frozen (varint); everyone sees the shivering, the player gets the overlay
        self.broadcast(None, || { let mut w = Writer::with_id(cb::SET_ENTITY_DATA); w.varint(eid).u8(7).varint(1).varint(v).u8(0xff); w });
    }

    fn send_air(&self, eid: i32) {
        let Some(p) = self.players.get(&eid) else { return };
        let mut w = Writer::with_id(cb::SET_ENTITY_DATA); w.varint(eid).u8(1).varint(1).varint(p.air).u8(0xff); p.send(w);
    }

    /// Shared entity flags (index 0): on fire, crouching, sprinting.
    fn send_flags(&self, eid: i32) {
        let Some(p) = self.players.get(&eid) else { return };
        let flags = if p.fire_ticks > 0 { FIRE_FLAG } else { 0 } | if p.sneaking { 0x02 } else { 0 } | if p.sprinting { 0x08 } else { 0 };
        self.broadcast(None, || { let mut w = Writer::with_id(cb::SET_ENTITY_DATA); w.varint(eid).u8(0).varint(0).u8(flags).u8(0xff); w });
    }

    /// Fluid surface height inside the block at (x,y,z) as a fraction of the block (0 if no water).
    fn water_height(&self, x: i32, y: i32, z: i32) -> f64 {
        let s = self.world.get_block(x, y, z);
        if !is_water(s) { return 0.0; }
        if is_water(self.world.get_block(x, y + 1, z)) { return 1.0; }
        let level: f64 = BlockState(s).get("level").and_then(|l| l.parse().ok()).unwrap_or(0.0);
        if level >= 8.0 { 1.0 } else { (8.0 - level) / 9.0 }
    }

    fn eyes_in_water(&self, p: &Player) -> bool {
        let eye = p.pos[1] + if p.sneaking { 1.27 } else { 1.62 };
        let (x, y, z) = (p.pos[0].floor() as i32, eye.floor() as i32, p.pos[2].floor() as i32);
        let h = self.water_height(x, y, z);
        h > 0.0 && eye < y as f64 + h && BlockState(self.world.get_block(x, y, z)).name() != "bubble_column"
    }

    fn body_blocks(p: &Player, inflate: f64) -> Vec<[i32; 3]> {
        let (lo, hi) = ([p.pos[0] - 0.3 - inflate, p.pos[1] - inflate, p.pos[2] - 0.3 - inflate], [p.pos[0] + 0.3 + inflate, p.pos[1] + 1.8 + inflate, p.pos[2] + 0.3 + inflate]);
        let mut v = Vec::new();
        for x in lo[0].floor() as i32..=(hi[0] - 1e-7).floor() as i32 { for y in lo[1].floor() as i32..=(hi[1] - 1e-7).floor() as i32 { for z in lo[2].floor() as i32..=(hi[2] - 1e-7).floor() as i32 { v.push([x, y, z]); } } }
        v
    }

    /// Active conduits around a player in water: count prismarine frame blocks, range = frame/7*16.
    fn near_active_conduit(&self, p: &Player) -> bool {
        let c = [p.pos[0].floor() as i32, (p.pos[1] + 1.0).floor() as i32, p.pos[2].floor() as i32];
        let r = 32;
        for dx in -r..=r { for dz in -r..=r { for dy in -r..=r {
            let b = [c[0] + dx, c[1] + dy, c[2] + dz];
            if BlockState(self.world.get_block(b[0], b[1], b[2])).name() != "conduit" { continue; }
            // needs water in the surrounding 3×3×3
            let wet = (-1..=1).all(|x| (-1..=1).all(|y| (-1..=1).all(|z| (x, y, z) == (0, 0, 0) || is_water(self.world.get_block(b[0] + x, b[1] + y, b[2] + z)))));
            if !wet { continue; }
            let frame = conduit_frame().filter(|o| matches!(BlockState(self.world.get_block(b[0] + o[0], b[1] + o[1], b[2] + o[2])).name(), "prismarine" | "prismarine_bricks" | "dark_prismarine" | "sea_lantern")).count() as i32;
            if frame < 16 { continue; }
            let range = (frame / 7 * 16) as f64;
            let d2 = (b[0] as f64 + 0.5 - p.pos[0]).powi(2) + (b[1] as f64 + 0.5 - p.pos[1]).powi(2) + (b[2] as f64 + 0.5 - p.pos[2]).powi(2);
            if d2 <= range * range { return true; }
        } } }
        false
    }

    /// Effects ticking, air supply, fire and contact damage. Once per tick.
    pub(super) fn tick_environment(&mut self) {
        let ids: Vec<i32> = self.players.keys().copied().collect();
        for eid in ids {
            // --- effects countdown
            let expired: Vec<i32> = {
                let p = self.players.get_mut(&eid).unwrap();
                for e in p.effects.values_mut() { if e.1 > 0 { e.1 -= 1; } }
                p.effects.iter().filter(|(_, e)| e.1 == 0).map(|(id, _)| *id).collect()
            };
            for id in expired { self.clear_effect(eid, id); }
            let p = &self.players[&eid];
            if p.dead { continue; }
            let vulnerable = !matches!(p.gamemode, 1 | 3);
            let eyes_wet = self.eyes_in_water(p);
            let body = Self::body_blocks(p, 0.0);
            let body_states: Vec<([i32; 3], u32)> = body.iter().map(|b| (*b, self.world.get_block(b[0], b[1], b[2]))).collect();
            let in_water = body_states.iter().any(|(b, s)| is_water(*s) && p.pos[1] < b[1] as f64 + self.water_height(b[0], b[1], b[2]));
            let in_powder_snow = body_states.iter().any(|(_, s)| BlockState(*s).name() == "powder_snow");
            let can_freeze = vulnerable && !self.freeze_immune_gear(p) && !self.entity_tag_has("minecraft:freeze_immune_entity_types", "player");
            let freeze_dmg = if self.entity_tag_has("minecraft:freeze_hurts_extra_types", "player") { 5.0 } else { 1.0 };
            let in_lava = body_states.iter().any(|(_, s)| BlockState(*s).name() == "lava");
            let fire_block = body_states.iter().filter_map(|(_, s)| match BlockState(*s).name() { "fire" => Some(1.0), "soul_fire" => Some(2.0), _ => None }).fold(0.0f32, f32::max);
            let below = BlockState(self.world.get_block(p.pos[0].floor() as i32, (p.pos[1] - 0.2).floor() as i32, p.pos[2].floor() as i32));
            let moved = ((p.pos[0] - p.prev_tick_pos[0]).powi(2) + (p.pos[2] - p.prev_tick_pos[2]).powi(2)).sqrt() > 0.003;
            let fire_res = Self::has_effect(p, "fire_resistance");
            let water_breathing = Self::has_effect(p, "water_breathing") || Self::has_effect(p, "conduit_power");
            let respiration = self.respiration_level(p);
            let turtle = p.inv[5].count > 0 && basalt_data::registries::ITEM.get(p.inv[5].item as usize) == Some(&"turtle_helmet");
            let check_conduit = in_water && self.tick % 40 == 0;
            let conduit = check_conduit && self.near_active_conduit(p);
            // --- suffocation: eye-level points inside a full solid block
            let eye = p.pos[1] + if p.sneaking { 1.27 } else { 1.62 };
            // a one-block gap (solid only at head height) makes the player crawl: no suffocation. Needs the feet block solid too.
            let feet_blocked = [(-0.24, -0.24), (0.24, -0.24), (-0.24, 0.24), (0.24, 0.24)].iter().any(|(dx, dz)| {
                let s = self.world.get_block((p.pos[0] + dx).floor() as i32, p.pos[1].floor() as i32, (p.pos[2] + dz).floor() as i32);
                let pr = props::props(s); pr.blocks_motion() && pr.opacity >= 15
            });
            let suffocating = vulnerable && feet_blocked && [(-0.24, -0.24), (0.24, -0.24), (-0.24, 0.24), (0.24, 0.24)].iter().any(|(dx, dz)| {
                let s = self.world.get_block((p.pos[0] + dx).floor() as i32, eye.floor() as i32, (p.pos[2] + dz).floor() as i32);
                let pr = props::props(s); pr.blocks_motion() && pr.opacity >= 15
            });
            let cactus = body_states.iter().any(|(_, s)| BlockState(*s).name() == "cactus")
                || Self::body_blocks(p, 0.01).iter().any(|b| BlockState(self.world.get_block(b[0], b[1], b[2])).name() == "cactus");
            let berry = moved && body_states.iter().any(|(_, s)| { let st = BlockState(*s); st.name() == "sweet_berry_bush" && st.get("age").map(|a| a != "0").unwrap_or(false) });
            let campfire = if below.get("lit") == Some("true") { match below.name() { "campfire" => 1.0, "soul_campfire" => 2.0, _ => 0.0 } } else { 0.0 };
            let magma = below.name() == "magma_block" && !p.sneaking;
            let void = p.pos[1] < basalt_world::MIN_Y as f64 - 64.0;

            if conduit { self.add_effect(eid, "conduit_power", 0, 260, true); }
            if turtle && !eyes_wet { self.add_effect(eid, "water_breathing", 0, 200, false); }

            // --- air supply
            let hunger = effect_id("hunger").and_then(|id| self.players[&eid].effects.get(&id).map(|e| e.0));
            let p = self.players.get_mut(&eid).unwrap();
            p.prev_tick_pos = p.pos;
            // Hunger effect: 0.005 exhaustion per tick per level
            if let Some(amp) = hunger { if vulnerable { p.exhaustion += 0.005 * (amp as f32 + 1.0); } }
            let old_air = p.air;
            let mut drown = false;
            if eyes_wet && vulnerable {
                let water_breathing = water_breathing || conduit;
                if !water_breathing {
                    let skip = respiration > 0 && self.rng.gen_range(0..respiration as u32 + 1) > 0;
                    if !skip { p.air -= 1; }
                    if p.air <= -20 { p.air = 0; drown = true; }
                }
            } else if p.air < MAX_AIR {
                p.air = (p.air + 4).min(MAX_AIR);
            }
            let air_changed = p.air != old_air;
            // --- freezing (powder snow)
            let old_frozen = p.ticks_frozen;
            // in powder snow: +1 (up to fully frozen); outside: thaw 2 per tick; fire or lava thaws at once.
            // Every change (up or down) is re-synced below so the client's frost overlay also fades out.
            if in_lava || fire_block > 0.0 { p.ticks_frozen = 0; }
            else if in_powder_snow && can_freeze { p.ticks_frozen = (p.ticks_frozen + 1).min(FREEZE_TICKS); }
            else { p.ticks_frozen = (p.ticks_frozen - 2).max(0); }
            let frozen_changed = p.ticks_frozen != old_frozen;
            let freeze_hit = p.ticks_frozen >= FREEZE_TICKS && can_freeze && self.tick % 40 == 0;
            // --- fire
            let was_burning = p.fire_ticks > 0;
            if in_water { p.fire_ticks = 0; }
            if in_lava && !in_water { p.fire_ticks = p.fire_ticks.max(300); }
            if fire_block > 0.0 && !in_water { p.fire_exposure += 1; if p.fire_exposure >= 20 { p.fire_ticks = p.fire_ticks.max(160); } } else { p.fire_exposure = 0; }
            let burn_tick = p.fire_ticks > 0 && p.fire_ticks % 20 == 0;
            if p.fire_ticks > 0 { p.fire_ticks -= if fire_res { 4 } else { 1 }; p.fire_ticks = p.fire_ticks.max(0); }
            let burning_changed = was_burning != (p.fire_ticks > 0);

            if air_changed { self.send_air(eid); }
            if burning_changed { self.send_flags(eid); }
            if frozen_changed { self.send_frozen(eid); }
            if !vulnerable { continue; }
            // vanilla damage per source; the hurt cooldown spaces repeated hits (10 ticks)
            // vanilla: 4.0 per hit, spaced by the normal invulnerability frames (death after ~50 ticks)
            if void { self.hurt(eid, 4.0, "out_of_world"); continue; }
            if drown { self.hurt(eid, 2.0, "drown"); }
            if freeze_hit { self.hurt(eid, freeze_dmg, "freeze"); }
            if in_lava && !fire_res { self.hurt(eid, 4.0, "lava"); }
            // vanilla starts fire damage after the same 20-tick exposure delay as ignition
            if fire_block > 0.0 && !fire_res && self.players[&eid].fire_exposure >= 20 { self.hurt(eid, fire_block, "in_fire"); }
            if campfire > 0.0 && !fire_res { self.hurt(eid, campfire, "campfire"); }
            if magma && !fire_res { self.hurt(eid, 1.0, "hot_floor"); }
            if burn_tick && !fire_res && !in_lava { self.hurt(eid, 1.0, "on_fire"); }
            if suffocating { self.hurt(eid, 1.0, "in_wall"); }
            if cactus { self.hurt(eid, 1.0, "cactus"); }
            if berry { self.hurt(eid, 1.0, "sweet_berry_bush"); }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn conduit_frame_has_42_positions() {
        // vanilla's full frame is 42 blocks (3 rings of 16 minus 6 shared)
        assert_eq!(super::conduit_frame().count(), 42);
    }
}

//! Test-harness queries (only with `--debug`): one-line system-chat replies that crates/check parses.
//!
//! - `/basalt:state [player]` → `BASALT_STATE {json}` (health, food, saturation, exhaustion, air,
//!   ticks_frozen, fire_ticks, effects, inventory by window-0 slot, pos, gamemode, dead, on_ground)
//! - `/basalt:block x y z` → `BASALT_BLOCK minecraft:name[props]`
//! - `/basalt:items [radius]` → `BASALT_ITEMS [json]` (item entities near the sender)
use super::Game;
use basalt_data::{registries as reg, BlockState};
use basalt_protocol::text;
use serde_json::json;

impl Game {
    pub(super) fn debug_command(&mut self, eid: i32, args: &[&str]) {
        let reply = |g: &Game, s: String| g.tell(eid, text::plain(&s));
        match args[0] {
            "basalt:state" => {
                let target = args.get(1).and_then(|n| self.players.values().find(|p| p.name.eq_ignore_ascii_case(n))).or_else(|| self.players.get(&eid));
                let Some(p) = target else { reply(self, "BASALT_ERROR no such player".into()); return };
                let enchs = self.sh.registries.entries("minecraft:enchantment");
                let inv: Vec<_> = p.inv.iter().enumerate().filter(|(_, s)| s.count > 0).map(|(i, s)| json!({
                    "slot": i, "item": reg::ITEM.get(s.item as usize).copied().unwrap_or("?"), "count": s.count, "damage": s.damage,
                    "ench": s.ench.iter().filter(|e| e.1 > 0).map(|e| (enchs.get(e.0 as usize).map(|n| n.trim_start_matches("minecraft:").to_string()).unwrap_or_else(|| e.0.to_string()), json!(e.1))).collect::<serde_json::Map<_, _>>(),
                })).collect();
                let effects: serde_json::Map<_, _> = p.effects.iter().map(|(id, (a, t))| (reg::MOB_EFFECT.get(*id as usize).copied().unwrap_or("?").to_string(), json!([a, t]))).collect();
                let j = json!({ "name": p.name, "health": p.health, "food": p.food, "saturation": p.saturation, "exhaustion": p.exhaustion,
                    "air": p.air, "ticks_frozen": p.ticks_frozen, "fire_ticks": p.fire_ticks, "effects": effects, "inventory": inv,
                    "pos": p.pos, "gamemode": p.gamemode, "dead": p.dead, "on_ground": p.on_ground, "xp_total": p.xp });
                reply(self, format!("BASALT_STATE {j}"));
            }
            "basalt:block" => {
                let c: Option<Vec<i32>> = args.iter().skip(1).take(3).map(|a| a.parse().ok()).collect();
                match c { Some(c) if c.len() == 3 => { let s = BlockState(self.world.get_block(c[0], c[1], c[2])).to_string(); reply(self, format!("BASALT_BLOCK {s}")); }
                    _ => reply(self, "BASALT_ERROR usage: /basalt:block x y z".into()) }
            }
            "basalt:container" => {
                let c: Option<Vec<i32>> = args.iter().skip(1).take(3).map(|a| a.parse().ok()).collect();
                let Some(c) = c.filter(|c| c.len() == 3) else { reply(self, "BASALT_ERROR usage: /basalt:container x y z".into()); return };
                let pos = [c[0], c[1], c[2]];
                match self.bes.get(&pos) {
                    Some(b) => {
                        let items: serde_json::Map<_, _> = b.items.iter().enumerate().filter(|(_, s)| s.count > 0).map(|(i, s)| (i.to_string(), json!([reg::ITEM.get(s.item as usize).copied().unwrap_or("?"), s.count, s.damage]))).collect();
                        let j = json!({ "kind": b.kind.name(), "items": items, "lit": b.lit, "lit_total": b.lit_total, "cook": b.cook, "cook_total": b.cook_total, "used": b.used, "viewers": b.viewers.len() });
                        reply(self, format!("BASALT_CONTAINER {j}"));
                    }
                    None => reply(self, "BASALT_CONTAINER null".into()),
                }
            }
            "basalt:items" => {
                let r: f64 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(16.0);
                let Some(p) = self.players.get(&eid) else { return };
                let v: Vec<_> = self.items.values().filter(|e| (0..3).map(|i| (e.pos[i] - p.pos[i]).powi(2)).sum::<f64>() <= r * r).map(|e| json!({
                    "eid": e.eid, "item": reg::ITEM.get(e.stack.item as usize).copied().unwrap_or("?"), "count": e.stack.count, "pos": e.pos, "age": e.age, "on_ground": e.on_ground,
                })).collect();
                reply(self, format!("BASALT_ITEMS {}", serde_json::Value::Array(v)));
            }
            other => reply(self, format!("BASALT_ERROR unknown {other}")),
        }
    }
}

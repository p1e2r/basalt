//! Server-side state queries, normalized across servers.
//!
//! Basalt (`--debug`): `/basalt:state` → `BASALT_STATE {json}`, `/basalt:block x y z` →
//! `BASALT_BLOCK minecraft:name[props]`, `/basalt:items` → `BASALT_ITEMS [json]`.
//! Vanilla: `/data get entity @s <path>` (reply `commands.data.entity.query(<name>, <snbt>)`),
//! `/execute if block x y z <state>` (`commands.execute.conditional.pass` / `.fail`),
//! `/execute if entity @e[type=item,distance=..R]` (`commands.execute.conditional.pass_count(N)`).
use crate::bot::Bot;
use crate::proto::parse_snbt;
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Basalt, Vanilla }

impl Kind { pub fn name(self) -> &'static str { match self { Kind::Basalt => "basalt", Kind::Vanilla => "vanilla" } } }

#[derive(Clone, Debug, Default, PartialEq)]
pub struct InvItem { pub item: String, pub count: i64, pub damage: i64, pub ench: BTreeMap<String, i64> }

#[derive(Clone, Debug, Default)]
pub struct ServerState {
    pub health: f64,
    pub food: i64,
    pub saturation: f64,
    pub air: i64,
    pub ticks_frozen: i64,
    pub fire_ticks: i64,
    /// effect name -> (amplifier, remaining ticks)
    pub effects: BTreeMap<String, (i64, i64)>,
    /// window-0 slot -> stack
    pub inv: BTreeMap<usize, InvItem>,
    pub pos: [f64; 3],
    pub gamemode: i64,
    pub dead: bool,
    /// experience points (total)
    pub xp_total: i64,
}

impl ServerState {
    pub fn count(&self, item: &str) -> i64 { self.inv.values().filter(|i| i.item == item).map(|i| i.count).sum() }
    pub fn to_json(&self) -> Value {
        json!({ "health": self.health, "food": self.food, "saturation": self.saturation, "air": self.air, "ticks_frozen": self.ticks_frozen,
            "fire_ticks": self.fire_ticks, "effects": self.effects.iter().map(|(k, v)| (k.clone(), json!([v.0, v.1]))).collect::<serde_json::Map<_, _>>(),
            "inventory": self.inv.iter().map(|(k, v)| (k.to_string(), json!({"item": v.item, "count": v.count, "damage": v.damage, "ench": v.ench}))).collect::<serde_json::Map<_, _>>(),
            "pos": self.pos, "gamemode": self.gamemode, "dead": self.dead })
    }
}

fn strip(s: &str) -> String { s.trim_start_matches("minecraft:").to_string() }

fn basalt_line(bot: &Bot, cmd: &str, prefix: &str) -> Result<String> {
    let lines = bot.cmd_reply(cmd)?;
    lines.iter().find_map(|l| l.plain.strip_prefix(prefix).map(|s| s.to_string()))
        .ok_or_else(|| anyhow!("/{cmd}: unexpected reply {:?} (is the server running with --debug?)", lines.iter().map(|l| &l.plain).collect::<Vec<_>>()))
}

/// `data get entity @s <path>` on vanilla: Some(value) or None when the path doesn't exist.
pub fn vanilla_get(bot: &Bot, path: &str) -> Result<Option<Value>> {
    let lines = bot.cmd_reply(&format!("data get entity @s {path}"))?;
    for l in &lines {
        match l.key.as_deref() {
            Some("commands.data.entity.query") => {
                let v = l.args.get(1).ok_or_else(|| anyhow!("query reply without value"))?;
                return parse_snbt(v).map(Some).ok_or_else(|| anyhow!("unparseable SNBT {v}"));
            }
            Some(k) if k.starts_with("arguments.nbtpath") || k.starts_with("commands.data") => return Ok(None),
            _ => {}
        }
    }
    bail!("data get {path}: unexpected reply {:?}", lines.iter().map(|l| &l.plain).collect::<Vec<_>>())
}

fn vanilla_item(v: &Value) -> InvItem {
    let c = &v["components"];
    let mut ench = BTreeMap::new();
    let e = &c["minecraft:enchantments"];
    let levels = if e.get("levels").is_some() { &e["levels"] } else { e };
    if let Some(m) = levels.as_object() { for (k, l) in m { ench.insert(strip(k), l.as_f64().unwrap_or(0.0) as i64); } }
    InvItem { item: strip(v["id"].as_str().unwrap_or("")), count: v["count"].as_f64().unwrap_or(1.0) as i64, damage: c["minecraft:damage"].as_f64().unwrap_or(0.0) as i64, ench }
}

pub fn state(kind: Kind, bot: &Bot) -> Result<ServerState> {
    match kind {
        Kind::Basalt => {
            let j: Value = serde_json::from_str(&basalt_line(bot, "basalt:state", "BASALT_STATE ")?)?;
            let mut s = ServerState {
                health: j["health"].as_f64().unwrap_or(0.0), food: j["food"].as_i64().unwrap_or(0), saturation: j["saturation"].as_f64().unwrap_or(0.0),
                air: j["air"].as_i64().unwrap_or(0), ticks_frozen: j["ticks_frozen"].as_i64().unwrap_or(0), fire_ticks: j["fire_ticks"].as_i64().unwrap_or(0),
                gamemode: j["gamemode"].as_i64().unwrap_or(0), dead: j["dead"].as_bool().unwrap_or(false), xp_total: j["xp_total"].as_i64().unwrap_or(0), ..Default::default()
            };
            if let Some(p) = j["pos"].as_array() { for i in 0..3 { s.pos[i] = p.get(i).and_then(|v| v.as_f64()).unwrap_or(0.0); } }
            if let Some(m) = j["effects"].as_object() { for (k, v) in m { s.effects.insert(k.clone(), (v[0].as_i64().unwrap_or(0), v[1].as_i64().unwrap_or(0))); } }
            for it in j["inventory"].as_array().into_iter().flatten() {
                let mut ench = BTreeMap::new();
                if let Some(m) = it["ench"].as_object() { for (k, l) in m { ench.insert(k.clone(), l.as_i64().unwrap_or(0)); } }
                s.inv.insert(it["slot"].as_u64().unwrap_or(0) as usize, InvItem { item: strip(it["item"].as_str().unwrap_or("")), count: it["count"].as_i64().unwrap_or(0), damage: it["damage"].as_i64().unwrap_or(0), ench });
            }
            Ok(s)
        }
        Kind::Vanilla => {
            let num = |p: &str| -> Result<f64> { Ok(vanilla_get(bot, p)?.and_then(|v| v.as_f64()).unwrap_or(0.0)) };
            let mut s = ServerState { health: num("Health")?, food: num("foodLevel")? as i64, saturation: num("foodSaturationLevel")?,
                air: num("Air")? as i64, ticks_frozen: num("TicksFrozen")? as i64, fire_ticks: (num("Fire")? as i64).max(0), gamemode: num("playerGameType")? as i64, xp_total: num("XpTotal")? as i64, ..Default::default() };
            s.dead = s.health <= 0.0;
            if let Some(Value::Array(p)) = vanilla_get(bot, "Pos")? { for i in 0..3 { s.pos[i] = p.get(i).and_then(|v| v.as_f64()).unwrap_or(0.0); } }
            if let Some(Value::Array(es)) = vanilla_get(bot, "active_effects")? {
                for e in es { s.effects.insert(strip(e["id"].as_str().unwrap_or("")), (e["amplifier"].as_f64().unwrap_or(0.0) as i64, e["duration"].as_f64().unwrap_or(0.0) as i64)); }
            }
            if let Some(Value::Array(items)) = vanilla_get(bot, "Inventory")? {
                for it in items {
                    let slot = it["Slot"].as_f64().unwrap_or(-1.0) as i64;
                    let idx = match slot { 0..=8 => 36 + slot as usize, 9..=35 => slot as usize, 100 => 8, 101 => 7, 102 => 6, 103 => 5, -106 => 45, _ => continue };
                    s.inv.insert(idx, vanilla_item(&it));
                }
            }
            if let Some(Value::Object(eq)) = vanilla_get(bot, "equipment")? {
                for (k, v) in eq { let idx = match k.as_str() { "head" => 5, "chest" => 6, "legs" => 7, "feet" => 8, "offhand" => 45, _ => continue }; s.inv.insert(idx, vanilla_item(&v)); }
            }
            Ok(s)
        }
    }
}

/// Server-side block state test (`stone`, `oak_log[axis=x]`; unspecified properties match any).
pub fn block_is(kind: Kind, bot: &Bot, p: [i32; 3], want: &str) -> Result<bool> {
    match kind {
        Kind::Basalt => {
            let got = basalt_line(bot, &format!("basalt:block {} {} {}", p[0], p[1], p[2]), "BASALT_BLOCK ")?;
            Ok(state_matches(&got, want))
        }
        Kind::Vanilla => {
            let lines = bot.cmd_reply(&format!("execute if block {} {} {} {}", p[0], p[1], p[2], want))?;
            for l in &lines {
                match l.key.as_deref() { Some("commands.execute.conditional.pass") => return Ok(true), Some("commands.execute.conditional.fail") => return Ok(false), _ => {} }
            }
            bail!("execute if block: unexpected reply {:?}", lines.iter().map(|l| &l.plain).collect::<Vec<_>>())
        }
    }
}

/// Basalt only: exact server-side block state string.
pub fn basalt_block(bot: &Bot, p: [i32; 3]) -> Result<String> { basalt_line(bot, &format!("basalt:block {} {} {}", p[0], p[1], p[2]), "BASALT_BLOCK ") }

/// `minecraft:oak_log[axis=x]` matches `oak_log`, `oak_log[axis=x]`.
pub fn state_matches(got: &str, want: &str) -> bool {
    let (gn, gp) = split_state(got); let (wn, wp) = split_state(want);
    gn == wn && wp.iter().all(|kv| gp.contains(kv))
}
fn split_state(s: &str) -> (String, Vec<String>) {
    let s = s.trim_start_matches("minecraft:");
    match s.split_once('[') { Some((n, p)) => (n.to_string(), p.trim_end_matches(']').split(',').map(|x| x.trim().to_string()).collect()), None => (s.to_string(), Vec::new()) }
}

/// Number of item entities within `r` blocks of the player, according to the server.
pub fn items_near(kind: Kind, bot: &Bot, r: f64) -> Result<i64> {
    match kind {
        Kind::Basalt => {
            let j: Value = serde_json::from_str(&basalt_line(bot, &format!("basalt:items {r}"), "BASALT_ITEMS ")?)?;
            Ok(j.as_array().map(|a| a.len() as i64).unwrap_or(0))
        }
        Kind::Vanilla => {
            let lines = bot.cmd_reply(&format!("execute if entity @e[type=item,distance=..{r}]"))?;
            for l in &lines {
                match l.key.as_deref() {
                    Some("commands.execute.conditional.pass_count") => return Ok(l.args.first().and_then(|a| a.parse().ok()).unwrap_or(0)),
                    Some("commands.execute.conditional.pass") => return Ok(1),
                    Some("commands.execute.conditional.fail") => return Ok(0),
                    _ => {}
                }
            }
            bail!("execute if entity: unexpected reply {:?}", lines.iter().map(|l| &l.plain).collect::<Vec<_>>())
        }
    }
}

/// A container block entity as seen by the server: slot -> (item, count) and furnace properties.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContState {
    pub items: BTreeMap<usize, (String, i64)>,
    /// potion name per slot (potion_contents), brewing stand BrewTime / Fuel
    pub potions: BTreeMap<usize, String>,
    pub brew: i64,
    pub fuel: i64,
    pub lit: i64,
    pub lit_total: i64,
    pub cook: i64,
    pub cook_total: i64,
    /// RecipesUsed (recipe id -> count)
    pub used: BTreeMap<String, i64>,
}

impl ContState {
    pub fn count(&self, item: &str) -> i64 { self.items.values().filter(|i| i.0 == item).map(|i| i.1).sum() }
    pub fn slot(&self, i: usize) -> Option<(&str, i64)> { self.items.get(&i).map(|x| (x.0.as_str(), x.1)) }
}

/// Server-side container contents at `p` (vanilla: `data get block`, Basalt: `/basalt:container`); None = no block entity.
pub fn container(kind: Kind, bot: &Bot, p: [i32; 3]) -> Result<Option<ContState>> {
    match kind {
        Kind::Basalt => {
            let line = basalt_line(bot, &format!("basalt:container {} {} {}", p[0], p[1], p[2]), "BASALT_CONTAINER ")?;
            let j: Value = serde_json::from_str(&line)?;
            if j.is_null() { return Ok(None); }
            let mut c = ContState { lit: j["lit"].as_i64().unwrap_or(0), lit_total: j["lit_total"].as_i64().unwrap_or(0), cook: j["cook"].as_i64().unwrap_or(0), cook_total: j["cook_total"].as_i64().unwrap_or(0), ..Default::default() };
            if let Some(m) = j["items"].as_object() { for (k, v) in m { c.items.insert(k.parse().unwrap_or(0), (strip(v[0].as_str().unwrap_or("")), v[1].as_i64().unwrap_or(0))); } }
            if let Some(m) = j["used"].as_object() { for (k, v) in m { c.used.insert(strip(k), v.as_i64().unwrap_or(0)); } }
            if let Some(m) = j["potions"].as_object() { for (k, v) in m { c.potions.insert(k.parse().unwrap_or(0), v.as_str().unwrap_or("").to_string()); } }
            c.brew = j["brew"].as_i64().unwrap_or(0); c.fuel = j["fuel"].as_i64().unwrap_or(0);
            Ok(Some(c))
        }
        Kind::Vanilla => {
            let lines = bot.cmd_reply(&format!("data get block {} {} {}", p[0], p[1], p[2]))?;
            for l in &lines {
                match l.key.as_deref() {
                    Some("commands.data.block.query") => {
                        let v = l.args.get(3).ok_or_else(|| anyhow!("query reply without value"))?;
                        let j = parse_snbt(v).ok_or_else(|| anyhow!("unparseable SNBT {v}"))?;
                        let n = |k: &str| j[k].as_f64().unwrap_or(0.0) as i64;
                        let mut c = ContState { lit: n("lit_time_remaining"), lit_total: n("lit_total_time"), cook: n("cooking_time_spent"), cook_total: n("cooking_total_time"), ..Default::default() };
                        for it in j["Items"].as_array().into_iter().flatten() { c.items.insert(it["Slot"].as_f64().unwrap_or(0.0) as usize, (strip(it["id"].as_str().unwrap_or("")), it["count"].as_f64().unwrap_or(1.0) as i64)); }
                        c.brew = n("BrewTime"); c.fuel = n("Fuel");
                        for it in j["Items"].as_array().into_iter().flatten() { if let Some(p) = it["components"]["minecraft:potion_contents"]["potion"].as_str() { c.potions.insert(it["Slot"].as_f64().unwrap_or(0.0) as usize, strip(p)); } }
                        if let Some(m) = j["RecipesUsed"].as_object() { for (k, v) in m { c.used.insert(strip(k), v.as_f64().unwrap_or(0.0) as i64); } }
                        return Ok(Some(c));
                    }
                    Some(k) if k.starts_with("commands.data.block") => return Ok(None),
                    _ => {}
                }
            }
            bail!("data get block: unexpected reply {:?}", lines.iter().map(|l| &l.plain).collect::<Vec<_>>())
        }
    }
}

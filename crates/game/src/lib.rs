//! Gameplay rules driven by the data generator output in `./data`: item properties, block
//! mining time and harvestability, loot tables, crafting / cooking recipes and furnace fuels.
//!
//! Hardness, "requires correct tool" and fuel burn times are not part of the data reports and
//! come from public knowledge (Minecraft Wiki); see [`hardness`] and [`fuel`].

mod combat;
mod fuel;
mod hardness;
mod loot;
mod recipe;
mod tags;

pub use hardness::{hardness, requires_correct_tool};
pub use loot::{EntityProps, LootContext};
pub use combat::{armor_reduced, blast_resistance, difficulty_scaled, explosion_damage};
pub use recipe::{CookKind, CraftResult, Ingredient, Recipe, Recipes};
pub use tags::Tags;

use basalt_data::{registries, BlockState};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

/// One rule of a `minecraft:tool` component.
#[derive(Debug, Clone)]
pub struct ToolRule {
    /// Block selectors: `#minecraft:tag` or `minecraft:block`.
    pub blocks: Vec<String>,
    pub speed: Option<f32>,
    pub correct_for_drops: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct Tool {
    pub rules: Vec<ToolRule>,
    pub default_mining_speed: f32,
    pub damage_per_block: i32,
}

#[derive(Debug, Clone)]
pub struct Food {
    pub nutrition: i32,
    pub saturation: f32,
    pub can_always_eat: bool,
}

#[derive(Debug, Clone)]
pub struct ItemInfo {
    pub id: i32,
    pub name: String,
    pub max_stack: i32,
    pub max_damage: i32,
    pub tool: Option<Tool>,
    pub food: Option<Food>,
    /// Ticks to consume (0 = not consumable). Default consumable time is 1.6 s = 32 ticks.
    pub consume_ticks: u32,
    /// Total main-hand attack damage including the player's base 1.0 (hand = 1.0).
    pub attack_damage: f32,
    /// Total main-hand attack speed including the player's base 4.0 (hand = 4.0).
    pub attack_speed: f32,
    /// Sum of armor modifiers (any slot).
    pub armor: f32,
    pub toughness: f32,
    /// `equippable.slot` ("head", "chest", "legs", "feet", "body", "saddle", …).
    pub equip_slot: Option<String>,
    /// Default state of the block this item places (same name, plus a few seed/crop aliases).
    pub block: Option<u32>,
}

pub struct GameData {
    pub tags: Tags,
    /// Index = item id.
    pub items: Vec<ItemInfo>,
    pub recipes: Recipes,
    /// Loot tables by id without namespace ("blocks/stone", "entities/zombie").
    pub loot_tables: HashMap<String, Value>,
    fuels: HashMap<i32, u32>,
}

fn read_json(p: &Path) -> anyhow::Result<Value> {
    let s = std::fs::read_to_string(p).map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?;
    serde_json::from_str(&s).map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))
}

pub(crate) fn item_id(name: &str) -> Option<i32> {
    basalt_data::registry_id(registries::ITEM, name)
}

impl GameData {
    /// `data_dir` is the repo's `./data` (containing `reports/` and `data/`).
    pub fn load(data_dir: &Path) -> anyhow::Result<GameData> {
        let mc = data_dir.join("data/minecraft");
        anyhow::ensure!(mc.is_dir(), "{} missing — run tools/extract-data.sh", mc.display());
        let tags = Tags::load(&mc.join("tags"))?;
        let items = load_items(&data_dir.join("reports/minecraft/components/item"))?;
        let loot_tables = loot::load_tables(&mc.join("loot_table"))?;
        let recipes = recipe::Recipes::load(&mc.join("recipe"), &tags)?;
        let fuels = fuel::build(&tags);
        Ok(GameData { tags, items, recipes, loot_tables, fuels })
    }

    pub fn item(&self, id: i32) -> Option<&ItemInfo> {
        usize::try_from(id).ok().and_then(|i| self.items.get(i))
    }

    pub fn item_by_name(&self, name: &str) -> Option<&ItemInfo> {
        self.item(item_id(name)?)
    }

    /// Mining speed multiplier and whether the held item is "correct for drops" on this block.
    /// Rules are evaluated in order: the first matching rule with a `speed` gives the speed, the
    /// first matching rule with `correct_for_drops` gives the flag.
    pub fn tool_speed(&self, block_name: &str, held: Option<i32>) -> (f32, bool) {
        let Some(tool) = held.and_then(|h| self.item(h)).and_then(|i| i.tool.as_ref()) else { return (1.0, false) };
        let mut speed = None;
        let mut correct = None;
        for r in &tool.rules {
            if speed.is_some() && correct.is_some() {
                break;
            }
            if ((r.speed.is_some() && speed.is_none()) || (r.correct_for_drops.is_some() && correct.is_none()))
                && r.blocks.iter().any(|s| self.tags.block_matches(s, block_name)) {
                    if speed.is_none() {
                        speed = r.speed;
                    }
                    if correct.is_none() {
                        correct = r.correct_for_drops;
                    }
                }
        }
        (speed.unwrap_or(tool.default_mining_speed), correct.unwrap_or(false))
    }

    /// Ticks to break `state` with `held` (None = hand). Efficiency/haste are ignored. Pass
    /// `in_water = false` for players with Aqua Affinity. None = unbreakable, 0 = instant.
    pub fn dig_ticks(&self, state: u32, held: Option<i32>, on_ground: bool, in_water: bool) -> Option<u32> {
        if !BlockState::is_valid(state) {
            return None;
        }
        let name = BlockState(state).name();
        let h = hardness(name);
        if h < 0.0 {
            return None;
        }
        if h == 0.0 {
            return Some(0);
        }
        let (mut speed, _) = self.tool_speed(name, held);
        if !on_ground {
            speed /= 5.0;
        }
        if in_water {
            speed /= 5.0;
        }
        let div = if self.can_harvest(state, held) { 30.0 } else { 100.0 };
        // ticks = ceil(1 / (speed / hardness / div)); computed in f64 with a small epsilon so
        // exact values like 150.0 do not round up from float error.
        let t = h as f64 * div / speed as f64;
        if t <= 1.0 {
            return Some(0);
        }
        Some((t - 1e-3).ceil().min(u32::MAX as f64) as u32)
    }

    /// True if breaking `state` with `held` yields drops (block doesn't need a tool, or the
    /// tool's rule marks it correct for drops).
    pub fn can_harvest(&self, state: u32, held: Option<i32>) -> bool {
        if !BlockState::is_valid(state) {
            return false;
        }
        let name = BlockState(state).name();
        !requires_correct_tool(name) || self.tool_speed(name, held).1
    }

    /// Evaluate `loot_table/blocks/<name>.json` for a block broken by a player. Returns nothing
    /// when the block needs a correct tool that isn't held.
    pub fn block_drops(&self, state: u32, held: Option<i32>, silk_touch: bool, fortune: u8, rng: &mut impl rand::Rng) -> Vec<(i32, i32)> {
        if !self.can_harvest(state, held) {
            return Vec::new();
        }
        let ctx = LootContext { tool: held, silk_touch, fortune, ..Default::default() };
        let table = format!("blocks/{}", BlockState(state).name());
        loot::eval_table(self, &table, &ctx, Some(BlockState(state)), rng)
    }

    /// Drops of a block destroyed by an explosion of `radius` (survives_explosion / explosion_decay).
    pub fn explosion_drops(&self, state: u32, radius: f32, rng: &mut impl rand::Rng) -> Vec<(i32, i32)> {
        if !BlockState::is_valid(state) {
            return Vec::new();
        }
        let ctx = LootContext { explosion_radius: Some(radius), ..Default::default() };
        let table = format!("blocks/{}", BlockState(state).name());
        loot::eval_table(self, &table, &ctx, Some(BlockState(state)), rng)
    }

    /// Evaluate any loot table by id ("minecraft:entities/zombie" or "entities/zombie").
    pub fn loot(&self, table: &str, ctx: &LootContext, rng: &mut impl rand::Rng) -> Vec<(i32, i32)> {
        loot::eval_table(self, table, ctx, None, rng)
    }

    /// Crafting: `grid` is row-major `width`×`height` (2×2 or 3×3) of item ids.
    pub fn craft(&self, grid: &[Option<i32>], width: usize, height: usize) -> Option<CraftResult> {
        self.recipes.craft(grid, width, height)
    }

    /// Furnace (`minecraft:smelting`) recipe: (result, xp, cook ticks).
    pub fn smelt(&self, item: i32) -> Option<(i32, f32, u32)> {
        self.recipes.cook(CookKind::Smelting, item)
    }

    /// Blast furnace / smoker / campfire recipes.
    pub fn cook(&self, kind: CookKind, item: i32) -> Option<(i32, f32, u32)> {
        self.recipes.cook(kind, item)
    }

    /// Stonecutter outputs for an input item: (result, count).
    pub fn stonecut(&self, item: i32) -> Vec<(i32, i32)> {
        self.recipes.stonecut(item)
    }

    /// Furnace burn time in ticks.
    pub fn fuel_ticks(&self, item: i32) -> Option<u32> {
        self.fuels.get(&item).copied()
    }
}

fn f32_of(v: &Value) -> Option<f32> {
    v.as_f64().map(|x| x as f32)
}

fn load_items(dir: &Path) -> anyhow::Result<Vec<ItemInfo>> {
    let aliases: HashMap<&str, &str> = [
        ("redstone", "redstone_wire"),
        ("string", "tripwire"),
        ("wheat_seeds", "wheat"),
        ("beetroot_seeds", "beetroots"),
        ("carrot", "carrots"),
        ("potato", "potatoes"),
        ("melon_seeds", "melon_stem"),
        ("pumpkin_seeds", "pumpkin_stem"),
        ("sweet_berries", "sweet_berry_bush"),
        ("glow_berries", "cave_vines"),
        ("cocoa_beans", "cocoa"),
        ("torchflower_seeds", "torchflower_crop"),
        ("pitcher_pod", "pitcher_crop"),
    ]
    .into_iter()
    .collect();
    let mut out = Vec::with_capacity(registries::ITEM.len());
    for (id, name) in registries::ITEM.iter().enumerate() {
        let p = dir.join(format!("{name}.json"));
        let v = if p.exists() { read_json(&p)? } else { Value::Null };
        let c = &v["components"];
        let g = |k: &str| &c[format!("minecraft:{k}")];
        let tool = g("tool").as_object().map(|t| Tool {
            rules: t
                .get("rules")
                .and_then(|r| r.as_array())
                .map(|rs| {
                    rs.iter()
                        .map(|r| ToolRule {
                            blocks: match &r["blocks"] {
                                Value::String(s) => vec![s.clone()],
                                Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
                                _ => vec![],
                            },
                            speed: f32_of(&r["speed"]),
                            correct_for_drops: r["correct_for_drops"].as_bool(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            default_mining_speed: t.get("default_mining_speed").and_then(f32_of).unwrap_or(1.0),
            damage_per_block: t.get("damage_per_block").and_then(|x| x.as_i64()).unwrap_or(1) as i32,
        });
        let food = g("food").as_object().map(|f| Food {
            nutrition: f.get("nutrition").and_then(|x| x.as_i64()).unwrap_or(0) as i32,
            saturation: f.get("saturation").and_then(f32_of).unwrap_or(0.0),
            can_always_eat: f.get("can_always_eat").and_then(|x| x.as_bool()).unwrap_or(false),
        });
        let consume_ticks = g("consumable")
            .as_object()
            .map(|cn| (cn.get("consume_seconds").and_then(|x| x.as_f64()).unwrap_or(1.6) * 20.0).round() as u32)
            .unwrap_or(0);
        let (mut dmg, mut spd, mut armor, mut tough) = (1.0f32, 4.0f32, 0.0f32, 0.0f32);
        for m in g("attribute_modifiers").as_array().into_iter().flatten() {
            let amt = f32_of(&m["amount"]).unwrap_or(0.0);
            let main = m["slot"].as_str().is_none_or(|s| s == "mainhand" || s == "hand" || s == "any");
            let add = m["operation"].as_str().is_none_or(|o| o == "add_value");
            if !add {
                continue;
            }
            match m["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
                "attack_damage" if main => dmg += amt,
                "attack_speed" if main => spd += amt,
                "armor" => armor += amt,
                "armor_toughness" => tough += amt,
                _ => {}
            }
        }
        let block_name = aliases.get(name).copied().unwrap_or(name);
        let block = BlockState::from_name(block_name).filter(|s| !s.is_air() || *name == "air").map(|s| s.0);
        out.push(ItemInfo {
            id: id as i32,
            name: name.to_string(),
            max_stack: g("max_stack_size").as_i64().unwrap_or(64) as i32,
            max_damage: g("max_damage").as_i64().unwrap_or(0) as i32,
            tool,
            food,
            consume_ticks,
            attack_damage: dmg,
            attack_speed: spd,
            armor,
            toughness: tough,
            equip_slot: g("equippable")["slot"].as_str().map(str::to_string),
            block,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests;

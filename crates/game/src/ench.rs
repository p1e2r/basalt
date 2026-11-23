//! Data-driven enchantment rules (`data/minecraft/enchantment/*.json`), smithing recipes and the
//! enchanting-table offer maths. Costs, weights, exclusive sets and supported items all come from the
//! data generator output; the table algorithm is the public gameplay rule (wiki "Enchanting mechanics"),
//! checked against vanilla over the wire by the m10_* scenarios (statistically, the RNG is not reproduced).
use crate::recipe::Ingredient;
use crate::tags::{load_kind, short, Tags};
use crate::{item_id, read_json};
use rand::Rng;
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct EnchDef {
    /// Name without namespace (`sharpness`).
    pub name: String,
    pub weight: i32,
    pub max_level: i32,
    pub anvil_cost: i32,
    pub min_cost: (i32, i32),
    pub max_cost: (i32, i32),
    pub supported: Ingredient,
    /// Items the enchanting table may roll this enchantment for (`primary_items`, else `supported_items`).
    pub primary: Ingredient,
    /// Names (no namespace) this one cannot be combined with.
    pub exclusive: HashSet<String>,
    pub in_table: bool,
    pub treasure: bool,
    pub curse: bool,
}

impl EnchDef {
    pub fn min_cost_at(&self, level: i32) -> i32 {
        self.min_cost.0 + self.min_cost.1 * (level - 1)
    }
    pub fn max_cost_at(&self, level: i32) -> i32 {
        self.max_cost.0 + self.max_cost.1 * (level - 1)
    }
}

#[derive(Debug, Clone)]
pub enum SmithKind {
    /// Result item replaces the base (netherite upgrade).
    Transform { result: i32 },
    /// Armor trim with this pattern (`minecraft:bolt` -> `bolt`).
    Trim { pattern: String },
}

#[derive(Debug, Clone)]
pub struct SmithRecipe {
    pub id: String,
    pub template: Ingredient,
    pub base: Ingredient,
    pub addition: Ingredient,
    pub kind: SmithKind,
}

#[derive(Debug, Default)]
pub struct EnchData {
    pub list: Vec<EnchDef>,
    pub smithing: Vec<SmithRecipe>,
}

fn pair(v: &Value) -> (i32, i32) {
    (
        v["base"].as_i64().unwrap_or(0) as i32,
        v["per_level_above_first"].as_i64().unwrap_or(0) as i32,
    )
}

fn json_files(dir: &Path) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let mut files: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    Ok(files)
}

impl EnchData {
    pub(crate) fn load(mc: &Path, tags: &Tags) -> anyhow::Result<EnchData> {
        let etags = load_kind(&mc.join("tags/enchantment"))?;
        let in_tag = |t: &str, n: &str| etags.get(short(t)).is_some_and(|s| s.contains(n));
        let mut list = Vec::new();
        for p in json_files(&mc.join("enchantment"))? {
            let name = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
            let v = read_json(&p)?;
            let sel = |k: &str| Ingredient::parse(&v[k], tags).unwrap_or_default();
            let supported = sel("supported_items");
            let primary = if v["primary_items"].is_null() {
                supported.clone()
            } else {
                sel("primary_items")
            };
            let exclusive: HashSet<String> = match &v["exclusive_set"] {
                Value::String(s) if s.starts_with('#') => etags.get(short(s)).cloned().unwrap_or_default(),
                Value::String(s) => [short(s).to_string()].into(),
                Value::Array(a) => a.iter().filter_map(|x| x.as_str()).map(|s| short(s).to_string()).collect(),
                _ => HashSet::new(),
            };
            list.push(EnchDef {
                weight: v["weight"].as_i64().unwrap_or(1) as i32,
                max_level: v["max_level"].as_i64().unwrap_or(1) as i32,
                anvil_cost: v["anvil_cost"].as_i64().unwrap_or(1) as i32,
                min_cost: pair(&v["min_cost"]),
                max_cost: pair(&v["max_cost"]),
                supported,
                primary,
                exclusive,
                in_table: in_tag("in_enchanting_table", &name),
                treasure: in_tag("treasure", &name),
                curse: in_tag("curse", &name),
                name,
            });
        }
        let mut smithing = Vec::new();
        for p in json_files(&mc.join("recipe"))? {
            let v = read_json(&p)?;
            let ty = short(v["type"].as_str().unwrap_or(""));
            if ty != "smithing_transform" && ty != "smithing_trim" {
                continue;
            }
            let (Some(template), Some(base), Some(addition)) = (
                Ingredient::parse(&v["template"], tags),
                Ingredient::parse(&v["base"], tags),
                Ingredient::parse(&v["addition"], tags),
            ) else {
                continue;
            };
            let kind = if ty == "smithing_transform" {
                let r = &v["result"];
                let Some(result) = r["id"].as_str().or_else(|| r.as_str()).and_then(item_id) else {
                    continue;
                };
                SmithKind::Transform { result }
            } else {
                SmithKind::Trim {
                    pattern: short(v["pattern"].as_str().unwrap_or("")).to_string(),
                }
            };
            smithing.push(SmithRecipe {
                id: p.file_stem().unwrap_or_default().to_string_lossy().to_string(),
                template,
                base,
                addition,
                kind,
            });
        }
        Ok(EnchData { list, smithing })
    }

    pub fn by_name(&self, name: &str) -> Option<&EnchDef> {
        let n = short(name);
        self.list.iter().find(|e| e.name == n)
    }

    pub fn conflicts(&self, a: &str, b: &str) -> bool {
        let (a, b) = (short(a), short(b));
        a != b && self.by_name(a).is_some_and(|d| d.exclusive.contains(b))
    }

    pub fn smithing_for(&self, template: i32, base: i32, addition: i32) -> Option<&SmithRecipe> {
        self.smithing
            .iter()
            .find(|r| r.template.matches(template) && r.base.matches(base) && r.addition.matches(addition))
    }

    /// Enchanting-table rule: the enchantments (name, level) rolled for an item of `enchantability` at the
    /// button `cost`. Books accept every non-treasure table enchantment, other items use `primary_items`.
    pub fn roll(&self, rng: &mut impl Rng, item: i32, is_book: bool, enchantability: i32, cost: i32) -> Vec<(String, i32)> {
        // 1. modified level (rand(e/4+1) twice, +-15% bonus)
        let q = enchantability / 4 + 1;
        let mut level = cost + 1 + rng.gen_range(0..q) + rng.gen_range(0..q);
        let bonus = (rng.gen::<f32>() + rng.gen::<f32>() - 1.0) * 0.15;
        level = ((level as f32 * (1.0 + bonus)).round() as i32).max(1);
        // 2. candidates: highest level whose cost window contains the modified level
        let mut pool: Vec<(&EnchDef, i32)> = Vec::new();
        for d in &self.list {
            if !d.in_table || d.treasure || !(is_book || d.primary.matches(item)) {
                continue;
            }
            for l in (1..=d.max_level).rev() {
                if level >= d.min_cost_at(l) && level <= d.max_cost_at(l) {
                    pool.push((d, l));
                    break;
                }
            }
        }
        let mut out: Vec<(String, i32)> = Vec::new();
        if pool.is_empty() {
            return out;
        }
        fn pick(rng: &mut impl Rng, pool: &[(&EnchDef, i32)]) -> usize {
            let total: i32 = pool.iter().map(|p| p.0.weight).sum();
            let mut r = rng.gen_range(0..total.max(1));
            for (i, p) in pool.iter().enumerate() {
                r -= p.0.weight;
                if r < 0 {
                    return i;
                }
            }
            pool.len() - 1
        }
        let i = pick(rng, &pool);
        let mut last = pool[i];
        out.push((last.0.name.clone(), last.1));
        // 3. keep adding with probability (level + 1) / 50, halving the level each time
        while rng.gen_range(0..50) <= level {
            pool.retain(|p| p.0.name != last.0.name && !self.conflicts(&last.0.name, &p.0.name));
            if pool.is_empty() {
                break;
            }
            let i = pick(rng, &pool);
            last = pool[i];
            out.push((last.0.name.clone(), last.1));
            level /= 2;
        }
        out
    }
}

/// Cost of table button `slot` for `power` bookshelves (each slot rolls its own base, measured on vanilla: at power 0 the
/// three costs are independent): base = rand(1..=8) + power/2 + rand(0..=power); slot 0 = max(base/3, 1),
/// slot 1 = base*2/3 + 1, slot 2 = max(base, power*2); a cost below slot+1 is removed (0).
pub fn offer_cost(rng: &mut impl Rng, power: i32, slot: usize) -> i32 {
    let base = rng.gen_range(1..=8) + power / 2 + rng.gen_range(0..=power);
    let c = match slot {
        0 => (base / 3).max(1),
        1 => base * 2 / 3 + 1,
        _ => base.max(power * 2),
    };
    if c < slot as i32 + 1 {
        0
    } else {
        c
    }
}

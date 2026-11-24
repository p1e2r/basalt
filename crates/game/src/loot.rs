//! Loot table interpreter over the raw JSON in `data/minecraft/loot_table`.
//!
//! Supported: entries item / tag / alternatives / group / sequence / loot_table / empty (dynamic
//! skipped); the common conditions and count functions. Component-only functions are ignored.
use crate::tags::short;
use crate::{item_id, GameData};
use basalt_data::BlockState;
use rand::Rng;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

/// Context for evaluating a loot table.
#[derive(Debug, Clone, Default)]
pub struct LootContext {
    pub killed_by_player: bool,
    pub looting: u8,
    /// Item used (block tables: the tool; entity tables: the killer's weapon).
    pub tool: Option<i32>,
    pub silk_touch: bool,
    pub fortune: u8,
    /// Set when the loot comes from an explosion (survives_explosion / explosion_decay).
    pub explosion_radius: Option<f32>,
    /// State of the entity the table runs for (`entity_properties` with `entity: this`).
    pub this_entity: Option<EntityProps>,
    /// Block states directly below / above the block a block table runs for (`location_check` with an offsetY).
    pub around: Option<(u32, u32)>,
    /// The block goes without anything breaking it (popped off its support, `/setblock .. destroy`): tables that
    /// test the breaking entity (snow layers) drop nothing.
    pub no_entity: bool,
}

/// The parts of an entity's state that entity loot tables test.
#[derive(Debug, Clone, Default)]
pub struct EntityProps {
    pub on_fire: bool,
    /// Sheep wool colour name ("white", "lime", …).
    pub sheep_color: Option<String>,
    pub sheared: bool,
    pub baby: bool,
}

pub(crate) fn load_tables(dir: &Path) -> anyhow::Result<HashMap<String, Value>> {
    let mut out = HashMap::new();
    fn walk(root: &Path, dir: &Path, out: &mut HashMap<String, Value>) -> anyhow::Result<()> {
        for e in std::fs::read_dir(dir)? {
            let p = e?.path();
            if p.is_dir() {
                walk(root, &p, out)?;
            } else if p.extension().is_some_and(|x| x == "json") {
                let id = p
                    .strip_prefix(root)?
                    .with_extension("")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(id, crate::read_json(&p)?);
            }
        }
        Ok(())
    }
    if dir.is_dir() {
        walk(dir, dir, &mut out)?;
    }
    Ok(out)
}

struct Ctx<'a> {
    gd: &'a GameData,
    lc: &'a LootContext,
    state: Option<BlockState>,
    depth: u32,
}

pub(crate) fn eval_table<R: Rng + ?Sized>(
    gd: &GameData,
    table: &str,
    lc: &LootContext,
    state: Option<BlockState>,
    rng: &mut R,
) -> Vec<(i32, i32)> {
    let ctx = Ctx {
        gd,
        lc,
        state,
        depth: 0,
    };
    let mut raw = Vec::new();
    // wall variants (wall_torch, oak_wall_sign, ...) have no table of their own: they drop like the standing block
    let name = short(table);
    let t = gd.loot_tables.get(name).or_else(|| {
        let (head, base) = name.split_once('/')?;
        let alias = if base.contains("_wall_") {
            base.replacen("_wall_", "_", 1)
        } else {
            base.strip_prefix("wall_")?.to_string()
        };
        gd.loot_tables.get(format!("{head}/{alias}").as_str())
    });
    if let Some(t) = t {
        run_table(&ctx, t, rng, &mut raw);
    }
    // Merge nothing; split oversize stacks by max stack size like vanilla does.
    let mut out = Vec::new();
    for (id, mut n) in raw {
        let max = gd.item(id).map_or(64, |i| i.max_stack.max(1));
        while n > 0 {
            let c = n.min(max);
            out.push((id, c));
            n -= c;
        }
    }
    out
}

fn run_table<R: Rng + ?Sized>(ctx: &Ctx, t: &Value, rng: &mut R, out: &mut Vec<(i32, i32)>) {
    if ctx.depth > 8 {
        return;
    }
    let start = out.len();
    for pool in t["pools"].as_array().into_iter().flatten() {
        if !conds(ctx, &pool["conditions"], rng) {
            continue;
        }
        let rolls = num_int(ctx, &pool["rolls"], rng, 1); // bonus_rolls scale with luck, which is 0
        for _ in 0..rolls.max(0) {
            let mut leaves = Vec::new();
            for e in pool["entries"].as_array().into_iter().flatten() {
                expand(ctx, e, rng, &mut leaves);
            }
            let total: i64 = leaves.iter().map(|l| l.weight.max(0)).sum();
            let pick = match leaves.len() {
                0 => continue,
                1 => 0,
                _ if total <= 0 => continue,
                _ => {
                    let mut r = rng.gen_range(0..total);
                    let mut idx = 0;
                    for (i, l) in leaves.iter().enumerate() {
                        r -= l.weight.max(0);
                        if r < 0 {
                            idx = i;
                            break;
                        }
                    }
                    idx
                }
            };
            let pool_start = out.len();
            generate(ctx, &leaves[pick], rng, out);
            apply_fns(ctx, &pool["functions"], rng, &mut out[pool_start..]);
        }
    }
    apply_fns(ctx, &t["functions"], rng, &mut out[start..]);
    out.retain(|s| s.1 > 0);
}

enum LeafKind<'a> {
    Item(i32),
    TagAll(&'a str),
    Table(&'a Value),
    Empty,
}

struct Leaf<'a> {
    weight: i64,
    kind: LeafKind<'a>,
    entry: &'a Value,
}

/// Expand an entry into the leaves it contributes; returns whether the entry "succeeded".
fn expand<'a, R: Rng + ?Sized>(
    ctx: &Ctx,
    e: &'a Value,
    rng: &mut R,
    out: &mut Vec<Leaf<'a>>,
) -> bool {
    if !conds(ctx, &e["conditions"], rng) {
        return false;
    }
    let ty = short(e["type"].as_str().unwrap_or(""));
    let weight = e["weight"].as_i64().unwrap_or(1);
    match ty {
        "alternatives" => {
            for c in e["children"].as_array().into_iter().flatten() {
                if expand(ctx, c, rng, out) {
                    return true;
                }
            }
            false
        }
        "group" => {
            for c in e["children"].as_array().into_iter().flatten() {
                expand(ctx, c, rng, out);
            }
            true
        }
        "sequence" => {
            for c in e["children"].as_array().into_iter().flatten() {
                if !expand(ctx, c, rng, out) {
                    break;
                }
            }
            true
        }
        "item" => {
            if let Some(id) = e["name"].as_str().and_then(item_id) {
                out.push(Leaf {
                    weight,
                    kind: LeafKind::Item(id),
                    entry: e,
                });
            }
            true
        }
        "tag" => {
            let name = e["name"].as_str().unwrap_or("");
            if e["expand"].as_bool().unwrap_or(false) {
                let mut ids: Vec<i32> = ctx.gd.tags.item_tag(name).collect();
                ids.sort();
                for id in ids {
                    out.push(Leaf {
                        weight,
                        kind: LeafKind::Item(id),
                        entry: e,
                    });
                }
            } else {
                out.push(Leaf {
                    weight,
                    kind: LeafKind::TagAll(name),
                    entry: e,
                });
            }
            true
        }
        "loot_table" => {
            out.push(Leaf {
                weight,
                kind: LeafKind::Table(&e["value"]),
                entry: e,
            });
            true
        }
        "empty" => {
            out.push(Leaf {
                weight,
                kind: LeafKind::Empty,
                entry: e,
            });
            true
        }
        _ => false, // dynamic (block entity contents) and unknown types
    }
}

fn generate<R: Rng + ?Sized>(ctx: &Ctx, leaf: &Leaf, rng: &mut R, out: &mut Vec<(i32, i32)>) {
    let start = out.len();
    match &leaf.kind {
        LeafKind::Item(id) => out.push((*id, 1)),
        LeafKind::TagAll(name) => {
            let mut ids: Vec<i32> = ctx.gd.tags.item_tag(name).collect();
            ids.sort();
            out.extend(ids.into_iter().map(|i| (i, 1)));
        }
        LeafKind::Table(v) => {
            let sub = Ctx {
                gd: ctx.gd,
                lc: ctx.lc,
                state: ctx.state,
                depth: ctx.depth + 1,
            };
            match v {
                Value::String(s) => {
                    if let Some(t) = ctx.gd.loot_tables.get(short(s)) {
                        run_table(&sub, t, rng, out);
                    }
                }
                Value::Object(_) => run_table(&sub, v, rng, out),
                _ => {}
            }
        }
        LeafKind::Empty => {}
    }
    apply_fns(ctx, &leaf.entry["functions"], rng, &mut out[start..]);
}

// ---------------------------------------------------------------- number providers

fn num<R: Rng + ?Sized>(ctx: &Ctx, v: &Value, rng: &mut R, default: f32) -> f32 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(default as f64) as f32,
        Value::Object(o) => {
            let ty = o.get("type").and_then(|t| t.as_str()).map(short);
            match ty {
                Some("constant") => num(ctx, &v["value"], rng, default),
                Some("binomial") => binomial(
                    num_int(ctx, &v["n"], rng, 0),
                    num(ctx, &v["p"], rng, 0.0),
                    rng,
                ) as f32,
                Some("uniform") | None if o.contains_key("min") || o.contains_key("max") => {
                    let a = num(ctx, &v["min"], rng, 0.0);
                    let b = num(ctx, &v["max"], rng, a);
                    if b > a {
                        rng.gen_range(a..b)
                    } else {
                        a
                    }
                }
                _ => default,
            }
        }
        _ => default,
    }
}

fn num_int<R: Rng + ?Sized>(ctx: &Ctx, v: &Value, rng: &mut R, default: i32) -> i32 {
    match v {
        Value::Object(o) => {
            let ty = o.get("type").and_then(|t| t.as_str()).map(short);
            match ty {
                Some("uniform") | None if o.contains_key("min") || o.contains_key("max") => {
                    let a = num_int(ctx, &v["min"], rng, 0);
                    let b = num_int(ctx, &v["max"], rng, a);
                    if b > a {
                        rng.gen_range(a..=b)
                    } else {
                        a
                    }
                }
                _ => num(ctx, v, rng, default as f32).round() as i32,
            }
        }
        _ => num(ctx, v, rng, default as f32).round() as i32,
    }
}

fn binomial<R: Rng + ?Sized>(n: i32, p: f32, rng: &mut R) -> i32 {
    (0..n.max(0)).filter(|_| rng.gen::<f32>() < p).count() as i32
}

// ---------------------------------------------------------------- conditions

fn conds<R: Rng + ?Sized>(ctx: &Ctx, v: &Value, rng: &mut R) -> bool {
    match v {
        Value::Array(a) => a.iter().all(|c| cond(ctx, c, rng)),
        Value::Object(_) => cond(ctx, v, rng),
        _ => true,
    }
}

fn level_for(ctx: &Ctx, ench: &str) -> u8 {
    match short(ench) {
        "fortune" => ctx.lc.fortune,
        "looting" => ctx.lc.looting,
        "silk_touch" => ctx.lc.silk_touch as u8,
        _ => 0,
    }
}

fn cond<R: Rng + ?Sized>(ctx: &Ctx, c: &Value, rng: &mut R) -> bool {
    let ty = short(c["condition"].as_str().unwrap_or(""));
    match ty {
        "survives_explosion" => match ctx.lc.explosion_radius {
            Some(r) if r > 0.0 => rng.gen::<f32>() <= 1.0 / r,
            _ => true,
        },
        "random_chance" => rng.gen::<f32>() < num(ctx, &c["chance"], rng, 0.0),
        "random_chance_with_enchanted_bonus" => {
            let lvl = level_for(
                ctx,
                c["enchantment"].as_str().unwrap_or("minecraft:looting"),
            );
            let p = if lvl == 0 {
                num(ctx, &c["unenchanted_chance"], rng, 0.0)
            } else {
                level_based(&c["enchanted_chance"], lvl)
            };
            rng.gen::<f32>() < p
        }
        "killed_by_player" => ctx.lc.killed_by_player,
        "table_bonus" => {
            let lvl = level_for(ctx, c["enchantment"].as_str().unwrap_or("")) as usize;
            let ch = c["chances"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);
            if ch.is_empty() {
                return false;
            }
            let p = ch[lvl.min(ch.len() - 1)].as_f64().unwrap_or(0.0) as f32;
            rng.gen::<f32>() < p
        }
        "match_tool" => match_tool(ctx, &c["predicate"]),
        "block_state_property" => block_state_prop(ctx, c),
        "inverted" => !cond(ctx, &c["term"], rng),
        "any_of" | "alternative" => c["terms"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|t| cond(ctx, t, rng)),
        "all_of" => c["terms"]
            .as_array()
            .into_iter()
            .flatten()
            .all(|t| cond(ctx, t, rng)),
        "entity_properties" => entity_props(ctx, c),
        "location_check" => location_check(ctx, c),
        "reference" => false,
        _ => false, // location_check, damage_source_properties, weather/time checks, …
    }
}

/// Level-based value: plain number, {type: linear, base, per_level_above_first}, constant…
fn level_based(v: &Value, lvl: u8) -> f32 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0) as f32,
        Value::Object(o) => match o.get("type").and_then(|t| t.as_str()).map(short) {
            Some("linear") => {
                let base = o.get("base").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                let per = o
                    .get("per_level_above_first")
                    .and_then(|x| x.as_f64())
                    .unwrap_or(0.0) as f32;
                base + per * (lvl as f32 - 1.0)
            }
            Some("constant") => o.get("value").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32,
            _ => 0.0,
        },
        _ => 0.0,
    }
}

fn item_selector_matches(ctx: &Ctx, sel: &Value, item: i32) -> bool {
    match sel {
        Value::String(s) if s.starts_with('#') => ctx.gd.tags.item_has(s, item),
        Value::String(s) => item_id(s) == Some(item),
        Value::Array(a) => a.iter().any(|x| item_selector_matches(ctx, x, item)),
        _ => false,
    }
}

fn match_tool(ctx: &Ctx, p: &Value) -> bool {
    let Some(obj) = p.as_object() else {
        return true;
    };
    for (k, v) in obj {
        match k.as_str() {
            "items" => {
                let Some(t) = ctx.lc.tool else { return false };
                if !item_selector_matches(ctx, v, t) {
                    return false;
                }
            }
            "predicates" => {
                for (pk, pv) in v.as_object().into_iter().flatten() {
                    match short(pk) {
                        "enchantments" => {
                            for e in pv.as_array().into_iter().flatten() {
                                let names: Vec<&str> = match &e["enchantments"] {
                                    Value::String(s) => vec![s.as_str()],
                                    Value::Array(a) => {
                                        a.iter().filter_map(|x| x.as_str()).collect()
                                    }
                                    _ => vec![],
                                };
                                let lvl = names.iter().map(|n| level_for(ctx, n)).max().unwrap_or(0)
                                    as i64;
                                let (min, max) = match &e["levels"] {
                                    Value::Number(n) => {
                                        (n.as_i64().unwrap_or(1), n.as_i64().unwrap_or(1))
                                    }
                                    Value::Object(o) => (
                                        o.get("min").and_then(|x| x.as_i64()).unwrap_or(1),
                                        o.get("max").and_then(|x| x.as_i64()).unwrap_or(i64::MAX),
                                    ),
                                    _ => (1, i64::MAX),
                                };
                                if lvl < min || lvl > max {
                                    return false;
                                }
                            }
                        }
                        _ => return false,
                    }
                }
            }
            _ => return false,
        }
    }
    true
}

/// `location_check` for the block one above/below (offsetY +-1): block id and state properties of its predicate.
fn location_check(ctx: &Ctx, c: &Value) -> bool {
    let Some((below, above)) = ctx.lc.around else {
        return false;
    };
    let st = match c["offsetY"].as_i64() {
        Some(1) => BlockState(above),
        Some(-1) => BlockState(below),
        _ => return false,
    };
    let b = &c["predicate"]["block"];
    if let Some(name) = b["blocks"].as_str() {
        if short(name) != st.name() {
            return false;
        }
    }
    for (k, want) in b["state"].as_object().into_iter().flatten() {
        if want.as_str() != st.get(k) {
            return false;
        }
    }
    true
}

fn block_state_prop(ctx: &Ctx, c: &Value) -> bool {
    let Some(st) = ctx.state else { return false };
    if let Some(b) = c["block"].as_str() {
        if short(b) != st.name() {
            return false;
        }
    }
    for (k, want) in c["properties"].as_object().into_iter().flatten() {
        let Some(have) = st.get(k) else { return false };
        let ok = match want {
            Value::String(s) => s == have,
            Value::Number(n) => have.parse::<f64>().ok() == n.as_f64(),
            Value::Bool(b) => have == if *b { "true" } else { "false" },
            Value::Object(o) => {
                let Ok(h) = have.parse::<f64>() else {
                    return false;
                };
                let min = o.get("min").map(|x| {
                    x.as_f64()
                        .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
                });
                let max = o.get("max").map(|x| {
                    x.as_f64()
                        .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
                });
                min.flatten().is_none_or(|m| h >= m) && max.flatten().is_none_or(|m| h <= m)
            }
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    true
}

/// `entity_properties` for `this` against [`LootContext::this_entity`]; predicates on other
/// entities (killer equipment etc.) are false since no enchantments are modelled.
fn entity_props(ctx: &Ctx, c: &Value) -> bool {
    if short(c["entity"].as_str().unwrap_or("this")) != "this" {
        return false;
    }
    if ctx.lc.no_entity {
        return false;
    }
    let e = ctx.lc.this_entity.clone().unwrap_or_default();
    let p = &c["predicate"];
    let flags = if p["minecraft:flags"].is_object() {
        &p["minecraft:flags"]
    } else {
        &p["flags"]
    };
    if let Some(f) = flags["is_on_fire"].as_bool() {
        if f != e.on_fire {
            return false;
        }
    }
    if let Some(f) = flags["is_baby"].as_bool() {
        if f != e.baby {
            return false;
        }
    }
    if let Some(col) = p["minecraft:components"]["minecraft:sheep/color"].as_str() {
        if Some(short(col)) != e.sheep_color.as_deref() {
            return false;
        }
    }
    if let Some(sh) = p["minecraft:type_specific/sheep"]["sheared"].as_bool() {
        if sh != e.sheared {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------- functions

fn apply_fns<R: Rng + ?Sized>(ctx: &Ctx, fns: &Value, rng: &mut R, stacks: &mut [(i32, i32)]) {
    for f in fns.as_array().into_iter().flatten() {
        for s in stacks.iter_mut() {
            if !conds(ctx, &f["conditions"], rng) {
                continue;
            }
            apply_fn(ctx, f, rng, s);
        }
    }
}

fn apply_fn<R: Rng + ?Sized>(ctx: &Ctx, f: &Value, rng: &mut R, s: &mut (i32, i32)) {
    match short(f["function"].as_str().unwrap_or("")) {
        "set_count" => {
            let n = num_int(ctx, &f["count"], rng, 1);
            s.1 = if f["add"].as_bool().unwrap_or(false) {
                s.1 + n
            } else {
                n
            };
        }
        "apply_bonus" => {
            let lvl = level_for(ctx, f["enchantment"].as_str().unwrap_or("")) as i32;
            match short(f["formula"].as_str().unwrap_or("")) {
                "ore_drops" => {
                    if lvl > 0 {
                        let i = (rng.gen_range(0..lvl + 2) - 1).max(0);
                        s.1 *= i + 1;
                    }
                }
                "uniform_bonus_count" => {
                    let m = f["parameters"]["bonusMultiplier"].as_i64().unwrap_or(1) as i32;
                    if lvl > 0 {
                        s.1 += rng.gen_range(0..=m * lvl);
                    }
                }
                "binomial_with_bonus_count" => {
                    let extra = f["parameters"]["extra"].as_i64().unwrap_or(0) as i32;
                    let p = f["parameters"]["probability"].as_f64().unwrap_or(0.0) as f32;
                    s.1 += binomial(lvl + extra, p, rng);
                }
                _ => {}
            }
        }
        "explosion_decay" => {
            if let Some(r) = ctx.lc.explosion_radius.filter(|r| *r > 0.0) {
                let p = 1.0 / r;
                s.1 = (0..s.1).filter(|_| rng.gen::<f32>() <= p).count() as i32;
            }
        }
        "limit_count" => {
            let l = &f["limit"];
            match l {
                Value::Number(_) => s.1 = s.1.min(num_int(ctx, l, rng, s.1)),
                Value::Object(_) => {
                    if !l["min"].is_null() {
                        s.1 = s.1.max(num_int(ctx, &l["min"], rng, s.1));
                    }
                    if !l["max"].is_null() {
                        s.1 = s.1.min(num_int(ctx, &l["max"], rng, s.1));
                    }
                }
                _ => {}
            }
        }
        "enchanted_count_increase" | "looting_enchant" => {
            let lvl = level_for(
                ctx,
                f["enchantment"].as_str().unwrap_or("minecraft:looting"),
            );
            if lvl > 0 {
                let inc = num(ctx, &f["count"], rng, 0.0) * lvl as f32;
                s.1 += inc.round() as i32;
                let limit = f["limit"].as_i64().unwrap_or(0) as i32;
                if limit > 0 {
                    s.1 = s.1.min(limit);
                }
            }
        }
        "furnace_smelt" => {
            if let Some((out, _, _)) = ctx.gd.recipes.cook(crate::CookKind::Smelting, s.0) {
                s.0 = out;
            }
        }
        // copy_*/set_* only touch components.
        _ => {}
    }
}

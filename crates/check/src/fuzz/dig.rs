//! Dig-time fuzzer: for a stratified sample of (block, held item, on_ground, in_water) combos, measure
//! the shortest wait (in ticks) between `start_destroy` and `stop_destroy` that the server accepts as
//! a break, on vanilla and on Basalt, and compare. Also checks that waiting what Basalt's
//! `GameData::dig_ticks` says (+1, what `Bot::dig` does and what a real client would wait) is accepted
//! by vanilla. Vanilla measurements are cached in `vanilla/harness-cache/dig.json` keyed by
//! (data version, block, item, flags), so reruns are instant.
//!
//! "Accepts" = the block is gone within 400 ms after the stop: either the stop passed the server's
//! 0.7-progress check or the server's delayed destroy (rejected stops are still tracked and break the block when
//! its progress reaches 1.0) finished inside that window. `ground=false` means the bot *claims* to be airborne
//! while standing on the floor (both servers must ignore the flag); real altitude is covered by m4_dig_accept.
//!
//! Tolerance: both servers count server ticks between the two packets and wall-clock jitter adds up to
//! one tick, so thresholds may differ by `max(2, 5%)` ticks.
use super::{arena::worker_arena, barrier, parallel, Harness, Mismatch, Rng};
use crate::bot::Bot;
use crate::runner::Ctx;
use anyhow::{bail, Result};
use basalt_data::BlockState;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub static HARNESS: Harness = Harness { id: "dig", desc: "dig acceptance time: block x tool x on_ground/in_water", default_cases: 120, long_cases: 2000, gen, run, compare };

/// Blocks that cannot stand alone on a stone floor (or are multi-part): not measured.
const SKIP: &[&str] = &["door", "bed", "torch", "sign", "banner", "button", "plate", "lever", "rail", "carpet", "sapling", "flower", "grass", "fern", "bush", "vine", "ladder", "coral",
    "kelp", "seagrass", "lily", "snow", "fire", "water", "lava", "bubble", "piston", "light", "portal", "frame", "skull", "head", "potted", "tripwire", "redstone_wire", "comparator", "repeater",
    "amethyst_cluster", "bud", "candle", "sprouts", "roots", "fungus", "mushroom", "wheat", "carrots", "potatoes", "beetroots", "stem", "cocoa", "pickle", "egg", "scaffold", "dripstone", "cactus",
    "sugar_cane", "bamboo", "sweet", "torchflower", "pitcher", "spore", "hanging", "lantern", "chorus", "cave_vines", "weeping", "twisting", "nether_wart", "end_rod", "lightning_rod", "bell", "grindstone",
    "cauldron", "flower_pot", "pot", "air", "moving", "structure", "jigsaw", "command", "barrier", "bedrock", "end_gateway", "end_portal", "test", "dead_bush", "azalea", "dried_ghast", "creaking",
    "vault", "trial", "leaves", "orchid", "allium", "tulip", "poppy", "dandelion", "cornflower", "bluet", "oxeye", "lilac", "rose", "peony", "concrete_powder", "lichen", "dripleaf", "eyeblossom", "frogspawn", "sculk_vein", "sculk_sensor", "shrieker", "catalyst", "firefly", "leaf_litter", "wildflowers", "tall_", "double", "petals", "bush", "cobweb", "iron_bars", "pane", "chain", "fence", "wall", "glass_pane", "conduit", "mud", "sniffer", "turtle"];

fn candidates() -> &'static Vec<&'static str> {
    static C: OnceLock<Vec<&'static str>> = OnceLock::new();
    C.get_or_init(|| basalt_data::blocks::BLOCKS.iter().map(|b| b.name)
        .filter(|n| basalt_game::hardness(n) >= 0.0 && !SKIP.iter().any(|s| n.contains(s))).collect())
}

fn tools() -> &'static Vec<String> {
    static T: OnceLock<Vec<String>> = OnceLock::new();
    T.get_or_init(|| {
        let mut v = vec!["hand".to_string(), "stick".to_string(), "shears".to_string(), "mace".to_string()];
        for tier in ["wooden", "stone", "copper", "iron", "golden", "diamond", "netherite"] { for k in ["pickaxe", "axe", "shovel", "hoe", "sword"] { v.push(format!("{tier}_{k}")); } }
        v.retain(|t| t == "hand" || basalt_data::registry_id(basalt_data::registries::ITEM, t).is_some());
        v
    })
}

fn item_id(item: &str) -> Option<i32> { if item == "hand" { None } else { basalt_data::registry_id(basalt_data::registries::ITEM, item) } }

/// Basalt's own (client-side) dig time for the combo.
pub fn formula_ticks(block: &str, item: &str, ground: bool, water: bool) -> Option<u32> {
    let st = BlockState::from_name(block)?;
    crate::game_data().dig_ticks(st.0, item_id(item), ground, water)
}

const BUCKETS: &[(u32, u32)] = &[(0, 0), (1, 4), (5, 15), (16, 40), (41, 90), (91, 150), (151, 400)];

fn gen(r: &mut Rng, i: usize) -> Value {
    let nb = if super::opts().long { BUCKETS.len() } else { BUCKETS.len() - 1 };
    let (lo, hi) = BUCKETS[i % nb];
    let mut last = json!({});
    for _ in 0..400 {
        let block = *r.pick(candidates());
        let item = if r.chance(0.3) { "hand".to_string() } else { r.pick(tools()).clone() };
        let (ground, water) = (r.chance(0.85), r.chance(0.15));
        let c = json!({ "block": block, "item": item, "ground": ground, "water": water });
        if let Some(t) = formula_ticks(block, &item, ground, water) { last = c.clone(); if t >= lo && t <= hi { return c; } }
    }
    last
}

// ------------------------------------------------------------------ vanilla measurement cache

fn cache_path() -> PathBuf { PathBuf::from("vanilla/harness-cache/dig.json") }
/// Bump when the measuring procedure changes (v3: every probe starts by clearing the cell, which ends vanilla's delayed destroy).
const MEASURE_VERSION: &str = "v3";

fn key(case: &Value) -> String { format!("{MEASURE_VERSION}|{}{}|{}|{}|{}|{}/{}", case.get("scan").map(|s| format!("{s}|")).unwrap_or_default(), basalt_data::VERSION_NAME, case["block"].as_str().unwrap_or(""), case["item"].as_str().unwrap_or(""), case["ground"], case["water"], basalt_data::WORLD_VERSION) }

fn load_cache() -> serde_json::Map<String, Value> {
    std::fs::read_to_string(cache_path()).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default()
}

fn run(ctx: &mut Ctx, cases: &[Value]) -> Result<Vec<Value>> {
    let vanilla = ctx.is_vanilla();
    let mut cache = if vanilla { load_cache() } else { Default::default() };
    let mut out = vec![Value::Null; cases.len()];
    let mut todo: Vec<usize> = Vec::new();
    for (i, c) in cases.iter().enumerate() {
        match cache.get(&key(c)) { Some(v) => out[i] = v.clone(), None => todo.push(i) }
    }
    if vanilla { ctx.note(format!("{} of {} vanilla measurements cached", cases.len() - todo.len(), cases.len())); }
    if !todo.is_empty() {
        let sub: Vec<Value> = todo.iter().map(|&i| cases[i].clone()).collect();
        let jobs = super::opts().jobs.clamp(1, 8);
        let homes: std::sync::Mutex<Vec<[f64; 3]>> = std::sync::Mutex::new(vec![[0.0; 3]; jobs]);
        let res = parallel(ctx, &sub, jobs,
            |ctx, w| { let bot = ctx.bot()?; worker_arena(ctx, &bot, w, 9, 9, 6, "survival")?; homes.lock().unwrap()[w] = bot.st().body.pos; Ok(bot) },
            |bot, w, _i, case| { let home = homes.lock().unwrap()[w]; measure(bot, home, case) })?;
        for (k, &i) in todo.iter().enumerate() {
            if vanilla && !super::is_error(&res[k]) && cases[i].get("scan").is_none() { cache.insert(key(&cases[i]), res[k].clone()); }
            out[i] = res[k].clone();
        }
        if vanilla {
            if let Some(d) = cache_path().parent() { let _ = std::fs::create_dir_all(d); }
            let _ = std::fs::write(cache_path(), serde_json::to_string_pretty(&Value::Object(cache)).unwrap_or_default());
        }
    }
    Ok(out)
}

/// A stop vanilla rejects (too early) is not forgotten: the server keeps tracking the attempt from the start tick
/// and breaks the block at that position by itself once its progress reaches 1.0 -- even a block placed there by a
/// later probe (measured: probes just below the threshold alternated reject/accept, and fresh blocks vanished).
/// The tracking ends when the position is air at a server tick, so every probe starts by clearing the cell.
fn reset_dig_state(bot: &Bot, b: [i32; 3]) -> Result<()> {
    bot.cmd(&format!("setblock {} {} {} air", b[0], b[1], b[2]));
    bot.wait_ticks(3);
    Ok(())
}

/// One probe: place the block, start digging, wait `w` ticks, stop; did the block break?
fn probe(bot: &Bot, b: [i32; 3], block: &str, w: u32) -> Result<bool> {
    reset_dig_state(bot, b)?;
    for attempt in 0..2 {
        bot.cmd(&format!("setblock {} {} {} {block}", b[0], b[1], b[2]));
        if bot.wait_until(Duration::from_millis(1500), |s| s.world.name(b[0], b[1], b[2]) == block) { break; }
        if attempt == 1 { bail!("could not place {block} (view has {})", bot.st().world.name(b[0], b[1], b[2])); }
    }
    bot.wait_ticks(1);
    if bot.st().world.name(b[0], b[1], b[2]) != block { bail!("{block} did not stay in place"); }
    let t0 = Instant::now();
    bot.player_action(0, b, 1);
    bot.swing();
    if w > 0 {
        let end = t0 + Duration::from_millis(50 * w as u64);
        while Instant::now() < end { std::thread::sleep(Duration::from_millis(2).min(end.saturating_duration_since(Instant::now()))); if w > 5 && t0.elapsed().as_millis() % 250 < 3 { bot.swing(); } }
    }
    bot.player_action(2, b, 1);
    let broke = bot.wait_until(Duration::from_millis(400), |s| s.world.name(b[0], b[1], b[2]) != block);
    if !broke { bot.player_action(1, b, 1); bot.wait_ticks(1); }
    Ok(broke)
}

/// A bot that died (drowned in a long water case, etc.) respawns and goes back to its arena spot.
fn revive(bot: &Bot, home: [f64; 3]) -> Result<()> {
    if !bot.st().dead { return Ok(()); }
    bot.respawn();
    bot.wait_until(Duration::from_secs(5), |s| !s.dead);
    bot.cmd(&format!("tp @s {:.2} {:.2} {:.2} 0 0", home[0], home[1], home[2]));
    bot.cmd("gamemode survival");
    barrier(bot)?;
    bot.wait_on_ground(Duration::from_secs(5));
    Ok(())
}

fn measure(bot: &Bot, home: [f64; 3], case: &Value) -> Result<Value> {
    revive(bot, home)?;
    let (block, item) = (case["block"].as_str().unwrap_or("stone"), case["item"].as_str().unwrap_or("hand"));
    let (ground, water) = (case["ground"].as_bool().unwrap_or(true), case["water"].as_bool().unwrap_or(false));
    // experiment mode: dig while genuinely above the floor (`alt` blocks), not just claiming it
    let alt = case["alt"].as_f64().unwrap_or(0.0);
    if alt > 0.0 { let q = bot.st().body.pos; bot.tp([q[0], q[1] + alt, q[2]])?; }
    let pos = bot.st().body.pos;
    let p = [pos[0].floor() as i32, pos[1].floor() as i32, pos[2].floor() as i32];
    let target = [p[0] + 2, p[1], p[2]];
    // inventory: just the tool in hotbar slot 0
    bot.cmd("clear");
    bot.cmd("kill @e[type=item,distance=..12]");
    if item != "hand" { bot.cmd(&format!("give @s minecraft:{item}")); }
    barrier(bot)?;
    if bot.st().body.flying { bail!("bot is flying"); }
    bot.set_held(0);
    if water {
        // Water Breathing does not change dig speed; it keeps long measurements from drowning the bot
        bot.cmd("effect give @s water_breathing 1000 0 true");
        bot.cmd(&format!("fill {} {} {} {} {} {} water", p[0], p[1], p[2], p[0], p[1] + 1, p[2]));
        barrier(bot)?; bot.wait_ticks(4);
    }
    if !ground { bot.set_frozen(true); bot.st().body.on_ground = false; bot.wait_ticks(2); }
    bot.look_at([target[0] as f64 + 0.5, target[1] as f64 + 0.5, target[2] as f64 + 0.5]);
    bot.wait_ticks(3);
    let r = (|| -> Result<Value> {
        // experiment mode (replay files only): accepted count out of `reps` probes at each wait in `scan`
        if let Some(scan) = case.get("scan").and_then(|s| s.as_array()) {
            let reps = case["reps"].as_u64().unwrap_or(5) as u32;
            let mut res = serde_json::Map::new();
            for w in scan.iter().filter_map(|w| w.as_u64()) {
                let mut bits = String::new();
                let gap = case["gap"].as_u64().unwrap_or(0) as u32;
                for _ in 0..reps { bits.push(if probe(bot, target, block, w as u32)? { '1' } else { '0' }); if gap > 0 { bot.wait_ticks(gap); } }
                res.insert(w.to_string(), json!(bits));
            }
            return Ok(json!({ "scan": res, "reps": reps }));
        }
        let t_f = formula_ticks(block, item, ground, water);
        let Some(t_f) = t_f else { bail!("{block} is unbreakable per dig_ticks") };
        // the client-side wait (dig_ticks + 1) must be accepted
        let mut hi = t_f + 1;
        // one retry per stage: a vanilla tick hiccup during a long wait made single probes fail now and then
        // (replaying such a case with scans never reproduced a real "never breaks")
        let mut accepted = probe(bot, target, block, hi)? || probe(bot, target, block, hi)?;
        if !accepted { hi = t_f * 2 + 10; accepted = probe(bot, target, block, hi)? || probe(bot, target, block, hi)?; }
        if !accepted { return Ok(json!({ "never": true, "formula": t_f, "tried": hi })); }
        let mut lo = 0u32;
        if probe(bot, target, block, 0)? { return Ok(json!({ "n": 0, "formula": t_f })); }
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if probe(bot, target, block, mid)? { hi = mid; } else { lo = mid; }
        }
        Ok(json!({ "n": hi, "formula": t_f }))
    })();
    // restore
    bot.set_frozen(false);
    if water { bot.cmd("effect clear @s"); bot.cmd(&format!("fill {} {} {} {} {} {} air", p[0] - 1, p[1], p[2] - 1, p[0] + 3, p[1] + 3, p[2] + 1)); barrier(bot)?; }
    revive(bot, home)?;
    bot.cmd(&format!("setblock {} {} {} air", target[0], target[1], target[2]));
    r
}

// ------------------------------------------------------------------ compare

fn compare(case: &Value, v: &Value, b: &Value) -> Vec<Mismatch> {
    let (block, item) = (case["block"].as_str().unwrap_or(""), case["item"].as_str().unwrap_or("hand"));
    let (g, w) = (case["ground"].as_bool().unwrap_or(true), case["water"].as_bool().unwrap_or(false));
    let cat = item.rsplit('_').next().unwrap_or(item);
    let flags = format!("{}{}", if w { "water" } else { "dry" }, if g { "" } else { "+air" });
    let desc = format!("{block} with {item} ({flags})");
    let mut out = Vec::new();
    if case.get("scan").is_some() { return vec![Mismatch::info("scan", format!("{desc}: vanilla {} basalt {}", v["scan"], b["scan"]))]; }
    if v["never"] == json!(true) || b["never"] == json!(true) {
        // someone refuses even at 2x the client time
        if v["never"] != b["never"] { out.push(Mismatch::fail(format!("never-breaks/{cat}"), format!("{desc}: vanilla {v} basalt {b}"))); }
        else { out.push(Mismatch::info("never-breaks-both", format!("{desc}: {v}"))); }
        return out;
    }
    let (nv, nb) = (v["n"].as_i64().unwrap_or(-1), b["n"].as_i64().unwrap_or(-1));
    let tf = v["formula"].as_i64().unwrap_or(0);
    let tol = ((nv as f64 * 0.05).ceil() as i64).max(2);
    if (nv - nb).abs() > tol {
        let dir = if nb < nv { "basalt-accepts-too-early" } else { "basalt-accepts-too-late" };
        out.push(Mismatch::fail(format!("threshold/{dir}/{flags}"), format!("{desc}: vanilla accepts at {nv} ticks, basalt at {nb} (dig_ticks {tf}, tol {tol})")));
    }
    // vanilla must accept what the Basalt client formula waits (dig_ticks + 1): by construction the probe at t_f+1 succeeded
    // on vanilla (else `never`), so additionally report formulas that are far slower than vanilla needs (> 2x + 3): it means dig_ticks is too slow
    if tf > nv * 2 + 3 && nv > 0 { out.push(Mismatch::info(format!("formula-slower-than-vanilla/{cat}"), format!("{desc}: dig_ticks {tf} but vanilla accepts from {nv}"))); }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn candidates_exist() {
        assert!(candidates().len() > 100);
        assert!(candidates().contains(&"stone"));
        assert!(tools().contains(&"hand".to_string()));
    }
}

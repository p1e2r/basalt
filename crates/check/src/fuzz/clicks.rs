//! Container click fuzzer: random click sequences (modes 0-6 incl. drag phases, shift-click, number
//! keys, double-click, drop) over the player inventory (2x2 grid) and a crafting table (3x3), run on
//! vanilla and Basalt from the same starting inventory. Final states (per step, so the first
//! diverging click names the class) must match; the container_set_slot / set_content / cursor updates
//! the server sends per click are diffed too and reported as info.
//!
//! Not covered (Basalt has no chest/furnace screens yet): chests, furnaces, hoppers, etc. are
//! vanilla-only and therefore not part of this differential harness.
use super::{arena::worker_arena, parallel, stack_str, Harness, Mismatch, Rng};
use crate::bot::Bot;
use crate::runner::Ctx;
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub static HARNESS: Harness = Harness { id: "clicks", desc: "container click sequences: all modes, inventory 2x2 + crafting table 3x3", default_cases: 400, long_cases: 8000, gen, run, compare };

/// (item, max stack)
const ITEMS: &[(&str, i32)] = &[
    ("dirt", 64), ("stone", 64), ("cobblestone", 64), ("oak_planks", 64), ("oak_log", 64), ("stick", 64), ("coal", 64), ("iron_ingot", 64),
    ("ender_pearl", 16), ("snowball", 16), ("egg", 16),
    ("wooden_pickaxe", 1), ("iron_sword", 1), ("diamond_pickaxe", 1), ("shield", 1),
    ("iron_helmet", 1), ("iron_chestplate", 1), ("iron_leggings", 1), ("iron_boots", 1), ("leather_helmet", 1), ("diamond_chestplate", 1),
];
const BIASED: &[&str] = &["oak_planks", "oak_log", "stick", "cobblestone", "iron_ingot", "dirt"];

fn max_stack(item: &str) -> i32 { ITEMS.iter().find(|i| i.0 == item).map(|i| i.1).unwrap_or(64) }

/// Where `/give`s land: merge into existing stacks, else the first empty slot, hotbar (36..44) first
/// then 9..35. Returns (slot, item, count) in slot order.
fn layout(inv: &[(String, i32)]) -> Vec<(usize, String, i32)> {
    let order: Vec<usize> = (36..45).chain(9..36).collect();
    let mut slots: Vec<Option<(String, i32)>> = vec![None; 46];
    for (item, n) in inv {
        let max = max_stack(item);
        let mut left = *n;
        for &i in &order {
            if left == 0 { break; }
            if let Some((it, c)) = &mut slots[i] { if it == item && *c < max { let k = (max - *c).min(left); *c += k; left -= k; } }
        }
        for &i in &order {
            if left == 0 { break; }
            if slots[i].is_none() { let k = max.min(left); slots[i] = Some((item.clone(), k)); left -= k; }
        }
    }
    slots.into_iter().enumerate().filter_map(|(i, s)| s.map(|(it, c)| (i, it, c))).collect()
}

fn inv_of(case: &Value) -> Vec<(String, i32)> {
    case["inv"].as_array().into_iter().flatten().map(|e| (e[0].as_str().unwrap_or("dirt").to_string(), e[1].as_i64().unwrap_or(1) as i32)).collect()
}

fn gen(r: &mut Rng, _i: usize) -> Value {
    let table = r.chance(0.35);
    let n = r.range(2, 7) as usize;
    let mut inv: Vec<Value> = Vec::new();
    for _ in 0..n {
        let item = if r.chance(0.5) { *r.pick(BIASED) } else { r.pick(ITEMS).0 };
        let max = max_stack(item);
        let count = if max == 1 { 1 } else if r.chance(0.5) { r.range(1, 4) as i32 } else { r.range(1, max as i64) as i32 };
        inv.push(json!([item, count]));
    }
    let lay = layout(&inv.iter().map(|e| (e[0].as_str().unwrap().to_string(), e[1].as_i64().unwrap() as i32)).collect::<Vec<_>>());
    let occupied: Vec<i64> = lay.iter().map(|l| if table { l.0 as i64 + 1 } else { l.0 as i64 }).collect();
    // slot numbering differs between the windows: the table's inventory is shifted by one (10..45)
    let hot = |r: &mut Rng| -> i64 {
        let u = r.f64();
        if u < 0.6 && !occupied.is_empty() { *r.pick(&occupied) }
        else if u < 0.8 { if table { r.range(0, 10) } else { r.range(0, 8) } }
        else { r.range(0, 45) }
    };
    let steps = r.range(1, 9) as usize;
    let mut clicks: Vec<Value> = Vec::new();
    for _ in 0..steps {
        match r.weighted(&[40, 16, 12, 3, 8, 14, 8]) {
            0 => { let slot = if r.chance(0.06) { -999 } else { hot(r) }; clicks.push(json!([slot, r.range(0, 1), 0])); }
            1 => clicks.push(json!([hot(r), r.range(0, 1), 1])),
            2 => { let b = if r.chance(0.12) { 40 } else { r.range(0, 8) }; clicks.push(json!([hot(r), b, 2])); }
            3 => clicks.push(json!([hot(r), 2, 3])),
            4 => { let slot = if r.chance(0.05) { -999 } else { hot(r) }; clicks.push(json!([slot, r.range(0, 1), 4])); }
            5 => {
                let ty = r.range(0, 2);
                clicks.push(json!([-999, ty << 2, 5]));
                for _ in 0..r.range(1, 5) { clicks.push(json!([hot(r), (ty << 2) | 1, 5])); }
                clicks.push(json!([-999, (ty << 2) | 2, 5]));
            }
            _ => clicks.push(json!([hot(r), 0, 6])),
        }
    }
    json!({ "table": table, "inv": inv, "clicks": clicks })
}

fn snapshot(bot: &Bot, table: bool) -> Value {
    let s = bot.st();
    let slots: Vec<String> = if table && s.screen.is_some() && s.container.len() >= 46 { s.container.iter().take(46).map(stack_str).collect() } else { s.inv.iter().take(46).map(stack_str).collect() };
    json!({ "slots": slots, "cursor": stack_str(&s.cursor) })
}

fn run(ctx: &mut Ctx, cases: &[Value]) -> Result<Vec<Value>> {
    let jobs = super::opts().jobs.clamp(1, 6);
    ctx.note("chest/furnace screens: vanilla-only, not covered here (Basalt has none yet)");
    parallel(ctx, cases, jobs,
        |ctx, w| { let bot = ctx.bot()?; worker_arena(ctx, &bot, w, 9, 9, 5, "survival")?; Ok(bot) },
        |bot, w, _i, case| run_case(bot, w, case))
}

fn run_case(bot: &Bot, _w: usize, case: &Value) -> Result<Value> {
    let table = case["table"].as_bool().unwrap_or(false);
    // reset: leave any screen, clear everything, remove stray item entities. Commands run on vanilla's
    // async chat chain, so use barriers instead of relying on packet order.
    if bot.st().screen.is_some() { bot.close_container(); bot.wait_ticks(3); }
    bot.cmd("clear");
    bot.cmd("kill @e[type=item,distance=..12]");
    super::barrier(bot)?;
    if bot.st().cursor.count > 0 { bot.click(-999, 0, 0); bot.wait_ticks(2); }
    let inv = inv_of(case);
    for (item, n) in &inv { bot.cmd(&format!("give @s minecraft:{item} {n}")); }
    super::barrier(bot)?;
    let want = layout(&inv);
    // force a full resync like a client that lost track (stale state id): vanilla only sends the slots
    // it thinks changed, and its record of what we have can disagree with this bot's view
    let mark = bot.st().cont_log.len();
    { let mut s = bot.st(); s.state_id = s.state_id.wrapping_add(777) & 0x7fff; }
    bot.click(-999, 0, 0);
    bot.wait_until(Duration::from_millis(400), |s| s.cont_log.len() > mark);
    let t0 = Instant::now();
    let matches = |s: &crate::bot::State| (0..46).all(|i| { let w = want.iter().find(|x| x.0 == i); let h = &s.inv[i]; match w { Some((_, it, c)) => h.count == *c && h.name() == it, None => h.count <= 0 } }) && s.cursor.count <= 0;
    let ok = bot.wait_until(Duration::from_millis(1500), matches);
    if !ok && t0.elapsed() < Duration::from_millis(1600) { /* record whatever we have: setup divergence is a finding */ }
    let (pos, o) = { let p = bot.st().body.pos; (p, [p[0].floor() as i32, p[1].floor() as i32 - 1, p[2].floor() as i32]) };
    let _ = pos;
    if table {
        let tp = [o[0] + 2, o[1] + 1, o[2]];
        bot.cmd(&format!("setblock {} {} {} crafting_table", tp[0], tp[1], tp[2]));
        if !bot.wait_until(Duration::from_millis(1500), |s| s.world.name(tp[0], tp[1], tp[2]) == "crafting_table") { bail!("crafting table not placed"); }
        bot.look_at([tp[0] as f64 + 0.5, tp[1] as f64 + 0.5, tp[2] as f64 + 0.5]);
        bot.wait_ticks(2);
        bot.place_at(tp, 1, [0.5, 1.0, 0.5]);
        if !bot.wait_until(Duration::from_millis(1500), |s| s.screen.is_some() && s.container.len() >= 46) { return Ok(json!({ "table_open": false, "init": snapshot(bot, false), "steps": [] })); }
    }
    let init = snapshot(bot, table);
    let mut steps = Vec::new();
    for c in case["clicks"].as_array().into_iter().flatten() {
        let (slot, button, mode) = (c[0].as_i64().unwrap_or(0) as i16, c[1].as_i64().unwrap_or(0) as i8, c[2].as_i64().unwrap_or(0) as i32);
        let mark = bot.st().cont_log.len();
        bot.click(slot, button, mode);
        // wait for the server's reply (a packet), then let the burst finish
        let drag_mid = mode == 5 && (button & 3) < 2; // drag start/add: the server answers at the end
        let t = Instant::now();
        let max = if drag_mid { 3 } else { 70 };
        while t.elapsed() < Duration::from_millis(max) { if bot.st().cont_log.len() > mark { break; } std::thread::sleep(Duration::from_millis(2)); }
        if !drag_mid { super::settle_quiet(bot, 10, 50); }
        let mut u: Vec<String> = bot.st().cont_log[mark..].to_vec();
        u.sort();
        let mut st = snapshot(bot, table);
        st["u"] = json!(u);
        steps.push(st);
    }
    Ok(json!({ "table_open": true, "init": init, "steps": steps }))
}

fn diff_slots(a: &Value, b: &Value) -> String {
    let (a, b) = (a["slots"].as_array().cloned().unwrap_or_default(), b["slots"].as_array().cloned().unwrap_or_default());
    let mut d: Vec<String> = (0..a.len().max(b.len())).filter(|&i| a.get(i) != b.get(i)).take(6).map(|i| format!("slot {i}: vanilla {} basalt {}", a.get(i).and_then(|x| x.as_str()).unwrap_or("-"), b.get(i).and_then(|x| x.as_str()).unwrap_or("-"))).collect();
    d.dedup();
    d.join("; ")
}

fn compare(case: &Value, v: &Value, b: &Value) -> Vec<Mismatch> {
    let tbl = if case["table"].as_bool().unwrap_or(false) { "table/" } else { "" };
    if v["table_open"] != b["table_open"] { return vec![Mismatch::fail("table-open", format!("vanilla opened={} basalt opened={}", v["table_open"], b["table_open"]))]; }
    if v["table_open"] == json!(false) { return vec![]; }
    if v["init"] != b["init"] { return vec![Mismatch::info("setup-diverged", format!("starting inventory differs: {}", diff_slots(&v["init"], &b["init"])))]; }
    let (vs, bs) = (v["steps"].as_array().cloned().unwrap_or_default(), b["steps"].as_array().cloned().unwrap_or_default());
    let clicks = case["clicks"].as_array().cloned().unwrap_or_default();
    let mut out = Vec::new();
    for i in 0..vs.len().min(bs.len()) {
        let mode = clicks.get(i).map(|c| c[2].as_i64().unwrap_or(-1)).unwrap_or(-1);
        let (sv, sb) = (&vs[i], &bs[i]);
        if sv["slots"] != sb["slots"] || sv["cursor"] != sb["cursor"] {
            let cur = if sv["cursor"] != sb["cursor"] { format!("; cursor vanilla {} basalt {}", sv["cursor"], sb["cursor"]) } else { String::new() };
            out.push(Mismatch::fail(format!("state/{tbl}mode{mode}"), format!("after click {i} {}: {}{cur}", clicks[i], diff_slots(sv, sb))));
            return out;
        }
        if sv["u"] != sb["u"] {
            // vanilla sends incremental set_slot / set_cursor_item; Basalt resends the whole window
            let has_content = |x: &Value| x["u"].as_array().map(|a| a.iter().any(|l| l.as_str().map(|s| s.starts_with("content")).unwrap_or(false))).unwrap_or(false);
            let class = if has_content(sb) != has_content(sv) { "packets/full-content-vs-incremental" } else { "packets/slot-set" };
            out.push(Mismatch::info(format!("{class}/{tbl}mode{mode}"), format!("click {i} {}: vanilla sent {} basalt sent {}", clicks[i], sv["u"], sb["u"])));
            return out;
        }
    }
    out
}

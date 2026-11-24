//! Pickup-order fuzzer: a random inventory (partial stacks, a damaged tool, an offhand stack, a
//! random selected hotbar slot) and 1-4 item entities summoned at the bot's feet (pickup delay 0,
//! some with a damage component so "identical components" matters, some full stacks against a full
//! inventory). After each pickup the 46 slots and whether the item is still on the ground are
//! compared between vanilla and Basalt: the first diverging pickup names the class.
use super::{arena::worker_arena, parallel, stack_str, Harness, Mismatch, Rng};
use crate::bot::Bot;
use crate::runner::Ctx;
use anyhow::Result;
use serde_json::{json, Value};
use std::time::Duration;

pub static HARNESS: Harness = Harness {
    id: "pickup",
    desc: "item pickup slot order: random inventories x picked-up stacks",
    default_cases: 150,
    long_cases: 2000,
    gen,
    run,
    compare,
};

/// (item, max stack)
const ITEMS: &[(&str, i32)] = &[
    ("dirt", 64),
    ("cobblestone", 64),
    ("stick", 64),
    ("oak_planks", 64),
    ("ender_pearl", 16),
    ("egg", 16),
    ("iron_sword", 1),
    ("wooden_pickaxe", 1),
];

fn max_stack(item: &str) -> i32 {
    ITEMS
        .iter()
        .find(|i| i.0 == item)
        .map(|i| i.1)
        .unwrap_or(64)
}

fn gen(r: &mut Rng, _i: usize) -> Value {
    // starting slots: (window slot, item, count, damage); hotbar 36..44, main 9..35, offhand 45
    let mut slots: Vec<Value> = Vec::new();
    let mut used = std::collections::HashSet::new();
    let fill = if r.chance(0.15) {
        36
    } else {
        r.range(0, 12) as usize
    };
    for _ in 0..fill {
        let slot = if r.chance(0.5) {
            r.range(36, 44)
        } else if r.chance(0.1) {
            45
        } else {
            r.range(9, 35)
        } as usize;
        if !used.insert(slot) {
            continue;
        }
        let (item, max) = *r.pick(ITEMS);
        let count = if max == 1 {
            1
        } else if r.chance(0.3) {
            max
        } else {
            r.range(1, max as i64) as i32
        };
        let damage = if max == 1 && r.chance(0.5) {
            r.range(1, 20) as i32
        } else {
            0
        };
        slots.push(json!([slot, item, count, damage]));
    }
    let held = r.range(0, 8);
    let mut picks: Vec<Value> = Vec::new();
    for _ in 0..r.range(1, 4) {
        let (item, max) = *r.pick(ITEMS);
        let count = if max == 1 {
            1
        } else if r.chance(0.3) {
            max
        } else {
            r.range(1, max as i64) as i32
        };
        let damage = if max == 1 && r.chance(0.5) {
            r.range(1, 20) as i32
        } else {
            0
        };
        picks.push(json!([item, count, damage]));
    }
    json!({ "slots": slots, "held": held, "picks": picks })
}

fn item_arg(item: &str, damage: i64) -> String {
    if damage > 0 {
        format!("minecraft:{item}[damage={damage}]")
    } else {
        format!("minecraft:{item}")
    }
}

fn snapshot(bot: &Bot) -> Vec<String> {
    bot.st().inv.iter().take(46).map(stack_str).collect()
}

fn run(ctx: &mut Ctx, cases: &[Value]) -> Result<Vec<Value>> {
    let jobs = super::opts().jobs.clamp(1, 6);
    parallel(
        ctx,
        cases,
        jobs,
        |ctx, w| {
            let bot = ctx.bot()?;
            worker_arena(ctx, &bot, w, 7, 7, 5, "survival")?;
            Ok(bot)
        },
        |bot, w, _i, case| run_case(bot, w, case),
    )
}

/// `/item replace` slot name for a window slot.
fn slot_name(slot: usize) -> String {
    match slot {
        36..=44 => format!("hotbar.{}", slot - 36),
        45 => "weapon.offhand".into(),
        _ => format!("inventory.{}", slot - 9),
    }
}

fn run_case(bot: &Bot, _w: usize, case: &Value) -> Result<Value> {
    if bot.st().screen.is_some() {
        bot.close_container();
        bot.wait_ticks(3);
    }
    bot.cmd("clear");
    bot.cmd("kill @e[type=item,distance=..12]");
    super::barrier(bot)?;
    for s in case["slots"].as_array().into_iter().flatten() {
        let (slot, item, count, damage) = (
            s[0].as_u64().unwrap_or(36) as usize,
            s[1].as_str().unwrap_or("dirt"),
            s[2].as_i64().unwrap_or(1),
            s[3].as_i64().unwrap_or(0),
        );
        bot.cmd(&format!(
            "item replace entity @s {} with {} {count}",
            slot_name(slot),
            item_arg(item, damage)
        ));
    }
    bot.set_held(case["held"].as_u64().unwrap_or(0) as u8);
    super::barrier(bot)?;
    bot.wait_ticks(4);
    let init = snapshot(bot);
    let mut steps = Vec::new();
    for pk in case["picks"].as_array().into_iter().flatten() {
        let (item, count, damage) = (
            pk[0].as_str().unwrap_or("dirt"),
            pk[1].as_i64().unwrap_or(1),
            pk[2].as_i64().unwrap_or(0),
        );
        let comp = if damage > 0 {
            format!(",components:{{\"minecraft:damage\":{damage}}}")
        } else {
            String::new()
        };
        let before = snapshot(bot);
        bot.cmd(&format!("summon item ~ ~0.2 ~ {{Item:{{id:\"minecraft:{item}\",count:{count}{comp}}},PickupDelay:0,Motion:[0.0,0.0,0.0]}}"));
        // picked up (the inventory changes) or not (full / no room): give it up to 1 s
        bot.wait_until(Duration::from_millis(1000), |s| {
            s.inv.iter().take(46).map(stack_str).collect::<Vec<_>>() != before
        });
        bot.wait_ticks(4);
        // "left on the ground" only counts when nothing was taken (a picked-up entity's removal can
        // still be in flight on vanilla)
        let now = snapshot(bot);
        let left = now == before && bot.st().entities.values().any(|e| e.kind == "item");
        steps.push(json!({ "slots": now, "on_ground": left }));
        bot.cmd("kill @e[type=item,distance=..12]");
        super::barrier(bot)?;
    }
    let _ = max_stack;
    Ok(json!({ "init": init, "steps": steps }))
}

fn diff(a: &Value, b: &Value) -> String {
    let (a, b) = (
        a.as_array().cloned().unwrap_or_default(),
        b.as_array().cloned().unwrap_or_default(),
    );
    (0..a.len().max(b.len()))
        .filter(|&i| a.get(i) != b.get(i))
        .take(6)
        .map(|i| {
            format!(
                "slot {i}: vanilla {} basalt {}",
                a.get(i).and_then(|x| x.as_str()).unwrap_or("-"),
                b.get(i).and_then(|x| x.as_str()).unwrap_or("-")
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn compare(case: &Value, v: &Value, b: &Value) -> Vec<Mismatch> {
    if v["init"] != b["init"] {
        return vec![Mismatch::info(
            "setup-diverged",
            format!(
                "starting inventory differs: {}",
                diff(&v["init"], &b["init"])
            ),
        )];
    }
    let (vs, bs) = (
        v["steps"].as_array().cloned().unwrap_or_default(),
        b["steps"].as_array().cloned().unwrap_or_default(),
    );
    let picks = case["picks"].as_array().cloned().unwrap_or_default();
    for i in 0..vs.len().min(bs.len()) {
        if vs[i]["slots"] != bs[i]["slots"] {
            let full = vs[i]["on_ground"] == json!(true);
            let class = if full { "slots/partial" } else { "slots/order" };
            return vec![Mismatch::fail(
                class,
                format!(
                    "after pickup {i} {}: {}",
                    picks.get(i).cloned().unwrap_or_default(),
                    diff(&vs[i]["slots"], &bs[i]["slots"])
                ),
            )];
        }
        if vs[i]["on_ground"] != bs[i]["on_ground"] {
            return vec![Mismatch::fail(
                "left-on-ground",
                format!(
                    "pickup {i} {}: vanilla left={} basalt left={}",
                    picks.get(i).cloned().unwrap_or_default(),
                    vs[i]["on_ground"],
                    bs[i]["on_ground"]
                ),
            )];
        }
    }
    vec![]
}

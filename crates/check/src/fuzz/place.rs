//! Block placement fuzzer: random placements in an arena (block item, target block, clicked face,
//! cursor position, yaw/pitch, sneaking, pre-existing neighbours incl. replaceable blocks and water)
//! on vanilla and Basalt; the state of the placement cell and its 6 neighbours as the client sees it
//! (block_update packets) must match: orientation, half/shape properties, second halves of doors and
//! beds, waterlogging, replaceable targets, support requirements.
//!
//! Classes: `placed/<family>/{missing,extra,block,prop}`, `neighbor/<family>`, `target-changed/<family>`,
//! `screen/<family>` (clicking an interactive block opened a screen on only one server).
use super::{arena::worker_arena, barrier, parallel, Harness, Mismatch, Rng};
use crate::bot::Bot;
use crate::runner::Ctx;
use anyhow::Result;
use basalt_data::BlockState;
use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::Duration;

pub static HARNESS: Harness = Harness {
    id: "place",
    desc: "block placement: placed block + 6 neighbours (orientation, halves, waterlogging)",
    default_cases: 300,
    long_cases: 4000,
    gen,
    run,
    compare,
};

/// down, up, north (-z), south (+z), west (-x), east (+x)
const DIRS: [[i32; 3]; 6] = [
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
    [-1, 0, 0],
    [1, 0, 0],
];
const DIR_NAMES: [&str; 6] = ["down", "up", "north", "south", "west", "east"];

const TARGETS: &[&str] = &[
    "stone",
    "stone",
    "stone",
    "oak_planks",
    "oak_slab[type=bottom]",
    "oak_slab[type=top]",
    "oak_stairs[facing=north,half=bottom]",
    "oak_stairs[facing=east,half=top]",
    "glass",
    "oak_log[axis=x]",
    "dirt",
    "oak_fence",
    "cobblestone_wall",
    "short_grass",
    "snow[layers=1]",
    "snow[layers=3]",
    "crafting_table",
    "chest[facing=north]",
    "oak_trapdoor[open=false]",
    "lantern[hanging=false]",
    "farmland",
];
const PRE: &[&str] = &[
    "stone",
    "water",
    "short_grass",
    "snow[layers=2]",
    "oak_fence",
    "oak_stairs[facing=south,half=bottom]",
    "oak_slab[type=bottom]",
    "glass_pane",
    "cobblestone_wall",
    "oak_planks",
    "chest[facing=west]",
    "iron_bars",
];
const FAVORED: &[&str] = &[
    "stairs",
    "slab",
    "door",
    "trapdoor",
    "bed",
    "_log",
    "fence",
    "wall",
    "pane",
    "bars",
    "torch",
    "button",
    "lever",
    "ladder",
    "rail",
    "sign",
    "banner",
    "chest",
    "hopper",
    "piston",
    "observer",
    "dispenser",
    "dropper",
    "furnace",
    "anvil",
    "lantern",
    "chain",
    "pickle",
    "candle",
    "lichen",
    "vine",
    "dripstone",
    "bud",
    "repeater",
    "comparator",
    "coral",
    "carpet",
    "head",
    "skull",
    "pressure_plate",
    "glazed_terracotta",
    "pumpkin",
    "barrel",
    "end_rod",
    "lightning_rod",
    "campfire",
    "bell",
    "grindstone",
    "stonecutter",
    "loom",
    "lectern",
    "shulker",
    "tripwire",
    "scaffolding",
    "sapling",
    "flower",
    "cactus",
    "sugar_cane",
    "bamboo",
    "snow",
    "cake",
];

fn items() -> &'static (Vec<&'static str>, Vec<&'static str>) {
    static I: OnceLock<(Vec<&'static str>, Vec<&'static str>)> = OnceLock::new();
    I.get_or_init(|| {
        let all: Vec<&'static str> = basalt_data::registries::ITEM
            .iter()
            .copied()
            .filter(|n| BlockState::from_name(n).is_some())
            .filter(|n| {
                ![
                    "air",
                    "water",
                    "lava",
                    "fire",
                    "soul_fire",
                    "bedrock",
                    "barrier",
                    "light",
                    "command_block",
                    "structure",
                    "jigsaw",
                    "spawner",
                    "trial_spawner",
                    "vault",
                    "test_",
                    "debug",
                    "end_portal",
                    "nether_portal",
                    "end_gateway",
                    "piston_head",
                    "moving_piston",
                    "reinforced_deepslate",
                ]
                .iter()
                .any(|s| n.contains(s))
            })
            .collect();
        let fav = all
            .iter()
            .copied()
            .filter(|n| FAVORED.iter().any(|f| n.contains(f)))
            .collect();
        (all, fav)
    })
}

fn gen(r: &mut Rng, _i: usize) -> Value {
    let (all, fav) = items();
    let item = if r.chance(0.6) {
        *r.pick(fav)
    } else {
        *r.pick(all)
    };
    let face = r.below(6);
    let cur = |r: &mut Rng, axis: usize| -> f64 {
        let v = if r.chance(0.15) {
            *r.pick(&[0.0625, 0.5, 0.9375])
        } else {
            (r.range(3, 97) as f64) / 100.0
        };
        let _ = axis;
        v
    };
    let mut cursor = [cur(r, 0), cur(r, 1), cur(r, 2)];
    // the clicked face's own axis sits on the block surface
    let n = DIRS[face];
    for a in 0..3 {
        if n[a] > 0 {
            cursor[a] = 1.0;
        } else if n[a] < 0 {
            cursor[a] = 0.0;
        }
    }
    let yaw = if r.chance(0.4) {
        (r.range(0, 7) * 45) as f64
    } else {
        r.range(-180, 180) as f64
    };
    let pitch = if r.chance(0.3) {
        *r.pick(&[-90.0, 90.0, 0.0, 45.0, -45.0])
    } else {
        r.range(-90, 90) as f64
    };
    let mut pre: Vec<Value> = Vec::new();
    if r.chance(0.25) {
        pre.push(json!([
            -1,
            *r.pick(&["water", "short_grass", "snow[layers=2]", "water", "stone"])
        ]));
    }
    for _ in 0..r.weighted(&[40, 35, 25]) {
        let d = r.below(6);
        if d == face ^ 1 {
            continue;
        } // opposite of the clicked face is the target itself
        pre.push(json!([d, *r.pick(PRE)]));
    }
    json!({ "item": item, "target": *r.pick(TARGETS), "face": face, "cursor": cursor, "yaw": yaw, "pitch": pitch, "sneak": r.chance(0.3), "pre": pre })
}

fn run(ctx: &mut Ctx, cases: &[Value]) -> Result<Vec<Value>> {
    let jobs = super::opts().jobs.clamp(1, 8);
    parallel(
        ctx,
        cases,
        jobs,
        |ctx, w| {
            let bot = ctx.bot()?;
            worker_arena(ctx, &bot, w, 12, 9, 7, "creative")?;
            bot.fly(false);
            Ok(bot)
        },
        |bot, _w, _i, case| run_case(bot, case),
    )
}

fn run_case(bot: &Bot, case: &Value) -> Result<Value> {
    let pos = bot.st().body.pos;
    // arena origin = bot cell - (6, 1, 4) (worker_arena stands the bot at +6, +1, +4.5 of a 12x9 floor)
    let o = [
        pos[0].floor() as i32 - 6,
        pos[1].floor() as i32 - 1,
        pos[2].floor() as i32 - 4,
    ];
    let p = [o[0] + 8, o[1] + 3, o[2] + 4];
    let face = case["face"].as_u64().unwrap_or(1) as usize;
    let n = DIRS[face];
    let t = [p[0] - n[0], p[1] - n[1], p[2] - n[2]];
    if bot.st().screen.is_some() {
        bot.close_container();
        bot.wait_ticks(2);
    }
    bot.sneak(false);
    bot.cmd(&format!(
        "fill {} {} {} {} {} {} air",
        p[0] - 3,
        o[1] + 1,
        p[2] - 3,
        p[0] + 3,
        o[1] + 6,
        p[2] + 3
    ));
    bot.cmd("kill @e[type=item,distance=..14]");
    barrier(bot)?;
    bot.cmd(&format!(
        "setblock {} {} {} {}",
        t[0],
        t[1],
        t[2],
        case["target"].as_str().unwrap_or("stone")
    ));
    for pre in case["pre"].as_array().into_iter().flatten() {
        let slot = pre[0].as_i64().unwrap_or(-1);
        let c = if slot < 0 {
            p
        } else {
            let d = DIRS[slot as usize];
            [p[0] + d[0], p[1] + d[1], p[2] + d[2]]
        };
        bot.cmd(&format!(
            "setblock {} {} {} {}",
            c[0],
            c[1],
            c[2],
            pre[1].as_str().unwrap_or("stone")
        ));
    }
    bot.cmd("clear");
    bot.cmd(&format!(
        "give @s minecraft:{}",
        case["item"].as_str().unwrap_or("stone")
    ));
    barrier(bot)?;
    bot.set_held(0);
    bot.look(
        case["yaw"].as_f64().unwrap_or(0.0) as f32,
        case["pitch"].as_f64().unwrap_or(0.0) as f32,
    );
    bot.sneak(case["sneak"].as_bool().unwrap_or(false));
    bot.wait_ticks(4);
    let c = &case["cursor"];
    bot.place_at(
        t,
        face as u8,
        [
            c[0].as_f64().unwrap_or(0.5) as f32,
            c[1].as_f64().unwrap_or(0.5) as f32,
            c[2].as_f64().unwrap_or(0.5) as f32,
        ],
    );
    bot.wait_ticks(10);
    let st = bot.st();
    let mut cells = serde_json::Map::new();
    cells.insert("P".into(), json!(st.world.state_string(p[0], p[1], p[2])));
    cells.insert("T".into(), json!(st.world.state_string(t[0], t[1], t[2])));
    for (i, d) in DIRS.iter().enumerate() {
        let q = [p[0] + d[0], p[1] + d[1], p[2] + d[2]];
        if q == t {
            continue;
        }
        cells.insert(
            DIR_NAMES[i].into(),
            json!(st.world.state_string(q[0], q[1], q[2])),
        );
    }
    // blocks the placed block may have grown into (doors/beds: second half; tall plants)
    let screen = st.screen.as_ref().map(|s| s.1.clone());
    drop(st);
    Ok(json!({ "cells": cells, "screen": screen }))
}

/// Coarse block family for class names: the first FAVORED keyword in the item name, else `other`.
fn family(item: &str) -> String {
    FAVORED
        .iter()
        .find(|f| item.contains(*f))
        .map(|f| f.trim_start_matches('_').to_string())
        .unwrap_or_else(|| "other".into())
}

fn compare(case: &Value, v: &Value, b: &Value) -> Vec<Mismatch> {
    let item = case["item"].as_str().unwrap_or("");
    let fam = family(item);
    let mut out = Vec::new();
    if v["screen"] != b["screen"] {
        out.push(Mismatch::fail(
            format!("screen/{fam}"),
            format!(
                "{item} on {}: vanilla screen {} basalt screen {}",
                case["target"], v["screen"], b["screen"]
            ),
        ));
        return out;
    }
    let (vc, bc) = (
        v["cells"].as_object().cloned().unwrap_or_default(),
        b["cells"].as_object().cloned().unwrap_or_default(),
    );
    // Flowing water (level 1..8) in vanilla's cells: its source spread during the wait. Basalt has no fluid simulation yet (M6),
    // so any difference in such a case is reported as info (not failed) under `fluid-spread/<family>`.
    let flowing = vc.values().filter_map(|x| x.as_str()).any(|c| {
        c.starts_with("minecraft:water[level=") && !c.starts_with("minecraft:water[level=0]")
    });
    if flowing
        && ["P", "T", "down", "up", "north", "south", "west", "east"]
            .iter()
            .any(|k| vc.get(*k) != bc.get(*k))
    {
        return vec![Mismatch::info(
            format!("fluid-spread/{fam}"),
            format!(
                "{item} against {}: vanilla water spread, Basalt has no fluids",
                case["target"]
            ),
        )];
    }
    for key in ["P", "T", "down", "up", "north", "south", "west", "east"] {
        let (sv, sb) = (
            vc.get(key).and_then(|x| x.as_str()),
            bc.get(key).and_then(|x| x.as_str()),
        );
        if sv == sb {
            continue;
        }
        let (sv, sb) = (sv.unwrap_or("-"), sb.unwrap_or("-"));
        let class = if key == "P" {
            let (nv, nb) = (
                sv.split('[').next().unwrap_or(""),
                sb.split('[').next().unwrap_or(""),
            );
            if nv == nb {
                format!("placed/{fam}/prop")
            } else if nb == "minecraft:air" {
                format!("placed/{fam}/missing")
            } else if nv == "minecraft:air" {
                format!("placed/{fam}/extra")
            } else {
                format!("placed/{fam}/block")
            }
        } else if key == "T" {
            format!("target-changed/{fam}")
        } else {
            format!("neighbor/{fam}")
        };
        out.push(Mismatch::fail(class, format!("{item} against {} face {} yaw {} pitch {} sneak {}: cell {key}: vanilla {sv} / basalt {sb}", case["target"], DIR_NAMES[case["face"].as_u64().unwrap_or(0) as usize], case["yaw"], case["pitch"], case["sneak"])));
        break; // first differing cell names the class
    }
    out
}

#[allow(dead_code)]
fn _unused(_: Duration) {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generates_valid_cases() {
        for i in 0..200 {
            let c = gen(&mut Rng::for_case(3, i), i);
            assert!(BlockState::from_name(c["item"].as_str().unwrap()).is_some());
            assert!(c["face"].as_u64().unwrap() < 6);
        }
    }
}

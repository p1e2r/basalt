//! Fluid ticking (M6): water / lava spreading, falling, draining, infinite sources, slope finding
//! towards holes, lava + water forming obsidian / cobblestone / stone, fluids washing away blocks.
//! Each case builds a few cells on a walled stone floor (two layers thick), runs timed /setblock
//! actions and samples every cell around the centre once per tick. Metrics: the final state of each
//! changed cell (exact) and the tick it first changed (±3: packets arrive with jitter on both
//! servers). Compared against vanilla 26.2 with --diff. `FLUID_CASE=<substring>` filters cases.
use crate::bot::Bot;
use crate::runner::{Ctx, ARENA_Y};
use anyhow::Result;
use std::collections::BTreeMap;
use std::time::Instant;

const Y: i32 = ARENA_Y + 1;
const R: i32 = 7;

type Cells = &'static [(i32, i32, i32, &'static str)];

struct Case {
    name: &'static str,
    setup: Cells,
    /// (tick after start, cell, block) — run by /setblock
    acts: &'static [(u32, [i32; 3], &'static str)],
    ticks: u32,
}

fn short(s: &str) -> String {
    s.trim_start_matches("minecraft:").to_string()
}

fn run_case(ctx: &mut Ctx, bot: &Bot, o: [i32; 3], case: &Case) -> Result<()> {
    let c = [o[0] + 10, Y, o[2] + 10];
    let abs = |r: [i32; 3]| [c[0] + r[0], c[1] + r[1], c[2] + r[2]];
    // walled floor: stone y-2..-1, air above, a stone ring at R+1 so nothing escapes
    let (a, b) = (abs([-R - 1, -2, -R - 1]), abs([R + 1, 4, R + 1]));
    bot.cmd_reply(&format!(
        "fill {} {} {} {} {} {} stone",
        a[0], a[1], a[2], b[0], b[1], b[2]
    ))?;
    let (a, b) = (abs([-R, 0, -R]), abs([R, 4, R]));
    // cleared twice: fluid ticks still pending from the previous case may refill a cell once
    for _ in 0..2 {
        bot.cmd_reply(&format!(
            "fill {} {} {} {} {} {} air",
            a[0], a[1], a[2], b[0], b[1], b[2]
        ))?;
        bot.wait_ticks(8);
    }
    bot.cmd("kill @e[type=minecraft:item,distance=..20]");
    bot.wait_ticks(4);
    for (dx, dy, dz, blk) in case.setup {
        let p = abs([*dx, *dy, *dz]);
        bot.cmd_reply(&format!("setblock {} {} {} {blk}", p[0], p[1], p[2]))?;
    }
    bot.wait_ticks(4);
    let region: Vec<[i32; 3]> = (-R..=R)
        .flat_map(|dx| (-2..=3).flat_map(move |dy| (-R..=R).map(move |dz| [dx, dy, dz])))
        .collect();
    let snap = |bot: &Bot| -> Vec<String> {
        let s = bot.st();
        region
            .iter()
            .map(|r| {
                let a = abs(*r);
                s.world.state_string(a[0], a[1], a[2])
            })
            .collect()
    };
    let before = snap(bot);
    let mut first: BTreeMap<usize, u32> = BTreeMap::new();
    let t0 = Instant::now();
    let mut items: BTreeMap<i32, (String, i32)> = BTreeMap::new();
    for tick in 0..case.ticks {
        for (t, r, blk) in case.acts {
            if *t == tick {
                let p = abs(*r);
                bot.cmd(&format!("setblock {} {} {} {blk}", p[0], p[1], p[2]));
            }
        }
        bot.wait_ticks(1);
        let now = snap(bot);
        if std::env::var("FLUID_TRACE").is_ok() {
            let row: Vec<String> = (-3..=3)
                .map(|x| {
                    let i = region.iter().position(|r| *r == [x, 0, 2]).unwrap();
                    short(&now[i])
                        .replace("water[level=", "w")
                        .replace("lava[level=", "l")
                        .replace(']', "")
                })
                .collect();
            ctx.note(format!("{} t{tick}: {}", case.name, row.join(" ")));
        }
        for (i, st) in now.iter().enumerate() {
            if *st != before[i] && !first.contains_key(&i) {
                first.insert(i, tick);
            }
        }
        let s = bot.st();
        for (id, e) in s
            .entities
            .iter()
            .filter(|(_, e)| e.kind == "item" && e.added >= t0)
        {
            if let Some(it) = &e.item {
                items
                    .entry(*id)
                    .or_insert_with(|| (it.name().to_string(), it.count));
            }
        }
    }
    let fin = snap(bot);
    let mut desc = Vec::new();
    let mut counts: BTreeMap<String, i32> = BTreeMap::new();
    for (i, r) in region.iter().enumerate() {
        if fin[i] != before[i] {
            let st = short(&fin[i]);
            ctx.metric(
                &format!("{}/{},{},{}:{}", case.name, r[0], r[1], r[2], st),
                1.0,
                0.0,
            );
            *counts.entry(st.clone()).or_default() += 1;
        }
        if let Some(t) = first.get(&i) {
            // only cells that end up changed, or the source-removal drain would double count
            ctx.metric(
                &format!("{}/{},{},{}@", case.name, r[0], r[1], r[2]),
                *t as f64,
                3.0,
            );
            if r[1] == 0 && r[2] == 0 && r[0] >= 0 {
                desc.push(format!("x{}@{}:{}", r[0], t, short(&fin[i])));
            }
        }
    }
    let mut it: BTreeMap<String, i32> = BTreeMap::new();
    for (n, k) in items.values() {
        *it.entry(n.clone()).or_default() += k;
    }
    for (k, v) in &it {
        ctx.metric(&format!("{}/item:{k}", case.name), *v as f64, 0.0);
    }
    ctx.note(format!(
        "{}: {:?} | +x row {} | items {:?}",
        case.name,
        counts,
        desc.join(" "),
        it
    ));
    Ok(())
}

const SRC: [i32; 3] = [0, 0, 0];

static CASES: &[Case] = &[
    Case {
        name: "water_flat",
        setup: &[],
        acts: &[(0, SRC, "water")],
        ticks: 50,
    },
    Case {
        name: "water_hole",
        setup: &[(3, -1, 0, "air")],
        acts: &[(0, SRC, "water")],
        ticks: 50,
    },
    Case {
        name: "water_far_hole",
        setup: &[(6, -1, 1, "air")],
        acts: &[(0, SRC, "water")],
        ticks: 50,
    },
    Case {
        name: "water_pillar",
        setup: &[(0, 0, 0, "stone"), (0, 1, 0, "stone")],
        acts: &[(0, [0, 2, 0], "water")],
        ticks: 60,
    },
    Case {
        name: "water_drain",
        setup: &[],
        acts: &[(0, SRC, "water"), (45, SRC, "air")],
        ticks: 90,
    },
    Case {
        name: "water_infinite",
        setup: &[],
        acts: &[(0, [-1, 0, 0], "water"), (0, [1, 0, 0], "water")],
        ticks: 30,
    },
    Case {
        name: "water_torch_grass",
        setup: &[
            (1, 0, 0, "torch"),
            (0, 0, 1, "dandelion"),
            (0, -1, 1, "grass_block"),
            (-1, 0, 0, "poppy"),
            (-1, -1, 0, "grass_block"),
        ],
        acts: &[(0, SRC, "water")],
        ticks: 20,
    },
    Case {
        name: "lava_flat",
        setup: &[],
        acts: &[(0, SRC, "lava")],
        ticks: 130,
    },
    Case {
        name: "lava_water",
        setup: &[],
        acts: &[(0, [-2, 0, 0], "water"), (0, [2, 0, 0], "lava")],
        ticks: 80,
    },
    Case {
        name: "lava_onto_water",
        setup: &[(0, 0, 0, "stone"), (0, 1, 0, "stone"), (1, 0, 0, "water")],
        acts: &[(0, [0, 2, 0], "lava")],
        ticks: 100,
    },
];

pub fn fluids(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    let o = ctx.arena(&bot, 21, 21, 6)?;
    ctx.stand(&bot, o[0] + 1, o[2] + 1, "creative")?;
    let filter = std::env::var("FLUID_CASE").ok();
    for case in CASES {
        if let Some(f) = &filter {
            if !f.split(',').any(|f| case.name.contains(f)) {
                continue;
            }
        }
        if let Err(e) = run_case(ctx, &bot, o, case) {
            ctx.check(false, format!("{}: {e:#}", case.name));
        }
    }
    Ok(())
}

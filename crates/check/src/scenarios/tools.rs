//! Tool actions on blocks (use_item_on with an item that changes the clicked block): hoe tilling,
//! shovel paths + campfire dousing, axe stripping / copper scraping / unwaxing, shears on pumpkins,
//! honeycomb waxing, bone meal. Table driven like m4_blocks: each case builds a few cells, clicks one
//! face with the item and records which cells changed (and to what), the item entities that appeared,
//! the held stack afterwards (count + damage) and the sounds/level events heard. Compared against
//! vanilla 26.2 with --diff. `TOOLS_CASE=<substring>` restricts a run to matching cases.
use super::util::*;
use crate::bot::Bot;
use crate::runner::{Ctx, ARENA_Y};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

const Y: i32 = ARENA_Y + 1;

type Cells = &'static [(i32, i32, i32, &'static str)];

struct Case {
    name: &'static str,
    gm: &'static str,
    item: &'static str,
    setup: Cells,
    /// clicked cell (relative) and face (0 down 1 up 2 north 3 south 4 west 5 east)
    at: [i32; 3],
    face: u8,
    /// outcome random (bone meal): compare only whether the clicked cell changed + the held count
    rand: bool,
    /// use_item along the look ray (buckets) instead of use_item_on
    ray: bool,
    /// how many of the item to give (stacking buckets)
    count: i32,
    /// ray cases: point to look at inside the `at` cell (default: the centre of `face`)
    aim: Option<[f64; 3]>,
}

const fn c(
    name: &'static str,
    gm: &'static str,
    item: &'static str,
    setup: Cells,
    face: u8,
) -> Case {
    Case {
        name,
        gm,
        item,
        setup,
        at: [0, 0, 0],
        face,
        rand: false,
        ray: false,
        count: 4,
        aim: None,
    }
}

const fn r(
    name: &'static str,
    gm: &'static str,
    item: &'static str,
    setup: Cells,
    face: u8,
) -> Case {
    Case {
        name,
        gm,
        item,
        setup,
        at: [0, 0, 0],
        face,
        rand: true,
        ray: false,
        count: 4,
        aim: None,
    }
}

/// A bucket case: look at the centre of face `face` of cell `at` and use_item.
const fn b(
    name: &'static str,
    gm: &'static str,
    item: &'static str,
    count: i32,
    setup: Cells,
    at: [i32; 3],
    face: u8,
) -> Case {
    Case {
        name,
        gm,
        item,
        setup,
        at,
        face,
        rand: false,
        ray: true,
        count,
        aim: None,
    }
}

fn short(s: &str) -> String {
    s.trim_start_matches("minecraft:").to_string()
}

fn sound_name(id: i32) -> String {
    basalt_data::registries::SOUND_EVENT
        .get(id as usize)
        .copied()
        .unwrap_or("?")
        .to_string()
}

fn run_case(ctx: &mut Ctx, bot: &Bot, o: [i32; 3], case: &Case) -> Result<()> {
    let cell = [o[0] + 4, Y, o[2] + 5];
    let abs = |r: [i32; 3]| [cell[0] + r[0], cell[1] + r[1], cell[2] + r[2]];
    ctx.gamemode(bot, case.gm)?;
    bot.cmd_reply(&format!(
        "fill {} {} {} {} {} {} air",
        cell[0] - 2,
        Y,
        cell[2] - 2,
        cell[0] + 2,
        Y + 4,
        cell[2] + 2
    ))?;
    bot.cmd_reply(&format!(
        "fill {} {} {} {} {} {} stone",
        cell[0] - 2,
        Y - 2,
        cell[2] - 2,
        cell[0] + 2,
        Y - 1,
        cell[2] + 2
    ))?;
    bot.cmd("kill @e[type=minecraft:item,distance=..12]");
    // a fresh stack every case (durability / count start from scratch)
    bot.cmd_reply("clear @s")?;
    bot.wait_ticks(4);
    for (dx, dy, dz, b) in case.setup {
        let p = abs([*dx, *dy, *dz]);
        bot.cmd_reply(&format!("setblock {} {} {} {b}", p[0], p[1], p[2]))?;
    }
    give(bot, case.item, case.count)?;
    hold(bot, case.item)?;
    bot.wait_ticks(6);
    let region: Vec<[i32; 3]> = (-2..=2)
        .flat_map(|dx| (-1..=3).flat_map(move |dy| (-2..=2).map(move |dz| [dx, dy, dz])))
        .collect();
    let before: Vec<String> = {
        let s = bot.st();
        region
            .iter()
            .map(|r| {
                let a = abs(*r);
                s.world.state_string(a[0], a[1], a[2])
            })
            .collect()
    };
    let t0 = Instant::now();
    if case.ray {
        let a = abs(case.at);
        let f = [
            [0.0, -0.49, 0.0],
            [0.0, 0.49, 0.0],
            [0.0, 0.0, -0.49],
            [0.0, 0.0, 0.49],
            [-0.49, 0.0, 0.0],
            [0.49, 0.0, 0.0],
        ][case.face as usize];
        let aim = case.aim.unwrap_or([0.5 + f[0], 0.5 + f[1], 0.5 + f[2]]);
        bot.look_at([
            a[0] as f64 + aim[0],
            a[1] as f64 + aim[1],
            a[2] as f64 + aim[2],
        ]);
        bot.wait_ticks(3);
        bot.use_item();
    } else {
        bot.place(abs(case.at), case.face);
    }
    // fluids start flowing 5 ticks after placement: look earlier for bucket cases
    let snap = if case.ray { 2 } else { 4 };
    // cell states a few ticks after the click (later random ticks may dry farmland out)
    let mut early: Option<Vec<String>> = None;
    let mut seen: BTreeMap<i32, (String, i32)> = BTreeMap::new();
    let mut where_: Vec<String> = Vec::new();
    for tick in 0..20u32 {
        bot.wait_ticks(1);
        let s = bot.st();
        if tick == snap {
            early = Some(
                region
                    .iter()
                    .map(|r| {
                        let a = abs(*r);
                        s.world.state_string(a[0], a[1], a[2])
                    })
                    .collect(),
            );
        }
        for (id, e) in s
            .entities
            .iter()
            .filter(|(_, e)| e.kind == "item" && e.added >= t0)
        {
            if let Some(i) = &e.item {
                if !seen.contains_key(id) {
                    where_.push(format!(
                        "({:.2},{:.2},{:.2}) v({:.3},{:.3},{:.3})",
                        e.pos[0] - cell[0] as f64,
                        e.pos[1] - cell[1] as f64,
                        e.pos[2] - cell[2] as f64,
                        e.vel[0],
                        e.vel[1],
                        e.vel[2]
                    ));
                }
                seen.entry(*id)
                    .or_insert_with(|| (i.name().to_string(), i.count));
            }
        }
    }
    let s = bot.st();
    let mut items: BTreeMap<String, i32> = BTreeMap::new();
    for (name, n) in seen.values() {
        *items.entry(name.clone()).or_default() += n;
    }
    let held = s.held_stack().clone();
    let sounds: BTreeSet<String> = s
        .sounds
        .iter()
        .filter(|x| x.0 >= t0)
        .map(|x| sound_name(x.1))
        .collect();
    let events: BTreeSet<i32> = s
        .level_events
        .iter()
        .filter(|x| x.0 >= t0)
        .map(|x| x.1)
        .collect();
    let event_data: Vec<(i32, i32)> = s
        .level_events
        .iter()
        .filter(|x| x.0 >= t0)
        .map(|x| (x.1, x.3))
        .collect();
    let mut desc = Vec::new();
    let early = early.unwrap_or_default();
    for (i, r) in region.iter().enumerate() {
        let now = early[i].clone();
        // ray cases: fluids set up by the case flow on vanilla (Basalt has no fluid ticks yet):
        // compare only the clicked cell and the one in front of its face
        let fo = [
            [0, -1, 0],
            [0, 1, 0],
            [0, 0, -1],
            [0, 0, 1],
            [-1, 0, 0],
            [1, 0, 0],
        ][case.face as usize];
        let front = [case.at[0] + fo[0], case.at[1] + fo[1], case.at[2] + fo[2]];
        let compared = !case.ray || *r == case.at || *r == front;
        if now != before[i] {
            desc.push(format!("{},{},{}->{}", r[0], r[1], r[2], short(&now)));
            if !case.rand && compared {
                ctx.metric(
                    &format!("{}/{},{},{}:{}", case.name, r[0], r[1], r[2], short(&now)),
                    1.0,
                    0.0,
                );
            }
        }
    }
    let held_count = if held.name() == case.item {
        held.count
    } else {
        0
    };
    ctx.metric(&format!("{}/held_count", case.name), held_count as f64, 0.0);
    if case.ray || case.item.ends_with("bucket") {
        ctx.metric(
            &format!("{}/held:{}", case.name, held.name()),
            held.count as f64,
            0.0,
        );
        for k in [
            "bucket",
            "water_bucket",
            "lava_bucket",
            "powder_snow_bucket",
        ] {
            ctx.metric(&format!("{}/inv:{k}", case.name), s.count(k) as f64, 0.0);
        }
    }
    if case.rand {
        // growth itself is random (a sapling advances 45 % of the time): compare the application
        ctx.metric(
            &format!("{}/applied", case.name),
            events.contains(&1505) as i32 as f64,
            0.0,
        );
    } else {
        ctx.metric(
            &format!("{}/held_damage", case.name),
            held.damage as f64,
            0.0,
        );
        for (k, v) in &items {
            ctx.metric(&format!("{}/item:{k}", case.name), *v as f64, 0.0);
        }
        for e in &events {
            ctx.metric(&format!("{}/level_event:{e}", case.name), 1.0, 0.0);
        }
    }
    ctx.note(format!(
        "{}: {} | items {:?} {} | held {}x{} dmg {} | sounds {:?} | events {:?}",
        case.name,
        desc.join("; "),
        items,
        where_.join(" "),
        held.name(),
        held.count,
        held.damage,
        sounds,
        event_data
    ));
    Ok(())
}

fn run_cases(ctx: &mut Ctx, cases: &[Case]) -> Result<()> {
    let bot = ctx.bot()?;
    let o = ctx.arena(&bot, 10, 10, 6)?;
    ctx.stand(&bot, o[0] + 2, o[2] + 2, "survival")?;
    let filter = std::env::var("TOOLS_CASE").ok();
    for gm in ["survival", "creative"] {
        for case in cases.iter().filter(|k| k.gm == gm) {
            if let Some(f) = &filter {
                if !f.split(',').any(|f| case.name.contains(f)) {
                    continue;
                }
            }
            if let Err(e) = run_case(ctx, &bot, o, case) {
                ctx.check(false, format!("{}: {e:#}", case.name));
            }
        }
    }
    Ok(())
}

const UP: u8 = 1;
const NORTH: u8 = 2;
const DOWN: u8 = 0;

const GRASS: Cells = &[(0, 0, 0, "grass_block")];
const DIRT: Cells = &[(0, 0, 0, "dirt")];
const COARSE: Cells = &[(0, 0, 0, "coarse_dirt")];
const ROOTED: Cells = &[(0, 0, 0, "rooted_dirt")];
const PATH: Cells = &[(0, 0, 0, "dirt_path")];
const PODZOL: Cells = &[(0, 0, 0, "podzol")];
const MYCELIUM: Cells = &[(0, 0, 0, "mycelium")];
const FARMLAND: Cells = &[(0, 0, 0, "farmland")];
const GRASS_COVERED: Cells = &[(0, 0, 0, "grass_block"), (0, 1, 0, "stone")];
const GRASS_FLOWER: Cells = &[(0, 0, 0, "grass_block"), (0, 1, 0, "poppy")];
const GRASS_SHORT: Cells = &[(0, 0, 0, "grass_block"), (0, 1, 0, "short_grass")];
const CAMPFIRE: Cells = &[(0, 0, 0, "campfire[lit=true]")];
const SOUL_CAMPFIRE: Cells = &[(0, 0, 0, "soul_campfire[lit=true]")];
const OAK_LOG_X: Cells = &[(0, 0, 0, "oak_log[axis=x]")];
const OAK_WOOD: Cells = &[(0, 0, 0, "oak_wood")];
const CRIMSON: Cells = &[(0, 0, 0, "crimson_stem[axis=z]")];
const BAMBOO: Cells = &[(0, 0, 0, "bamboo_block")];
const STRIPPED: Cells = &[(0, 0, 0, "stripped_oak_log")];
const EXPOSED: Cells = &[(0, 0, 0, "exposed_copper")];
const OXIDIZED_STAIRS: Cells = &[(0, 0, 0, "oxidized_cut_copper_stairs[facing=east,half=top]")];
const WAXED: Cells = &[(0, 0, 0, "waxed_weathered_copper")];
const COPPER: Cells = &[(0, 0, 0, "copper_block")];
const WAXED_DOOR: Cells = &[
    (0, 0, 0, "waxed_copper_door[half=lower,facing=south]"),
    (0, 1, 0, "waxed_copper_door[half=upper,facing=south]"),
];
const PUMPKIN: Cells = &[(0, 0, 0, "pumpkin")];
const CARVED: Cells = &[(0, 0, 0, "carved_pumpkin")];
const WHEAT: Cells = &[(0, -1, 0, "farmland"), (0, 0, 0, "wheat[age=0]")];
const WHEAT_RIPE: Cells = &[(0, -1, 0, "farmland"), (0, 0, 0, "wheat[age=7]")];
const SAPLING: Cells = &[(0, -1, 0, "grass_block"), (0, 0, 0, "oak_sapling")];
const STONE: Cells = &[(0, 0, 0, "stone")];

static CASES: &[Case] = &[
    // hoe
    c("till_grass", "survival", "wooden_hoe", GRASS, UP),
    c("till_grass_side", "survival", "wooden_hoe", GRASS, NORTH),
    c("till_grass_bottom", "survival", "wooden_hoe", GRASS, DOWN),
    c("till_dirt", "survival", "wooden_hoe", DIRT, UP),
    c("till_coarse", "survival", "wooden_hoe", COARSE, UP),
    c("till_rooted", "survival", "wooden_hoe", ROOTED, UP),
    c("till_path", "survival", "wooden_hoe", PATH, UP),
    c("till_podzol", "survival", "wooden_hoe", PODZOL, UP),
    c("till_mycelium", "survival", "wooden_hoe", MYCELIUM, UP),
    c("till_covered", "survival", "wooden_hoe", GRASS_COVERED, UP),
    c(
        "till_under_flower",
        "survival",
        "wooden_hoe",
        GRASS_FLOWER,
        UP,
    ),
    c(
        "till_under_grass",
        "survival",
        "wooden_hoe",
        GRASS_SHORT,
        UP,
    ),
    c("till_stone", "survival", "wooden_hoe", STONE, UP),
    // shovel
    c("path_grass", "survival", "wooden_shovel", GRASS, UP),
    c("path_grass_side", "survival", "wooden_shovel", GRASS, NORTH),
    c(
        "path_grass_bottom",
        "survival",
        "wooden_shovel",
        GRASS,
        DOWN,
    ),
    c("path_dirt", "survival", "wooden_shovel", DIRT, UP),
    c("path_coarse", "survival", "wooden_shovel", COARSE, UP),
    c("path_rooted", "survival", "wooden_shovel", ROOTED, UP),
    c("path_podzol", "survival", "wooden_shovel", PODZOL, UP),
    c("path_mycelium", "survival", "wooden_shovel", MYCELIUM, UP),
    c("path_farmland", "survival", "wooden_shovel", FARMLAND, UP),
    c(
        "path_covered",
        "survival",
        "wooden_shovel",
        GRASS_COVERED,
        UP,
    ),
    c(
        "path_under_flower",
        "survival",
        "wooden_shovel",
        GRASS_FLOWER,
        UP,
    ),
    c("path_campfire", "survival", "wooden_shovel", CAMPFIRE, UP),
    c(
        "path_soul_campfire",
        "survival",
        "wooden_shovel",
        SOUL_CAMPFIRE,
        NORTH,
    ),
    // axe
    c("strip_log", "survival", "wooden_axe", OAK_LOG_X, UP),
    c("strip_wood", "survival", "wooden_axe", OAK_WOOD, NORTH),
    c("strip_stem", "survival", "wooden_axe", CRIMSON, UP),
    c("strip_bamboo", "survival", "wooden_axe", BAMBOO, UP),
    c("strip_stripped", "survival", "wooden_axe", STRIPPED, UP),
    c("scrape_exposed", "survival", "wooden_axe", EXPOSED, UP),
    c(
        "scrape_stairs",
        "survival",
        "wooden_axe",
        OXIDIZED_STAIRS,
        NORTH,
    ),
    c("unwax", "survival", "wooden_axe", WAXED, UP),
    c("unwax_door", "survival", "wooden_axe", WAXED_DOOR, NORTH),
    c("axe_copper", "survival", "wooden_axe", COPPER, UP),
    c("axe_stone", "survival", "wooden_axe", STONE, UP),
    // shears / honeycomb
    c("carve_pumpkin", "survival", "shears", PUMPKIN, NORTH),
    c("carve_pumpkin_top", "survival", "shears", PUMPKIN, UP),
    c("carve_carved", "survival", "shears", CARVED, NORTH),
    c("wax_copper", "survival", "honeycomb", COPPER, UP),
    c("wax_stairs", "survival", "honeycomb", OXIDIZED_STAIRS, UP),
    c("wax_waxed", "survival", "honeycomb", WAXED, UP),
    // bone meal
    r("bonemeal_wheat", "survival", "bone_meal", WHEAT, UP),
    c(
        "bonemeal_wheat_ripe",
        "survival",
        "bone_meal",
        WHEAT_RIPE,
        UP,
    ),
    r("bonemeal_sapling", "survival", "bone_meal", SAPLING, UP),
    r("bonemeal_grass", "survival", "bone_meal", GRASS, UP),
    c("bonemeal_stone", "survival", "bone_meal", STONE, UP),
    // creative: tools take no damage, items aren't used up
    c("till_grass_creative", "creative", "wooden_hoe", GRASS, UP),
    c(
        "strip_log_creative",
        "creative",
        "wooden_axe",
        OAK_LOG_X,
        UP,
    ),
    c("wax_copper_creative", "creative", "honeycomb", COPPER, UP),
    r(
        "bonemeal_wheat_creative",
        "creative",
        "bone_meal",
        WHEAT,
        UP,
    ),
];

const WATER: Cells = &[(0, 0, 0, "water")];
const LAVA: Cells = &[(0, 0, 0, "lava")];
const FLOWING: Cells = &[(0, 0, 0, "water[level=3]")];
const SLAB_WET: Cells = &[(0, 0, 0, "oak_slab[waterlogged=true]")];
const SLAB: Cells = &[(0, 0, 0, "oak_slab")];
const POWDER: Cells = &[(0, 0, 0, "powder_snow")];
const SNOW_LAYER: Cells = &[(0, 0, 0, "snow[layers=1]")];
const NONE: Cells = &[];
const FLOOR: [i32; 3] = [0, -1, 0];
const AT: [i32; 3] = [0, 0, 0];

static BUCKETS: &[Case] = &[
    b("fill_water", "survival", "bucket", 1, WATER, AT, UP),
    b("fill_water_stack", "survival", "bucket", 2, WATER, AT, UP),
    b("fill_lava", "survival", "bucket", 1, LAVA, AT, UP),
    b("fill_flowing", "survival", "bucket", 1, FLOWING, AT, UP),
    b(
        "fill_waterlogged",
        "survival",
        "bucket",
        1,
        SLAB_WET,
        AT,
        UP,
    ),
    b("fill_powder_snow", "survival", "bucket", 1, POWDER, AT, UP),
    b("fill_stone", "survival", "bucket", 1, STONE, AT, UP),
    b(
        "empty_water_floor",
        "survival",
        "water_bucket",
        1,
        NONE,
        FLOOR,
        UP,
    ),
    b(
        "empty_water_side",
        "survival",
        "water_bucket",
        1,
        STONE,
        AT,
        NORTH,
    ),
    b(
        "empty_water_slab",
        "survival",
        "water_bucket",
        1,
        SLAB,
        AT,
        UP,
    ),
    b(
        "empty_water_snow",
        "survival",
        "water_bucket",
        1,
        SNOW_LAYER,
        AT,
        UP,
    ),
    b(
        "empty_water_on_water",
        "survival",
        "water_bucket",
        1,
        WATER,
        FLOOR,
        UP,
    ),
    Case {
        aim: Some([0.5, 0.49, 0.5]),
        ..b(
            "empty_water_slab_surface",
            "survival",
            "water_bucket",
            1,
            SLAB,
            AT,
            UP,
        )
    },
    Case {
        aim: Some([0.5, 0.12, 0.5]),
        ..b(
            "empty_water_snow_surface",
            "survival",
            "water_bucket",
            1,
            SNOW_LAYER,
            AT,
            UP,
        )
    },
    Case {
        aim: Some([0.5, 0.49, 0.5]),
        ..b(
            "empty_lava_slab_surface",
            "survival",
            "lava_bucket",
            1,
            SLAB,
            AT,
            UP,
        )
    },
    b(
        "empty_lava_floor",
        "survival",
        "lava_bucket",
        1,
        NONE,
        FLOOR,
        UP,
    ),
    b(
        "empty_lava_slab",
        "survival",
        "lava_bucket",
        1,
        SLAB,
        AT,
        UP,
    ),
    Case {
        at: FLOOR,
        count: 1,
        ..c(
            "place_powder_snow",
            "survival",
            "powder_snow_bucket",
            NONE,
            UP,
        )
    },
    b(
        "fill_water_creative",
        "creative",
        "bucket",
        1,
        WATER,
        AT,
        UP,
    ),
    b(
        "empty_water_creative",
        "creative",
        "water_bucket",
        1,
        NONE,
        FLOOR,
        UP,
    ),
];

/// Buckets: picking up / emptying water, lava and powder snow (use_item along the look ray).
pub fn buckets(ctx: &mut Ctx) -> Result<()> {
    run_cases(ctx, BUCKETS)
}

pub fn tool_actions(ctx: &mut Ctx) -> Result<()> {
    run_cases(ctx, CASES)
}

//! Block dependencies: double blocks (breaking either half removes the other) and support checks
//! (a block whose support goes pops, with its drops). Table driven: each case builds a small
//! structure in one arena cell, performs an action (survival dig, creative dig, /setblock, /fill)
//! and records which cells changed (and to what) plus the item entities that appeared. The
//! metrics are compared against vanilla (--diff); everything was first measured on vanilla 26.2.
//! `BLOCKS_CASE=<substring>` restricts a run to matching cases (notes show cells + tick + items).
use super::util::*;
use crate::bot::Bot;
use crate::runner::{Ctx, ARENA_Y};
use anyhow::Result;
use std::collections::BTreeMap;
use std::time::Instant;

const Y: i32 = ARENA_Y + 1;

type Cells = &'static [(i32, i32, i32, &'static str)];

#[derive(Clone, Copy)]
enum Act {
    /// Dig this cell (relative to the case cell) as the bot's gamemode does.
    Dig([i32; 3]),
    /// `setblock <cell> <block> <mode>`
    Set([i32; 3], &'static str, &'static str),
    /// `fill <a> <b> <block> <mode>`
    Fill([i32; 3], [i32; 3], &'static str, &'static str),
}

struct Case {
    name: &'static str,
    gm: &'static str,
    tool: &'static str,
    /// (dx, dy, dz, block) placed in order; y 0 is the first layer above the stone floor.
    setup: Cells,
    act: Act,
    /// The drop is random (seeds): compare only the total item count with slack.
    rand: bool,
}

const fn c(name: &'static str, gm: &'static str, tool: &'static str, setup: Cells, act: Act) -> Case {
    Case { name, gm, tool, setup, act, rand: false }
}

const fn r(name: &'static str, gm: &'static str, tool: &'static str, setup: Cells, act: Act) -> Case {
    Case { name, gm, tool, setup, act, rand: true }
}

fn short(s: &str) -> String { s.trim_start_matches("minecraft:").to_string() }

fn run_case(ctx: &mut Ctx, bot: &Bot, o: [i32; 3], case: &Case) -> Result<()> {
    let cell = [o[0] + 4, Y, o[2] + 5];
    let abs = |r: [i32; 3]| [cell[0] + r[0], cell[1] + r[1], cell[2] + r[2]];
    ctx.gamemode(bot, case.gm)?;
    // clean the cell (floor back to stone), wait for any pops to settle
    bot.cmd_reply(&format!("fill {} {} {} {} {} {} air", cell[0] - 2, Y - 1, cell[2] - 2, cell[0] + 2, Y + 5, cell[2] + 2))?;
    bot.cmd_reply(&format!("fill {} {} {} {} {} {} stone", cell[0] - 2, Y - 2, cell[2] - 2, cell[0] + 2, Y - 1, cell[2] + 2))?;
    bot.cmd(&format!("kill @e[type=minecraft:item,distance=..12]"));
    bot.wait_ticks(6);
    for (dx, dy, dz, b) in case.setup {
        let p = abs([*dx, *dy, *dz]);
        bot.cmd_reply(&format!("setblock {} {} {} {b}", p[0], p[1], p[2]))?;
    }
    bot.wait_ticks(8);
    if case.gm == "survival" {
        if case.tool == "hand" { bot.set_held(8); bot.wait_ticks(2); }
        else {
            if bot.st().count(case.tool) == 0 { give(bot, case.tool, 1)?; }
            hold(bot, case.tool)?;
        }
    }
    let region: Vec<[i32; 3]> = (-2..=2).flat_map(|dx| (-1..=5).flat_map(move |dy| (-2..=2).map(move |dz| [dx, dy, dz]))).collect();
    let before: Vec<String> = { let s = bot.st(); region.iter().map(|r| { let a = abs(*r); s.world.state_string(a[0], a[1], a[2]) }).collect() };
    let t0 = Instant::now();
    match case.act {
        Act::Dig(r) => { bot.dig(abs(r))?; }
        Act::Set(r, b, m) => { let a = abs(r); bot.cmd_reply(&format!("setblock {} {} {} {b} {m}", a[0], a[1], a[2]))?; }
        Act::Fill(r, r2, b, m) => { let (a, d) = (abs(r), abs(r2)); bot.cmd_reply(&format!("fill {} {} {} {} {} {} {b} {m}", a[0], a[1], a[2], d[0], d[1], d[2]))?; }
    }
    let mut when: BTreeMap<usize, u32> = BTreeMap::new();
    let mut seen: BTreeMap<i32, String> = BTreeMap::new();
    let mut seen_items: Vec<(String, i32)> = Vec::new();
    for tick in 0..30u32 {
        bot.wait_ticks(1);
        let s = bot.st();
        for (id, e) in s.entities.iter().filter(|(_, e)| e.kind == "item" && e.added >= t0) {
            if let Some(i) = &e.item { if !seen.contains_key(id) { seen_items.push((i.name().to_string(), i.count)); } seen.entry(*id).or_insert_with(|| format!("{}x{}@{} ({:.1},{:.1},{:.1})", i.name(), i.count, tick, e.pos[0] - cell[0] as f64, e.pos[1] - cell[1] as f64, e.pos[2] - cell[2] as f64)); }
        }
        for (i, r) in region.iter().enumerate() {
            let a = abs(*r);
            if !when.contains_key(&i) && s.world.state_string(a[0], a[1], a[2]) != before[i] { when.insert(i, tick); }
        }
    }
    let s = bot.st();
    // counted when first seen: items merge later, and a merge must not change the total
    let mut items: BTreeMap<String, i32> = BTreeMap::new();
    for (name, n) in &seen_items { *items.entry(name.clone()).or_default() += n; }
    let mut desc = Vec::new();
    for (i, r) in region.iter().enumerate() {
        let a = abs(*r);
        let now = s.world.state_string(a[0], a[1], a[2]);
        // water-adjacent cases: flowing water refills vacated cells on its own (Basalt has no fluid ticking
        // yet), so only "became air or water" is compared there
        let wet = case.setup.iter().any(|(_, _, _, b)| *b == "water");
        let norm = |st: &str| if wet && (st.starts_with("minecraft:water") || st == "minecraft:air") { "fluid".to_string() } else { short(st) };
        let (nb, nn) = (norm(&before[i]), norm(&now));
        if nb != nn {
            let key = format!("{}/{},{},{}:{}", case.name, r[0], r[1], r[2], nn);
            ctx.metric(&key, 1.0, 0.0);
            desc.push(format!("{},{},{}->{} @{}", r[0], r[1], r[2], short(&now), when.get(&i).copied().unwrap_or(0)));
        }
    }
    if case.rand {
        let total: i32 = items.values().sum();
        ctx.metric(&format!("{}/items_total", case.name), total as f64, 4.0);
    } else {
        for (k, v) in &items { ctx.metric(&format!("{}/item:{k}", case.name), *v as f64, 0.0); }
    }
    ctx.note(format!("{}: {} | items {:?} | {}", case.name, desc.join("; "), items, seen.values().cloned().collect::<Vec<_>>().join(", ")));
    Ok(())
}

fn run_cases(ctx: &mut Ctx, cases: &[Case]) -> Result<()> {
    let bot = ctx.bot()?;
    let o = ctx.arena(&bot, 10, 10, 8)?;
    ctx.stand(&bot, o[0] + 2, o[2] + 2, "survival")?;
    let filter = std::env::var("BLOCKS_CASE").ok();
    // survival cases first, then creative (fewer game mode switches)
    for gm in ["survival", "creative"] {
        for case in cases.iter().filter(|k| k.gm == gm) {
            if let Some(f) = &filter { if !f.split(',').any(|f| case.name.contains(f)) { continue; } }
            if let Err(e) = run_case(ctx, &bot, o, case) { ctx.check(false, format!("{}: {e:#}", case.name)); }
        }
    }
    Ok(())
}

// ---- double blocks ---------------------------------------------------------------------------

const SUN: Cells = &[(0, -1, 0, "grass_block"), (0, 0, 0, "sunflower[half=lower]"), (0, 1, 0, "sunflower[half=upper]")];
const LILAC: Cells = &[(0, -1, 0, "grass_block"), (0, 0, 0, "lilac[half=lower]"), (0, 1, 0, "lilac[half=upper]")];
const ROSE: Cells = &[(0, -1, 0, "grass_block"), (0, 0, 0, "rose_bush[half=lower]"), (0, 1, 0, "rose_bush[half=upper]")];
const PEONY: Cells = &[(0, -1, 0, "grass_block"), (0, 0, 0, "peony[half=lower]"), (0, 1, 0, "peony[half=upper]")];
const TGRASS: Cells = &[(0, -1, 0, "grass_block"), (0, 0, 0, "tall_grass[half=lower]"), (0, 1, 0, "tall_grass[half=upper]")];
const LFERN: Cells = &[(0, -1, 0, "grass_block"), (0, 0, 0, "large_fern[half=lower]"), (0, 1, 0, "large_fern[half=upper]")];
const PITCHER: Cells = &[(0, -1, 0, "grass_block"), (0, 0, 0, "pitcher_plant[half=lower]"), (0, 1, 0, "pitcher_plant[half=upper]")];
const SDRIP: Cells = &[(0, -1, 0, "clay"), (0, 0, 0, "small_dripleaf[half=lower,facing=north]"), (0, 1, 0, "small_dripleaf[half=upper,facing=north]")];
const SEAGRASS: Cells = &[
    (-1, 0, -1, "stone"), (-1, 0, 0, "stone"), (-1, 0, 1, "stone"), (1, 0, -1, "stone"), (1, 0, 0, "stone"), (1, 0, 1, "stone"), (0, 0, -1, "stone"), (0, 0, 1, "stone"),
    (-1, 1, -1, "stone"), (-1, 1, 0, "stone"), (-1, 1, 1, "stone"), (1, 1, -1, "stone"), (1, 1, 0, "stone"), (1, 1, 1, "stone"), (0, 1, -1, "stone"), (0, 1, 1, "stone"),
    (0, 2, 0, "water"),
    (0, -1, 0, "sand"), (0, 0, 0, "tall_seagrass[half=lower]"), (0, 1, 0, "tall_seagrass[half=upper]"),
];
const DOOR: Cells = &[(0, 0, 0, "oak_door[half=lower,facing=north]"), (0, 1, 0, "oak_door[half=upper,facing=north]")];
const DOOR_DIRT: Cells = &[(0, -1, 0, "dirt"), (0, 0, 0, "oak_door[half=lower,facing=north]"), (0, 1, 0, "oak_door[half=upper,facing=north]")];
const IDOOR: Cells = &[(0, 0, 0, "iron_door[half=lower,facing=north]"), (0, 1, 0, "iron_door[half=upper,facing=north]")];
const DRIP: Cells = &[(0, -1, 0, "clay"), (0, 0, 0, "big_dripleaf_stem[facing=north]"), (0, 1, 0, "big_dripleaf_stem[facing=north]"), (0, 2, 0, "big_dripleaf[facing=north]")];

const LO: [i32; 3] = [0, 0, 0];
const UP: [i32; 3] = [0, 1, 0];
const FLOOR: [i32; 3] = [0, -1, 0];

fn double_cases() -> Vec<Case> {
    let mut v = vec![
        c("sunflower_lower_hand", "survival", "hand", SUN, Act::Dig(LO)),
        c("sunflower_upper_hand", "survival", "hand", SUN, Act::Dig(UP)),
        c("lilac_lower_hand", "survival", "hand", LILAC, Act::Dig(LO)),
        c("lilac_upper_hand", "survival", "hand", LILAC, Act::Dig(UP)),
        c("rose_bush_lower_hand", "survival", "hand", ROSE, Act::Dig(LO)),
        c("rose_bush_upper_hand", "survival", "hand", ROSE, Act::Dig(UP)),
        c("peony_lower_hand", "survival", "hand", PEONY, Act::Dig(LO)),
        c("peony_upper_hand", "survival", "hand", PEONY, Act::Dig(UP)),
        c("tall_grass_lower_shears", "survival", "shears", TGRASS, Act::Dig(LO)),
        c("tall_grass_upper_shears", "survival", "shears", TGRASS, Act::Dig(UP)),
        c("large_fern_lower_shears", "survival", "shears", LFERN, Act::Dig(LO)),
        c("large_fern_upper_shears", "survival", "shears", LFERN, Act::Dig(UP)),
        c("pitcher_lower_hand", "survival", "hand", PITCHER, Act::Dig(LO)),
        c("pitcher_upper_hand", "survival", "hand", PITCHER, Act::Dig(UP)),
        c("small_dripleaf_lower_shears", "survival", "shears", SDRIP, Act::Dig(LO)),
        c("small_dripleaf_upper_shears", "survival", "shears", SDRIP, Act::Dig(UP)),
        c("small_dripleaf_lower_hand", "survival", "hand", SDRIP, Act::Dig(LO)),
        c("tall_seagrass_upper_shears", "survival", "shears", SEAGRASS, Act::Dig(UP)),
        c("door_lower_hand", "survival", "hand", DOOR, Act::Dig(LO)),
        c("door_upper_hand", "survival", "hand", DOOR, Act::Dig(UP)),
        c("iron_door_lower_pick", "survival", "diamond_pickaxe", IDOOR, Act::Dig(LO)),
        c("iron_door_upper_pick", "survival", "diamond_pickaxe", IDOOR, Act::Dig(UP)),
        c("iron_door_upper_hand", "survival", "hand", IDOOR, Act::Dig(UP)),
        c("big_dripleaf_leaf_shears", "survival", "shears", DRIP, Act::Dig([0, 2, 0])),
        // commands: setblock / fill replace and destroy
        c("sunflower_set_air_lower", "survival", "hand", SUN, Act::Set(LO, "air", "replace")),
        c("sunflower_set_air_upper", "survival", "hand", SUN, Act::Set(UP, "air", "replace")),
        c("sunflower_set_destroy_lower", "survival", "hand", SUN, Act::Set(LO, "air", "destroy")),
        c("sunflower_set_destroy_upper", "survival", "hand", SUN, Act::Set(UP, "air", "destroy")),
        c("sunflower_fill_air_lower", "survival", "hand", SUN, Act::Fill(LO, LO, "air", "replace")),
        c("sunflower_fill_destroy_lower", "survival", "hand", SUN, Act::Fill(LO, LO, "air", "destroy")),
        c("sunflower_fill_air_both", "survival", "hand", SUN, Act::Fill(LO, UP, "air", "replace")),
        c("door_set_air_lower", "survival", "hand", DOOR, Act::Set(LO, "air", "replace")),
        c("door_set_destroy_upper", "survival", "hand", DOOR, Act::Set(UP, "air", "destroy")),
        c("sunflower_support_dug", "survival", "hand", SUN, Act::Dig(FLOOR)),
        c("door_support_dug", "survival", "hand", DOOR_DIRT, Act::Dig(FLOOR)),
        // creative: no drops
        c("sunflower_lower_creative", "creative", "hand", SUN, Act::Dig(LO)),
        c("sunflower_upper_creative", "creative", "hand", SUN, Act::Dig(UP)),
        c("tall_grass_upper_creative", "creative", "hand", TGRASS, Act::Dig(UP)),
        c("door_lower_creative", "creative", "hand", DOOR, Act::Dig(LO)),
        c("door_upper_creative", "creative", "hand", DOOR, Act::Dig(UP)),
    ];
    // tall grass by hand drops seeds at random
    for (name, set, act) in [("tall_grass_lower_hand", TGRASS, Act::Dig(LO)), ("tall_grass_upper_hand", TGRASS, Act::Dig(UP)), ("large_fern_lower_hand", LFERN, Act::Dig(LO))] {
        v.push(Case { name, gm: "survival", tool: "hand", setup: set, act, rand: true });
    }
    v
}

pub fn double_block_break(ctx: &mut Ctx) -> Result<()> { run_cases(ctx, &double_cases()) }

// ---- support checks --------------------------------------------------------------------------

pub fn support_break(ctx: &mut Ctx) -> Result<()> { run_cases(ctx, &support_cases()) }

const D: [i32; 3] = [0, -1, 0]; // floor cell
const S: [i32; 3] = [0, 0, 1]; // wall support cell (south of the block)
const T: [i32; 3] = [0, 1, 0];
const CANE3: Cells = &[(1, -1, 0, "water"), (0, -1, 0, "sand"), (0, 0, 0, "sugar_cane"), (0, 1, 0, "sugar_cane"), (0, 2, 0, "sugar_cane")];

fn support_cases() -> Vec<Case> {
    const PICK: &str = "diamond_pickaxe";
    vec![
        // vegetation on dirt-like blocks
        c("poppy_grass_dug", "survival", "hand", &[(0, -1, 0, "grass_block"), (0, 0, 0, "poppy")], Act::Dig(D)),
        c("dandelion_dirt_dug", "survival", "hand", &[(0, -1, 0, "dirt"), (0, 0, 0, "dandelion")], Act::Dig(D)),
        c("sapling_dirt_dug", "survival", "hand", &[(0, -1, 0, "dirt"), (0, 0, 0, "oak_sapling[stage=0]")], Act::Dig(D)),
        r("short_grass_dug", "survival", "hand", &[(0, -1, 0, "grass_block"), (0, 0, 0, "short_grass")], Act::Dig(D)),
        r("fern_dug", "survival", "hand", &[(0, -1, 0, "grass_block"), (0, 0, 0, "fern")], Act::Dig(D)),
        r("dead_bush_sand_dug", "survival", "hand", &[(0, -1, 0, "sand"), (0, 0, 0, "dead_bush")], Act::Dig(D)),
        c("brown_mushroom_dug", "survival", "hand", &[(0, -1, 0, "dirt"), (0, 0, 0, "brown_mushroom")], Act::Dig(D)),
        r("wheat_dug", "survival", "hand", &[(0, -1, 0, "farmland[moisture=7]"), (0, 0, 0, "wheat[age=7]")], Act::Dig(D)),
        c("snow_dirt_dug", "survival", "hand", &[(0, -1, 0, "dirt"), (0, 0, 0, "snow[layers=1]")], Act::Dig(D)),
        c("snow3_dirt_dug", "survival", "hand", &[(0, -1, 0, "dirt"), (0, 0, 0, "snow[layers=3]")], Act::Dig(D)),
        // torches
        c("torch_floor_dug", "survival", PICK, &[(0, 0, 0, "torch")], Act::Dig(D)),
        c("wall_torch_dug", "survival", PICK, &[(0, 0, 1, "stone"), (0, 0, 0, "wall_torch[facing=north]")], Act::Dig(S)),
        c("redstone_torch_dug", "survival", PICK, &[(0, 0, 0, "redstone_torch[lit=true]")], Act::Dig(D)),
        c("redstone_wall_torch_dug", "survival", PICK, &[(0, 0, 1, "stone"), (0, 0, 0, "redstone_wall_torch[facing=north,lit=true]")], Act::Dig(S)),
        c("torch_and_wall_torch_dug", "survival", "hand", &[(0, 0, 0, "dirt"), (0, 1, 0, "torch"), (1, 0, 0, "wall_torch[facing=east]")], Act::Dig(LO)),
        // flat things
        c("carpet_dug", "survival", PICK, &[(0, 0, 0, "white_carpet")], Act::Dig(D)),
        c("rail_dug", "survival", PICK, &[(0, 0, 0, "rail[shape=north_south]")], Act::Dig(D)),
        c("powered_rail_dug", "survival", PICK, &[(0, 0, 0, "powered_rail[shape=north_south,powered=false]")], Act::Dig(D)),
        c("redstone_wire_dug", "survival", PICK, &[(0, 0, 0, "redstone_wire")], Act::Dig(D)),
        c("pressure_plate_dug", "survival", PICK, &[(0, 0, 0, "stone_pressure_plate")], Act::Dig(D)),
        // signs
        c("sign_standing_dug", "survival", PICK, &[(0, 0, 0, "oak_sign[rotation=0]")], Act::Dig(D)),
        c("sign_wall_dug", "survival", PICK, &[(0, 0, 1, "stone"), (0, 0, 0, "oak_wall_sign[facing=north]")], Act::Dig(S)),
        c("sign_hanging_dug", "survival", PICK, &[(0, 1, 0, "stone"), (0, 0, 0, "oak_hanging_sign[attached=false,rotation=0]")], Act::Dig(T)),
        // buttons / levers
        c("button_floor_dug", "survival", PICK, &[(0, 0, 0, "stone_button[face=floor,facing=north]")], Act::Dig(D)),
        c("button_wall_dug", "survival", PICK, &[(0, 0, 1, "stone"), (0, 0, 0, "stone_button[face=wall,facing=north]")], Act::Dig(S)),
        c("button_ceiling_dug", "survival", PICK, &[(0, 1, 0, "stone"), (0, 0, 0, "stone_button[face=ceiling,facing=north]")], Act::Dig(T)),
        c("lever_floor_dug", "survival", PICK, &[(0, 0, 0, "lever[face=floor,facing=north]")], Act::Dig(D)),
        c("lever_wall_dug", "survival", PICK, &[(0, 0, 1, "stone"), (0, 0, 0, "lever[face=wall,facing=north]")], Act::Dig(S)),
        // cactus / sugar cane (cascades)
        c("cactus_dug", "survival", "hand", &[(0, -1, 0, "sand"), (0, 0, 0, "cactus")], Act::Dig(D)),
        c("cactus3_dug", "survival", "hand", &[(0, -1, 0, "sand"), (0, 0, 0, "cactus"), (0, 1, 0, "cactus"), (0, 2, 0, "cactus")], Act::Dig(D)),
        c("cane_dug", "survival", "hand", &[(1, -1, 0, "water"), (0, -1, 0, "sand"), (0, 0, 0, "sugar_cane")], Act::Dig(D)),
        c("cane3_dug", "survival", "hand", CANE3, Act::Dig(D)),
        c("cane3_mid_dug", "survival", "hand", CANE3, Act::Dig(T)),
        // commands
        c("torch_set_air", "survival", "hand", &[(0, 0, 0, "torch")], Act::Set(D, "air", "replace")),
        c("torch_set_destroy", "survival", "hand", &[(0, 0, 0, "torch")], Act::Set(D, "air", "destroy")),
        c("torch_fill_air", "survival", "hand", &[(0, 0, 0, "torch")], Act::Fill(D, D, "air", "replace")),
        c("torch_fill_destroy", "survival", "hand", &[(0, 0, 0, "torch")], Act::Fill(D, D, "air", "destroy")),
        c("poppy_set_air", "survival", "hand", &[(0, -1, 0, "grass_block"), (0, 0, 0, "poppy")], Act::Set(D, "air", "replace")),
        c("poppy_set_destroy", "survival", "hand", &[(0, -1, 0, "grass_block"), (0, 0, 0, "poppy")], Act::Set(D, "air", "destroy")),
        c("poppy_set_water", "survival", "hand", &[(0, -1, 0, "grass_block"), (0, 0, 0, "poppy")], Act::Set(D, "water", "replace")),
        c("carpet_set_air", "survival", "hand", &[(0, 0, 0, "white_carpet")], Act::Set(D, "air", "replace")),
        c("wall_sign_set_air", "survival", "hand", &[(0, 0, 1, "stone"), (0, 0, 0, "oak_wall_sign[facing=north]")], Act::Set(S, "air", "replace")),
        c("button_wall_set_destroy", "survival", "hand", &[(0, 0, 1, "stone"), (0, 0, 0, "stone_button[face=wall,facing=north]")], Act::Set(S, "air", "destroy")),
        c("wire_fill_air", "survival", "hand", &[(0, 0, 0, "redstone_wire")], Act::Fill(D, D, "air", "replace")),
        c("cane3_set_air", "survival", "hand", CANE3, Act::Set(D, "air", "replace")),
        c("cane_water_removed", "survival", "hand", &[(1, -1, 0, "water"), (0, -1, 0, "sand"), (0, 0, 0, "sugar_cane")], Act::Set([1, -1, 0], "air", "replace")),
        // creative: the dependent block still pops (drops measured)
        c("torch_creative_dug", "creative", "hand", &[(0, 0, 0, "torch")], Act::Dig(D)),
        c("poppy_creative_dug", "creative", "hand", &[(0, -1, 0, "grass_block"), (0, 0, 0, "poppy")], Act::Dig(D)),
        c("cane3_creative_dug", "creative", "hand", CANE3, Act::Dig(D)),
        c("sunflower_creative_support", "creative", "hand", SUN, Act::Dig(D)),
    ]
}

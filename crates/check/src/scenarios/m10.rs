//! M10 scenarios: enchanting table, anvil, grindstone, smithing table, brewing stand (vanilla vs Basalt).
use super::util::*;
use crate::bot::Bot;
use crate::fuzz::stack_str;
use crate::runner::{Ctx, ARENA_Y};
use anyhow::{bail, Result};
use std::time::Duration;

const Y: i32 = ARENA_Y + 1;

fn fnv(s: &str) -> f64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    (h & 0xF_FFFF_FFFF_FFFF) as f64
}

fn arena(ctx: &mut Ctx, sx: i32, sz: i32) -> Result<(Bot, [i32; 3])> {
    let bot = ctx.bot()?;
    let o = ctx.arena(&bot, sx, sz, 5)?;
    ctx.stand(&bot, o[0] + 2, o[2] + 2, "survival")?;
    Ok((bot, o))
}

fn open_at(bot: &Bot, p: [i32; 3]) -> Result<()> {
    bot.look_at([p[0] as f64 + 0.5, p[1] as f64 + 0.5, p[2] as f64 + 0.5]);
    bot.wait_ticks(2);
    {
        let mut s = bot.st();
        s.screen = None;
        s.container.clear();
    }
    bot.place(p, 1);
    if !bot.wait_until(Duration::from_secs(3), |s| {
        s.screen.is_some() && !s.container.is_empty()
    }) {
        bail!("station at {p:?} did not open");
    }
    bot.wait_ticks(3);
    Ok(())
}

fn menu(bot: &Bot) -> (String, String, usize) {
    let s = bot.st();
    (
        s.screen.as_ref().map(|x| x.1.clone()).unwrap_or_default(),
        s.screen_title.clone(),
        s.container.len(),
    )
}

/// Give `spec` (e.g. `iron_sword[damage=5]`, count) and move it into container slot `slot` (n_cont container slots).
fn put(
    bot: &Bot,
    n_cont: usize,
    _hotbar: usize,
    slot: usize,
    spec: &str,
    count: i32,
) -> Result<()> {
    bot.cmd_reply(&format!("give @s minecraft:{spec} {count}"))?;
    let name = spec.split('[').next().unwrap_or(spec).to_string();
    // the stack lands in the first free slot (hotbar first): find it by name
    let mut hb = None;
    bot.wait_until(Duration::from_secs(3), |s| {
        hb = (0..9).find(|&i| s.inv[36 + i].name() == name);
        hb.is_some()
    });
    let Some(hb) = hb else {
        bail!("{spec} did not arrive")
    };
    bot.wait_ticks(2);
    bot.click((n_cont + 27 + hb) as i16, 0, 0);
    bot.wait_ticks(2);
    bot.click(slot as i16, 0, 0);
    bot.wait_ticks(2);
    Ok(())
}

fn prop(bot: &Bot, k: i16) -> i16 {
    *bot.st().props.get(&k).unwrap_or(&-999)
}

/// Fresh bot state: empty inventory, levels, closed window.
fn reset(bot: &Bot, levels: i32) -> Result<()> {
    bot.close_container();
    bot.wait_ticks(4);
    bot.cmd_reply("clear @s")?;
    bot.cmd_reply(&format!("xp set @s {levels} levels"))?;
    bot.wait_ticks(3);
    let left: Vec<String> = bot.st().inv[5..]
        .iter()
        .map(stack_str)
        .filter(|x| !x.is_empty())
        .collect();
    if !left.is_empty() {
        bail!("inventory not empty after clear: {left:?}");
    }
    Ok(())
}

/// Menus: type, title key, slot count, initial container_set_data ids for every station.
pub fn menus(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = arena(ctx, 12, 8)?;
    ctx.stand(&bot, o[0] + 5, o[2] + 3, "survival")?;
    let list = [
        ("enchanting_table", 13),
        ("anvil", 8),
        ("grindstone", 15),
        ("smithing_table", 21),
        ("brewing_stand", 11),
    ];
    for (k, (block, _)) in list.iter().enumerate() {
        let p = [o[0] + 1 + 2 * k as i32, Y, o[2] + 5];
        setblock(&bot, p, block)?;
        open_at(&bot, p)?;
        let (m, t, n) = menu(&bot);
        note(
            ctx,
            format!("{block}: menu {m} title {t} slots {n} props {:?}", {
                let mut v: Vec<_> = bot.st().props.iter().map(|(a, b)| (*a, *b)).collect();
                v.sort();
                v
            }),
        );
        ctx.metric(&format!("{block}_menu"), fnv(&m), 0.0);
        ctx.metric(&format!("{block}_title"), fnv(&t), 0.0);
        ctx.metric(&format!("{block}_slots"), n as f64, 0.0);
        ctx.metric(&format!("{block}_nprops"), bot.st().props.len() as f64, 0.0);
        bot.close_container();
        bot.wait_ticks(2);
    }
    Ok(())
}

/// Enchanting table: bookshelf power from the offered costs (power 15 always offers 30 for the top slot), the gap rule,
/// level/lapis consumption and the resulting enchantments.
pub fn enchant_table(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = arena(ctx, 14, 14)?;
    let t = [o[0] + 7, Y, o[2] + 7];
    setblock(&bot, t, "enchanting_table")?;
    bot.tp([o[0] as f64 + 7.5, (ARENA_Y + 1) as f64, o[2] as f64 + 10.5])?;
    bot.wait_on_ground(Duration::from_secs(5));
    // ring positions: (dx, dz) with |dx| == 2 or |dz| == 2, y 0 and 1
    let ring: Vec<(i32, i32)> = (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| (x, z)))
        .filter(|(x, z): &(i32, i32)| x.abs() == 2 || z.abs() == 2)
        .collect();
    let non_corner: Vec<(i32, i32)> = ring
        .iter()
        .copied()
        .filter(|(x, z)| !(x.abs() == 2 && z.abs() == 2))
        .collect();
    let place = |bot: &Bot, list: &[(i32, i32)], n: usize, block: &str| -> Result<()> {
        let mut k = 0;
        for &(dx, dz) in list {
            for dy in 0..=1 {
                if k < n {
                    setblock(bot, [t[0] + dx, t[1] + dy, t[2] + dz], block)?;
                }
                k += 1;
            }
        }
        Ok(())
    };
    let clear_ring = |bot: &Bot| -> Result<()> {
        for &(dx, dz) in &ring {
            for dy in 0..=1 {
                setblock(bot, [t[0] + dx, t[1] + dy, t[2] + dz], "air")?;
            }
        }
        Ok(())
    };
    let cost2 = |ctx: &mut Ctx, label: &str, bot: &Bot| -> Result<(i16, i16, i16)> {
        reset(bot, 30)?;
        bot.cmd_reply("give @s minecraft:diamond_pickaxe")?;
        bot.cmd_reply("give @s minecraft:lapis_lazuli 10")?;
        bot.wait_ticks(3);
        open_at(bot, t)?;
        bot.click(2 + 27, 0, 0);
        bot.wait_ticks(2);
        bot.click(0, 0, 0);
        bot.wait_ticks(3);
        let c = (prop(bot, 0), prop(bot, 1), prop(bot, 2));
        note(
            ctx,
            format!(
                "{label}: costs {c:?} seed {} clue {:?} lvl {:?}",
                prop(bot, 3),
                [prop(bot, 4), prop(bot, 5), prop(bot, 6)],
                [prop(bot, 7), prop(bot, 8), prop(bot, 9)]
            ),
        );
        bot.close_container();
        Ok(c)
    };
    let c0 = cost2(ctx, "no shelves", &bot)?;
    ctx.check(
        c0.0 <= 2 && c0.1 <= 5 && c0.2 <= 8 && c0.2 >= 1,
        format!("costs without shelves {c0:?}"),
    );
    ctx.metric("p0_cost2_le_8", (c0.2 <= 8) as u8 as f64, 0.0);
    // 15 non-corner shelves -> top cost exactly 30
    place(&bot, &non_corner, 15, "bookshelf")?;
    let c15 = cost2(ctx, "15 shelves (non-corner)", &bot)?;
    ctx.metric("p15_cost2", c15.2 as f64, 0.0);
    // 14 non-corner + 1 corner: counts only when corners are valid positions
    clear_ring(&bot)?;
    place(&bot, &non_corner, 14, "bookshelf")?;
    setblock(&bot, [t[0] + 2, t[1], t[2] + 2], "bookshelf")?;
    let cc = cost2(ctx, "14 non-corner + corner", &bot)?;
    ctx.metric("corner_counts", (cc.2 == 30) as u8 as f64, 0.0);
    // a gap blocked by stone hides the shelves behind it: 14 shelves + 1 whose gap is blocked -> not 30
    clear_ring(&bot)?;
    place(&bot, &non_corner, 14, "bookshelf")?;
    setblock(
        &bot,
        [t[0] + non_corner[7].0, t[1] + 1, t[2] + non_corner[7].1],
        "bookshelf",
    )?;
    setblock(&bot, [t[0] + 1, t[1], t[2]], "stone")?;
    let cg = cost2(ctx, "gap blocked (stone east)", &bot)?;
    ctx.metric("gap_blocked_cost2_is_30", (cg.2 == 30) as u8 as f64, 0.0);
    setblock(&bot, [t[0] + 1, t[1], t[2]], "air")?;
    // layer 3 (y+2) does not count
    clear_ring(&bot)?;
    place(&bot, &non_corner, 14, "bookshelf")?;
    setblock(&bot, [t[0] + 2, t[1] + 2, t[2]], "bookshelf")?;
    let cy = cost2(ctx, "14 + one at y+2", &bot)?;
    ctx.metric("y2_counts", (cy.2 == 30) as u8 as f64, 0.0);
    // apply: button 2 with 15 shelves: lapis -3, levels -3 (point total unchanged), item enchanted
    clear_ring(&bot)?;
    place(&bot, &non_corner, 15, "bookshelf")?;
    reset(&bot, 30)?;
    bot.cmd_reply("give @s minecraft:diamond_pickaxe")?;
    bot.cmd_reply("give @s minecraft:lapis_lazuli 10")?;
    bot.wait_ticks(3);
    open_at(&bot, t)?;
    bot.click(2 + 27, 0, 0);
    bot.wait_ticks(2);
    bot.click(0, 0, 0);
    bot.wait_ticks(2);
    bot.click(2 + 27 + 1, 0, 0);
    bot.wait_ticks(2);
    bot.click(1, 0, 0);
    bot.wait_ticks(3);
    let xp0 = bot.st().xp_total;
    let seed0 = prop(&bot, 3);
    bot.button(2);
    bot.wait_ticks(5);
    let (item, lap) = {
        let s = bot.st();
        (s.container[0].clone(), s.container[1].clone())
    };
    note(
        ctx,
        format!(
            "after enchant: item {} lapis {} xp total {} -> {}",
            stack_str(&item),
            stack_str(&lap),
            xp0,
            bot.st().xp_total
        ),
    );
    ctx.check(!item.ench.is_empty(), "item not enchanted");
    ctx.metric(
        "enchants_applied",
        (!item.ench.is_empty()) as u8 as f64,
        0.0,
    );
    ctx.metric("lapis_left", lap.count as f64, 0.0);
    ctx.metric(
        "xp_total_unchanged",
        (bot.st().xp_total == xp0) as u8 as f64,
        0.0,
    );
    ctx.metric("level_after", bot.st().xp_level as f64, 0.0);
    ctx.metric("seed_changed", (prop(&bot, 3) != seed0) as u8 as f64, 0.0);
    // a second click on an enchanted item does nothing
    bot.button(2);
    bot.wait_ticks(3);
    let lap2 = bot.st().container[1].count;
    ctx.metric("second_click_noop", (lap2 == lap.count) as u8 as f64, 0.0);
    bot.close_container();
    Ok(())
}

fn digest_case(bot: &Bot, res: usize) -> String {
    let s = bot.st();
    let r = s.container.get(res).map(stack_str).unwrap_or_default();
    format!("{r}|{}", s.props.get(&0).copied().unwrap_or(-1))
}

/// Anvil cases: result stack + displayed cost for a list of input combinations.
pub fn anvil_cases(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = arena(ctx, 6, 6)?;
    let p = [o[0] + 2, Y, o[2] + 4];
    setblock(&bot, p, "anvil")?;
    #[allow(clippy::type_complexity)]
    let cases: &[(&str, &str, i32, &str, i32, Option<&str>)] = &[
        ("rename", "iron_sword", 1, "", 0, Some("Foo")),
        (
            "repair_ingot",
            "iron_sword[damage=100]",
            1,
            "iron_ingot",
            2,
            None,
        ),
        (
            "repair_one_ingot",
            "iron_sword[damage=100]",
            1,
            "iron_ingot",
            1,
            None,
        ),
        (
            "combine_damaged",
            "iron_sword[damage=100]",
            1,
            "iron_sword[damage=50]",
            1,
            None,
        ),
        (
            "book_sharp3",
            "iron_sword",
            1,
            "enchanted_book[stored_enchantments={sharpness:3}]",
            1,
            None,
        ),
        (
            "books_equal",
            "enchanted_book[stored_enchantments={sharpness:3}]",
            1,
            "enchanted_book[stored_enchantments={sharpness:3}]",
            1,
            None,
        ),
        (
            "books_unequal",
            "enchanted_book[stored_enchantments={sharpness:4}]",
            1,
            "enchanted_book[stored_enchantments={sharpness:2}]",
            1,
            None,
        ),
        (
            "conflict",
            "diamond_sword[enchantments={sharpness:2}]",
            1,
            "enchanted_book[stored_enchantments={smite:2}]",
            1,
            None,
        ),
        (
            "penalty7",
            "iron_sword[repair_cost=7]",
            1,
            "enchanted_book[stored_enchantments={sharpness:1}]",
            1,
            None,
        ),
        (
            "too_expensive",
            "iron_sword[repair_cost=31]",
            1,
            "enchanted_book[stored_enchantments={sharpness:5}]",
            1,
            None,
        ),
        (
            "unbreaking_pick",
            "diamond_pickaxe",
            1,
            "enchanted_book[stored_enchantments={unbreaking:3}]",
            1,
            None,
        ),
        (
            "sword_ench_ench",
            "diamond_sword[enchantments={sharpness:1}]",
            1,
            "diamond_sword[enchantments={sharpness:1}]",
            1,
            None,
        ),
        (
            "unsupported",
            "iron_sword",
            1,
            "enchanted_book[stored_enchantments={power:3}]",
            1,
            None,
        ),
        (
            "rename_penalty",
            "iron_sword[repair_cost=3]",
            1,
            "",
            0,
            Some("Bar"),
        ),
        (
            "rename_named",
            "iron_sword[custom_name='\"Old\"']",
            1,
            "",
            0,
            Some(""),
        ),
        ("different_items", "iron_sword", 1, "iron_pickaxe", 1, None),
        (
            "mending_book",
            "diamond_chestplate",
            1,
            "enchanted_book[stored_enchantments={mending:1}]",
            1,
            None,
        ),
    ];
    for (name, a, ac, b, bc, rn) in cases {
        reset(&bot, 30)?;
        open_at(&bot, p)?;
        put(&bot, 3, 0, 0, a, *ac)?;
        if !b.is_empty() {
            put(&bot, 3, 1, 1, b, *bc)?;
        }
        if let Some(r) = rn {
            bot.rename(r);
            bot.wait_ticks(3);
        }
        bot.wait_ticks(2);
        let d = digest_case(&bot, 2);
        let ins: Vec<String> = bot.st().container.iter().take(3).map(stack_str).collect();
        note(ctx, format!("{name}: {d}  [inputs {ins:?}]"));
        ctx.metric(&format!("anvil_{name}"), fnv(&d), 0.0);
        bot.close_container();
    }
    Ok(())
}

/// Taking an anvil result: levels spent, inputs consumed, material units, anvil damage chance observed over many uses.
pub fn anvil_take(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = arena(ctx, 6, 6)?;
    let p = [o[0] + 2, Y, o[2] + 4];
    setblock(&bot, p, "anvil")?;
    reset(&bot, 20)?;
    open_at(&bot, p)?;
    put(&bot, 3, 0, 0, "iron_sword[damage=100]", 1)?;
    put(&bot, 3, 1, 1, "iron_ingot", 5)?;
    let cost = prop(&bot, 0);
    bot.click(2, 0, 1);
    bot.wait_ticks(4);
    let (c1, level) = {
        let s = bot.st();
        (
            s.container.get(1).map(|x| x.count).unwrap_or(-1),
            s.xp_level,
        )
    };
    note(ctx, format!("cost {cost}, ingots left {c1}, level {level}"));
    ctx.metric("take_cost", cost as f64, 0.0);
    ctx.metric("take_ingots_left", c1 as f64, 0.0);
    ctx.metric("take_level_after", level as f64, 0.0);
    ctx.metric(
        "take_sword_in_inventory",
        (bot.st().count("iron_sword") == 1) as u8 as f64,
        0.0,
    );
    // not enough levels: result cannot be taken
    bot.close_container();
    reset(&bot, 1)?;
    open_at(&bot, p)?;
    put(&bot, 3, 0, 0, "iron_sword", 1)?;
    bot.rename("Poor");
    bot.wait_ticks(3);
    bot.click(2, 0, 1);
    bot.wait_ticks(3);
    ctx.metric(
        "poor_cannot_take",
        (bot.st().count("iron_sword") == 0) as u8 as f64,
        0.0,
    );
    bot.close_container();
    Ok(())
}

pub fn grindstone_cases(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = arena(ctx, 6, 6)?;
    let p = [o[0] + 2, Y, o[2] + 4];
    setblock(&bot, p, "grindstone[face=floor,facing=north]")?;
    let cases: &[(&str, &str, &str)] = &[
        (
            "sword_sharp",
            "diamond_sword[enchantments={sharpness:3,unbreaking:2},damage=10]",
            "",
        ),
        (
            "sword_curse",
            "diamond_sword[enchantments={sharpness:3,vanishing_curse:1}]",
            "",
        ),
        (
            "book",
            "enchanted_book[stored_enchantments={sharpness:3}]",
            "",
        ),
        (
            "book_curse",
            "enchanted_book[stored_enchantments={sharpness:3,binding_curse:1}]",
            "",
        ),
        (
            "two_damaged",
            "iron_pickaxe[damage=100]",
            "iron_pickaxe[damage=50]",
        ),
        (
            "two_enchanted",
            "iron_pickaxe[damage=100,enchantments={efficiency:2}]",
            "iron_pickaxe[damage=50,enchantments={unbreaking:1}]",
        ),
        (
            "different",
            "iron_pickaxe[damage=10]",
            "iron_sword[damage=10]",
        ),
        ("plain_damaged", "iron_pickaxe[damage=10]", ""),
        (
            "penalty",
            "diamond_sword[enchantments={sharpness:3},repair_cost=15]",
            "",
        ),
    ];
    for (name, a, b) in cases {
        reset(&bot, 0)?;
        open_at(&bot, p)?;
        put(&bot, 3, 0, 0, a, 1)?;
        if !b.is_empty() {
            put(&bot, 3, 1, 1, b, 1)?;
        }
        let d = digest_case(&bot, 2)
            .split('|')
            .next()
            .unwrap_or("")
            .to_string();
        note(ctx, format!("{name}: {d}"));
        ctx.metric(&format!("grind_{name}"), fnv(&d), 0.0);
        bot.close_container();
    }
    // take: xp is paid out as orbs (min cost of the removed enchantments, halved)
    reset(&bot, 0)?;
    open_at(&bot, p)?;
    put(
        &bot,
        3,
        0,
        0,
        "diamond_sword[enchantments={sharpness:5}]",
        1,
    )?;
    bot.click(2, 0, 1);
    bot.wait_ticks(4);
    ctx.metric(
        "grind_take_sword",
        (bot.st().count("diamond_sword") == 1) as u8 as f64,
        0.0,
    );
    bot.close_container();
    Ok(())
}

pub fn smithing_cases(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = arena(ctx, 6, 6)?;
    let p = [o[0] + 2, Y, o[2] + 4];
    setblock(&bot, p, "smithing_table")?;
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "netherite_sword",
            "netherite_upgrade_smithing_template",
            "diamond_sword[enchantments={sharpness:2},damage=5]",
            "netherite_ingot",
        ),
        (
            "trim_bolt_amethyst",
            "bolt_armor_trim_smithing_template",
            "iron_chestplate",
            "amethyst_shard",
        ),
        (
            "trim_wrong_base",
            "bolt_armor_trim_smithing_template",
            "iron_sword",
            "amethyst_shard",
        ),
        (
            "upgrade_on_iron",
            "netherite_upgrade_smithing_template",
            "iron_sword",
            "netherite_ingot",
        ),
        (
            "trim_copper",
            "coast_armor_trim_smithing_template",
            "diamond_helmet",
            "copper_ingot",
        ),
    ];
    for (name, t, b, a) in cases {
        reset(&bot, 0)?;
        open_at(&bot, p)?;
        put(&bot, 4, 0, 0, t, 1)?;
        put(&bot, 4, 1, 1, b, 1)?;
        put(&bot, 4, 2, 2, a, 1)?;
        let d = digest_case(&bot, 3)
            .split('|')
            .next()
            .unwrap_or("")
            .to_string();
        note(ctx, format!("{name}: {d}"));
        ctx.metric(&format!("smith_{name}"), fnv(&d), 0.0);
        if *name == "netherite_sword" {
            bot.click(3, 0, 1);
            bot.wait_ticks(4);
            ctx.metric(
                "smith_take",
                (bot.st().count("netherite_sword") == 1) as u8 as f64,
                0.0,
            );
            let left: Vec<String> = bot.st().container.iter().take(4).map(stack_str).collect();
            note(ctx, format!("after take: {left:?}"));
            ctx.metric("smith_inputs_left", fnv(&left.join(",")), 0.0);
        }
        bot.close_container();
    }
    Ok(())
}

/// Brewing stand: window, fuel use, 400-tick brew, data props, results.
pub fn brewing(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = arena(ctx, 6, 6)?;
    let p = [o[0] + 2, Y, o[2] + 4];
    setblock(&bot, p, "brewing_stand")?;
    reset(&bot, 0)?;
    open_at(&bot, p)?;
    let (m, t, n) = menu(&bot);
    ctx.check(
        m == "brewing_stand" && n == 41,
        format!("brewing menu {m} {t} {n}"),
    );
    put(
        &bot,
        5,
        0,
        0,
        "potion[potion_contents={potion:\"minecraft:water\"}]",
        1,
    )?;
    put(
        &bot,
        5,
        1,
        1,
        "potion[potion_contents={potion:\"minecraft:water\"}]",
        1,
    )?;
    put(&bot, 5, 2, 4, "blaze_powder", 2)?;
    put(&bot, 5, 3, 3, "nether_wart", 3)?;
    let t0 = std::time::Instant::now();
    let started = bot.wait_until(Duration::from_secs(3), |s| {
        s.props.get(&0).copied().unwrap_or(0) > 300
    });
    ctx.check(
        started,
        format!("brew did not start: props {:?}", bot.st().props),
    );
    let (bt, fuel) = (prop(&bot, 0), prop(&bot, 1));
    note(ctx, format!("started: brew time {bt}, fuel {fuel}"));
    ctx.metric("fuel_after_start", fuel as f64, 0.0);
    ctx.metric("brew_time_start", (bt >= 395) as u8 as f64, 0.0);
    let done = bot.wait_until(Duration::from_secs(25), |s| {
        s.container
            .first()
            .map(|x| x.potion != 0 && x.potion != -1 && stack_str(x).contains("+p"))
            .unwrap_or(false)
            && s.props.get(&0).copied().unwrap_or(1) == 0
    });
    let ticks = t0.elapsed().as_secs_f64() * 20.0;
    ctx.check(done, "brew did not finish within 25 s");
    note(ctx, format!("brew finished after {ticks:.0} ticks"));
    ctx.metric("brew_ticks", ticks, 12.0);
    bot.wait_ticks(3);
    let items: Vec<String> = bot.st().container.iter().take(5).map(stack_str).collect();
    note(ctx, format!("{items:?}"));
    ctx.metric("brew_result", fnv(&items.join(",")), 0.0);
    bot.close_container();
    Ok(())
}

/// Brewing recipe table: which ingredient turns which base into what. One brew per (base, ingredient), all stands brew in
/// parallel (the m10_brew_table batch size is kept small so a diff run stays under a minute).
pub fn brew_table(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = arena(ctx, 16, 16)?;
    ctx.stand(&bot, o[0] + 1, o[2] + 1, "creative")?;
    let bases = [
        "water",
        "awkward",
        "healing",
        "poison",
        "swiftness",
        "night_vision",
        "weakness",
        "strong_swiftness",
        "long_leaping",
    ];
    let ingredients = [
        "nether_wart",
        "redstone",
        "glowstone_dust",
        "fermented_spider_eye",
        "gunpowder",
        "sugar",
        "golden_carrot",
        "magma_cream",
        "rabbit_foot",
        "pufferfish",
        "glistering_melon_slice",
        "spider_eye",
        "ghast_tear",
        "blaze_powder",
        "turtle_helmet",
        "phantom_membrane",
        "stone",
        "slime_block",
        "cobweb",
        "breeze_rod",
    ];
    let mut stands = Vec::new();
    for (bi, b) in bases.iter().enumerate() {
        for (ii, ing) in ingredients.iter().enumerate() {
            let k = bi * ingredients.len() + ii;
            if k >= 14 * 14 {
                break;
            }
            let pos = [
                o[0] + 1 + (k % 14) as i32,
                Y + 1,
                o[2] + 1 + (k / 14) as i32,
            ];
            stands.push((pos, *b, *ing));
        }
    }
    // stands are placed with their contents (bottles, fuel, ingredient) in one setblock; a brew starts on the next tick
    for (pos, b, ing) in &stands {
        let bottle = |slot: i32| {
            format!("{{Slot:{slot}b,id:\"minecraft:potion\",count:1,components:{{\"minecraft:potion_contents\":{{potion:\"minecraft:{b}\"}}}}}}")
        };
        let nbt = format!(
            "brewing_stand{{Items:[{},{},{{Slot:3b,id:\"minecraft:{ing}\",count:1}},{{Slot:4b,id:\"minecraft:blaze_powder\",count:1}}]}}",
            bottle(0),
            bottle(1)
        );
        bot.cmd(&format!("setblock {} {} {} {nbt}", pos[0], pos[1], pos[2]));
        bot.wait_ticks(1);
    }
    bot.wait_ticks(20 * 21);
    let mut table = Vec::new();
    for (pos, b, ing) in &stands {
        let items = ctx.container(&bot, *pos)?.unwrap_or_default();
        let f = |i: usize| {
            format!(
                "{}:{}",
                items.slot(i).map(|x| x.0).unwrap_or("-"),
                items.potions.get(&i).map(|s| s.as_str()).unwrap_or("-")
            )
        };
        let (r, r2) = (f(0), f(1));
        table.push(format!("{b}+{ing}={r}/{r2}"));
    }
    // per ingredient hash over all bases so a difference names the ingredient
    for ing in ingredients {
        let part: Vec<&String> = table
            .iter()
            .filter(|l| l.contains(&format!("+{ing}=")))
            .collect();
        ctx.metric(
            &format!("brew_{ing}"),
            fnv(&part
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(";")),
            0.0,
        );
    }
    note(ctx, table.join(" "));
    Ok(())
}

/// Record a note; with `M10_NOTES=1` also print it (the result table only shows the first note).
fn note(ctx: &mut Ctx, s: String) {
    if std::env::var("M10_NOTES").is_ok() {
        eprintln!(
            "[{}] {s}",
            if ctx.is_vanilla() {
                "vanilla"
            } else {
                "basalt"
            }
        );
    }
    ctx.note(s);
}

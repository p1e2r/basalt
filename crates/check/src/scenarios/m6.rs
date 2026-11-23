//! M6 world logic scenarios.
use super::util::*;
use crate::bot::Bot;
use crate::runner::Ctx;
use anyhow::Result;
use std::time::Duration;

/// `/weather rain|thunder|clear`: begin/end rain events and the 0.01-per-tick level changes, vanilla vs Basalt.
/// Leaves the weather clear (the vanilla server is shared with other scenarios).
pub fn weather(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    bot.cmd("weather clear 600s");
    // let any rain from before drain away (levels move 0.01 per tick)
    bot.wait_until(Duration::from_secs(9), |s| s.rain <= 0.01 && s.thunder <= 0.01);
    bot.st().weather_events.clear();
    bot.cmd("weather rain 600s");
    ctx.check(bot.wait_until(Duration::from_secs(5), |s| s.weather_events.iter().any(|e| e.0 == 1)), "no BEGIN_RAINING event");
    ctx.check(bot.wait_until(Duration::from_secs(9), |s| s.rain >= 0.99), format!("rain level never reached 1 (now {})", bot.st().rain));
    let (begin, levels) = { let s = bot.st(); (s.weather_events.iter().filter(|e| e.0 == 1).count(), s.weather_events.iter().filter(|e| e.0 == 7).count()) };
    ctx.check(levels >= 50, format!("only {levels} rain level events"));
    ctx.metric("rain_begin_events", begin as f64, 0.0);
    ctx.metric("rain_level_events_ge_90", (levels >= 90) as u8 as f64, 0.0);
    bot.cmd("weather thunder 600s");
    ctx.check(bot.wait_until(Duration::from_secs(9), |s| s.thunder >= 0.99), format!("thunder level never reached 1 (now {})", bot.st().thunder));
    bot.st().weather_events.clear();
    bot.cmd("weather clear 600s");
    ctx.check(bot.wait_until(Duration::from_secs(5), |s| s.weather_events.iter().any(|e| e.0 == 2)), "no END_RAINING event");
    let back_to_clear = bot.wait_until(Duration::from_secs(9), |s| s.rain <= 0.01 && s.thunder <= 0.01);
    let (rain, thunder) = { let s = bot.st(); (s.rain, s.thunder) };   // one lock: two bot.st() in one expression would deadlock
    ctx.check(back_to_clear, format!("levels did not return to 0 (rain {rain}, thunder {thunder})"));
    let ends = bot.st().weather_events.iter().filter(|e| e.0 == 2).count();
    ctx.metric("rain_end_events", ends as f64, 0.0);
    bot.cmd("weather clear 600s");
    Ok(())
}

// ------------------------------------------------------------------ containers (chests, barrels, furnaces)

mod cont {
    use crate::bot::Bot;
    use crate::proto::Stack;
    use crate::runner::{Ctx, ARENA_Y};
    use anyhow::{bail, Result};
    use std::time::Duration;

    pub const Y: i32 = ARENA_Y + 1;

    /// Survival bot standing in a small arena; returns the arena origin.
    pub fn arena(ctx: &mut Ctx, sx: i32, sz: i32) -> Result<(Bot, [i32; 3])> {
        let bot = ctx.bot()?;
        let o = ctx.arena(&bot, sx, sz, 5)?;
        ctx.stand(&bot, o[0] + 2, o[2] + 2, "survival")?;
        Ok((bot, o))
    }

    /// Right-click the top of the block at `p` and wait for the screen + its contents.
    pub fn open_at(bot: &Bot, p: [i32; 3]) -> Result<()> {
        bot.look_at([p[0] as f64 + 0.5, p[1] as f64 + 0.5, p[2] as f64 + 0.5]);
        bot.wait_ticks(2);
        { let mut s = bot.st(); s.screen = None; s.container.clear(); }
        bot.place(p, 1);
        if !bot.wait_until(Duration::from_secs(3), |s| s.screen.is_some() && !s.container.is_empty()) { bail!("container at {p:?} did not open"); }
        Ok(())
    }

    /// (menu type, title key, slot count) of the open screen.
    pub fn screen(bot: &Bot) -> (String, String, usize) {
        let s = bot.st();
        (s.screen.as_ref().map(|x| x.1.clone()).unwrap_or_default(), s.screen_title.clone(), s.container.len())
    }

    pub fn fnv(s: &str) -> f64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in s.bytes() { h ^= b as u64; h = h.wrapping_mul(0x100000001b3); }
        (h & 0xF_FFFF_FFFF_FFFF) as f64
    }

    fn st(s: &Stack) -> String { if s.count > 0 { format!("{}x{}", s.name(), s.count) } else { "-".into() } }

    /// Window view as the client sees it: every container slot, the player's 36 slots and the cursor.
    pub fn window_digest(bot: &Bot) -> String {
        let s = bot.st();
        let parts: Vec<String> = s.container.iter().map(st).collect();
        format!("{} | cursor {}", parts.join(","), st(&s.cursor))
    }

    /// Server-side container contents + the bot's window view, hashed (state parity between servers, step by step).
    pub fn step(ctx: &mut Ctx, bot: &Bot, c: [i32; 3], name: &str) -> Result<()> {
        bot.wait_ticks(4);
        let server = ctx.container(bot, c)?.map(|x| format!("{:?}", x.items)).unwrap_or_else(|| "none".into());
        let view = window_digest(bot);
        let line = format!("{server} || {view}");
        if std::env::var("M6_DEBUG").is_ok() { eprintln!("[{} {name}] {line}", ctx.kind.name()); }
        ctx.metric(name, fnv(&line), 0.0);
        ctx.note(format!("{name}: {line}"));
        Ok(())
    }

    /// Window slot of player-inventory index `i` (window-0 numbering: 9..=35 main, 36..=44 hotbar) in a window with `n` container slots.
    pub fn w(n: usize, i: usize) -> i16 { (if i >= 36 { n + 27 + (i - 36) } else { n + (i - 9) }) as i16 }

    pub fn put(ctx: &Ctx, bot: &Bot, c: [i32; 3], slot: usize, item: &str, n: i32) -> Result<()> {
        let _ = ctx;
        bot.cmd_reply(&format!("item replace block {} {} {} container.{slot} with minecraft:{item} {n}", c[0], c[1], c[2]))?;
        Ok(())
    }
}

/// Chest open/close: screen type, title, contents, lid block_event(1, viewers), open/close sounds.
pub fn chest_open_close(ctx: &mut Ctx) -> Result<()> {
    use cont::*;
    let (bot, o) = arena(ctx, 10, 10)?;
    let c = [o[0] + 4, Y, o[2] + 2];
    setblock(&bot, c, "chest[facing=south]")?;
    put(ctx, &bot, c, 0, "diamond", 3)?; put(ctx, &bot, c, 5, "iron_ore", 64)?; put(ctx, &bot, c, 26, "stone", 1)?;
    give(&bot, "cobblestone", 10)?;
    bot.wait_ticks(2);
    { let mut s = bot.st(); s.block_events.clear(); s.sounds.clear(); }
    open_at(&bot, c)?;
    let (menu, title, n) = screen(&bot);
    ctx.check(menu == "generic_9x3", format!("menu {menu}"));
    ctx.check(title == "container.chest", format!("title {title}"));
    ctx.check(n == 63, format!("{n} slots"));
    let (s0, s5, s26, inv) = { let s = bot.st(); (s.container[0].clone(), s.container[5].clone(), s.container[26].clone(), s.container[27..].iter().filter(|x| x.name() == "cobblestone").map(|x| x.count).sum::<i32>()) };
    ctx.check(s0.name() == "diamond" && s0.count == 3, format!("slot 0 {:?}", s0.name()));
    ctx.check(s5.name() == "iron_ore" && s5.count == 64, "slot 5");
    ctx.check(s26.name() == "stone", "slot 26");
    ctx.check(inv == 10, format!("inventory part has {inv} cobblestone"));
    ctx.metric("slots", n as f64, 0.0);
    // lid animation: event(1, 1) at open, re-sent about every 5 ticks while open
    bot.wait_ticks(14);
    let open_events = bot.st().block_events.iter().filter(|e| e.1 == c && e.2 == 1 && e.3 == 1).count();
    ctx.check(open_events >= 2, format!("only {open_events} lid events while open"));
    ctx.metric("lid_events_open_ge2", (open_events >= 2) as i32 as f64, 0.0);
    let open_snd = basalt_data::registry_id(basalt_data::registries::SOUND_EVENT, "block.chest.open").unwrap();
    let close_snd = basalt_data::registry_id(basalt_data::registries::SOUND_EVENT, "block.chest.close").unwrap();
    ctx.check(bot.st().sounds.iter().any(|x| x.1 == open_snd), "no chest open sound");
    bot.st().block_events.clear();
    bot.close_container();
    ctx.check(bot.wait_until(Duration::from_secs(2), |s| s.block_events.iter().any(|e| e.1 == c && e.2 == 1 && e.3 == 0)), "no lid event with 0 viewers after closing");
    ctx.check(bot.st().sounds.iter().any(|x| x.1 == close_snd), "no chest close sound");
    ctx.metric("close_event_seen", 1.0, 0.0);
    // a second player opening it: count 2, then back to 1 when one closes
    let b2 = ctx.bot()?;
    b2.tp([o[0] as f64 + 4.5, Y as f64, o[2] as f64 + 4.5])?; b2.wait_on_ground(Duration::from_secs(5));
    open_at(&bot, c)?;
    bot.wait_ticks(4);
    open_at(&b2, c)?;
    ctx.check(b2.wait_until(Duration::from_secs(2), |s| s.block_events.iter().any(|e| e.1 == c && e.2 == 1 && e.3 == 2)), "no lid event with 2 viewers");
    bot.close_container();
    ctx.check(b2.wait_until(Duration::from_secs(2), |s| s.block_events.last().map(|e| e.1 == c && e.3 == 1).unwrap_or(false)), format!("after one close: {:?}", b2.st().block_events.last()));
    b2.close_container();
    Ok(())
}

/// Every container_click mode in a single chest; state after each step compared between servers.
pub fn chest_clicks(ctx: &mut Ctx) -> Result<()> {
    use cont::*;
    let (bot, o) = arena(ctx, 10, 10)?;
    let c = [o[0] + 4, Y, o[2] + 2];
    setblock(&bot, c, "chest[facing=south]")?;
    give(&bot, "iron_ore", 64)?; give(&bot, "cobblestone", 40)?; give(&bot, "diamond", 3)?; give(&bot, "dirt", 10)?; give(&bot, "oak_planks", 20)?; give(&bot, "stick", 7)?;
    // layout in the hotbar after /give: iron_ore 36, cobblestone 37, diamond 38, dirt 39, oak_planks 40, stick 41
    open_at(&bot, c)?;
    let n = 27usize;
    step(ctx, &bot, c, "s00_open")?;
    // 0: pick up a stack and put it down; right-click puts one; swap with a different item
    bot.click(w(n, 36), 0, 0); step(ctx, &bot, c, "s01_pick_iron")?;
    bot.click(0, 0, 0); step(ctx, &bot, c, "s02_place_iron")?;
    bot.click(w(n, 37), 1, 0); step(ctx, &bot, c, "s03_right_pick_half_cobble")?;
    bot.click(1, 1, 0); step(ctx, &bot, c, "s04_right_place_one")?;
    bot.click(2, 1, 0); bot.click(2, 1, 0); step(ctx, &bot, c, "s05_right_place_two_more")?;
    bot.click(0, 0, 0); step(ctx, &bot, c, "s06_swap_cursor_with_iron")?;
    bot.click(0, 0, 0); step(ctx, &bot, c, "s07_swap_back")?;
    bot.click(w(n, 36), 0, 0); step(ctx, &bot, c, "s08_drop_cursor_into_hotbar")?;
    // 1: shift-click both directions
    bot.click(w(n, 38), 0, 1); step(ctx, &bot, c, "s09_shift_diamond_in")?;
    bot.click(w(n, 39), 0, 1); step(ctx, &bot, c, "s10_shift_dirt_in")?;
    bot.click(w(n, 40), 0, 1); bot.click(w(n, 41), 0, 1); step(ctx, &bot, c, "s11_shift_planks_sticks_in")?;
    bot.click(0, 0, 1); step(ctx, &bot, c, "s12_shift_iron_out")?;
    bot.click(1, 0, 1); bot.click(2, 0, 1); step(ctx, &bot, c, "s13_shift_cobble_out")?;
    // 2: swap with hotbar keys
    bot.click(3, 2, 2); step(ctx, &bot, c, "s14_swap_slot3_hotbar2")?;
    bot.click(4, 0, 2); step(ctx, &bot, c, "s15_swap_slot4_hotbar0")?;
    // 5: drag. left-drag splits the cursor evenly, right-drag puts one in each
    bot.click(w(n, 36), 0, 0); bot.wait_ticks(2);
    bot.click(-999, 0, 5); bot.click(10, 1, 5); bot.click(11, 1, 5); bot.click(12, 1, 5); bot.click(-999, 2, 5);
    step(ctx, &bot, c, "s18_left_drag_three")?;
    bot.click(w(n, 37), 0, 0); bot.wait_ticks(2);
    bot.click(-999, 4, 5); bot.click(13, 5, 5); bot.click(14, 5, 5); bot.click(10, 5, 5); bot.click(-999, 6, 5);
    step(ctx, &bot, c, "s19_right_drag_three")?;
    // 6: double click collects matching stacks into the cursor
    bot.click(w(n, 42), 0, 0); bot.wait_ticks(2);
    bot.click(w(n, 42), 0, 0); step(ctx, &bot, c, "s20_pick_empty_hotbar")?;
    bot.click(10, 0, 6); step(ctx, &bot, c, "s21_double_click_on_busy_slot_is_noop")?;
    bot.click(20, 0, 0); bot.click(20, 0, 0); bot.click(20, 0, 6); step(ctx, &bot, c, "s21b_double_click_collect_on_empty_slot")?;
    // 3: creative clone
    ctx.gamemode(&bot, "creative")?; bot.wait_ticks(3);
    bot.click(11, 2, 3); step(ctx, &bot, c, "s24_creative_clone")?;
    bot.click(-999, 0, 0); bot.wait_ticks(2);
    ctx.gamemode(&bot, "survival")?;
    // 4: drop from the chest (the dropped items would be picked up again after a couple of seconds, so these come last)
    let t0 = std::time::Instant::now();
    bot.click(10, 0, 4); step(ctx, &bot, c, "s16_drop_one")?;
    bot.click(11, 1, 4); step(ctx, &bot, c, "s17_drop_stack")?;
    bot.wait_ticks(6);
    let dropped: i32 = bot.st().entities.values().filter(|e| e.kind == "item" && e.added >= t0).filter_map(|e| e.item.as_ref().map(|i| i.count)).sum();
    ctx.metric("dropped_items", dropped as f64, 0.0);
    // 0 outside: drop the cursor
    bot.click(12, 0, 0); step(ctx, &bot, c, "s21c_pick_for_outside_drop")?;
    bot.click(-999, 1, 0); step(ctx, &bot, c, "s22_drop_one_outside")?;
    bot.click(-999, 0, 0); step(ctx, &bot, c, "s23_drop_all_outside")?;
    // consistency: the bot's view equals the server's
    let srv = ctx.container(&bot, c)?.unwrap_or_default();
    let bad: Vec<usize> = (0..27).filter(|&i| { let v = bot.st().container[i].clone(); srv.items.get(&i).map(|x| (x.0.as_str(), x.1)) != if v.count > 0 { Some((v.name(), v.count as i64)) } else { None } }).collect();
    ctx.check(bad.is_empty(), format!("client view differs from server at slots {bad:?}"));
    bot.close_container(); bot.wait_ticks(3);
    // closing returns the cursor (empty here) and keeps the container
    ctx.check(ctx.container(&bot, c)?.is_some(), "container block entity vanished");
    Ok(())
}

/// Closing with an item on the cursor puts it back in the inventory.
pub fn chest_close_cursor(ctx: &mut Ctx) -> Result<()> {
    use cont::*;
    let (bot, o) = arena(ctx, 10, 10)?;
    let c = [o[0] + 4, Y, o[2] + 2];
    setblock(&bot, c, "chest[facing=south]")?;
    put(ctx, &bot, c, 3, "gold_ingot", 16)?;
    open_at(&bot, c)?;
    bot.click(3, 0, 0); bot.wait_ticks(3);
    ctx.check(bot.st().cursor.name() == "gold_ingot", "cursor not picked");
    bot.close_container(); bot.wait_ticks(5);
    let st = ctx.state(&bot)?;
    ctx.check(st.count("gold_ingot") == 16, format!("server inventory has {} gold ingots after closing with the cursor", st.count("gold_ingot")));
    ctx.metric("gold_back_in_inventory", st.count("gold_ingot") as f64, 0.0);
    let left = ctx.container(&bot, c)?.map(|x| x.count("gold_ingot")).unwrap_or(-1);
    ctx.check(left == 0, format!("chest still has {left}"));
    Ok(())
}

/// Two chests placed side by side pair up (type left/right), the double window has 54 slots, both halves animate,
/// breaking one half un-pairs the other; chests with different facing never pair.
pub fn double_chest(ctx: &mut Ctx) -> Result<()> {
    use cont::*;
    let (bot, o) = arena(ctx, 12, 10)?;
    give(&bot, "chest", 6)?;
    hold(&bot, "chest")?;
    let z = o[2] + 4;
    // stand south of the chests looking north: placed chests face south
    let place_chest = |bot: &Bot, x: i32, z: i32| -> Result<()> {
        bot.tp([x as f64 + 0.5, Y as f64, z as f64 + 2.5])?;
        bot.wait_on_ground(Duration::from_secs(3));
        bot.look_at([x as f64 + 0.5, Y as f64 - 0.5, z as f64 + 0.5]);
        bot.wait_ticks(3);
        bot.place([x, Y - 1, z], 1);
        bot.wait_ticks(5);
        Ok(())
    };
    let (a, b) = ([o[0] + 3, Y, z], [o[0] + 4, Y, z]);
    place_chest(&bot, a[0], z)?;
    ctx.check(ctx.block_is(&bot, a, "chest[type=single,facing=south]")?, "first chest not single/south");
    place_chest(&bot, b[0], z)?;
    // measured on vanilla: the chest placed east of a south-facing chest is the left half, the first one becomes right
    let (sa, sb) = { let s = bot.st(); (s.world.name(a[0], a[1], a[2]), s.world.name(b[0], b[1], b[2])) };
    ctx.check(sa == "chest" && sb == "chest", format!("client sees {sa} {sb}"));
    let types: Vec<&str> = [a, b].iter().map(|p| ["single", "left", "right"].into_iter().find(|t| ctx.block_is(&bot, *p, &format!("chest[type={t}]")).unwrap_or(false)).unwrap_or("?")).collect();
    ctx.note(format!("types after placing west then east: {types:?}"));
    ctx.check(ctx.block_is(&bot, a, "chest[type=right,facing=south]")?, "west chest should be the right half");
    ctx.check(ctx.block_is(&bot, b, "chest[type=left,facing=south]")?, "east chest should be the left half");
    // contents: one marker per half, window order is measured against vanilla
    put(ctx, &bot, a, 0, "diamond", 1)?; put(ctx, &bot, b, 0, "emerald", 1)?; put(ctx, &bot, a, 26, "gold_ingot", 2)?; put(ctx, &bot, b, 26, "iron_ingot", 3)?;
    bot.tp([b[0] as f64 + 0.5, Y as f64, z as f64 + 2.5])?; bot.wait_on_ground(Duration::from_secs(3));
    { let mut s = bot.st(); s.block_events.clear(); }
    open_at(&bot, b)?;
    let (menu, title, n) = screen(&bot);
    ctx.check(menu == "generic_9x6", format!("menu {menu}"));
    ctx.check(title == "container.chestDouble", format!("title {title}"));
    ctx.check(n == 90, format!("{n} slots"));
    ctx.metric("double_slots", n as f64, 0.0);
    let order: Vec<String> = { let s = bot.st(); [0usize, 26, 27, 53].iter().map(|&i| s.container[i].name().to_string()).collect() };
    ctx.note(format!("window slots 0/26/27/53: {order:?}"));
    ctx.metric("double_window_order", fnv(&order.join(",")), 0.0);
    bot.wait_ticks(3);
    let halves: Vec<[i32; 3]> = { let s = bot.st(); let mut v: Vec<[i32; 3]> = s.block_events.iter().filter(|e| e.2 == 1 && e.3 == 1).map(|e| e.1).collect(); v.sort(); v.dedup(); v };
    ctx.check(halves == vec![a, b], format!("lid events at {halves:?}, want both halves"));
    ctx.metric("lid_events_both_halves", (halves.len() == 2) as i32 as f64, 0.0);
    // moving an item into the second half reaches the right block entity
    bot.click(31, 0, 0); bot.wait_ticks(2);
    step(ctx, &bot, a, "d01_pick_slot31")?;
    bot.click(w(54, 36), 0, 0); step(ctx, &bot, b, "d02_swap_with_hotbar_stack")?;
    bot.close_container(); bot.wait_ticks(4);
    // chests with different facing don't pair: c faces north next to a single chest facing south
    let (c1, c2) = ([o[0] + 7, Y, z], [o[0] + 8, Y, z]);
    place_chest(&bot, c1[0], z)?;
    bot.tp([c2[0] as f64 + 0.5, Y as f64, z as f64 - 2.5])?; bot.wait_on_ground(Duration::from_secs(3));
    bot.look_at([c2[0] as f64 + 0.5, Y as f64 - 0.5, c2[2] as f64 + 0.5]); bot.wait_ticks(3);
    bot.place([c2[0], Y - 1, c2[2]], 1); bot.wait_ticks(5);
    ctx.check(ctx.block_is(&bot, c1, "chest[type=single,facing=south]")?, "mismatched facing chest paired (first)");
    ctx.check(ctx.block_is(&bot, c2, "chest[type=single,facing=north]")?, "mismatched facing chest paired (second)");
    // breaking one half un-pairs the other
    ctx.gamemode(&bot, "creative")?;
    bot.tp([a[0] as f64 + 0.5, Y as f64, z as f64 + 2.5])?; bot.wait_on_ground(Duration::from_secs(3)); bot.wait_ticks(3);
    bot.dig(a)?;
    ctx.check(bot.wait_until(Duration::from_secs(2), |s| s.world.name(a[0], a[1], a[2]) == "air"), "chest half not broken");
    bot.wait_ticks(3);
    ctx.check(ctx.block_is(&bot, b, "chest[type=single,facing=south]")?, "right half did not become single");
    Ok(())
}

/// Barrel: facing from the look direction, `open` state while a window is open, open/close sounds.
pub fn barrel(ctx: &mut Ctx) -> Result<()> {
    use cont::*;
    let (bot, o) = arena(ctx, 10, 10)?;
    give(&bot, "barrel", 2)?;
    hold(&bot, "barrel")?;
    let b = [o[0] + 4, Y, o[2] + 2];
    bot.look_at([b[0] as f64 + 0.5, Y as f64 - 0.5, b[2] as f64 + 0.5]); bot.wait_ticks(3);
    bot.place([b[0], Y - 1, b[2]], 1); bot.wait_ticks(5);
    // looking down onto the floor: nearest looking direction is down, facing = up
    ctx.check(ctx.block_is(&bot, b, "barrel[facing=up,open=false]")?, format!("barrel placed looking down: {}", bot.st().world.name(b[0], b[1], b[2])));
    put(ctx, &bot, b, 4, "diamond", 5)?;
    { let mut s = bot.st(); s.sounds.clear(); }
    open_at(&bot, b)?;
    let (menu, title, n) = screen(&bot);
    ctx.check(menu == "generic_9x3", format!("menu {menu}"));
    ctx.check(title == "container.barrel", format!("title {title}"));
    ctx.check(n == 63, format!("{n} slots"));
    ctx.check(bot.st().container[4].name() == "diamond", "slot 4");
    bot.wait_ticks(3);
    ctx.check(ctx.block_is(&bot, b, "barrel[open=true]")?, "barrel not open while a window is open");
    let (o_snd, c_snd) = (basalt_data::registry_id(basalt_data::registries::SOUND_EVENT, "block.barrel.open").unwrap(), basalt_data::registry_id(basalt_data::registries::SOUND_EVENT, "block.barrel.close").unwrap());
    ctx.check(bot.st().sounds.iter().any(|x| x.1 == o_snd), "no barrel open sound");
    bot.click(4, 0, 1); step(ctx, &bot, b, "b01_shift_out")?;
    bot.close_container(); bot.wait_ticks(4);
    ctx.check(ctx.block_is(&bot, b, "barrel[open=false]")?, "barrel still open after closing");
    ctx.check(bot.st().sounds.iter().any(|x| x.1 == c_snd), "no barrel close sound");
    // facing horizontally: looking north at a side face gives facing=south
    bot.tp([b[0] as f64 + 0.5, Y as f64, b[2] as f64 + 3.5])?; bot.wait_on_ground(Duration::from_secs(3));
    let b2 = [b[0] + 2, Y, b[2]];
    bot.look_at([b2[0] as f64 + 0.5, Y as f64 + 0.5, b2[2] as f64 + 0.5]); bot.wait_ticks(3);
    bot.place([b2[0], Y - 1, b2[2]], 1); bot.wait_ticks(5);
    let f = bot.st().world.name(b2[0], b2[1], b2[2]);
    ctx.check(f == "barrel", format!("second barrel: {f}"));
    ctx.check(ctx.block_is(&bot, b2, "barrel[facing=south]")?, "barrel placed looking north (slightly down) should face south");
    Ok(())
}

/// Breaking a container spills its contents (also in creative) and removes the block entity; the chest itself drops.
pub fn break_contents(ctx: &mut Ctx) -> Result<()> {
    use cont::*;
    let (bot, o) = arena(ctx, 12, 12)?;
    let c = [o[0] + 3, Y, o[2] + 2];
    setblock(&bot, c, "chest[facing=south]")?;
    put(ctx, &bot, c, 0, "diamond", 3)?; put(ctx, &bot, c, 5, "iron_ore", 64)?; put(ctx, &bot, c, 9, "stick", 7)?;
    // far enough that nothing is picked up
    bot.tp([o[0] as f64 + 9.5, Y as f64, o[2] as f64 + 9.5])?; bot.wait_on_ground(Duration::from_secs(5));
    bot.tp([c[0] as f64 + 0.5, Y as f64, c[2] as f64 + 2.5])?; bot.wait_on_ground(Duration::from_secs(5));
    let t0 = std::time::Instant::now();
    ctx.gamemode(&bot, "survival")?;
    bot.dig(c)?;
    ctx.check(bot.wait_until(Duration::from_secs(5), |s| s.world.name(c[0], c[1], c[2]) == "air"), "chest not broken");
    bot.tp([o[0] as f64 + 10.5, Y as f64, o[2] as f64 + 10.5])?;
    bot.wait_ticks(10);
    let mut totals = std::collections::BTreeMap::<String, i32>::new();
    for e in bot.st().entities.values().filter(|e| e.kind == "item" && e.added >= t0) { if let Some(i) = &e.item { *totals.entry(i.name().to_string()).or_default() += i.count; } }
    ctx.note(format!("spilled {totals:?}"));
    ctx.check(totals.get("diamond") == Some(&3) && totals.get("iron_ore") == Some(&64) && totals.get("stick") == Some(&7), format!("spilled items {totals:?}"));
    ctx.check(totals.get("chest") == Some(&1), format!("chest item drops {:?}", totals.get("chest")));
    ctx.metric("spilled_diamond", *totals.get("diamond").unwrap_or(&0) as f64, 0.0);
    ctx.metric("spilled_iron_ore", *totals.get("iron_ore").unwrap_or(&0) as f64, 0.0);
    ctx.metric("spilled_stick", *totals.get("stick").unwrap_or(&0) as f64, 0.0);
    ctx.metric("chest_drop", *totals.get("chest").unwrap_or(&0) as f64, 0.0);
    ctx.check(ctx.container(&bot, c)?.is_none(), "block entity still there after breaking");
    // an open window closes when the chest is broken from under the viewer; creative breaking spills too
    let c2 = [o[0] + 3, Y, o[2] + 6];
    setblock(&bot, c2, "chest[facing=south]")?;
    put(ctx, &bot, c2, 0, "emerald", 2)?;
    let viewer = ctx.bot()?;
    viewer.tp([c2[0] as f64 + 0.5, Y as f64, c2[2] as f64 + 2.5])?; viewer.wait_on_ground(Duration::from_secs(5));
    open_at(&viewer, c2)?;
    ctx.gamemode(&bot, "creative")?;
    bot.tp([c2[0] as f64 - 2.5, Y as f64, c2[2] as f64 + 0.5])?; bot.wait_on_ground(Duration::from_secs(5)); bot.wait_ticks(3);
    let t1 = std::time::Instant::now();
    bot.dig(c2)?;
    ctx.check(viewer.wait_until(Duration::from_secs(3), |s| s.screen.is_none()), "viewer's window did not close when the chest was broken");
    ctx.metric("viewer_window_closed", viewer.st().screen.is_none() as i32 as f64, 0.0);
    bot.wait_ticks(8);
    let em: i32 = bot.st().entities.values().filter(|e| e.kind == "item" && e.added >= t1).filter_map(|e| e.item.as_ref()).filter(|i| i.name() == "emerald").map(|i| i.count).sum();
    ctx.check(em == 2, format!("creative break spilled {em} emeralds"));
    ctx.metric("creative_spill", em as f64, 0.0);
    Ok(())
}

/// Contents of chests and furnaces survive a restart (Anvil block_entities); the chunk network encoding carries no NBT for them.
pub fn container_persistence(ctx: &mut Ctx) -> Result<()> {
    use cont::*;
    let bot = ctx.bot()?;
    let name = bot.name.clone();
    let o = ctx.arena(&bot, 10, 10, 4)?;
    ctx.stand(&bot, o[0] + 2, o[2] + 2, "survival")?;
    let (c, f) = ([o[0] + 5, Y, o[2] + 5], [o[0] + 7, Y, o[2] + 5]);
    setblock(&bot, c, "chest[facing=south]")?;
    setblock(&bot, f, "furnace[facing=south]")?;
    put(ctx, &bot, c, 0, "diamond", 3)?; put(ctx, &bot, c, 13, "iron_ore", 64)?;
    put(ctx, &bot, f, 0, "iron_ore", 5)?; put(ctx, &bot, f, 1, "coal", 3)?;
    bot.wait_ticks(60);
    let before = ctx.container(&bot, f)?.unwrap_or_default();
    ctx.check(before.cook > 40 && before.lit > 0, format!("furnace not cooking before restart: {before:?}"));
    bot.cmd_reply("save-all")?;
    bot.quit();
    std::thread::sleep(Duration::from_millis(300));
    ctx.restart()?;
    let bot = Bot::connect(&ctx.addr(), &name)?;
    bot.wait_chunks(1, Duration::from_secs(10))?;
    let cs = ctx.container(&bot, c)?.unwrap_or_default();
    ctx.check(cs.slot(0) == Some(("diamond", 3)) && cs.slot(13) == Some(("iron_ore", 64)), format!("chest after restart {:?}", cs.items));
    let fs = ctx.container(&bot, f)?.unwrap_or_default();
    ctx.check(fs.slot(0).map(|x| x.0) == Some("iron_ore") && fs.slot(1).map(|x| x.0) == Some("coal"), format!("furnace items after restart {:?}", fs.items));
    ctx.check(fs.cook >= before.cook && fs.lit > 0 && fs.lit < before.lit, format!("furnace progress after restart {fs:?} (before {before:?})"));
    ctx.check(ctx.block_is(&bot, f, "furnace[lit=true]")?, "furnace lit state lost");
    // the client sees containers as plain blocks with block entities that carry no NBT
    let (cn, fnm) = { let s = bot.st(); (s.world.name(c[0], c[1], c[2]), s.world.name(f[0], f[1], f[2])) };
    ctx.check(cn == "chest" && fnm == "furnace", "client doesn't see the containers");
    // still works after the restart: open the chest and take the diamonds
    bot.wait_on_ground(Duration::from_secs(5));
    open_at(&bot, c)?;
    ctx.check(bot.st().container[0].name() == "diamond", "window after restart has no diamond");
    bot.click(0, 0, 1); bot.wait_ticks(3);
    bot.close_container(); bot.wait_ticks(3);
    let dia = bot.st().count("diamond");
    ctx.check(dia == 3, format!("diamonds in inventory {dia}"));
    // lit toggling and progress continue after the restart
    bot.wait_ticks(20);
    let later = ctx.container(&bot, f)?.unwrap_or_default();
    ctx.check(later.cook > fs.cook, format!("furnace stopped cooking after restart: {fs:?} -> {later:?}"));
    Ok(())
}

/// Furnace: lit state, container_set_data values, time to the first result, RecipesUsed, taking the result.
pub fn furnace_smelt(ctx: &mut Ctx) -> Result<()> {
    use cont::*;
    let (bot, o) = arena(ctx, 10, 10)?;
    let f = [o[0] + 4, Y, o[2] + 2];
    setblock(&bot, f, "furnace[facing=south]")?;
    put(ctx, &bot, f, 0, "iron_ore", 3)?; put(ctx, &bot, f, 1, "coal", 2)?;
    open_at(&bot, f)?;
    let (menu, title, n) = screen(&bot);
    ctx.check(menu == "furnace", format!("menu {menu}"));
    ctx.check(title == "container.furnace", format!("title {title}"));
    ctx.check(n == 39, format!("{n} slots"));
    ctx.check(bot.wait_until(Duration::from_secs(2), |s| s.props.len() == 4), format!("container_set_data ids {:?}", bot.st().props));
    let p = bot.st().props.clone();
    ctx.check(p.get(&1) == Some(&1600) && p.get(&3) == Some(&200), format!("props {p:?}"));
    let p0 = *p.get(&2).unwrap_or(&0) as i32;
    let t0 = std::time::Instant::now();
    ctx.check(ctx.block_is(&bot, f, "furnace[lit=true]")?, "furnace not lit while cooking");
    // lit time + progress is constant while burning (the first cooking tick also lights the fuel)
    let (lit, cook) = { let s = bot.st(); (*s.props.get(&0).unwrap_or(&0) as i32, *s.props.get(&2).unwrap_or(&0) as i32) };
    ctx.metric("lit_plus_cook", (lit + cook) as f64, 3.0);
    // the first ingot
    let got = bot.wait_until(Duration::from_secs(15), |s| s.container.get(2).map(|x| x.name() == "iron_ingot").unwrap_or(false));
    ctx.check(got, "no iron ingot after 15 s");
    let ticks = t0.elapsed().as_secs_f64() * 20.0 + p0 as f64;
    ctx.note(format!("first ingot after {ticks:.1} cook ticks (opened at progress {p0})"));
    ctx.metric("ticks_to_first_ingot", ticks, 4.0);
    bot.wait_ticks(3);
    let cs = ctx.container(&bot, f)?.unwrap_or_default();
    ctx.check(cs.slot(0) == Some(("iron_ore", 2)) && cs.slot(2) == Some(("iron_ingot", 1)) && cs.slot(1) == Some(("coal", 1)), format!("furnace contents {:?}", cs.items));
    ctx.check(cs.used.get("iron_ingot_from_smelting_iron_ore") == Some(&1), format!("RecipesUsed {:?}", cs.used));
    ctx.check(cs.cook < 10, format!("progress after finishing: {}", cs.cook));
    let pr = bot.st().props.clone();
    ctx.check(pr.get(&3) == Some(&200) && pr.get(&1) == Some(&1600), format!("props after finishing {pr:?}"));
    // take the result with shift-click: lands in the inventory, XP is paid out (0 or 1 for 0.7 xp)
    bot.click(2, 0, 1); bot.wait_ticks(4);
    let ing = bot.st().count("iron_ingot");
    ctx.check(ing == 1, format!("ingots in inventory {ing}"));
    let cs = ctx.container(&bot, f)?.unwrap_or_default();
    ctx.check(cs.used.is_empty(), format!("RecipesUsed not cleared after taking: {:?}", cs.used));
    bot.close_container();
    Ok(())
}

/// Shift-click rules and slot restrictions of the furnace window.
pub fn furnace_clicks(ctx: &mut Ctx) -> Result<()> {
    use cont::*;
    let (bot, o) = arena(ctx, 10, 10)?;
    let f = [o[0] + 4, Y, o[2] + 2];
    setblock(&bot, f, "furnace[facing=south]")?;
    give(&bot, "dirt", 5)?; give(&bot, "coal", 5)?; give(&bot, "stick", 3)?; give(&bot, "iron_ore", 10)?; give(&bot, "cobblestone", 8)?; give(&bot, "bucket", 1)?; give(&bot, "iron_ingot", 4)?;
    // hotbar: dirt 36, coal 37, stick 38, iron_ore 39, cobblestone 40, bucket 41, iron_ingot 42
    open_at(&bot, f)?;
    let n = 3usize;
    step(ctx, &bot, f, "f00_open")?;
    bot.click(w(n, 36), 0, 1); step(ctx, &bot, f, "f01_shift_dirt")?;           // not smeltable, not fuel: hotbar -> main
    bot.click(w(n, 37), 0, 1); step(ctx, &bot, f, "f02_shift_coal")?;           // fuel slot
    bot.click(w(n, 38), 0, 1); step(ctx, &bot, f, "f03_shift_sticks")?;         // fuel merge refuses different item: stays
    bot.click(w(n, 42), 0, 1); step(ctx, &bot, f, "f04_shift_ingot")?;          // not smeltable / not fuel
    bot.click(w(n, 41), 0, 1); step(ctx, &bot, f, "f05_shift_bucket")?;         // empty bucket is allowed in the fuel slot only when empty
    // placing: non-fuel into the fuel slot, anything into the result slot
    bot.click(w(n, 9 + 0), 0, 0); step(ctx, &bot, f, "f06_pick_dirt")?;
    bot.click(1, 0, 0); step(ctx, &bot, f, "f07_dirt_into_fuel_rejected")?;
    bot.click(2, 0, 0); step(ctx, &bot, f, "f08_dirt_into_result_rejected")?;
    bot.click(0, 0, 0); step(ctx, &bot, f, "f09_dirt_into_input")?;
    bot.click(0, 0, 1); step(ctx, &bot, f, "f10_shift_input_out")?;
    // smeltable items go to the input, in order: iron ore, then cobblestone does not merge
    bot.click(w(n, 39), 0, 1); step(ctx, &bot, f, "f11_shift_iron_ore")?;
    bot.click(w(n, 40), 0, 1); step(ctx, &bot, f, "f12_shift_cobble_while_input_busy")?;
    // swap hotbar key with the fuel slot
    bot.click(1, 1, 2); step(ctx, &bot, f, "f13_hotbar_swap_fuel")?;
    // drag across the fuel and input slots
    bot.close_container(); bot.wait_ticks(2);
    Ok(())
}

/// Fuel running out: burn time counts down, `lit` turns off, progress decays 2 per tick afterwards.
pub fn furnace_fuel(ctx: &mut Ctx) -> Result<()> {
    use cont::*;
    let (bot, o) = arena(ctx, 10, 10)?;
    let f = [o[0] + 4, Y, o[2] + 2];
    setblock(&bot, f, "furnace[facing=south]")?;
    put(ctx, &bot, f, 0, "iron_ore", 2)?; put(ctx, &bot, f, 1, "stick", 1)?;
    open_at(&bot, f)?;
    bot.wait_ticks(2);
    let burn = bot.st().props.get(&1).copied().unwrap_or(0);
    ctx.check(burn == 100, format!("stick burn time {burn}"));
    ctx.metric("stick_burn_time", burn as f64, 0.0);
    ctx.check(bot.wait_until(Duration::from_secs(10), |s| s.props.get(&0) == Some(&0)), format!("fuel never ran out: {:?}", bot.st().props));
    let cook_at_out = bot.st().props.get(&2).copied().unwrap_or(-1);
    ctx.note(format!("progress when the fuel ran out: {cook_at_out}"));
    ctx.metric("cook_when_fuel_out", cook_at_out as f64, 3.0);
    bot.wait_ticks(4);
    ctx.check(ctx.block_is(&bot, f, "furnace[lit=false]")?, "furnace still lit with no fuel");
    // decay: progress falls by 2 per tick
    let a = bot.st().props.get(&2).copied().unwrap_or(-1);
    bot.wait_ticks(10);
    let b = bot.st().props.get(&2).copied().unwrap_or(-1);
    let rate = (a - b) as f64 / 10.0;
    ctx.note(format!("decay {a} -> {b} in 10 ticks"));
    ctx.metric("decay_per_tick", rate, 0.4);
    ctx.check(bot.wait_until(Duration::from_secs(10), |s| s.props.get(&2) == Some(&0)), "progress never decayed to 0");
    bot.close_container();
    Ok(())
}

/// Blast furnace (100 ticks, raw gold xp 1.0) and smoker (food only): menus, speed, experience on taking.
pub fn blast_smoker(ctx: &mut Ctx) -> Result<()> {
    use cont::*;
    let (bot, o) = arena(ctx, 12, 10)?;
    let (bf, sm) = ([o[0] + 3, Y, o[2] + 2], [o[0] + 6, Y, o[2] + 2]);
    setblock(&bot, bf, "blast_furnace[facing=south]")?;
    setblock(&bot, sm, "smoker[facing=south]")?;
    put(ctx, &bot, bf, 0, "raw_gold", 2)?; put(ctx, &bot, bf, 1, "coal", 1)?;
    put(ctx, &bot, sm, 0, "beef", 2)?; put(ctx, &bot, sm, 1, "coal", 1)?;
    open_at(&bot, bf)?;
    let (menu, title, n) = screen(&bot);
    ctx.check(menu == "blast_furnace", format!("menu {menu}"));
    ctx.check(title == "container.blast_furnace", format!("title {title}"));
    ctx.check(n == 39, format!("{n} slots"));
    ctx.check(bot.wait_until(Duration::from_secs(2), |s| s.props.len() == 4), "no container_set_data");
    let ct = bot.st().props.get(&3).copied();
    ctx.check(ct == Some(100), format!("blast cook total {ct:?}"));
    ctx.metric("blast_cook_total", *bot.st().props.get(&3).unwrap_or(&0) as f64, 0.0);
    let p0 = *bot.st().props.get(&2).unwrap_or(&0) as f64;
    let t0 = std::time::Instant::now();
    ctx.check(bot.wait_until(Duration::from_secs(8), |s| s.container.get(2).map(|x| x.name() == "gold_ingot").unwrap_or(false)), "no gold ingot");
    let ticks = t0.elapsed().as_secs_f64() * 20.0 + p0;
    ctx.metric("blast_ticks_to_ingot", ticks, 4.0);
    ctx.check(ctx.block_is(&bot, bf, "blast_furnace[lit=true]")?, "blast furnace not lit");
    // second ingot, then take both: raw gold gives exactly 1.0 xp each
    ctx.check(bot.wait_until(Duration::from_secs(8), |s| s.container.get(2).map(|x| x.count == 2).unwrap_or(false)), "no second gold ingot");
    let xp0 = ctx.state(&bot)?.xp_total;
    bot.click(2, 0, 1); bot.wait_ticks(8);
    bot.close_container(); bot.wait_ticks(25);
    let xp1 = ctx.state(&bot)?.xp_total;
    let gold = bot.st().count("gold_ingot");
    ctx.check(gold == 2, format!("gold ingots {gold}"));
    ctx.approx("xp gained", (xp1 - xp0) as f64, 2.0, 0.0);
    ctx.metric("xp_gained", (xp1 - xp0) as f64, 0.0);
    // smoker
    bot.tp([sm[0] as f64 + 0.5, Y as f64, sm[2] as f64 + 2.5])?; bot.wait_on_ground(Duration::from_secs(3));
    open_at(&bot, sm)?;
    let (menu, title, n) = screen(&bot);
    ctx.check(menu == "smoker", format!("menu {menu}"));
    ctx.check(title == "container.smoker", format!("title {title}"));
    ctx.check(n == 39, format!("{n} slots"));
    ctx.check(bot.wait_until(Duration::from_secs(2), |s| s.props.len() == 4), "no container_set_data (smoker)");
    let ct = bot.st().props.get(&3).copied();
    ctx.check(ct == Some(100), format!("smoker cook total {ct:?}"));
    ctx.metric("smoker_cook_total", *bot.st().props.get(&3).unwrap_or(&0) as f64, 0.0);
    ctx.check(bot.wait_until(Duration::from_secs(8), |s| s.container.get(2).map(|x| x.name() == "cooked_beef").unwrap_or(false)), "no cooked beef");
    // an iron ore does not cook in a smoker
    bot.close_container();
    let sm2 = [sm[0] + 2, Y, sm[2]];
    setblock(&bot, sm2, "smoker[facing=south]")?;
    put(ctx, &bot, sm2, 0, "iron_ore", 1)?; put(ctx, &bot, sm2, 1, "coal", 1)?;
    bot.wait_ticks(30);
    let cs = ctx.container(&bot, sm2)?.unwrap_or_default();
    ctx.check(cs.cook == 0 && cs.lit == 0 && cs.slot(2).is_none(), format!("smoker cooked a non-food item: {cs:?}"));
    ctx.metric("smoker_ignores_ore", (cs.cook == 0 && cs.lit == 0) as i32 as f64, 0.0);
    Ok(())
}

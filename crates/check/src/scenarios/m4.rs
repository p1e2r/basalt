//! M4 survival: digging, drops, durability, inventory clicks, crafting, armor, health and hunger,
//! death; item entity tracking.
use super::util::*;
use crate::bot::Bot;
use crate::runner::{Ctx, ARENA_Y};
use anyhow::{bail, Result};
use basalt_protocol::ids::play::clientbound as cb;
use std::time::{Duration, Instant};

const Y: i32 = ARENA_Y + 1;

fn survival_arena(ctx: &mut Ctx, sx: i32, sz: i32) -> Result<(Bot, [i32; 3])> {
    let bot = ctx.bot()?;
    let o = ctx.arena(&bot, sx, sz, 5)?;
    ctx.stand(&bot, o[0] + 2, o[2] + 2, "survival")?;
    Ok((bot, o))
}

pub fn dig_timing(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = survival_arena(ctx, 8, 8)?;
    give(&bot, "wooden_pickaxe", 1)?;
    hold(&bot, "wooden_pickaxe")?;
    let b = [o[0] + 4, Y, o[2] + 2];
    setblock(&bot, b, "stone")?;
    // too fast: start + finish in the same tick
    bot.look_at([b[0] as f64 + 0.5, b[1] as f64 + 0.5, b[2] as f64 + 0.5]); bot.wait_ticks(2);
    bot.player_action(0, b, 1); bot.player_action(2, b, 1);
    bot.wait_ticks(10);
    let fast_kept = ctx.block_is(&bot, b, "stone")?;
    ctx.check(fast_kept, "instant (too fast) break of stone with a wooden pickaxe was accepted");
    ctx.check(bot.st().world.name(b[0], b[1], b[2]) == "stone", "bot view: stone gone after rejected break");
    ctx.metric("too_fast_rejected", fast_kept as i32 as f64, 0.0);
    bot.player_action(1, b, 1);
    bot.wait_ticks(4);
    // proper timing: 1.5 hardness, wooden pickaxe speed 2 -> 23 ticks
    let t = bot.dig(b)?;
    let broke = bot.wait_until(Duration::from_secs(3), |s| s.world.name(b[0], b[1], b[2]) == "air");
    ctx.check(broke, "stone not broken after full dig time");
    ctx.check(ctx.block_is(&bot, b, "air")?, "server still has stone");
    ctx.metric("broke_with_timing", broke as i32 as f64, 0.0);
    ctx.note(format!("waited {t} ticks"));
    // hand-digging dirt (no tool needed) works too
    let d = [o[0] + 2, Y, o[2] + 4];
    setblock(&bot, d, "dirt")?;
    bot.set_held(5); bot.wait_ticks(2);
    bot.dig(d)?;
    ctx.check(bot.wait_until(Duration::from_secs(3), |s| s.world.name(d[0], d[1], d[2]) == "air"), "dirt not broken by hand");
    Ok(())
}

pub fn drops_pickup(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = survival_arena(ctx, 8, 8)?;
    let b = [o[0] + 3, Y, o[2] + 2];
    setblock(&bot, b, "dirt")?;
    let t0 = Instant::now();
    bot.set_held(0);
    bot.dig(b)?;
    let seen = bot.wait_until(Duration::from_secs(3), |s| s.entities.values().any(|e| e.kind == "item" && e.added >= t0 && e.item.as_ref().map(|i| i.name() == "dirt").unwrap_or(false)));
    ctx.check(seen, "no dirt item entity after breaking dirt");
    ctx.metric("item_entity_seen", seen as i32 as f64, 0.0);
    // walk onto it
    bot.walk_to(b[0] as f64 + 0.5, b[2] as f64 + 0.5, Duration::from_secs(5))?;
    let got = bot.wait_until(Duration::from_secs(5), |s| s.count("dirt") == 1);
    ctx.check(got, format!("dirt not picked up (inventory dirt {})", bot.st().count("dirt")));
    let eid = bot.st().eid;
    ctx.check(bot.st().taken.iter().any(|t| t.1 == eid && t.2 == 1), "no take_item_entity for the pickup");
    let st = ctx.state(&bot)?;
    ctx.check(st.count("dirt") == 1, format!("server inventory dirt {}", st.count("dirt")));
    ctx.metric("picked_up", st.count("dirt") as f64, 0.0);
    bot.wait_ticks(5);
    ctx.check(ctx.items_near(&bot, 5.0)? == 0, "item entity still exists after pickup");
    Ok(())
}

pub fn durability(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = survival_arena(ctx, 8, 8)?;
    give(&bot, "wooden_pickaxe", 1)?;
    give(&bot, "wooden_sword", 1)?;
    let damage_of = |bot: &Bot, item: &str| bot.st().inv.iter().find(|s| s.count > 0 && s.name() == item).map(|s| s.damage).unwrap_or(-1);
    hold(&bot, "wooden_pickaxe")?;
    for i in 0..2 {
        let b = [o[0] + 4, Y, o[2] + 1 + i];
        setblock(&bot, b, "stone")?;
        bot.dig(b)?;
        bot.wait_until(Duration::from_secs(3), |s| s.world.name(b[0], b[1], b[2]) == "air");
    }
    bot.wait_ticks(4);
    let pd = damage_of(&bot, "wooden_pickaxe");
    ctx.approx("pickaxe damage after 2 stone", pd as f64, 2.0, 0.0);
    hold(&bot, "wooden_sword")?;
    let b = [o[0] + 2, Y, o[2] + 4];
    setblock(&bot, b, "dirt")?;
    bot.dig(b)?;
    bot.wait_until(Duration::from_secs(3), |s| s.world.name(b[0], b[1], b[2]) == "air");
    bot.wait_ticks(4);
    let sd = damage_of(&bot, "wooden_sword");
    ctx.approx("sword damage after 1 dirt", sd as f64, 2.0, 0.0);
    let st = ctx.state(&bot)?;
    let sp = st.inv.values().find(|i| i.item == "wooden_pickaxe").map(|i| i.damage).unwrap_or(-1);
    ctx.check(sp == 2, format!("server pickaxe damage {sp}"));
    ctx.metric("pickaxe_damage", pd as f64, 0.0);
    ctx.metric("sword_damage", sd as f64, 0.0);
    Ok(())
}

pub fn q_drop(ctx: &mut Ctx) -> Result<()> {
    let (bot, _o) = survival_arena(ctx, 10, 10)?;
    give(&bot, "dirt", 10)?;
    hold(&bot, "dirt")?;
    bot.look(-90.0, 0.0); bot.wait_ticks(2);
    let t0 = Instant::now();
    bot.drop_held(false);
    ctx.check(bot.wait_until(Duration::from_secs(2), |s| s.count("dirt") == 9), format!("after Q: {} dirt", bot.st().count("dirt")));
    let one = bot.wait_until(Duration::from_secs(2), |s| s.entities.values().any(|e| e.added >= t0 && e.item.as_ref().map(|i| i.count == 1 && i.name() == "dirt").unwrap_or(false)));
    ctx.check(one, "no item entity with 1 dirt");
    let t1 = Instant::now();
    bot.drop_held(true);
    ctx.check(bot.wait_until(Duration::from_secs(2), |s| s.count("dirt") == 0), format!("after ctrl-Q: {} dirt", bot.st().count("dirt")));
    let nine = bot.wait_until(Duration::from_secs(2), |s| s.entities.values().any(|e| e.added >= t1 && e.item.as_ref().map(|i| i.count == 9).unwrap_or(false)));
    ctx.check(nine, "no item entity with 9 dirt");
    // thrown forward (yaw -90 = +x) from eye height
    let (v, p0) = { let s = bot.st(); let e = s.entities.values().filter(|e| e.added >= t1).max_by_key(|e| e.added).cloned(); (e.as_ref().map(|e| e.spawn_vel).unwrap_or_default(), e.map(|e| e.spawn_pos).unwrap_or_default()) };
    ctx.check(v[0] > 0.1, format!("thrown item velocity {v:?} not along +x"));
    let me = bot.st().pos();
    ctx.approx("thrown item spawn height above feet", p0[1] - me[1], 1.32, 0.05);
    ctx.metric("thrown_vx", (v[0] * 10.0).round() / 10.0, 0.1);
    ctx.metric("thrown_dy", ((p0[1] - me[1]) * 100.0).round() / 100.0, 0.02);
    ctx.metric("q_one", one as i32 as f64, 0.0);
    ctx.metric("q_stack", nine as i32 as f64, 0.0);
    Ok(())
}

pub fn craft_2x2(ctx: &mut Ctx) -> Result<()> {
    let (bot, _o) = survival_arena(ctx, 6, 6)?;
    give(&bot, "oak_log", 2)?;
    let slot = bot.st().find("oak_log").unwrap() as i16;
    // pick up the logs, right-click one into grid slot 1, put the rest back
    bot.click(slot, 0, 0); bot.wait_ticks(3);
    bot.click(1, 1, 0); bot.wait_ticks(3);
    bot.click(slot, 0, 0); bot.wait_ticks(3);
    let res = bot.wait_until(Duration::from_secs(2), |s| s.inv[0].name() == "oak_planks" && s.inv[0].count == 4);
    let r0 = bot.st().inv[0].clone();
    ctx.check(res, format!("craft result slot: {} x{}", r0.name(), r0.count));
    bot.click(0, 0, 1); // shift-click the result: crafts all (2 logs -> 8 planks)
    let got = bot.wait_until(Duration::from_secs(2), |s| s.count("oak_planks") >= 4);
    bot.wait_ticks(4);
    let planks = bot.st().count("oak_planks");
    ctx.check(got, format!("planks after shift-click: {planks}"));
    let st = ctx.state(&bot)?;
    ctx.check(st.count("oak_planks") == planks as i64, format!("server planks {} vs bot {planks}", st.count("oak_planks")));
    ctx.metric("planks", planks as f64, 0.0);
    ctx.metric("logs_left", st.count("oak_log") as f64, 0.0);
    Ok(())
}

pub fn craft_table(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = survival_arena(ctx, 6, 6)?;
    give(&bot, "crafting_table", 1)?;
    give(&bot, "oak_planks", 3)?;
    give(&bot, "stick", 2)?;
    hold(&bot, "crafting_table")?;
    let t = [o[0] + 4, Y, o[2] + 2];
    bot.look_at([t[0] as f64 + 0.5, t[1] as f64, t[2] as f64 + 0.5]); bot.wait_ticks(2);
    bot.place([t[0], t[1] - 1, t[2]], 1);
    ctx.check(bot.wait_until(Duration::from_secs(2), |s| s.world.name(t[0], t[1], t[2]) == "crafting_table"), "crafting table not placed");
    bot.look_at([t[0] as f64 + 0.5, t[1] as f64 + 0.5, t[2] as f64 + 0.5]); bot.wait_ticks(2);
    bot.place(t, 1);
    if !bot.wait_until(Duration::from_secs(2), |s| s.screen.as_ref().map(|x| x.1 == "crafting").unwrap_or(false) && s.container.len() >= 46) { bail!("crafting screen did not open ({:?})", bot.st().screen); }
    let cslot = |item: &str| -> Option<i16> { let s = bot.st(); (10..46).find(|&i| s.container.get(i).map(|x| x.count > 0 && x.name() == item).unwrap_or(false)).map(|i| i as i16) };
    let planks = cslot("oak_planks").ok_or_else(|| anyhow::anyhow!("planks not in crafting window"))?;
    bot.click(planks, 0, 0); bot.wait_ticks(2);
    for g in [1, 2, 3] { bot.click(g, 1, 0); bot.wait_ticks(2); }
    let sticks = cslot("stick").ok_or_else(|| anyhow::anyhow!("sticks not in crafting window"))?;
    bot.click(sticks, 0, 0); bot.wait_ticks(2);
    for g in [5, 8] { bot.click(g, 1, 0); bot.wait_ticks(2); }
    let res = bot.wait_until(Duration::from_secs(2), |s| s.container.first().map(|r| r.name() == "wooden_pickaxe").unwrap_or(false));
    ctx.check(res, format!("3x3 result: {:?}", bot.st().container.first().map(|r| r.name())));
    bot.click(0, 0, 1); bot.wait_ticks(4);
    bot.close_container(); bot.wait_ticks(4);
    let n = bot.st().count("wooden_pickaxe");
    ctx.check(n == 1, format!("wooden pickaxes in inventory: {n}"));
    let st = ctx.state(&bot)?;
    ctx.check(st.count("wooden_pickaxe") == 1 && st.count("oak_planks") == 0 && st.count("stick") == 0, format!("server inventory {:?}", st.inv));
    ctx.metric("pickaxes", st.count("wooden_pickaxe") as f64, 0.0);
    ctx.metric("leftover", (st.count("oak_planks") + st.count("stick")) as f64, 0.0);
    Ok(())
}

pub fn armor_slots(ctx: &mut Ctx) -> Result<()> {
    let (bot, _o) = survival_arena(ctx, 6, 6)?;
    give(&bot, "dirt", 1)?;
    give(&bot, "iron_helmet", 1)?;
    give(&bot, "iron_chestplate", 1)?;
    let find = |i: &str| bot.st().find(i).unwrap() as i16;
    let (dirt, helm, chest) = (find("dirt"), find("iron_helmet"), find("iron_chestplate"));
    // dirt onto the head slot: refused
    bot.click(dirt, 0, 0); bot.wait_ticks(2); bot.click(5, 0, 0); bot.wait_ticks(3);
    let dirt_refused = bot.st().inv[5].is_empty();
    ctx.check(dirt_refused, "head slot accepted dirt");
    bot.click(dirt, 0, 0); bot.wait_ticks(3);
    // helmet onto the head slot: accepted
    bot.click(helm, 0, 0); bot.wait_ticks(2); bot.click(5, 0, 0); bot.wait_ticks(3);
    let helm_ok = bot.st().inv[5].name() == "iron_helmet";
    ctx.check(helm_ok, format!("head slot has {}", bot.st().inv[5].name()));
    // chestplate onto the (occupied) head slot: refused, cursor keeps it
    bot.click(chest, 0, 0); bot.wait_ticks(2); bot.click(5, 0, 0); bot.wait_ticks(3);
    let chest_refused = bot.st().inv[5].name() == "iron_helmet";
    ctx.check(chest_refused, format!("head slot took a chestplate: {}", bot.st().inv[5].name()));
    bot.click(chest, 0, 0); bot.wait_ticks(3);
    // shift-click equips into the chest slot
    bot.click(chest, 0, 1); bot.wait_ticks(4);
    let shift_ok = bot.st().inv[6].name() == "iron_chestplate";
    ctx.check(shift_ok, format!("chest slot after shift-click: {}", bot.st().inv[6].name()));
    let st = ctx.state(&bot)?;
    ctx.check(st.inv.get(&5).map(|i| i.item.as_str()) == Some("iron_helmet") && st.inv.get(&6).map(|i| i.item.as_str()) == Some("iron_chestplate"), format!("server armor {:?} {:?}", st.inv.get(&5), st.inv.get(&6)));
    ctx.check(bot.st().cursor.is_empty(), "cursor not empty at the end");
    for (k, v) in [("dirt_refused", dirt_refused), ("helmet_ok", helm_ok), ("chest_refused", chest_refused), ("shift_equip", shift_ok)] { ctx.metric(k, v as i32 as f64, 0.0); }
    Ok(())
}

pub fn fall_damage(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = survival_arena(ctx, 6, 6)?;
    let p = [o[0] as f64 + 2.5, Y as f64, o[2] as f64 + 2.5];
    // 3 blocks: no damage
    let t0 = Instant::now();
    bot.tp([p[0], p[1] + 3.0, p[2]])?;
    bot.wait_ticks(5); bot.wait_on_ground(Duration::from_secs(3)); bot.wait_ticks(5);
    let small = hits(&bot, t0, "fall");
    ctx.check(small.is_empty(), format!("3-block fall hurt: {small:?}"));
    // 10 blocks: 7 damage
    let t1 = Instant::now();
    bot.tp([p[0], p[1] + 10.0, p[2]])?;
    bot.wait_ticks(5); bot.wait_on_ground(Duration::from_secs(4));
    bot.wait_until(Duration::from_secs(1), |s| s.damage.iter().any(|d| d.0 >= t1));
    bot.wait_ticks(2);
    let big = hits(&bot, t1, "fall");
    ctx.check(big.len() == 1, format!("10-block fall hits: {big:?}"));
    let dmg = big.first().copied().unwrap_or(0.0) as f64;
    ctx.approx("10-block fall damage", dmg, 7.0, 0.0);
    ctx.metric("fall_3", small.iter().sum::<f32>() as f64, 0.0);
    ctx.metric("fall_10", dmg, 0.0);
    // 20 blocks onto hay: 17 * 0.2 = 3.4 -> 3
    let hay = [o[0] + 4, Y, o[2] + 4];
    setblock(&bot, hay, "hay_block")?;
    let t2 = Instant::now();
    bot.wait_ticks(25); // let invulnerability/regen settle
    bot.tp([hay[0] as f64 + 0.5, Y as f64 + 21.0, hay[2] as f64 + 0.5])?;
    bot.wait_ticks(10); bot.wait_on_ground(Duration::from_secs(5)); bot.wait_ticks(4);
    let h = hits(&bot, t2, "fall");
    ctx.metric("fall_20_hay", h.iter().sum::<f32>() as f64, 0.0);
    ctx.note(format!("hay landing: {h:?}"));
    Ok(())
}

pub fn exhaustion(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    let o = ctx.arena(&bot, 52, 3, 4)?;
    ctx.stand(&bot, o[0] + 1, o[2] + 1, "survival")?;
    let s0 = bot.st().saturation;
    ctx.approx("initial saturation", s0 as f64, 5.0, 0.0);
    bot.sprint(true);
    let x0 = bot.st().pos()[0];
    bot.walk_to(o[0] as f64 + 49.0, o[2] as f64 + 1.5, Duration::from_secs(20))?;
    bot.sprint(false);
    bot.wait_ticks(10);
    let dist = bot.st().pos()[0] - x0;
    // vanilla only sends set_health when food/health change, so ask the server for saturation
    let (bot_sat, food) = { let s = bot.st(); (s.saturation, s.food) };
    let sat = ctx.state(&bot)?.saturation as f32;
    ctx.note(format!("bot-visible saturation {bot_sat}"));
    ctx.note(format!("sprinted {dist:.1} blocks: saturation {s0} -> {sat}, food {food}"));
    ctx.check(dist > 40.5, format!("only sprinted {dist:.1} blocks"));
    ctx.approx("saturation after ~45 blocks of sprinting (4.5 exhaustion)", sat as f64, 4.0, 0.0);
    ctx.approx("food", food as f64, 20.0, 0.0);
    ctx.metric("saturation_after_sprint", sat as f64, 0.0);
    // jumping: 0.05 exhaustion per jump (sprint-jump 0.2) — 20 sprint-jumps = 4.0 more
    if ctx.is_vanilla() { let e = crate::query::vanilla_get(&bot, "foodExhaustionLevel")?; ctx.note(format!("vanilla exhaustion {e:?}")); }
    Ok(())
}

pub fn eating(ctx: &mut Ctx) -> Result<()> {
    let (bot, _o) = survival_arena(ctx, 6, 6)?;
    bot.cmd_reply("effect give @s minecraft:hunger 10 100")?;
    let dropped = bot.wait_until(Duration::from_secs(8), |s| s.food <= 14);
    bot.cmd_reply("effect clear @s minecraft:hunger")?;
    ctx.check(dropped, format!("hunger effect didn't drain food (food {})", bot.st().food));
    bot.wait_ticks(5);
    let f0 = bot.st().food;
    give(&bot, "bread", 2)?;
    hold(&bot, "bread")?;
    let t0 = Instant::now();
    bot.use_item();
    let ate = bot.wait_until(Duration::from_secs(4), |s| s.food > f0);
    let ticks = t0.elapsed().as_secs_f64() * 20.0;
    bot.wait_ticks(4);
    let (f1, s1, n) = { let s = bot.st(); (s.food, s.saturation, s.count("bread")) };
    ctx.check(ate, "eating bread did nothing");
    ctx.approx("food gained from bread", (f1 - f0) as f64, 5.0, 0.0);
    ctx.check(n == 1, format!("bread left {n}"));
    ctx.approx("eat duration ticks", ticks, 32.0, 5.0);
    ctx.metric("food_gain", (f1 - f0) as f64, 0.0);
    ctx.metric("saturation_after", s1 as f64, 0.01);
    ctx.metric("eat_ticks", ticks.round(), 4.0);
    ctx.note(format!("food {f0} -> {f1}, saturation {s1}, {ticks:.0} ticks"));
    // can't eat at full food (bread is not always-edible)
    Ok(())
}

pub fn death_respawn(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = survival_arena(ctx, 8, 8)?;
    give(&bot, "dirt", 5)?;
    give(&bot, "cobblestone", 3)?;
    let t0 = Instant::now();
    let pos = bot.st().pos();
    bot.cmd("kill @s");
    ctx.check(bot.wait_until(Duration::from_secs(3), |s| s.dead), "no death after /kill");
    ctx.check(bot.wait_until(Duration::from_secs(1), |s| s.health <= 0.0), "health not 0 after death");
    let cause = bot.st().death_msg.clone().map(|m| death_cause(&m)).unwrap_or_default();
    ctx.check(cause == "kill", format!("death message cause {cause}"));
    bot.wait_ticks(10);
    let items = bot.st().items_near(pos, 6.0).into_iter().filter(|(_, e)| e.added >= t0).filter_map(|(_, e)| e.item).collect::<Vec<_>>();
    let total: i32 = items.iter().map(|i| i.count).sum();
    ctx.check(total == 8, format!("dropped items {:?}", items.iter().map(|i| (i.name(), i.count)).collect::<Vec<_>>()));
    ctx.metric("dropped_total", total as f64, 0.0);
    ctx.metric("dropped_stacks", items.len() as f64, 0.0);
    bot.respawn();
    ctx.check(bot.wait_until(Duration::from_secs(10), |s| !s.dead && s.spawned && s.world.loaded(s.pos()[0] as i32, s.pos()[2] as i32)), "respawn did not complete");
    bot.wait_ticks(5);
    let s = bot.st();
    ctx.approx("health after respawn", s.health as f64, 20.0, 0.0);
    ctx.approx("food after respawn", s.food as f64, 20.0, 0.0);
    ctx.check(s.inv.iter().all(|i| i.is_empty()), "inventory not empty after respawn");
    let away = ((s.pos()[0] - o[0] as f64).powi(2) + (s.pos()[2] - o[2] as f64).powi(2)).sqrt();
    ctx.check(away > 10.0 || (s.pos()[1] - pos[1]).abs() > 5.0, format!("respawned at the death spot {:?}", s.pos()));
    Ok(())
}

pub fn item_tracking(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = survival_arena(ctx, 10, 10)?;
    let b = [o[0] + 5, Y, o[2] + 2];
    setblock(&bot, b, "dirt")?;
    let t0 = Instant::now();
    bot.dig(b)?;
    if !bot.wait_until(Duration::from_secs(3), |s| s.entities.values().any(|e| e.kind == "item" && e.added >= t0)) { bail!("no item entity spawned"); }
    let (eid, ent) = bot.st().entities.iter().find(|(_, e)| e.kind == "item" && e.added >= t0).map(|(k, e)| (*k, e.clone())).unwrap();
    let v = ent.spawn_vel;
    ctx.check(v[1] > 0.05, format!("add_entity velocity {v:?} has no upward pop"));
    ctx.check(v[0].abs() <= 0.11 && v[2].abs() <= 0.11, format!("add_entity horizontal velocity {v:?}"));
    ctx.metric("spawn_vy", (v[1] * 100.0).round() / 100.0, 0.02);
    ctx.check(ent.item.as_ref().map(|i| i.name()) == Some("dirt"), "item entity data (index 8) missing or wrong");
    // watch 60 ticks of tracking
    bot.wait_ticks(60);
    let log: Vec<_> = { let s = bot.st(); s.plog.iter().filter(|l| l.eid == eid && l.at >= ent.added).cloned().collect() };
    let count = |id: i32| log.iter().filter(|l| l.id == id).count() as f64;
    let (moves, motion, syncs) = (count(cb::MOVE_ENTITY_POS) + count(cb::MOVE_ENTITY_POS_ROT), count(cb::SET_ENTITY_MOTION), count(cb::ENTITY_POSITION_SYNC));
    ctx.note(format!("60 ticks: {moves} move_entity_pos, {motion} set_entity_motion, {syncs} entity_position_sync"));
    ctx.metric("pos_packets_60t", moves + syncs, 1.0);
    ctx.metric("motion_packets_60t", motion, 3.0);
    ctx.metric("sync_packets_60t", syncs, 1.0);
    ctx.check(moves + syncs >= 1.0 && moves + syncs <= 8.0, "item position update count outside 1..8 per 60 ticks");
    // the bot's tracked position should match the server's
    let bp = bot.st().entities.get(&eid).map(|e| e.pos).unwrap_or_default();
    ctx.approx("item rests on the floor (bot view y)", bp[1], Y as f64, 0.01);
    let sp = match ctx.kind {
        crate::query::Kind::Basalt => {
            let lines = bot.cmd_reply("basalt:items 10")?;
            let j: serde_json::Value = lines.iter().find_map(|l| l.plain.strip_prefix("BASALT_ITEMS ").and_then(|s| serde_json::from_str(s).ok())).unwrap_or_default();
            j.as_array().and_then(|a| a.iter().find(|e| e["eid"] == eid)).map(|e| [e["pos"][0].as_f64().unwrap_or(0.0), e["pos"][1].as_f64().unwrap_or(0.0), e["pos"][2].as_f64().unwrap_or(0.0)])
        }
        crate::query::Kind::Vanilla => {
            let lines = bot.cmd_reply("data get entity @e[type=item,sort=nearest,limit=1] Pos")?;
            lines.iter().find(|l| l.key.as_deref() == Some("commands.data.entity.query")).and_then(|l| crate::proto::parse_snbt(l.args.get(1)?)).map(|v| [v[0].as_f64().unwrap_or(0.0), v[1].as_f64().unwrap_or(0.0), v[2].as_f64().unwrap_or(0.0)])
        }
    };
    match sp {
        Some(sp) => { let d: f64 = (0..3).map(|i| (sp[i] - bp[i]).powi(2)).sum::<f64>().sqrt(); ctx.check(d < 0.05, format!("bot sees item at {bp:?}, server has {sp:?}")); ctx.metric("tracked_pos_error", (d * 100.0).round() / 100.0, 0.05); }
        None => { ctx.check(false, "server doesn't report the item"); }
    }
    Ok(())
}

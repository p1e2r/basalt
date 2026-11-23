//! Environmental damage: drowning, freezing, suffocation, lava, fire, cactus, berry bushes, magma, void.
use super::util::*;
use crate::bot::Bot;
use crate::runner::{Ctx, ARENA_Y};
use anyhow::Result;
use std::time::{Duration, Instant};

const Y: i32 = ARENA_Y + 1;

fn setup(ctx: &mut Ctx) -> Result<(Bot, [i32; 3])> {
    let bot = ctx.bot()?;
    let o = ctx.arena(&bot, 12, 12, 6)?;
    // roofed: rain on the vanilla server would put out fires and refill nothing else
    fill(&bot, [o[0] - 1, Y + 5, o[2] - 1], [o[0] + 12, Y + 5, o[2] + 12], "stone")?;
    ctx.stand(&bot, o[0] + 1, o[2] + 1, "survival")?;
    Ok((bot, o))
}

/// A 1×1 column of `block` `h` high at (x, z), walled with stone, open on top.
fn column(bot: &Bot, x: i32, z: i32, h: i32, block: &str) -> Result<()> {
    fill(bot, [x - 1, Y, z - 1], [x + 1, Y + h, z + 1], "stone")?;
    fill(bot, [x, Y, z], [x, Y + h - 1, z], block)?;
    // open above (a block over the head would make the player crawl)
    fill(bot, [x, Y + h, z], [x, Y + h + 1, z], "air")?;
    Ok(())
}

fn enter(bot: &Bot, x: i32, z: i32) -> Result<Instant> {
    bot.tp([x as f64 + 0.5, Y as f64, z as f64 + 0.5])?;
    Ok(Instant::now())
}

pub fn drowning(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = setup(ctx)?;
    let (x, z) = (o[0] + 6, o[2] + 6);
    column(&bot, x, z, 3, "water")?;
    let t0 = enter(&bot, x, z)?;
    let air100 = { bot.wait_ticks(100); bot.st().air };
    ctx.approx("air after 100 ticks submerged", air100 as f64, 200.0, 6.0);
    ctx.metric("air_after_100", air100 as f64, 4.0);
    ticks_until(&bot, 300, |s| s.damage.iter().any(|d| d.0 >= t0 && d.1 == "drown"))?;
    let first = hit_ticks(&bot, t0, "drown")[0];
    ctx.approx("ticks until first drowning damage", first, 320.0, 8.0);
    ctx.metric("first_drown_tick", first.round(), 6.0);
    bot.wait_until(Duration::from_secs(3), |s| s.damage.iter().filter(|d| d.0 >= t0 && d.1 == "drown").count() >= 2);
    bot.wait_ticks(2);
    let ts = hit_ticks(&bot, t0, "drown");
    let dm = hits(&bot, t0, "drown");
    ctx.check(dm.first().copied() == Some(2.0), format!("drowning damage per hit {dm:?}"));
    if ts.len() >= 2 { ctx.approx("ticks between drowning hits", ts[1] - ts[0], 20.0, 3.0); ctx.metric("drown_interval", (ts[1] - ts[0]).round(), 3.0); } else { ctx.check(false, "second drowning hit missing"); }
    ctx.metric("drown_damage", dm.first().copied().unwrap_or(0.0) as f64, 0.0);
    // out of the water: +4 air per tick
    let a0 = bot.st().air;
    bot.tp([o[0] as f64 + 2.5, Y as f64, o[2] as f64 + 2.5])?;
    let refill = ticks_until(&bot, 120, |s| s.air >= 300)?;
    ctx.approx("ticks to refill from ~0", refill, (300 - a0.max(0)) as f64 / 4.0, 4.0);
    ctx.metric("refill_ticks", refill.round(), 3.0);
    Ok(())
}

pub fn respiration(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = setup(ctx)?;
    give(&bot, "iron_helmet", 1)?;
    hold(&bot, "iron_helmet")?;
    bot.cmd_reply("enchant @s minecraft:respiration 3")?;
    let slot = bot.st().find("iron_helmet").unwrap() as i16;
    bot.click(slot, 0, 1);
    ctx.check(bot.wait_until(Duration::from_secs(2), |s| s.inv[5].name() == "iron_helmet" && !s.inv[5].ench.is_empty()), format!("enchanted helmet not worn: {:?}", bot.st().inv[5]));
    let (x, z) = (o[0] + 6, o[2] + 6);
    column(&bot, x, z, 3, "water")?;
    enter(&bot, x, z)?;
    bot.wait_ticks(200);
    let air = bot.st().air;
    // vanilla: each tick air is lost with chance 1/(level+1) → ~50 of 200
    ctx.check(air > 200 && air < 290, format!("air after 200 ticks with Respiration III: {air}"));
    ctx.metric("air_after_200", air as f64, 30.0);
    Ok(())
}

pub fn water_breathing(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = setup(ctx)?;
    bot.cmd_reply("effect give @s minecraft:water_breathing 60 0")?;
    let (x, z) = (o[0] + 6, o[2] + 6);
    column(&bot, x, z, 3, "water")?;
    enter(&bot, x, z)?;
    let min = sample(&bot, 100, |s| s.air).into_iter().min().unwrap_or(0);
    ctx.approx("minimum air with Water Breathing", min as f64, 300.0, 0.0);
    ctx.metric("min_air", min as f64, 0.0);
    Ok(())
}

pub fn freezing(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = setup(ctx)?;
    let (x, z) = (o[0] + 6, o[2] + 6);
    column(&bot, x, z, 2, "powder_snow")?;
    let t0 = enter(&bot, x, z)?;
    bot.wait_ticks(60);
    let f60 = bot.st().ticks_frozen;
    ctx.approx("ticks_frozen after 60 ticks", f60 as f64, 60.0, 5.0);
    ctx.metric("frozen_after_60", f60 as f64, 4.0);
    let full = ticks_until(&bot, 150, |s| s.ticks_frozen >= 140)?;
    ctx.approx("ticks to fully frozen", full + 60.0, 140.0, 6.0);
    bot.wait_ticks(5);
    ctx.approx("ticks_frozen caps at 140", bot.st().ticks_frozen as f64, 140.0, 0.0);
    // damage every 40 ticks once fully frozen
    bot.wait_until(Duration::from_secs(5), |s| s.damage.iter().filter(|d| d.0 >= t0 && d.1 == "freeze").count() >= 2);
    bot.wait_ticks(2);
    let ts = hit_ticks(&bot, t0, "freeze");
    let dm = hits(&bot, t0, "freeze");
    ctx.check(ts.len() >= 2, format!("freeze hits {ts:?}"));
    if ts.len() >= 2 { ctx.approx("ticks between freeze hits", ts[1] - ts[0], 40.0, 3.0); ctx.metric("freeze_interval", (ts[1] - ts[0]).round(), 3.0); }
    ctx.metric("freeze_damage", dm.first().copied().unwrap_or(0.0) as f64, 0.0);
    ctx.check(dm.first().copied() == Some(1.0), format!("freeze damage {dm:?}"));
    // out: thaw 2 per tick
    bot.tp([o[0] as f64 + 2.5, Y as f64, o[2] as f64 + 2.5])?;
    let thaw = ticks_until(&bot, 120, |s| s.ticks_frozen == 0)?;
    ctx.approx("thaw ticks from 140", thaw, 70.0, 4.0);
    ctx.metric("thaw_ticks", thaw.round(), 3.0);
    Ok(())
}

pub fn freeze_leather(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = setup(ctx)?;
    give(&bot, "leather_boots", 1)?;
    let slot = bot.st().find("leather_boots").unwrap() as i16;
    bot.click(slot, 0, 1);
    ctx.check(bot.wait_until(Duration::from_secs(2), |s| s.inv[8].name() == "leather_boots"), "leather boots not worn");
    let (x, z) = (o[0] + 6, o[2] + 6);
    column(&bot, x, z, 2, "powder_snow")?;
    enter(&bot, x, z)?;
    let max = sample(&bot, 60, |s| s.ticks_frozen).into_iter().max().unwrap_or(0);
    ctx.approx("max ticks_frozen with leather boots", max as f64, 0.0, 0.0);
    ctx.metric("frozen_with_leather", max as f64, 0.0);
    Ok(())
}

pub fn freeze_fire_thaw(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = setup(ctx)?;
    let (x, z) = (o[0] + 6, o[2] + 6);
    column(&bot, x, z, 2, "powder_snow")?;
    fill(&bot, [o[0] + 2, Y, o[2] + 8], [o[0] + 2, Y, o[2] + 8], "lava")?;
    enter(&bot, x, z)?;
    bot.wait_until(Duration::from_secs(8), |s| s.ticks_frozen >= 100);
    let before = bot.st().ticks_frozen;
    bot.tp([o[0] as f64 + 2.5, Y as f64, o[2] as f64 + 8.5])?;
    bot.wait_ticks(3);
    let after = bot.st().ticks_frozen;
    ctx.note(format!("ticks_frozen {before} -> {after} three ticks after entering lava"));
    ctx.check(after <= before - 6, format!("no thaw in lava: {before} -> {after}"));
    ctx.metric("frozen_3t_after_lava", after as f64, 2.0);
    // leave the lava before it kills us
    bot.tp([o[0] as f64 + 9.5, Y as f64, o[2] as f64 + 2.5])?;
    Ok(())
}

pub fn suffocation(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = setup(ctx)?;
    let (x, z) = (o[0] + 6, o[2] + 6);
    enter(&bot, x, z)?;
    // fully enclosed (feet and head): 1 damage per 10 ticks
    fill(&bot, [x, Y, z], [x, Y + 1, z], "sand")?;
    let t0 = Instant::now();
    bot.wait_ticks(40);
    let dm = hits(&bot, t0, "in_wall");
    ctx.check(dm.len() >= 3 && dm.len() <= 5, format!("in_wall hits in 40 ticks: {dm:?}"));
    ctx.check(dm.iter().all(|d| *d == 1.0), format!("in_wall damage {dm:?}"));
    ctx.metric("in_wall_hits_40t", dm.len() as f64, 1.0);
    ctx.metric("in_wall_damage", dm.first().copied().unwrap_or(0.0) as f64, 0.0);
    // only the head block solid: vanilla switches the player to the crawling pose (eyes at 0.4) — no damage
    let (x2, z2) = (o[0] + 3, o[2] + 8);
    enter(&bot, x2, z2)?;
    setblock(&bot, [x2, Y + 1, z2], "stone")?;
    let t1 = Instant::now();
    bot.wait_ticks(40);
    let crawl = hits(&bot, t1, "in_wall");
    ctx.note(format!("head-only block: {} in_wall hits", crawl.len()));
    ctx.metric("head_only_in_wall_hits_40t", crawl.len() as f64, 0.0);
    Ok(())
}

pub fn lava(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = setup(ctx)?;
    let (x, z) = (o[0] + 6, o[2] + 6);
    column(&bot, x, z, 1, "lava")?;
    column(&bot, o[0] + 2, o[2] + 9, 2, "water")?;
    let t0 = enter(&bot, x, z)?;
    let flags = sample(&bot, 25, |s| s.flags);
    let burning = flags.iter().any(|f| f & 1 != 0);
    let dm = hits(&bot, t0, "lava");
    let fire_ticks = ctx.state(&bot)?.fire_ticks;
    ctx.note(format!("server fire ticks after 25+ ticks in lava: {fire_ticks}; flags seen {:?}", { let mut f = flags.clone(); f.dedup(); f }));
    let kinds = damage_kinds(&bot, t0);
    ctx.check(burning, "not burning in lava");
    ctx.check(dm.len() >= 2 && dm.len() <= 4, format!("lava hits in 25 ticks: {dm:?}"));
    ctx.check(dm.first().copied() == Some(4.0), format!("lava damage {dm:?}"));
    ctx.metric("lava_hits_25t", dm.len() as f64, 1.0);
    ctx.metric("lava_damage", dm.first().copied().unwrap_or(0.0) as f64, 0.0);
    ctx.metric("burning_in_lava", burning as i32 as f64, 0.0);
    ctx.note(format!("damage types: {kinds:?}"));
    // water puts the fire out
    bot.tp([o[0] as f64 + 2.5, Y as f64, o[2] as f64 + 9.5])?;
    let out = bot.wait_until(Duration::from_secs(2), |s| s.flags & 1 == 0);
    ctx.check(out, "still burning in water");
    ctx.metric("extinguished_in_water", out as i32 as f64, 0.0);
    Ok(())
}

pub fn fire(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = setup(ctx)?;
    let (x, z) = (o[0] + 6, o[2] + 6);
    setblock(&bot, [x, Y, z], "fire")?;
    let t0 = enter(&bot, x, z)?;
    // vanilla players start at Fire -20: standing in fire ignites after ~20 ticks
    let flags = sample(&bot, 40, |s| s.flags);
    let burning = flags.iter().any(|f| f & 1 != 0);
    let ignite_tick = flags.iter().position(|f| f & 1 != 0).map(|t| t as f64).unwrap_or(-1.0);
    ctx.metric("ignite_tick_in_fire", (ignite_tick / 5.0).round() * 5.0, 10.0);
    let infire = hits(&bot, t0, "in_fire");
    ctx.check(burning, "fire block didn't set the player on fire");
    ctx.check(!infire.is_empty(), "no in_fire damage");
    ctx.metric("in_fire_hits_40t", infire.len() as f64, 1.0);
    ctx.metric("in_fire_damage", infire.first().copied().unwrap_or(0.0) as f64, 0.0);
    let t1 = Instant::now();
    bot.tp([o[0] as f64 + 2.5, Y as f64, o[2] as f64 + 2.5])?;
    bot.wait_ticks(60);
    let burn = hits(&bot, t1, "on_fire");
    let still = bot.st().flags & 1 != 0;
    ctx.check(!burn.is_empty(), "no burning (on_fire) damage after leaving the fire");
    ctx.metric("on_fire_hits_60t", burn.len() as f64, 1.0);
    ctx.metric("on_fire_damage", burn.first().copied().unwrap_or(0.0) as f64, 0.0);
    let gone = bot.wait_until(Duration::from_secs(12), |s| s.flags & 1 == 0);
    let total = t1.elapsed().as_secs_f64() * 20.0;
    ctx.check(gone, "never stopped burning");
    ctx.metric("burn_ticks_after_leaving", (total / 10.0).round() * 10.0, 20.0);
    ctx.note(format!("burning after leaving: {still}, stopped after {total:.0} ticks"));
    Ok(())
}

pub fn cactus(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = setup(ctx)?;
    let (x, z) = (o[0] + 6, o[2] + 6);
    fill(&bot, [x, Y - 2, z], [x, Y - 2, z], "stone")?; // support for the sand
    setblock(&bot, [x, Y - 1, z], "sand")?;
    setblock(&bot, [x, Y, z], "cactus")?;
    ctx.stand(&bot, x - 2, z, "survival")?;
    let t0 = Instant::now();
    push(&bot, -90.0, 40);
    let dm = hits(&bot, t0, "cactus");
    ctx.check(!dm.is_empty(), format!("walking into a cactus did no damage (pos {:?})", bot.st().pos()));
    ctx.check(dm.iter().all(|d| *d == 1.0), format!("cactus damage {dm:?}"));
    ctx.metric("cactus_hit", (!dm.is_empty()) as i32 as f64, 0.0);
    ctx.metric("cactus_damage", dm.first().copied().unwrap_or(0.0) as f64, 0.0);
    ctx.note(format!("{} cactus hits in 40 ticks of pushing", dm.len()));
    Ok(())
}

pub fn berry_bush(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = setup(ctx)?;
    let (x, z) = (o[0] + 6, o[2] + 6);
    setblock(&bot, [x, Y - 1, z], "grass_block")?;
    setblock(&bot, [x, Y, z], "sweet_berry_bush[age=3]")?;
    ctx.stand(&bot, x - 3, z, "survival")?;
    let t0 = Instant::now();
    bot.walk_to(x as f64 + 3.5, z as f64 + 0.5, Duration::from_secs(20))?;
    let dm = hits(&bot, t0, "sweet_berry_bush");
    ctx.check(!dm.is_empty(), "walking through a grown berry bush did no damage");
    ctx.check(dm.iter().all(|d| *d == 1.0), format!("berry bush damage {dm:?}"));
    ctx.metric("berry_hit", (!dm.is_empty()) as i32 as f64, 0.0);
    ctx.metric("berry_damage", dm.first().copied().unwrap_or(0.0) as f64, 0.0);
    // standing still inside: no damage
    bot.tp([x as f64 + 0.5, Y as f64, z as f64 + 0.5])?;
    bot.wait_ticks(15);
    let t1 = Instant::now();
    bot.wait_ticks(30);
    let still = hits(&bot, t1, "sweet_berry_bush");
    ctx.check(still.is_empty(), format!("standing still in a berry bush hurt: {still:?}"));
    ctx.metric("berry_still_hits", still.len() as f64, 0.0);
    Ok(())
}

pub fn magma(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = setup(ctx)?;
    let (x, z) = (o[0] + 6, o[2] + 6);
    setblock(&bot, [x, Y - 1, z], "magma_block")?;
    ctx.stand(&bot, x, z, "survival")?;
    let t0 = Instant::now();
    bot.wait_ticks(40);
    let dm = hits(&bot, t0, "hot_floor");
    ctx.check(dm.len() >= 3 && dm.len() <= 5, format!("magma hits in 40 ticks: {dm:?}"));
    ctx.metric("magma_hits_40t", dm.len() as f64, 1.0);
    ctx.metric("magma_damage", dm.first().copied().unwrap_or(0.0) as f64, 0.0);
    bot.sneak(true);
    bot.wait_ticks(15);
    let t1 = Instant::now();
    bot.wait_ticks(40);
    let sn = hits(&bot, t1, "hot_floor");
    bot.sneak(false);
    ctx.check(sn.is_empty(), format!("magma hurt while sneaking: {sn:?}"));
    ctx.metric("magma_sneak_hits", sn.len() as f64, 0.0);
    Ok(())
}

pub fn void(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = setup(ctx)?;
    let t0 = Instant::now();
    bot.cmd(&format!("tp @s {} -140 {}", o[0] + 6, o[2] + 6));
    let died = ticks_until(&bot, 200, |s| s.dead)?;
    let dm = hits(&bot, t0, "out_of_world");
    let cause = bot.st().death_msg.clone().map(|m| death_cause(&m)).unwrap_or_default();
    ctx.check(cause == "void", format!("death cause {cause}"));
    ctx.check(!dm.is_empty() && dm[0] == 4.0, format!("void damage {dm:?}"));
    ctx.metric("void_damage", dm.first().copied().unwrap_or(0.0) as f64, 0.0);
    ctx.metric("void_hits_to_death", dm.len() as f64, 1.0);
    ctx.metric("void_death_ticks", (died / 5.0).round() * 5.0, 10.0);
    ctx.note(format!("died after {died:.0} ticks, {} hits", dm.len()));
    Ok(())
}

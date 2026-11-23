//! M5 mobs: melee, arrows, creeper explosions, loot, sunlight burning.
//! Summons always carry an NBT compound so vanilla skips random equipment/babies
//! (`{PersistenceRequired:1b}`; Basalt ignores tags it doesn't know).
use super::util::*;
use crate::bot::{Bot, Entity};
use crate::runner::{Ctx, ARENA_Y};
use anyhow::{bail, Result};
use std::time::{Duration, Instant};

const Y: i32 = ARENA_Y + 1;

fn summon(bot: &Bot, kind: &str, p: [f64; 3], nbt: &str) -> Result<(i32, Entity)> {
    let t0 = Instant::now();
    bot.cmd_reply(&format!("summon minecraft:{kind} {:.3} {:.3} {:.3} {nbt}", p[0], p[1], p[2]))?;
    let ok = bot.wait_until(Duration::from_secs(3), |s| s.entities.values().any(|e| e.kind == kind && e.added >= t0));
    if !ok { bail!("summoned {kind} never appeared"); }
    let s = bot.st();
    let (k, e) = s.entities.iter().filter(|(_, e)| e.kind == kind && e.added >= t0).max_by_key(|(_, e)| e.added).unwrap();
    Ok((*k, e.clone()))
}

/// Roofed survival arena (mobs neither burn nor see the sky).
fn roofed(ctx: &mut Ctx, sx: i32, sz: i32) -> Result<(Bot, [i32; 3])> {
    let bot = ctx.bot()?;
    let o = ctx.arena(&bot, sx, sz, 5)?;
    // walls + roof so knockback can't push anyone off the platform
    fill(&bot, [o[0] - 1, Y, o[2] - 1], [o[0] + sx, Y + 3, o[2] + sz], "stone")?;
    fill(&bot, [o[0], Y, o[2]], [o[0] + sx - 1, Y + 2, o[2] + sz - 1], "air")?;
    ctx.stand(&bot, o[0] + 1, o[2] + sz / 2, "survival")?;
    Ok((bot, o))
}

pub fn zombie_hit(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = roofed(ctx, 12, 5)?;
    let z = (o[2] + 2) as f64 + 0.5;
    let t0 = Instant::now();
    summon(&bot, "zombie", [o[0] as f64 + 6.5, Y as f64, z], "{PersistenceRequired:1b}")?;
    if !bot.wait_until(Duration::from_secs(15), |s| s.damage.iter().any(|d| d.0 >= t0 && d.1 == "mob_attack")) { bail!("zombie never hit the player"); }
    bot.wait_ticks(2);
    let dm = hits(&bot, t0, "mob_attack");
    ctx.approx("zombie hit (easy, unarmored)", dm[0] as f64, 2.5, 0.01);
    ctx.metric("zombie_hit", dm[0] as f64, 0.01);
    // with an iron chestplate (6 armor): 2.5 * (1 - 6/25) ≈ 1.9 (vanilla measured 2.025 incl. toughness rules)
    give(&bot, "iron_chestplate", 1)?;
    let slot = bot.st().find("iron_chestplate").unwrap() as i16;
    bot.click(slot, 0, 1);
    bot.wait_until(Duration::from_secs(2), |s| s.inv[6].name() == "iron_chestplate");
    let t1 = Instant::now();
    if !bot.wait_until(Duration::from_secs(15), |s| s.damage.iter().any(|d| d.0 >= t1 && d.1 == "mob_attack")) { bail!("zombie didn't hit again"); }
    bot.wait_ticks(2);
    let armored = hits(&bot, t1, "mob_attack")[0] as f64;
    ctx.note(format!("zombie hit {} unarmored, {armored:.3} with iron chestplate", dm[0]));
    ctx.approx("zombie hit with iron chestplate", armored, 2.025, 0.05);
    ctx.metric("zombie_hit_iron_chest", (armored * 1000.0).round() / 1000.0, 0.01);
    // knockback: the server pushes the player (set_entity_motion with our id)
    Ok(())
}

pub fn skeleton_arrow(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = roofed(ctx, 16, 5)?;
    let z = (o[2] + 2) as f64 + 0.5;
    ctx.stand(&bot, o[0] + 1, o[2] + 2, "survival")?;
    let t0 = Instant::now();
    // with NBT, vanilla skips the random equipment roll — hand the skeleton its bow explicitly
    summon(&bot, "skeleton", [o[0] as f64 + 11.5, Y as f64, z], "{equipment:{mainhand:{id:\"minecraft:bow\",count:1}}}")?;
    if !bot.wait_until(Duration::from_secs(20), |s| s.damage.iter().any(|d| d.0 >= t0 && d.1 == "arrow")) { bail!("no arrow hit within 20 s"); }
    bot.wait_ticks(2);
    let dm = hits(&bot, t0, "arrow");
    ctx.note(format!("arrow hits {dm:?}"));
    ctx.check(dm[0] >= 2.0 && dm[0] <= 5.0, format!("arrow damage {}", dm[0]));
    ctx.metric("arrow_damage", dm[0] as f64, 1.0);
    ctx.check(bot.st().entities.values().any(|e| e.kind == "arrow") || !dm.is_empty(), "no arrow entity seen");
    Ok(())
}

pub fn creeper(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    let o = ctx.arena(&bot, 16, 8, 6)?;
    fill(&bot, [o[0], ARENA_Y - 3, o[2]], [o[0] + 15, ARENA_Y - 1, o[2] + 7], "dirt")?;
    ctx.stand(&bot, o[0] + 2, o[2] + 4, "survival")?;
    let me = bot.st().pos();
    let t0 = Instant::now();
    bot.cmd(&format!("summon minecraft:creeper {:.3} {:.3} {:.3} {{Fuse:0,ignited:1b,NoAI:1b}}", me[0] + 4.36, me[1], me[2]));
    if !bot.wait_until(Duration::from_secs(10), |s| s.explosions.iter().any(|e| e.at >= t0)) { bail!("creeper didn't explode"); }
    let fuse = t0.elapsed().as_secs_f64() * 20.0;
    ctx.metric("creeper_fuse_ticks", (fuse / 5.0).round() * 5.0, 5.0);
    bot.wait_ticks(3);
    let ex = bot.st().explosions.iter().find(|e| e.at >= t0).cloned().unwrap();
    let dm: f32 = { let k = damage_kinds(&bot, t0); ctx.note(format!("damage kinds {k:?}")); hits(&bot, t0, "explosion").into_iter().chain(hits(&bot, t0, "player_explosion")).next().unwrap_or(0.0) };
    let kb = ex.knockback.map(|k| (k[0] * k[0] + k[1] * k[1] + k[2] * k[2]).sqrt()).unwrap_or(0.0);
    ctx.note(format!("radius {} blocks {} damage {dm} knockback {kb:.3}", ex.radius, ex.blocks));
    ctx.approx("creeper damage at 4.36 blocks", dm as f64, 5.157, 0.3);
    ctx.approx("explosion knockback", kb, 0.274, 0.03);
    ctx.approx("explosion radius", ex.radius as f64, 3.0, 0.0);
    ctx.metric("creeper_damage", (dm as f64 * 100.0).round() / 100.0, 0.1);
    ctx.metric("creeper_knockback", (kb * 1000.0).round() / 1000.0, 0.02);
    ctx.metric("explosion_block_count", ex.blocks as f64, 60.0);
    Ok(())
}

fn kill_with_sword(bot: &Bot, eid: i32) -> Result<()> {
    for _ in 0..10 {
        if !bot.st().entities.contains_key(&eid) || bot.st().entity_events.iter().any(|e| e.1 == eid && e.2 == 3) { return Ok(()); }
        let p = bot.st().entities.get(&eid).map(|e| e.pos);
        if let Some(p) = p { bot.look_at([p[0], p[1] + 0.5, p[2]]); }
        bot.wait_ticks(2);
        bot.attack(eid);
        bot.wait_ticks(14);
    }
    if bot.st().entity_events.iter().any(|e| e.1 == eid && e.2 == 3) { Ok(()) } else { bail!("mob {eid} not dead after 10 hits") }
}

/// Inventory snapshot (item name -> count), to add drops the bot already picked up.
fn inv_counts(bot: &Bot) -> std::collections::HashMap<String, i32> {
    let s = bot.st(); let mut m = std::collections::HashMap::new();
    for i in s.inv.iter().filter(|i| i.count > 0) { *m.entry(i.name().to_string()).or_insert(0) += i.count; }
    m
}

/// Item entities added since `t0` plus whatever reached the inventory since `before`.
fn drops_since(bot: &Bot, t0: Instant, before: &std::collections::HashMap<String, i32>) -> Vec<(String, i32)> {
    let now = inv_counts(bot);
    let s = bot.st();
    let mut v: Vec<(String, i32)> = now.iter().filter_map(|(k, n)| { let d = n - before.get(k).copied().unwrap_or(0); (d > 0).then(|| (k.clone(), d)) }).collect();
    for e in s.entities.values().filter(|e| e.kind == "item" && e.added >= t0) {
        if let Some(i) = &e.item { match v.iter_mut().find(|x| x.0 == i.name()) { Some(x) => x.1 += i.count, None => v.push((i.name().to_string(), i.count)) } }
    }
    v.sort(); v
}

pub fn cow_loot(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = roofed(ctx, 8, 5)?;
    give(&bot, "diamond_sword", 1)?;
    hold(&bot, "diamond_sword")?;
    let me = bot.st().pos();
    let (eid, _) = summon(&bot, "cow", [me[0] + 2.0, Y as f64, me[2]], "{NoAI:1b}")?;
    let (t0, before) = (Instant::now(), inv_counts(&bot));
    kill_with_sword(&bot, eid)?;
    bot.wait_ticks(10);
    let d = drops_since(&bot, t0, &before);
    let get = |n: &str| d.iter().find(|x| x.0 == n).map(|x| x.1).unwrap_or(0);
    ctx.note(format!("cow drops {d:?}"));
    ctx.check((1..=3).contains(&get("beef")), format!("beef {}", get("beef")));
    ctx.check((0..=2).contains(&get("leather")), format!("leather {}", get("leather")));
    ctx.check(d.iter().all(|x| x.0 == "beef" || x.0 == "leather"), format!("unexpected drops {d:?}"));
    ctx.metric("cow_beef_in_range", (1..=3).contains(&get("beef")) as i32 as f64, 0.0);
    let _ = o;
    Ok(())
}

pub fn sheep_loot(ctx: &mut Ctx) -> Result<()> {
    let (bot, _o) = roofed(ctx, 8, 6)?;
    give(&bot, "diamond_sword", 1)?;
    hold(&bot, "diamond_sword")?;
    let me = bot.st().pos();
    let (eid, _) = summon(&bot, "sheep", [me[0] + 2.0, Y as f64, me[2]], "{NoAI:1b,Color:14}")?;
    let (t0, before) = (Instant::now(), inv_counts(&bot));
    kill_with_sword(&bot, eid)?;
    bot.wait_ticks(10);
    let d = drops_since(&bot, t0, &before);
    ctx.note(format!("red sheep drops {d:?}"));
    let wool = d.iter().find(|x| x.0 == "red_wool").map(|x| x.1).unwrap_or(0);
    ctx.check(wool == 1, format!("red wool {wool} ({d:?})"));
    ctx.metric("red_wool", wool as f64, 0.0);
    let (eid2, _) = summon(&bot, "sheep", [me[0] + 2.0, Y as f64, me[2] + 1.5], "{NoAI:1b,Color:14,Sheared:1b}")?;
    let (t1, before) = (Instant::now(), inv_counts(&bot));
    kill_with_sword(&bot, eid2)?;
    bot.wait_ticks(10);
    let d2 = drops_since(&bot, t1, &before);
    ctx.note(format!("sheared sheep drops {d2:?}"));
    let wool2 = d2.iter().filter(|x| x.0.ends_with("_wool")).map(|x| x.1).sum::<i32>();
    ctx.check(wool2 == 0, format!("sheared sheep dropped wool: {d2:?}"));
    ctx.metric("sheared_wool", wool2 as f64, 0.0);
    Ok(())
}

pub fn undead_sun(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    let o = ctx.arena(&bot, 10, 10, 5)?;
    ctx.stand(&bot, o[0] + 1, o[2] + 1, "creative")?;
    bot.cmd_reply("time set noon")?;
    // the zombie stands in a pit it can't leave
    let c = [o[0] + 6, Y, o[2] + 6];
    fill(&bot, [c[0] - 1, Y, c[2] - 1], [c[0] + 1, Y + 1, c[2] + 1], "stone")?;
    setblock(&bot, [c[0], Y, c[2]], "air")?;
    setblock(&bot, [c[0], Y + 1, c[2]], "air")?;
    // open sky: the vanilla world is shared and persistent, and other scenarios roof their arenas,
    // so a leftover roof above this pit would shade the zombie (it never ignites)
    fill(&bot, [c[0] - 1, Y + 2, c[2] - 1], [c[0] + 1, Y + 60, c[2] + 1], "air")?;
    let (eid, _) = summon(&bot, "zombie", [c[0] as f64 + 0.5, Y as f64, c[2] as f64 + 0.5], "{PersistenceRequired:1b}")?;
    let t0 = Instant::now();
    // vanilla rolls ignition per tick (~1/25 at full sky light), so allow up to 10 s
    let burned = bot.wait_until(Duration::from_secs(10), |s| s.entities.get(&eid).map(|e| e.flags & 1 != 0).unwrap_or(false));
    let t_burn = t0.elapsed().as_secs_f64() * 20.0;
    ctx.check(burned, "zombie in noon sunlight never caught fire");
    ctx.metric("zombie_burning", burned as i32 as f64, 0.0);
    ctx.check(t_burn <= 200.0, format!("caught fire after {t_burn:.0} ticks"));
    bot.wait_ticks(60);
    let h = bot.st().entities.get(&eid).and_then(|e| e.health);
    ctx.note(format!("burning after {t_burn:.0} ticks; health after 3 s more: {h:?}"));
    ctx.check(h.map(|h| h < 20.0).unwrap_or(false), format!("burning zombie not losing health ({h:?})"));
    Ok(())
}

/// Experience orbs: a cow killed by a player drops 1-3 XP as experience_orb entities, coal ore
/// mined with a pickaxe drops 0-2; standing on them collects them (set_experience). Also: the
/// orb's add_entity data field and its value in entity data, recorded as metrics for the diff.
pub fn xp_orbs(ctx: &mut Ctx) -> Result<()> {
    let (bot, _o) = roofed(ctx, 8, 5)?;
    bot.cmd_reply("xp set @s 0 points")?;
    give(&bot, "diamond_sword", 1)?;
    hold(&bot, "diamond_sword")?;
    let me = bot.st().pos();
    let (eid, _) = summon(&bot, "cow", [me[0] + 1.5, Y as f64, me[2]], "{NoAI:1b}")?;
    let xp0 = bot.st().xp_total;
    kill_with_sword(&bot, eid)?;
    let orb = bot.wait_until(Duration::from_secs(3), |s| s.entities.values().any(|e| e.kind == "experience_orb") || s.xp_total > xp0);
    let kinds: Vec<&str> = bot.st().entities.values().map(|e| e.kind).collect();
    ctx.check(orb, format!("no experience orb from a cow killed by a player (entities {kinds:?}, xp {})", bot.st().xp_total));
    bot.wait_until(Duration::from_secs(3), |s| s.entities.values().any(|e| e.kind == "experience_orb"));
    let n_orbs = { let s = bot.st(); s.entities.values().chain(s.removed.iter()).filter(|e| e.kind == "experience_orb").count() };
    let meta: Vec<String> = { let s = bot.st(); s.entities.values().chain(s.removed.iter()).filter(|e| e.kind == "experience_orb").map(|e| format!("{:?} vel {:?}", e.meta, e.spawn_vel)).collect() };
    ctx.note(format!("orbs {n_orbs}: {meta:?}"));
    // walk onto the orbs
    let p = bot.st().pos();
    bot.tp([p[0] + 1.5, p[1], p[2]])?;
    let got = bot.wait_until(Duration::from_secs(6), |s| s.xp_total > xp0 && !s.entities.values().any(|e| e.kind == "experience_orb"));
    let gained = bot.st().xp_total - xp0;
    ctx.check(got, format!("orbs not collected (xp {} -> {})", xp0, bot.st().xp_total));
    ctx.check((1..=3).contains(&gained), format!("cow gave {gained} xp (vanilla 1-3)"));
    ctx.metric("cow_xp_in_range", (1..=3).contains(&gained) as u8 as f64, 0.0);
    let orbs_after = { let s = bot.st(); s.entities.values().chain(s.removed.iter()).filter(|e| e.kind == "experience_orb").count() };
    ctx.metric("cow_orbs_1_to_3", (1..=3).contains(&orbs_after) as u8 as f64, 0.0);
    let _ = n_orbs;
    Ok(())
}

/// /xp (experience) add/set/query points and levels: reply keys + arguments and the totals sent
/// with set_experience, vanilla vs Basalt.
pub fn xp_command(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    let mut log = Vec::new();
    for c in ["xp set @s 0 points", "xp add @s 20 points", "xp query @s levels", "xp query @s points", "xp add @s 3 levels", "xp query @s levels", "xp set @s 4 points", "xp set @s 500 points", "xp set @s 0 levels", "xp query @s points"] {
        let r = bot.cmd_reply(c)?;
        bot.wait_ticks(2);
        let total = bot.st().xp_total;
        let key = r.first().map(|l| format!("{} {:?}", l.key.clone().unwrap_or_default(), l.args)).unwrap_or_default();
        log.push(format!("{c} -> {key} total {total}"));
        ctx.metric(&format!("total_after_{}", c.replace(' ', "_").replace('@', "")), total as f64, 0.0);
    }
    ctx.note(log.join(" | "));
    Ok(())
}

/// /difficulty: query/set/unchanged replies, change_difficulty, zombie melee damage per difficulty
/// (easy 2.5, normal 3, hard 4.5 vanilla), monsters refused in peaceful. Always leaves Easy (the
/// vanilla server is shared).
pub fn difficulty(ctx: &mut Ctx) -> Result<()> {
    let r = difficulty_inner(ctx);
    if let Ok(bot) = ctx.bot() { let _ = bot.cmd_reply("difficulty easy"); }
    r
}

fn difficulty_inner(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = roofed(ctx, 12, 5)?;
    let key = |bot: &Bot, c: &str| -> Result<String> { Ok(bot.cmd_reply(c)?.first().map(|l| format!("{} {:?}", l.key.clone().unwrap_or_default(), l.args)).unwrap_or_default()) };
    bot.cmd_reply("difficulty easy")?;
    let q = key(&bot, "difficulty")?;
    let same = key(&bot, "difficulty easy")?;
    ctx.note(format!("query {q}; unchanged {same}"));
    ctx.metric("query_is_easy", q.contains("options.difficulty.easy") as u8 as f64, 0.0);
    ctx.metric("unchanged_fails", same.starts_with("commands.difficulty.failure") as u8 as f64, 0.0);
    let z = (o[2] + 2) as f64 + 0.5;
    for (name, want) in [("normal", 2u8), ("hard", 3)] {
        let k = key(&bot, &format!("difficulty {name}"))?;
        ctx.metric(&format!("set_{name}_ok"), k.starts_with("commands.difficulty.success") as u8 as f64, 0.0);
        let got = bot.wait_until(Duration::from_secs(2), |s| s.difficulty == Some(want));
        if !got { ctx.note(format!("{name}: reply {k}, last change_difficulty {:?}", bot.st().difficulty)); }
        ctx.metric(&format!("packet_{name}"), got as u8 as f64, 0.0);
        let t0 = Instant::now();
        let (eid, _) = summon(&bot, "zombie", [o[0] as f64 + 6.5, Y as f64, z], "{PersistenceRequired:1b}")?;
        if !bot.wait_until(Duration::from_secs(15), |s| s.damage.iter().any(|d| d.0 >= t0 && d.1 == "mob_attack")) { bail!("zombie never hit on {name}"); }
        bot.wait_ticks(2);
        let dm = hits(&bot, t0, "mob_attack");
        ctx.metric(&format!("zombie_hit_{name}"), dm[0] as f64, 0.01);
        bot.cmd_reply(&format!("kill {}", bot.st().entities.get(&eid).map(|e| e.uuid.to_string()).unwrap_or_default()))?;
        bot.cmd_reply("effect give @s instant_health 1 5")?;
        bot.wait_ticks(10);
    }
    let k = key(&bot, "difficulty peaceful")?;
    ctx.metric("set_peaceful_ok", k.starts_with("commands.difficulty.success") as u8 as f64, 0.0);
    let s = key(&bot, &format!("summon zombie {} {} {}", o[0] + 6, Y, o[2] + 2))?;
    ctx.note(format!("peaceful summon: {s}"));
    ctx.metric("peaceful_summon_refused", s.starts_with("commands.summon.failed.peaceful") as u8 as f64, 0.0);
    Ok(())
}

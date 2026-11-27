//! Thrown items and projectile weapons (use_item): snowball, egg, ender pearl, experience bottle,
//! splash / lingering potions, wind charge, bow, trident. Differential against vanilla 26.2.
use super::util::*;
use crate::bot::Bot;
use crate::proto::Meta;
use crate::runner::{Ctx, ARENA_Y};
use anyhow::{bail, Result};
use std::time::{Duration, Instant};

const Y: i32 = ARENA_Y + 1;

/// A walled survival arena: stone box with the bot at the -z end facing +z (yaw 0), `len` blocks of
/// free flight before the wall.
fn box_arena(ctx: &mut Ctx, len: i32, gm: &str) -> Result<(Bot, [i32; 3])> {
    let bot = ctx.bot()?;
    let (sx, sz) = (9, len);
    let o = ctx.arena(&bot, sx, sz, 6)?;
    fill(
        &bot,
        [o[0] - 1, Y, o[2] - 1],
        [o[0] + sx, Y + 4, o[2] + sz],
        "stone",
    )?;
    fill(
        &bot,
        [o[0], Y, o[2]],
        [o[0] + sx - 1, Y + 3, o[2] + sz - 1],
        "air",
    )?;
    ctx.stand(&bot, o[0] + sx / 2, o[2], gm)?;
    bot.look(0.0, 0.0);
    Ok((bot, o))
}

fn sound_name(id: i32) -> String {
    basalt_data::registries::SOUND_EVENT
        .get(id as usize)
        .copied()
        .unwrap_or("?")
        .to_string()
}

/// (name, category, volume, pitch) of positional sounds since `since`.
fn sounds_since(bot: &Bot, since: Instant) -> Vec<(String, i32, f32, f32)> {
    bot.st()
        .sound_detail
        .iter()
        .filter(|s| s.0 >= since)
        .map(|s| (sound_name(s.1), s.2, s.4, s.5))
        .collect()
}

/// The newest entity of `kind` spawned since `since` (alive or already removed).
fn spawned(bot: &Bot, kind: &str, since: Instant) -> Option<crate::bot::Entity> {
    let s = bot.st();
    s.entities
        .values()
        .chain(s.removed.iter())
        .filter(|e| e.kind == kind && e.added >= since)
        .max_by_key(|e| e.added)
        .cloned()
}

fn throw(bot: &Bot, item: &str, pitch: f32) -> Result<Instant> {
    hold(bot, item)?;
    bot.look(0.0, pitch);
    bot.wait_ticks(2);
    let t = Instant::now();
    bot.use_item();
    Ok(t)
}

/// Snowball + egg basics: spawn position/velocity/owner/metadata, throw sound, item consumption (not in
/// creative), the break event when it hits the wall.
pub fn throw_basics(ctx: &mut Ctx) -> Result<()> {
    let (bot, _o) = box_arena(ctx, 7, "survival")?;
    give(&bot, "snowball", 5)?;
    let me = bot.st().pos();
    let eid = bot.st().eid;
    for (item, ent, tag) in [("snowball", "snowball", "sn"), ("egg", "egg", "egg")] {
        if item == "egg" {
            give(&bot, "egg", 5)?;
        }
        let t0 = throw(&bot, item, 0.0)?;
        if !bot.wait_until(Duration::from_secs(3), |s| {
            s.entities
                .values()
                .chain(s.removed.iter())
                .any(|e| e.kind == ent && e.added >= t0)
        }) {
            bail!("no {ent} entity appeared");
        }
        let e = spawned(&bot, ent, t0).unwrap();
        let sp = (e.spawn_vel[0].powi(2) + e.spawn_vel[1].powi(2) + e.spawn_vel[2].powi(2)).sqrt();
        ctx.metric(&format!("{tag}_spawn_dy"), e.spawn_pos[1] - me[1], 0.011);
        ctx.metric(
            &format!("{tag}_spawn_dxz"),
            (e.spawn_pos[0] - me[0]).hypot(e.spawn_pos[2] - me[2]),
            0.011,
        );
        ctx.metric(&format!("{tag}_speed"), sp, 0.05);
        ctx.metric(
            &format!("{tag}_owner_ok"),
            (e.data == eid) as u8 as f64,
            0.0,
        );
        ctx.metric(
            &format!("{tag}_item_meta"),
            matches!(e.meta.get(&8), Some(Meta::Item(st)) if st.name() == item) as u8 as f64,
            0.0,
        );
        // gone after hitting the wall, with the break event
        let eidp = {
            let s = bot.st();
            s.entities
                .iter()
                .find(|(_, x)| x.kind == ent && x.added >= t0)
                .map(|(k, _)| *k)
        };
        let gone = bot.wait_until(Duration::from_secs(3), |s| {
            s.removed.iter().any(|x| x.kind == ent && x.added >= t0)
        });
        ctx.check(gone, format!("{ent} never removed after hitting the wall"));
        let ev = eidp
            .map(|id| {
                bot.st()
                    .entity_events
                    .iter()
                    .any(|(t, e, st)| *t >= t0 && *e == id && *st == 3)
            })
            .unwrap_or(false);
        ctx.metric(&format!("{tag}_break_event"), ev as u8 as f64, 0.0);
        let snd = sounds_since(&bot, t0);
        let name = format!("entity.{item}.throw");
        let f = snd.iter().find(|s| s.0 == name);
        ctx.metric(
            &format!("{tag}_sound_cat"),
            f.map(|s| s.1 as f64).unwrap_or(-1.0),
            0.0,
        );
        ctx.metric(
            &format!("{tag}_sound_vol"),
            f.map(|s| s.2 as f64).unwrap_or(-1.0),
            0.001,
        );
        ctx.metric(
            &format!("{tag}_sound_pitch_ok"),
            f.map(|s| (0.33..=0.51).contains(&s.3)).unwrap_or(false) as u8 as f64,
            0.0,
        );
        bot.wait_ticks(10);
        ctx.metric(&format!("{tag}_left"), bot.st().count(item) as f64, 0.0);
    }
    // creative: nothing is consumed
    bot.cmd_reply("gamemode creative")?;
    bot.wait_ticks(4);
    bot.cmd_reply("clear @s")?;
    give(&bot, "snowball", 3)?;
    bot.look(0.0, 0.0);
    hold(&bot, "snowball")?;
    bot.use_item();
    bot.wait_ticks(10);
    ctx.metric("creative_left", bot.st().count("snowball") as f64, 0.0);
    Ok(())
}

/// Eggs: 1/8 chance of a baby chicken at the impact, 1/32 of those four.
pub fn throw_eggs(ctx: &mut Ctx) -> Result<()> {
    let (bot, _o) = box_arena(ctx, 6, "survival")?;
    let t0 = Instant::now();
    let n = 48;
    give(&bot, "egg", 16)?;
    let mut thrown = 0;
    while thrown < n {
        if bot.st().count("egg") == 0 {
            give(&bot, "egg", 16)?;
        }
        hold(&bot, "egg")?;
        bot.use_item();
        thrown += 1;
        bot.wait_ticks(3);
    }
    bot.wait_ticks(10);
    let chickens: Vec<crate::bot::Entity> = {
        let s = bot.st();
        s.entities
            .values()
            .chain(s.removed.iter())
            .filter(|e| e.kind == "chicken" && e.added >= t0)
            .cloned()
            .collect()
    };
    let baby = chickens
        .iter()
        .filter(|e| matches!(e.meta.get(&16), Some(Meta::Bool(true))))
        .count();
    ctx.note(format!(
        "{} chickens from {n} eggs ({baby} babies)",
        chickens.len()
    ));
    ctx.metric(
        "eggs_chickens_in_range",
        (1..=16).contains(&chickens.len()) as u8 as f64,
        0.0,
    );
    ctx.metric(
        "eggs_all_baby",
        (baby == chickens.len() && baby > 0) as u8 as f64,
        0.0,
    );
    // chickens spawn at the wall (z within 1.6 of it)
    Ok(())
}

/// Ender pearl: teleports the thrower to the impact point, 5 damage, 20-tick cooldown (cooldown packets,
/// a throw during the cooldown does nothing), sounds.
pub fn throw_pearl(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = box_arena(ctx, 14, "survival")?;
    give(&bot, "ender_pearl", 4)?;
    let p0 = bot.st().pos();
    let t0 = throw(&bot, "ender_pearl", 0.0)?;
    let pearl_seen = bot.wait_until(Duration::from_secs(3), |s| {
        s.entities
            .values()
            .chain(s.removed.iter())
            .any(|e| e.kind == "ender_pearl" && e.added >= t0)
    });
    ctx.check(pearl_seen, "no ender_pearl entity");
    ctx.check(
        bot.wait_until(Duration::from_secs(4), |s| {
            s.teleports.iter().any(|t| t.0 >= t0)
        }),
        "never teleported",
    );
    bot.wait_ticks(4);
    let p1 = bot.st().pos();
    ctx.note(format!("pearl: {p0:?} -> {p1:?}"));
    ctx.metric("pearl_dz", p1[2] - p0[2], 0.2);
    ctx.metric("pearl_dx", p1[0] - p0[0], 0.2);
    ctx.metric(
        "pearl_dy_ok",
        (p1[1] - Y as f64 > -0.1 && p1[1] - Y as f64 <= 0.5) as u8 as f64,
        0.0,
    );
    let _ = o;
    let health = bot.st().health;
    ctx.metric("pearl_health", health as f64, 0.01);
    let dk = damage_kinds(&bot, t0);
    ctx.note(format!("damage kinds {dk:?}"));
    ctx.metric(
        "pearl_damage_kind_ender_pearl",
        dk.contains(&"ender_pearl".to_string()) as u8 as f64,
        0.0,
    );
    let snd = sounds_since(&bot, t0);
    let thr = snd.iter().find(|s| s.0 == "entity.ender_pearl.throw");
    let tp = snd.iter().find(|s| s.0 == "entity.player.teleport");
    ctx.metric(
        "pearl_throw_cat",
        thr.map(|s| s.1 as f64).unwrap_or(-1.0),
        0.0,
    );
    ctx.metric(
        "pearl_tp_sound_cat",
        tp.map(|s| s.1 as f64).unwrap_or(-1.0),
        0.0,
    );
    ctx.metric(
        "pearl_tp_sound_vol",
        tp.map(|s| s.2 as f64).unwrap_or(-1.0),
        0.001,
    );
    // cooldown packets: start 20, end 0
    let cd: Vec<(String, i32)> = bot
        .st()
        .cooldowns
        .iter()
        .filter(|c| c.0 >= t0)
        .map(|c| (c.1.clone(), c.2))
        .collect();
    ctx.metric(
        "pearl_cooldown_first_ticks",
        cd.iter()
            .find(|c| c.0 == "minecraft:ender_pearl")
            .map(|c| c.1 as f64)
            .unwrap_or(-1.0),
        0.0,
    );
    // a second throw during the cooldown is ignored (nothing consumed)
    let left = bot.st().count("ender_pearl");
    bot.look(0.0, 0.0);
    bot.use_item();
    bot.wait_ticks(4);
    ctx.metric(
        "pearl_left_after_blocked",
        bot.st().count("ender_pearl") as f64 - left as f64,
        0.0,
    );
    // after the cooldown ends (announced with 0 ticks) a throw works again
    let ended = bot.wait_until(Duration::from_secs(3), |s| {
        s.cooldowns
            .iter()
            .any(|c| c.0 >= t0 && c.1 == "minecraft:ender_pearl" && c.2 == 0)
    });
    ctx.metric("pearl_cooldown_end_packet", ended as u8 as f64, 0.0);
    let dur = {
        let s = bot.st();
        s.cooldowns
            .iter()
            .filter(|c| c.0 >= t0 && c.1 == "minecraft:ender_pearl")
            .map(|c| (c.0, c.2))
            .collect::<Vec<_>>()
    };
    if dur.len() >= 2 {
        ctx.metric(
            "pearl_cooldown_ticks",
            dur[1].0.duration_since(dur[0].0).as_secs_f64() * 20.0,
            2.5,
        );
    }
    Ok(())
}

/// Experience bottle thrown at the floor: level_event 2002 (colour), 3..11 xp in orbs.
pub fn throw_xp_bottle(ctx: &mut Ctx) -> Result<()> {
    let (bot, _o) = box_arena(ctx, 6, "survival")?;
    bot.cmd_reply("xp set @s 0 points")?;
    give(&bot, "experience_bottle", 6)?;
    let t0 = throw(&bot, "experience_bottle", 0.0)?;
    let seen = bot.wait_until(Duration::from_secs(3), |s| {
        s.entities
            .values()
            .chain(s.removed.iter())
            .any(|e| e.kind == "experience_bottle" && e.added >= t0)
    });
    ctx.check(seen, "no experience_bottle entity");
    let e = spawned(&bot, "experience_bottle", t0);
    if let Some(e) = e {
        let sp = (e.spawn_vel[0].powi(2) + e.spawn_vel[1].powi(2) + e.spawn_vel[2].powi(2)).sqrt();
        ctx.metric("bottle_speed", sp, 0.03);
        ctx.metric("bottle_vy", e.spawn_vel[1], 0.02);
    }
    let got = bot.wait_until(Duration::from_secs(6), |s| {
        s.level_events.iter().any(|l| l.0 >= t0 && l.1 == 2002)
    });
    ctx.check(got, "no level_event 2002 when the bottle broke");
    let ev = bot
        .st()
        .level_events
        .iter()
        .find(|l| l.0 >= t0 && l.1 == 2002)
        .cloned();
    ctx.metric(
        "bottle_event_data",
        ev.map(|l| l.3 as f64).unwrap_or(0.0),
        0.0,
    );
    bot.wait_until(Duration::from_secs(3), |s| {
        s.entities.values().any(|e| e.kind == "experience_orb")
    });
    let total0 = bot.st().xp_total;
    // walk onto the orbs
    let p = bot.st().pos();
    bot.tp([p[0], p[1], p[2] + 5.0])?;
    bot.wait_until(Duration::from_secs(8), |s| {
        s.xp_total > total0 && !s.entities.values().any(|e| e.kind == "experience_orb")
    });
    let gained = bot.st().xp_total;
    ctx.note(format!("xp gained {gained}"));
    ctx.metric(
        "bottle_xp_in_3_11",
        (3..=11).contains(&gained) as u8 as f64,
        0.0,
    );
    // thrown straight down: the orbs appear 0.25 above the floor
    let t1 = throw(&bot, "experience_bottle", 90.0)?;
    bot.wait_until(Duration::from_secs(4), |s| {
        s.entities
            .values()
            .chain(s.removed.iter())
            .any(|e| e.kind == "experience_orb" && e.added >= t1)
    });
    ctx.metric(
        "bottle_orb_dy_floor",
        spawned(&bot, "experience_orb", t1)
            .map(|e| e.spawn_pos[1] - Y as f64)
            .unwrap_or(-9.0),
        0.1,
    );
    ctx.metric(
        "bottle_left",
        bot.st().count("experience_bottle") as f64,
        0.0,
    );
    Ok(())
}

/// Snowball / egg hitting a cow: no damage, "thrown" damage event (cause = thrower), horizontal knockback,
/// the projectile breaks.
pub fn throw_hit_mob(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = box_arena(ctx, 8, "survival")?;
    bot.wait_ticks(30); // a cow killed by the arena clean-up is still lying there for 20 ticks and would take the hit
    let me = bot.st().pos();
    let t0 = Instant::now();
    bot.cmd_reply(&format!(
        "summon minecraft:cow {:.2} {} {:.2} {{NoAI:1b,PersistenceRequired:1b}}",
        me[0],
        Y,
        o[2] as f64 + 5.5
    ))?;
    if !bot.wait_until(Duration::from_secs(3), |s| {
        s.entities
            .values()
            .any(|e| e.kind == "cow" && e.added >= t0)
    }) {
        bail!("cow never appeared");
    }
    let cow = bot
        .st()
        .entities
        .iter()
        .find(|(_, e)| e.kind == "cow" && e.added >= t0)
        .map(|(k, _)| *k)
        .unwrap();
    give(&bot, "snowball", 2)?;
    let t1 = throw(&bot, "snowball", 0.0)?;
    let hit = bot.wait_until(Duration::from_secs(3), |s| {
        s.damage_events.iter().any(|d| d.0 >= t1 && d.1 == cow)
    });
    if !hit {
        let s = bot.st();
        ctx.note(format!(
            "entities {:?} removed {:?} events {:?} cowpos {:?} me {:?}",
            s.entities
                .values()
                .map(|e| (e.kind, e.pos))
                .collect::<Vec<_>>(),
            s.removed
                .iter()
                .map(|e| (e.kind, e.pos, e.spawn_vel))
                .collect::<Vec<_>>(),
            s.entity_events
                .iter()
                .filter(|e| e.0 >= t1)
                .collect::<Vec<_>>(),
            s.entities.get(&cow).map(|e| e.pos),
            s.body.pos
        ));
    }
    ctx.check(hit, "no damage_event for the cow");
    bot.wait_ticks(3);
    let de = bot
        .st()
        .damage_events
        .iter()
        .find(|d| d.0 >= t1 && d.1 == cow)
        .cloned();
    let eid = bot.st().eid;
    ctx.metric(
        "hit_damage_kind_thrown",
        de.as_ref()
            .map(|d| (d.2 == "thrown") as u8 as f64)
            .unwrap_or(-1.0),
        0.0,
    );
    ctx.metric(
        "hit_cause_is_thrower",
        de.as_ref()
            .map(|d| (d.3 == eid + 1) as u8 as f64)
            .unwrap_or(-1.0),
        0.0,
    );
    ctx.metric(
        "hit_direct_set",
        de.as_ref().map(|d| (d.4 > 0) as u8 as f64).unwrap_or(-1.0),
        0.0,
    );
    let (vel, health) = {
        let s = bot.st();
        let e = &s.entities[&cow];
        (e.vel, e.health)
    };
    ctx.note(format!("cow vel {vel:?} health {health:?}"));
    ctx.metric("hit_knockback_z", vel[2], 0.05);
    ctx.metric("hit_knockback_y", vel[1], 0.1);
    ctx.metric(
        "hit_cow_health_full",
        health.map(|h| (h - 10.0).abs() < 0.01).unwrap_or(true) as u8 as f64,
        0.0,
    );
    ctx.metric(
        "hit_hurt_sound",
        sounds_since(&bot, t1)
            .iter()
            .any(|s| s.0 == "entity.cow.hurt") as u8 as f64,
        0.0,
    );
    // the vanilla server is shared and persistent: don't leave the cow behind (it would be hit by the next run)
    bot.cmd_reply(&format!(
        "kill @e[type=minecraft:cow,x={},y={},z={},distance=..6]",
        me[0],
        Y,
        o[2] as f64 + 5.5
    ))?;
    bot.wait_ticks(25);
    Ok(())
}

fn potion_cmd(name: &str, kind: &str) -> String {
    format!("give @s minecraft:{kind}[potion_contents={{potion:\"minecraft:{name}\"}}]")
}

fn throw_potion(bot: &Bot, kind: &str, name: &str, pitch: f32) -> Result<Instant> {
    bot.cmd_reply(&potion_cmd(name, kind))?;
    bot.wait_until(Duration::from_secs(3), |s| s.count(kind) > 0);
    throw(bot, kind, pitch)
}

/// Splash potions: direct hit = full strength, near miss = scaled by distance, instant harming, water.
pub fn throw_splash(ctx: &mut Ctx) -> Result<()> {
    let (bot, _o) = box_arena(ctx, 10, "survival")?;
    for (tag, pot, pitch) in [
        ("near", "swiftness", 60.0f32),
        ("direct", "swiftness", 90.0),
        ("far", "regeneration", 30.0),
    ] {
        bot.cmd_reply("effect clear @s")?;
        bot.wait_ticks(4);
        let t0 = throw_potion(&bot, "splash_potion", pot, pitch)?;
        let ok = bot.wait_until(Duration::from_secs(5), |s| {
            s.level_events
                .iter()
                .any(|l| l.0 >= t0 && (l.1 == 2002 || l.1 == 2007))
        });
        ctx.check(ok, format!("{tag}: no splash level_event"));
        bot.wait_ticks(6);
        let ev = bot
            .st()
            .level_events
            .iter()
            .find(|l| l.0 >= t0 && l.1 == 2002)
            .cloned();
        ctx.metric(
            &format!("splash_{tag}_event_data"),
            ev.map(|l| l.3 as f64).unwrap_or(0.0),
            0.0,
        );
        let fx: Vec<(String, i32, i32)> = bot
            .st()
            .effect_log
            .iter()
            .filter(|e| e.0 >= t0)
            .map(|e| (e.1.clone(), e.2, e.3))
            .collect();
        ctx.note(format!("{tag}: effects {fx:?}"));
        ctx.metric(&format!("splash_{tag}_effect_count"), fx.len() as f64, 0.0);
        ctx.metric(
            &format!("splash_{tag}_duration"),
            fx.first().map(|e| e.2 as f64).unwrap_or(0.0),
            120.0,
        );
        let snd = sounds_since(&bot, t0);
        let thr = snd.iter().find(|s| s.0 == "entity.splash_potion.throw");
        ctx.metric(
            &format!("splash_{tag}_throw_cat"),
            thr.map(|s| s.1 as f64).unwrap_or(-1.0),
            0.0,
        );
    }
    // strong harming directly on the thrower: 12 damage
    bot.cmd_reply("effect clear @s")?;
    let t0 = throw_potion(&bot, "splash_potion", "strong_harming", 90.0)?;
    bot.wait_until(Duration::from_secs(5), |s| {
        s.level_events.iter().any(|l| l.0 >= t0 && l.1 == 2007)
    });
    bot.wait_ticks(5);
    let lost = 20.0 - bot.st().health;
    ctx.metric("harming_direct_damage", lost as f64, 0.01);
    let dk = damage_kinds(&bot, t0);
    ctx.note(format!("harming damage kinds {dk:?}"));
    ctx.metric(
        "harming_damage_kind_magic",
        dk.contains(&"indirect_magic".to_string()) as u8 as f64,
        0.0,
    );
    ctx.metric(
        "harming_event_2007",
        bot.st()
            .level_events
            .iter()
            .any(|l| l.0 >= t0 && l.1 == 2007) as u8 as f64,
        0.0,
    );
    // water bottle: no effects
    bot.cmd_reply("effect clear @s")?;
    let n0 = bot.st().effect_log.len();
    let _t0 = throw_potion(&bot, "splash_potion", "water", 90.0)?;
    bot.wait_ticks(10);
    ctx.metric(
        "water_effects",
        (bot.st().effect_log.len() - n0) as f64,
        0.0,
    );
    Ok(())
}

/// Lingering potion: area effect cloud at the hit position, effects at a quarter duration every 20 ticks,
/// shrinking radius.
pub fn throw_lingering(ctx: &mut Ctx) -> Result<()> {
    let (bot, _o) = box_arena(ctx, 10, "survival")?;
    bot.cmd_reply("effect clear @s")?;
    let me = bot.st().pos();
    let t0 = throw_potion(&bot, "lingering_potion", "swiftness", 60.0)?;
    let ok = bot.wait_until(Duration::from_secs(4), |s| {
        s.entities
            .values()
            .any(|e| e.kind == "area_effect_cloud" && e.added >= t0)
    });
    ctx.check(ok, "no area_effect_cloud");
    let cloud = spawned(&bot, "area_effect_cloud", t0);
    if let Some(c) = &cloud {
        ctx.metric("cloud_dz", c.spawn_pos[2] - me[2], 0.4);
        ctx.metric("cloud_dy", c.spawn_pos[1] - me[1], 0.3);
    }
    let ev = bot
        .st()
        .level_events
        .iter()
        .find(|l| l.0 >= t0 && l.1 == 2002)
        .cloned();
    ctx.metric(
        "lingering_event_data",
        ev.map(|l| l.3 as f64).unwrap_or(0.0),
        0.0,
    );
    // walk into the cloud and stay: effects arrive with a quarter of the duration, re-applied every 20 ticks
    if let Some(c) = &cloud {
        bot.tp([c.spawn_pos[0], c.spawn_pos[1], c.spawn_pos[2]])?;
    }
    bot.wait_ticks(70);
    let fx: Vec<(String, i32, i32)> = bot
        .st()
        .effect_log
        .iter()
        .filter(|e| e.0 >= t0)
        .map(|e| (e.1.clone(), e.2, e.3))
        .collect();
    ctx.note(format!("cloud effects {fx:?}"));
    ctx.metric(
        "cloud_first_duration",
        fx.first().map(|e| e.2 as f64).unwrap_or(0.0),
        5.0,
    );
    ctx.metric(
        "cloud_reapplications_ge_2",
        (fx.len() >= 2) as u8 as f64,
        0.0,
    );
    // radius metadata shrinks
    let r = cloud.as_ref().and_then(|c| {
        bot.st()
            .entities
            .values()
            .find(|e| e.uuid == c.uuid)
            .and_then(|e| match e.meta.get(&8) {
                Some(Meta::Float(f)) => Some(*f),
                _ => None,
            })
    });
    ctx.note(format!("cloud radius now {r:?}"));
    ctx.metric(
        "cloud_radius_shrunk",
        r.map(|r| (r < 3.0 && r > 0.4) as u8 as f64).unwrap_or(-1.0),
        0.0,
    );
    Ok(())
}

fn entity_pos_eid(bot: &Bot, kind: &str, since: Instant) -> Option<(i32, [f64; 3])> {
    let s = bot.st();
    s.entities
        .iter()
        .filter(|(_, e)| e.kind == kind && e.added >= since)
        .max_by_key(|(_, e)| e.added)
        .map(|(k, e)| (*k, e.pos))
}

/// Bow: draw time -> speed / critical flag, arrow consumption and bow wear, a tap does nothing, arrows
/// stuck in the wall can be picked up again.
pub fn bow_shoot(ctx: &mut Ctx) -> Result<()> {
    let (bot, _o) = box_arena(ctx, 8, "survival")?;
    give(&bot, "bow", 1)?;
    give(&bot, "arrow", 8)?;
    hold(&bot, "bow")?;
    let me = bot.st().pos();
    let eid = bot.st().eid;
    for (tag, ticks) in [("full", 24u32), ("half", 10), ("tap", 1)] {
        let before = bot.st().count("arrow");
        let t0 = Instant::now();
        bot.look(0.0, 0.0);
        bot.wait_ticks(2);
        bot.use_item();
        bot.wait_ticks(ticks);
        bot.release_use();
        bot.wait_ticks(4);
        let e = spawned(&bot, "arrow", t0);
        ctx.metric(&format!("bow_{tag}_shot"), e.is_some() as u8 as f64, 0.0);
        if let Some(e) = e {
            ctx.note(format!("{tag}: meta {:?}", e.meta));
            let sp =
                (e.spawn_vel[0].powi(2) + e.spawn_vel[1].powi(2) + e.spawn_vel[2].powi(2)).sqrt();
            ctx.metric(&format!("bow_{tag}_speed"), sp, 0.15);
            ctx.metric(&format!("bow_{tag}_dy"), e.spawn_pos[1] - me[1], 0.011);
            ctx.metric(
                &format!("bow_{tag}_owner"),
                (e.data == eid) as u8 as f64,
                0.0,
            );
        }
        let snd = sounds_since(&bot, t0);
        let sh = snd.iter().find(|s| s.0 == "entity.arrow.shoot");
        ctx.metric(
            &format!("bow_{tag}_sound_cat"),
            sh.map(|s| s.1 as f64).unwrap_or(-1.0),
            0.0,
        );
        ctx.metric(
            &format!("bow_{tag}_pitch"),
            sh.map(|s| s.3 as f64).unwrap_or(-1.0),
            0.2,
        );
        ctx.metric(
            &format!("bow_{tag}_arrows_used"),
            (before - bot.st().count("arrow")) as f64,
            0.0,
        );
    }
    // pick the arrows up again: stand where they stuck
    bot.wait_ticks(30);
    let before = bot.st().count("arrow");
    let spots: Vec<[f64; 3]> = bot
        .st()
        .entities
        .values()
        .filter(|e| e.kind == "arrow")
        .map(|e| e.pos)
        .collect();
    ctx.note(format!("stuck arrows at {spots:?}"));
    for p in &spots {
        bot.tp([p[0], p[1] - 0.5, p[2] - 0.5])?;
        bot.wait_ticks(8);
    }
    bot.wait_ticks(6);
    ctx.metric(
        "bow_arrows_picked_up",
        (bot.st().count("arrow") - before) as f64,
        0.0,
    );
    Ok(())
}

/// Trident throw: needs a 10-tick draw, leaves the inventory, flies at 2.5, can be picked up again.
pub fn trident_throw(ctx: &mut Ctx) -> Result<()> {
    let (bot, _o) = box_arena(ctx, 8, "survival")?;
    give(&bot, "trident", 1)?;
    hold(&bot, "trident")?;
    let me = bot.st().pos();
    let eid = bot.st().eid;
    // too short: nothing happens
    let t0 = Instant::now();
    bot.look(0.0, 0.0);
    bot.wait_ticks(2);
    bot.use_item();
    bot.wait_ticks(4);
    bot.release_use();
    bot.wait_ticks(4);
    ctx.metric(
        "trident_short_thrown",
        spawned(&bot, "trident", t0).is_some() as u8 as f64,
        0.0,
    );
    let t0 = Instant::now();
    bot.use_item();
    bot.wait_ticks(15);
    bot.release_use();
    bot.wait_ticks(4);
    let e = spawned(&bot, "trident", t0);
    ctx.metric("trident_thrown", e.is_some() as u8 as f64, 0.0);
    if let Some(e) = &e {
        let sp = (e.spawn_vel[0].powi(2) + e.spawn_vel[1].powi(2) + e.spawn_vel[2].powi(2)).sqrt();
        ctx.metric("trident_speed", sp, 0.15);
        ctx.metric("trident_dy", e.spawn_pos[1] - me[1], 0.011);
        ctx.metric("trident_owner", (e.data == eid) as u8 as f64, 0.0);
    }
    ctx.metric("trident_left", bot.st().count("trident") as f64, 0.0);
    ctx.metric(
        "trident_throw_sound",
        bot.st()
            .entity_sounds
            .iter()
            .any(|s| s.0 >= t0 && sound_name(s.1) == "item.trident.throw") as u8 as f64,
        0.0,
    );
    // fetch it
    bot.wait_ticks(30);
    if let Some((_, p)) = entity_pos_eid(&bot, "trident", t0) {
        bot.tp([p[0], p[1] - 0.5, p[2] - 0.5])?;
        bot.wait_ticks(10);
    }
    ctx.metric("trident_picked_up", bot.st().count("trident") as f64, 0.0);
    Ok(())
}

/// Wind charge: thrown at the floor it bursts 0.25 above it (explode packet: radius, knockback on the thrower),
/// 10-tick cooldown, no gravity.
pub fn wind_charge(ctx: &mut Ctx) -> Result<()> {
    let (bot, _o) = box_arena(ctx, 10, "survival")?;
    give(&bot, "wind_charge", 6)?;
    let me = bot.st().pos();
    for (tag, pitch) in [("down", 90.0f32), ("slant", 60.0)] {
        bot.tp(me)?;
        bot.wait_on_ground(Duration::from_secs(3));
        bot.wait_ticks(25);
        let n0 = bot.st().explosions.len();
        let t0 = throw(&bot, "wind_charge", pitch)?;
        let ok = bot.wait_until(Duration::from_secs(3), |s| s.explosions.len() > n0);
        ctx.check(ok, format!("{tag}: no explode packet"));
        if let Some(x) = bot.st().explosions.get(n0).cloned() {
            ctx.metric(&format!("wind_{tag}_dy"), x.pos[1] - me[1], 0.06);
            ctx.metric(&format!("wind_{tag}_dz"), x.pos[2] - me[2], 0.15);
            ctx.metric(&format!("wind_{tag}_radius"), x.radius as f64, 0.001);
            let kb = x.knockback.unwrap_or([0.0; 3]);
            ctx.metric(&format!("wind_{tag}_kb_y"), kb[1], 0.06);
            ctx.metric(&format!("wind_{tag}_kb_z"), kb[2], 0.06);
        }
        let cd: Vec<i32> = bot
            .st()
            .cooldowns
            .iter()
            .filter(|c| c.0 >= t0 && c.1 == "minecraft:wind_charge")
            .map(|c| c.2)
            .collect();
        ctx.metric(
            &format!("wind_{tag}_cooldown_start"),
            cd.first().copied().unwrap_or(-1) as f64,
            0.0,
        );
        ctx.metric(
            &format!("wind_{tag}_sound_burst"),
            sounds_since(&bot, t0)
                .iter()
                .any(|s| s.0 == "entity.wind_charge.wind_burst") as u8 as f64,
            0.0,
        );
    }
    // thrown during the cooldown: nothing happens
    let left = bot.st().count("wind_charge");
    bot.use_item();
    bot.wait_ticks(2);
    bot.use_item();
    bot.wait_ticks(3);
    ctx.metric(
        "wind_used_during_cooldown",
        (left - bot.st().count("wind_charge")) as f64,
        0.0,
    );
    Ok(())
}

/// Crossbow: hold use to charge (a short tap loads nothing), release after a full charge loads an
/// arrow (charged_projectiles component, one arrow used); the next use fires at once.
pub fn crossbow(ctx: &mut Ctx) -> Result<()> {
    let (bot, _o) = box_arena(ctx, 8, "survival")?;
    give(&bot, "crossbow", 1)?;
    give(&bot, "arrow", 8)?;
    hold(&bot, "crossbow")?;
    let me = bot.st().pos();
    let eid = bot.st().eid;
    let charged = |bot: &Bot| bot.st().held_stack().charged.len() as f64;
    for (tag, ticks) in [("tap", 5u32), ("short", 20), ("full", 30)] {
        let before = bot.st().count("arrow");
        let t0 = Instant::now();
        bot.look(0.0, 0.0);
        bot.wait_ticks(2);
        bot.use_item();
        bot.wait_ticks(ticks);
        let mid = charged(&bot);
        bot.release_use();
        bot.wait_ticks(4);
        ctx.metric(&format!("crossbow_{tag}_charged_before_release"), mid, 0.0);
        ctx.metric(&format!("crossbow_{tag}_charged"), charged(&bot), 0.0);
        ctx.metric(
            &format!("crossbow_{tag}_arrows_used"),
            (before - bot.st().count("arrow")) as f64,
            0.0,
        );
        ctx.metric(
            &format!("crossbow_{tag}_shot"),
            spawned(&bot, "arrow", t0).is_some() as u8 as f64,
            0.0,
        );
        let snd: Vec<(String, u32)> = bot
            .st()
            .sound_detail
            .iter()
            .filter(|s| s.0 >= t0)
            .map(|s| {
                (
                    sound_name(s.1),
                    (s.0.duration_since(t0).as_secs_f64() * 20.0).round() as u32,
                )
            })
            .collect();
        ctx.note(format!("{tag}: sounds {snd:?}"));
        for (n, t) in &snd {
            if let Some(k) = n.strip_prefix("item.crossbow.") {
                ctx.metric(&format!("crossbow_{tag}_sound_{k}_tick"), *t as f64, 3.0);
            }
        }
    }
    // loaded: the next use fires at once
    let t0 = Instant::now();
    bot.look(0.0, 0.0);
    bot.wait_ticks(2);
    bot.use_item();
    bot.wait_ticks(4);
    let e = spawned(&bot, "arrow", t0);
    ctx.metric("crossbow_fire_shot", e.is_some() as u8 as f64, 0.0);
    if let Some(e) = &e {
        let sp = (e.spawn_vel[0].powi(2) + e.spawn_vel[1].powi(2) + e.spawn_vel[2].powi(2)).sqrt();
        ctx.metric("crossbow_fire_speed", sp, 0.15);
        ctx.metric("crossbow_fire_dy", e.spawn_pos[1] - me[1], 0.011);
        ctx.metric("crossbow_fire_owner", (e.data == eid) as u8 as f64, 0.0);
        ctx.note(format!("fire: meta {:?}", e.meta));
    }
    let snd = sounds_since(&bot, t0);
    let sh = snd.iter().find(|s| s.0 == "item.crossbow.shoot");
    ctx.metric(
        "crossbow_fire_sound_cat",
        sh.map(|s| s.1 as f64).unwrap_or(-1.0),
        0.0,
    );
    ctx.metric(
        "crossbow_fire_pitch",
        sh.map(|s| s.3 as f64).unwrap_or(-1.0),
        0.3,
    );
    ctx.note(format!("fire sounds {snd:?}"));
    ctx.metric("crossbow_fire_charged_after", charged(&bot), 0.0);
    ctx.metric(
        "crossbow_fire_damage",
        bot.st().held_stack().damage as f64,
        0.0,
    );
    // creative: loads without arrows
    ctx.gamemode(&bot, "creative")?;
    bot.cmd_reply("clear @s minecraft:arrow")?;
    bot.wait_ticks(2);
    bot.use_item();
    bot.wait_ticks(30);
    bot.release_use();
    bot.wait_ticks(4);
    ctx.metric("crossbow_creative_charged", charged(&bot), 0.0);
    Ok(())
}

/// Fishing rod: cast (bobber spawn, owner, speed), floating, reeling in empty / from the ground,
/// and a catch with Lure III (bite -> loot item pulled to the player + experience).
pub fn fishing(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    let o = ctx.arena(&bot, 11, 14, 8)?;
    // a pool: stone basin y..y+2 with 3-deep water, the bot on its south rim
    fill(
        &bot,
        [o[0], Y, o[2] + 2],
        [o[0] + 10, Y + 2, o[2] + 13],
        "stone",
    )?;
    fill(
        &bot,
        [o[0] + 1, Y, o[2] + 4],
        [o[0] + 9, Y + 2, o[2] + 12],
        "water",
    )?;
    ctx.stand(&bot, o[0] + 5, o[2] + 2, "survival")?;
    bot.tp([o[0] as f64 + 5.5, (Y + 3) as f64, o[2] as f64 + 2.5])?;
    bot.wait_ticks(10);
    let surface = (Y + 3) as f64;
    give(&bot, "fishing_rod", 1)?;
    hold(&bot, "fishing_rod")?;
    let eid = bot.st().eid;
    let me = bot.st().pos();
    let bobber = |bot: &Bot, since: Instant| spawned(bot, "fishing_bobber", since);
    // 1. cast straight ahead into the pool
    let t0 = Instant::now();
    bot.look(0.0, 0.0);
    bot.wait_ticks(2);
    bot.use_item();
    bot.wait_ticks(3);
    let b = bobber(&bot, t0);
    ctx.metric("cast_spawned", b.is_some() as u8 as f64, 0.0);
    if let Some(b) = &b {
        let sp = (b.spawn_vel[0].powi(2) + b.spawn_vel[1].powi(2) + b.spawn_vel[2].powi(2)).sqrt();
        ctx.metric("cast_speed", sp, 0.25);
        ctx.metric("cast_dy", b.spawn_pos[1] - me[1], 0.05);
        ctx.metric("cast_owner", (b.data == eid) as u8 as f64, 0.0);
    }
    let snd: Vec<String> = sounds_since(&bot, t0).into_iter().map(|s| s.0).collect();
    ctx.metric(
        "cast_sound",
        snd.iter().any(|s| s == "entity.fishing_bobber.throw") as u8 as f64,
        0.0,
    );
    bot.wait_ticks(50);
    let (float_y, alive) = {
        let s = bot.st();
        let e = b
            .as_ref()
            .and_then(|b| s.entities.values().find(|e| e.uuid == b.uuid));
        (e.map(|e| e.pos[1] - surface), e.is_some())
    };
    ctx.metric("float_alive", alive as u8 as f64, 0.0);
    if let Some(y) = float_y {
        ctx.metric("float_y", y, 0.3);
    }
    ctx.note(format!("cast sounds {snd:?}, float y {float_y:?}"));
    // 2. reel in with nothing on the line
    let t1 = Instant::now();
    bot.use_item();
    bot.wait_ticks(4);
    let gone = {
        let s = bot.st();
        b.as_ref()
            .is_some_and(|b| !s.entities.values().any(|e| e.uuid == b.uuid))
    };
    ctx.metric("reel_empty_removed", gone as u8 as f64, 0.0);
    ctx.metric(
        "reel_empty_damage",
        bot.st().held_stack().damage as f64,
        0.0,
    );
    let snd: Vec<String> = sounds_since(&bot, t1).into_iter().map(|s| s.0).collect();
    ctx.note(format!("reel sounds {snd:?}"));
    // 3. cast onto the rim (ground), reel in
    let d0 = bot.st().held_stack().damage;
    let t2 = Instant::now();
    bot.look(0.0, 70.0);
    bot.wait_ticks(2);
    bot.use_item();
    bot.wait_ticks(30);
    let on_ground = bobber(&bot, t2).map(|e| e.pos[1] - surface);
    bot.use_item();
    bot.wait_ticks(4);
    ctx.note(format!("ground bobber y {on_ground:?}"));
    ctx.metric(
        "reel_ground_damage",
        (bot.st().held_stack().damage - d0) as f64,
        0.0,
    );
    // 4. a catch: Lure III, wait for the bite (entity data), reel in
    bot.cmd_reply("clear @s")?;
    bot.cmd_reply("give @s minecraft:fishing_rod[enchantments={lure:3}]")?;
    bot.wait_ticks(4);
    hold(&bot, "fishing_rod")?;
    ctx.note(format!(
        "lure rod enchantments {:?}",
        bot.st().held_stack().ench
    ));
    bot.look(0.0, 0.0);
    bot.wait_ticks(2);
    let mut caught = false;
    let mut waits = Vec::new();
    for _attempt in 0..3 {
        let t3 = Instant::now();
        bot.use_item();
        bot.wait_ticks(3);
        let Some(b) = bobber(&bot, t3) else { break };
        let bit = bot.wait_until(Duration::from_secs(40), |s| {
            s.entities
                .values()
                .find(|e| e.uuid == b.uuid)
                .and_then(|e| e.meta.get(&9))
                .is_some_and(|m| matches!(m, Meta::Bool(true)))
        });
        waits.push(t3.elapsed().as_secs_f64());
        let keys: Vec<u8> = bot
            .st()
            .entities
            .values()
            .find(|e| e.uuid == b.uuid)
            .map(|e| e.meta.keys().copied().collect())
            .unwrap_or_default();
        let bpos = bot
            .st()
            .entities
            .values()
            .find(|e| e.uuid == b.uuid)
            .map(|e| [e.pos[0] - me[0], e.pos[1] - surface, e.pos[2] - me[2]]);
        ctx.note(format!(
            "bite {bit} after {:.1}s, bobber meta keys {keys:?}, at {bpos:?}",
            t3.elapsed().as_secs_f64()
        ));
        let d1 = bot.st().held_stack().damage;
        let t4 = Instant::now();
        bot.use_item();
        bot.wait_ticks(30);
        let items: Vec<String> = {
            let s = bot.st();
            s.entities
                .values()
                .chain(s.removed.iter())
                .filter(|e| e.added >= t4 && e.kind == "item")
                .filter_map(|e| e.item.as_ref().map(|i| i.name().to_string()))
                .collect()
        };
        let orbs = {
            let s = bot.st();
            s.entities
                .values()
                .chain(s.removed.iter())
                .filter(|e| e.added >= t4 && e.kind == "experience_orb")
                .count()
        };
        ctx.note(format!("catch: items {items:?}, orbs {orbs}"));
        if bit {
            caught = true;
            ctx.metric("catch_item", (!items.is_empty()) as u8 as f64, 0.0);
            ctx.metric("catch_orb", (orbs > 0) as u8 as f64, 0.0);
            ctx.metric(
                "catch_damage",
                (bot.st().held_stack().damage - d1) as f64,
                0.0,
            );
            break;
        }
    }
    ctx.metric("caught", caught as u8 as f64, 0.0);
    ctx.note(format!("bite waits {waits:?}"));
    Ok(())
}

/// One shot at a fresh NoAI cow 5.5 blocks ahead: (cow damage, cow horizontal speed after the hit, cow
/// shared flags, the projectile entity, arrows used, sounds).
struct CowShot {
    damage: f64,
    kb: f64,
    cow_flags: u8,
    cow_fire_after_40: u8,
    damage_40: f64,
    proj: Option<crate::bot::Entity>,
    used: i32,
}

fn shoot_cow(bot: &Bot, bow: &str, arrow: &str, ticks: u32) -> Result<CowShot> {
    bot.cmd_reply("clear @s")?;
    bot.wait_ticks(2);
    bot.cmd_reply(&format!("give @s minecraft:{bow} 1"))?;
    bot.cmd_reply(&format!("give @s minecraft:{arrow} 4"))?;
    let base = |s: &str| s.split('[').next().unwrap_or(s).to_string();
    let (bow_n, arrow_n) = (base(bow), base(arrow));
    bot.wait_until(Duration::from_secs(3), |s| {
        s.count(&bow_n) > 0 && s.count(&arrow_n) >= 4
    });
    hold(bot, &bow_n)?;
    let me = bot.st().pos();
    let t0 = Instant::now();
    bot.cmd_reply(&format!(
        "summon minecraft:cow {:.2} {} {:.2} {{NoAI:1b,PersistenceRequired:1b}}",
        me[0],
        Y,
        me[2] + 5.0
    ))?;
    if !bot.wait_until(Duration::from_secs(3), |s| {
        s.entities
            .values()
            .any(|e| e.kind == "cow" && e.added >= t0)
    }) {
        bail!("cow never appeared");
    }
    let cow = bot
        .st()
        .entities
        .iter()
        .find(|(_, e)| e.kind == "cow" && e.added >= t0)
        .map(|(k, _)| *k)
        .unwrap();
    bot.wait_ticks(4);
    let before = bot.st().count(&arrow_n);
    let t1 = Instant::now();
    bot.look(0.0, 0.0);
    bot.wait_ticks(2);
    bot.use_item();
    bot.wait_ticks(ticks);
    bot.release_use();
    bot.wait_until(Duration::from_secs(2), |s| {
        s.damage_events.iter().any(|d| d.0 >= t1 && d.1 == cow)
    });
    bot.wait_ticks(2);
    let (health, vel, flags) = {
        let s = bot.st();
        let e = s.entities.get(&cow);
        (
            e.and_then(|e| e.health).unwrap_or(10.0),
            e.map(|e| e.vel).unwrap_or([0.0; 3]),
            e.map(|e| e.flags).unwrap_or(0),
        )
    };
    bot.wait_ticks(40);
    let (fire40, health40) = {
        let s = bot.st();
        let e = s.entities.get(&cow);
        (
            e.map(|e| e.flags & 1).unwrap_or(9),
            e.and_then(|e| e.health).unwrap_or(0.0),
        )
    };
    let used = before - bot.st().count(&arrow_n);
    let proj = ["arrow", "spectral_arrow"]
        .iter()
        .find_map(|k| spawned(bot, k, t1));
    bot.cmd_reply(&format!(
        "kill @e[type=minecraft:cow,x={},y={},z={},distance=..8]",
        me[0],
        Y,
        me[2] + 5.0
    ))?;
    bot.wait_ticks(25);
    Ok(CowShot {
        damage: 10.0 - health as f64,
        kb: (vel[0] * vel[0] + vel[2] * vel[2]).sqrt(),
        cow_flags: flags,
        cow_fire_after_40: fire40,
        damage_40: (health - health40) as f64,
        proj,
        used,
    })
}

/// Bow enchantments (Power, Punch, Flame, Infinity) and special arrows (spectral, tipped) shot at a NoAI cow
/// with a half draw (no critical randomness).
pub fn bow_enchants(ctx: &mut Ctx) -> Result<()> {
    let (bot, _o) = box_arena(ctx, 10, "survival")?;
    for (tag, bow, arrow) in [
        ("plain", "bow", "arrow"),
        ("power1", "bow[enchantments={power:1}]", "arrow"),
        ("power5", "bow[enchantments={power:5}]", "arrow"),
        ("punch2", "bow[enchantments={punch:2}]", "arrow"),
        ("flame", "bow[enchantments={flame:1}]", "arrow"),
        ("infinity", "bow[enchantments={infinity:1}]", "arrow"),
        (
            "inf_spectral",
            "bow[enchantments={infinity:1}]",
            "spectral_arrow",
        ),
        ("spectral", "bow", "spectral_arrow"),
        (
            "tipped_poison",
            "bow",
            "tipped_arrow[potion_contents={potion:\"minecraft:poison\"}]",
        ),
        (
            "tipped_harming",
            "bow",
            "tipped_arrow[potion_contents={potion:\"minecraft:harming\"}]",
        ),
    ] {
        let r = shoot_cow(&bot, bow, arrow, 10)?;
        ctx.note(format!(
            "{tag}: damage {} kb {:.3} cow flags {:#x} fire40 {} used {} proj {:?}",
            r.damage,
            r.kb,
            r.cow_flags,
            r.cow_fire_after_40,
            r.used,
            r.proj.as_ref().map(|e| (e.kind, &e.meta, e.flags))
        ));
        ctx.metric(&format!("{tag}_damage"), r.damage, 0.5);
        ctx.metric(&format!("{tag}_kb"), r.kb, 0.08);
        ctx.metric(&format!("{tag}_damage_next_40t"), r.damage_40, 1.0);
        ctx.metric(&format!("{tag}_cow_burning"), (r.cow_flags & 1) as f64, 0.0);
        ctx.metric(
            &format!("{tag}_cow_glowing"),
            (r.cow_flags >> 6 & 1) as f64,
            0.0,
        );
        ctx.metric(
            &format!("{tag}_cow_burning_40t"),
            r.cow_fire_after_40 as f64,
            0.0,
        );
        ctx.metric(&format!("{tag}_arrows_used"), r.used as f64, 0.0);
        let p = r.proj.as_ref();
        ctx.metric(
            &format!("{tag}_proj_kind_spectral"),
            p.map(|e| (e.kind == "spectral_arrow") as u8 as f64)
                .unwrap_or(-1.0),
            0.0,
        );
        ctx.metric(
            &format!("{tag}_proj_burning"),
            p.map(|e| (e.flags & 1) as f64).unwrap_or(-1.0),
            0.0,
        );
        let color = p.and_then(|e| match e.meta.get(&11) {
            Some(Meta::Int(c)) => Some(*c as f64),
            _ => None,
        });
        ctx.metric(&format!("{tag}_proj_color"), color.unwrap_or(-1.0), 0.0);
    }
    // Infinity without any arrow: no shot
    bot.cmd_reply("clear @s")?;
    bot.cmd_reply("give @s minecraft:bow[enchantments={infinity:1}] 1")?;
    bot.wait_until(Duration::from_secs(3), |s| s.count("bow") > 0);
    hold(&bot, "bow")?;
    let t0 = Instant::now();
    bot.use_item();
    bot.wait_ticks(22);
    bot.release_use();
    bot.wait_ticks(4);
    ctx.metric(
        "infinity_no_arrow_shot",
        spawned(&bot, "arrow", t0).is_some() as u8 as f64,
        0.0,
    );
    Ok(())
}

/// Crossbow enchantments: Quick Charge (charge time + loading sounds), Multishot (3 arrows loaded from 1, spread,
/// durability), Piercing (pierce level metadata, passing through a cow into a second one).
pub fn crossbow_enchants(ctx: &mut Ctx) -> Result<()> {
    let (bot, _o) = box_arena(ctx, 12, "survival")?;
    let charged = |bot: &Bot| bot.st().held_stack().charged.len() as f64;
    for (tag, ench, ticks) in [
        ("qc1_short", "quick_charge:1", 18u32),
        ("qc3_short", "quick_charge:3", 9),
        ("qc3", "quick_charge:3", 13),
        ("multishot", "multishot:1", 30),
        ("piercing", "piercing:2", 30),
    ] {
        bot.cmd_reply("clear @s")?;
        bot.wait_ticks(2);
        bot.cmd_reply(&format!(
            "give @s minecraft:crossbow[enchantments={{{ench}}}] 1"
        ))?;
        bot.cmd_reply("give @s minecraft:arrow 8")?;
        bot.wait_until(Duration::from_secs(3), |s| {
            s.count("crossbow") > 0 && s.count("arrow") >= 8
        });
        hold(&bot, "crossbow")?;
        let me = bot.st().pos();
        let t0 = Instant::now();
        bot.look(0.0, 0.0);
        bot.wait_ticks(2);
        bot.use_item();
        bot.wait_ticks(ticks);
        bot.release_use();
        bot.wait_ticks(4);
        ctx.metric(&format!("{tag}_charged"), charged(&bot), 0.0);
        ctx.metric(
            &format!("{tag}_arrows_used"),
            (8 - bot.st().count("arrow")) as f64,
            0.0,
        );
        let snd: Vec<(String, u32)> = bot
            .st()
            .sound_detail
            .iter()
            .filter(|s| s.0 >= t0)
            .map(|s| {
                (
                    sound_name(s.1),
                    (s.0.duration_since(t0).as_secs_f64() * 20.0).round() as u32,
                )
            })
            .collect();
        ctx.note(format!("{tag}: sounds {snd:?}"));
        for (n, t) in &snd {
            if let Some(k) = n.strip_prefix("item.crossbow.") {
                ctx.metric(&format!("{tag}_sound_{k}_tick"), *t as f64, 3.0);
            }
        }
        if !tag.ends_with("_short") || charged(&bot) > 0.0 {
            // cows in the line of fire (piercing goes through the first)
            let tc = Instant::now();
            for dz in [4.0, 6.0] {
                bot.cmd_reply(&format!(
                    "summon minecraft:cow {:.2} {} {:.2} {{NoAI:1b,PersistenceRequired:1b}}",
                    me[0],
                    Y,
                    me[2] + dz
                ))?;
            }
            bot.wait_until(Duration::from_secs(3), |s| {
                s.entities
                    .values()
                    .filter(|e| e.kind == "cow" && e.added >= tc)
                    .count()
                    >= 2
            });
            bot.wait_ticks(4);
            bot.look(0.0, 6.0);
            bot.wait_ticks(2);
            let t1 = Instant::now();
            bot.use_item();
            bot.wait_ticks(2);
            // pierce level as sent while flying (vanilla clears it when the arrow sticks)
            let pierce = {
                let s = bot.st();
                s.entities
                    .values()
                    .chain(s.removed.iter())
                    .filter(|e| e.kind == "arrow" && e.added >= t1)
                    .find_map(|e| match e.meta.get(&9) {
                        Some(Meta::Byte(b)) => Some(*b as f64),
                        _ => None,
                    })
                    .unwrap_or(0.0)
            };
            bot.wait_ticks(8);
            let (arrows, hurt, meta): (Vec<[f64; 3]>, usize, Vec<String>) = {
                let s = bot.st();
                let arrows: Vec<&crate::bot::Entity> = s
                    .entities
                    .values()
                    .chain(s.removed.iter())
                    .filter(|e| e.kind == "arrow" && e.added >= t1)
                    .collect();
                let cows: Vec<i32> = s
                    .entities
                    .iter()
                    .filter(|(_, e)| e.kind == "cow" && e.added >= tc)
                    .map(|(k, _)| *k)
                    .collect();
                let hurt = cows
                    .iter()
                    .filter(|c| s.damage_events.iter().any(|d| d.0 >= t1 && d.1 == **c))
                    .count();
                (
                    arrows.iter().map(|e| e.spawn_vel).collect(),
                    hurt,
                    arrows.iter().map(|e| format!("{:?}", e.meta)).collect(),
                )
            };
            ctx.note(format!(
                "{tag}: fired {arrows:?} meta {meta:?} cows hurt {hurt}"
            ));
            ctx.metric(&format!("{tag}_fired"), arrows.len() as f64, 0.0);
            ctx.metric(&format!("{tag}_cows_hurt"), hurt as f64, 0.0);
            // horizontal spread between the outermost arrows, in degrees
            let yaws: Vec<f64> = arrows
                .iter()
                .map(|v| v[0].atan2(v[2]).to_degrees())
                .collect();
            let spread = yaws.iter().cloned().fold(f64::MIN, f64::max)
                - yaws.iter().cloned().fold(f64::MAX, f64::min);
            ctx.metric(
                &format!("{tag}_spread_deg"),
                if yaws.is_empty() { -1.0 } else { spread },
                3.0,
            );
            ctx.metric(&format!("{tag}_pierce_meta"), pierce, 0.0);
            ctx.metric(
                &format!("{tag}_durability_used"),
                bot.st().held_stack().damage as f64,
                0.0,
            );
            bot.cmd_reply(&format!(
                "kill @e[type=minecraft:cow,x={},y={},z={},distance=..10]",
                me[0],
                Y,
                me[2] + 5.0
            ))?;
            bot.cmd_reply(&format!(
                "kill @e[type=minecraft:arrow,x={},y={},z={},distance=..14]",
                me[0],
                Y,
                me[2] + 5.0
            ))?;
            bot.wait_ticks(25);
            bot.look(0.0, 0.0);
        }
    }
    Ok(())
}

/// Drinking: milk clears every effect and leaves a bucket, a potion gives its effects and leaves a glass bottle,
/// honey cures poison (+food) and leaves a bottle; creative keeps the milk. Also the drink duration (an early
/// release drinks nothing).
pub fn drink(ctx: &mut Ctx) -> Result<()> {
    let (bot, _o) = box_arena(ctx, 6, "survival")?;
    let fx = |bot: &Bot| {
        let mut v: Vec<String> = bot.st().effects.keys().cloned().collect();
        v.sort();
        v.join(",")
    };
    for (tag, gm, item, effects, hold_ticks) in [
        ("milk_short", "survival", "milk_bucket", "speed", 20u32),
        ("milk", "survival", "milk_bucket", "speed+poison", 40),
        (
            "potion",
            "survival",
            "potion[potion_contents={potion:\"minecraft:swiftness\"}]",
            "",
            40,
        ),
        ("honey", "survival", "honey_bottle", "poison", 50),
        ("milk_creative", "creative", "milk_bucket", "speed", 40),
    ] {
        ctx.gamemode(&bot, gm)?;
        bot.cmd_reply("clear @s")?;
        bot.cmd_reply("effect clear @s")?;
        bot.cmd_reply("effect give @s minecraft:hunger 2 255 true")?;
        bot.wait_ticks(45);
        bot.cmd_reply("effect clear @s")?;
        bot.wait_ticks(4);
        for e in effects.split('+').filter(|e| !e.is_empty()) {
            bot.cmd_reply(&format!("effect give @s minecraft:{e} 60 0"))?;
        }
        bot.cmd_reply(&format!("give @s minecraft:{item} 1"))?;
        let base = item.split('[').next().unwrap_or(item).to_string();
        bot.wait_until(Duration::from_secs(3), |s| s.count(&base) > 0);
        bot.wait_ticks(3);
        hold(&bot, &base)?;
        let food0 = bot.st().food;
        let before = fx(&bot);
        bot.use_item();
        bot.wait_ticks(hold_ticks);
        bot.release_use();
        bot.wait_ticks(4);
        let after = fx(&bot);
        let held = bot.st().held_stack().clone();
        ctx.note(format!(
            "{tag}: effects {before:?} -> {after:?}, held {} x{}, food {food0} -> {}",
            held.name(),
            held.count,
            bot.st().food
        ));
        ctx.metric(
            &format!("{tag}_effects_left"),
            after.split(',').filter(|s| !s.is_empty()).count() as f64,
            0.0,
        );
        ctx.metric(
            &format!("{tag}_has_speed"),
            after.contains("speed") as u8 as f64,
            0.0,
        );
        ctx.metric(
            &format!("{tag}_has_poison"),
            after.contains("poison") as u8 as f64,
            0.0,
        );
        let speed_dur = bot
            .st()
            .effects
            .get("speed")
            .map(|e| e.1 as f64)
            .unwrap_or(0.0);
        ctx.metric(&format!("{tag}_speed_duration"), speed_dur, 60.0);
        for want in [
            "milk_bucket",
            "bucket",
            "potion",
            "glass_bottle",
            "honey_bottle",
        ] {
            ctx.metric(
                &format!("{tag}_holds_{want}"),
                (held.count > 0 && held.name() == want) as u8 as f64,
                0.0,
            );
        }
        ctx.metric(
            &format!("{tag}_food_gain"),
            (bot.st().food - food0) as f64,
            0.0,
        );
    }
    Ok(())
}

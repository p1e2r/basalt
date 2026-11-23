//! Helpers shared by scenarios.
use crate::bot::Bot;
use anyhow::{bail, Result};
use std::time::{Duration, Instant};

/// Damage taken per hit of `kind` (damage_type name) since `since`, from damage_event + set_health.
pub fn hits(bot: &Bot, since: Instant, kind: &str) -> Vec<f32> {
    let s = bot.st();
    s.damage.iter().filter(|(t, k)| *t >= since && k == kind).map(|(t, _)| {
        let before = s.health_log.iter().rev().find(|(ht, _)| ht < t).map(|h| h.1).unwrap_or(20.0);
        let after = s.health_log.iter().find(|(ht, _)| ht >= t).map(|h| h.1).unwrap_or(before);
        before - after
    }).collect()
}

/// Times (ticks after `since`) of damage events of `kind`.
pub fn hit_ticks(bot: &Bot, since: Instant, kind: &str) -> Vec<f64> {
    bot.st().damage.iter().filter(|(t, k)| *t >= since && k == kind).map(|(t, _)| t.duration_since(since).as_secs_f64() * 20.0).collect()
}

/// All damage types seen since `since`.
pub fn damage_kinds(bot: &Bot, since: Instant) -> Vec<String> {
    let mut v: Vec<String> = bot.st().damage.iter().filter(|(t, _)| *t >= since).map(|(_, k)| k.clone()).collect();
    v.dedup(); v
}

/// Walk forward with a fixed yaw for `ticks` ticks (pushing into whatever is in the way).
pub fn push(bot: &Bot, yaw: f32, ticks: u32) {
    { let mut s = bot.st(); s.body.yaw = yaw; s.input.forward = 1.0; }
    bot.wait_ticks(ticks);
    bot.st().input.forward = 0.0;
}

/// Sample a value once per tick for `ticks` ticks.
pub fn sample<T>(bot: &Bot, ticks: u32, mut f: impl FnMut(&crate::bot::State) -> T) -> Vec<T> {
    let t0 = Instant::now();
    let mut out = Vec::new();
    for i in 0..ticks {
        out.push(f(&bot.st()));
        let next = t0 + Duration::from_millis(50 * (i as u64 + 1));
        let now = Instant::now();
        if next > now { std::thread::sleep(next - now); }
    }
    out
}

/// Ticks (wall clock / 50 ms) until `f` holds, or error after `max` ticks.
pub fn ticks_until(bot: &Bot, max: u32, f: impl FnMut(&crate::bot::State) -> bool) -> Result<f64> {
    let t0 = Instant::now();
    if !bot.wait_until(Duration::from_millis(50 * max as u64), f) { bail!("condition not reached within {max} ticks"); }
    Ok(t0.elapsed().as_secs_f64() * 20.0)
}

/// Give items with /give and wait until the bot's inventory shows them.
pub fn give(bot: &Bot, item: &str, n: i32) -> Result<()> {
    let before = bot.st().count(item);
    bot.cmd_reply(&format!("give @s minecraft:{item} {n}"))?;
    if !bot.wait_until(Duration::from_secs(3), |s| s.count(item) >= before + n) { bail!("/give {item} {n} didn't arrive (have {})", bot.st().count(item)); }
    Ok(())
}

/// Select the hotbar slot holding `item`.
pub fn hold(bot: &Bot, item: &str) -> Result<()> {
    let slot = { let s = bot.st(); (36..45).find(|&i| s.inv[i].count > 0 && s.inv[i].name() == item) };
    match slot { Some(i) => { bot.set_held((i - 36) as u8); bot.wait_ticks(2); Ok(()) } None => bail!("{item} not in hotbar") }
}

pub fn setblock(bot: &Bot, p: [i32; 3], block: &str) -> Result<()> {
    bot.cmd_reply(&format!("setblock {} {} {} {block}", p[0], p[1], p[2]))?;
    let want = block.split('[').next().unwrap_or(block).trim_start_matches("minecraft:").to_string();
    if !bot.wait_until(Duration::from_secs(2), |s| s.world.name(p[0], p[1], p[2]) == want) { bail!("setblock {block} at {p:?} not seen (have {})", bot.st().world.name(p[0], p[1], p[2])); }
    Ok(())
}

pub fn fill(bot: &Bot, a: [i32; 3], b: [i32; 3], block: &str) -> Result<()> {
    bot.cmd_reply(&format!("fill {} {} {} {} {} {} {block}", a[0], a[1], a[2], b[0], b[1], b[2]))?;
    let want = block.split('[').next().unwrap_or(block).trim_start_matches("minecraft:").to_string();
    if !bot.wait_until(Duration::from_secs(2), |s| s.world.name(b[0], b[1], b[2]) == want && s.world.name(a[0], a[1], a[2]) == want) { bail!("fill {block} not seen"); }
    Ok(())
}

/// Death message normalized to a cause ("fall", "drown", "void", ...) for either server.
pub fn death_cause(t: &crate::proto::Text) -> String {
    let p = t.key.clone().unwrap_or_default() + " " + &t.plain;
    for (k, c) in [("fell out of the world", "void"), ("outOfWorld", "void"), ("out_of_world", "void"), ("hit the ground", "fall"), ("death.attack.fall", "fall"), ("drown", "drown"), ("was killed", "kill"), ("death.attack.generic", "kill"), ("genericKill", "kill"), ("lava", "lava"), ("froze", "freeze"), ("freeze", "freeze")] {
        if p.contains(k) { return c.to_string(); }
    }
    p
}

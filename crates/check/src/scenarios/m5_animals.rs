//! M5 land animals vs water: wandering avoids lakes, swimming animals return to land, chickens
//! fall slowly. Differential against vanilla (the observed share of time in the water etc.).
use super::util::*;
use crate::bot::{Bot, Entity};
use crate::runner::{Ctx, ARENA_Y};
use anyhow::{bail, Result};
use std::time::{Duration, Instant};

const Y: i32 = ARENA_Y + 1;
const ANIMALS: [&str; 4] = ["cow", "pig", "sheep", "chicken"];
const TAG: &str = "m5aw";

fn summon(bot: &Bot, kind: &str, p: [f64; 3]) -> Result<(i32, Entity)> {
    let t0 = Instant::now();
    bot.cmd_reply(&format!(
        "summon minecraft:{kind} {:.3} {:.3} {:.3} {{PersistenceRequired:1b,Tags:[\"{TAG}\"]}}",
        p[0], p[1], p[2]
    ))?;
    let ok = bot.wait_until(Duration::from_secs(3), |s| {
        s.entities.values().any(|e| e.kind == kind && e.added >= t0)
    });
    if !ok {
        bail!("summoned {kind} never appeared");
    }
    let s = bot.st();
    let (k, e) = s
        .entities
        .iter()
        .filter(|(_, e)| e.kind == kind && e.added >= t0)
        .max_by_key(|(_, e)| e.added)
        .unwrap();
    Ok((*k, e.clone()))
}

/// Walled pen 24x14: land on x 0..14 (grass over stone, top at ARENA_Y), a 10x14 lake 3 deep on
/// x 14..24 whose surface is flush with the land. The bot hovers above in creative.
fn lake_pen(ctx: &mut Ctx) -> Result<(Bot, [i32; 3])> {
    let (sx, sz) = (24, 14);
    let bot = ctx.bot()?;
    let o = ctx.arena(&bot, sx, sz, 8)?;
    fill(
        &bot,
        [o[0] - 1, ARENA_Y - 3, o[2] - 1],
        [o[0] + sx, Y + 3, o[2] + sz],
        "stone",
    )?;
    fill(
        &bot,
        [o[0], ARENA_Y, o[2]],
        [o[0] + sx - 1, Y + 2, o[2] + sz - 1],
        "air",
    )?;
    fill(
        &bot,
        [o[0], ARENA_Y, o[2]],
        [o[0] + 13, ARENA_Y, o[2] + sz - 1],
        "grass_block",
    )?;
    fill(
        &bot,
        [o[0] + 14, ARENA_Y - 2, o[2]],
        [o[0] + sx - 1, ARENA_Y, o[2] + sz - 1],
        "water",
    )?;
    bot.tp([o[0] as f64 + 6.0, (Y + 6) as f64, o[2] as f64 + 7.0])?;
    bot.fly(true);
    Ok((bot, o))
}

fn in_lake(o: [i32; 3], pos: [f64; 3]) -> bool {
    pos[0] >= (o[0] + 14) as f64 && pos[1] < (Y + 1) as f64
}

fn clean_up(ctx: &Ctx, bot: &Bot) {
    if ctx.is_vanilla() {
        let _ = bot.cmd_reply(&format!("kill @e[tag={TAG}]"));
    }
}

/// 16 animals (4 per kind) start on the shore of a small lake; the share of animal-samples (one
/// per second) spent inside the lake is compared with vanilla's.
pub fn animals_avoid_water(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = lake_pen(ctx)?;
    let mut ids = vec![];
    for (i, kind) in ANIMALS.iter().enumerate() {
        for j in 0..4 {
            let p = [
                o[0] as f64 + 9.5 + (j % 2) as f64 * 2.0,
                Y as f64,
                o[2] as f64 + 1.5 + (i * 3 + j) as f64,
            ];
            ids.push((*kind, summon(&bot, kind, p)?.0));
        }
    }
    let secs = 150;
    let (mut samples, mut wet, mut final_wet) = (0usize, 0usize, 0usize);
    let mut per_kind = [0usize; 4];
    for sec in 0..secs {
        bot.wait_ticks(20);
        let s = bot.st();
        for (kind, id) in &ids {
            let Some(e) = s.entities.get(id) else {
                continue;
            };
            samples += 1;
            if in_lake(o, e.pos) {
                wet += 1;
                per_kind[ANIMALS.iter().position(|k| k == kind).unwrap()] += 1;
                if sec == secs - 1 {
                    final_wet += 1;
                }
            }
        }
    }
    clean_up(ctx, &bot);
    let frac = wet as f64 / samples.max(1) as f64;
    ctx.note(format!("{secs} s, {samples} animal-samples: {wet} in the lake ({:.1}%), at the end {final_wet}/16; per kind cow/pig/sheep/chicken {per_kind:?}", frac * 100.0));
    ctx.check(samples > 0, "no animals tracked");
    ctx.check(
        frac < 0.25,
        format!("animals spent {:.0}% of the time in the lake", frac * 100.0),
    );
    ctx.metric("water_fraction", (frac * 100.0).round() / 100.0, 0.12);
    ctx.metric("water_final", final_wet as f64, 3.0);
    Ok(())
}

/// Animals dropped into the lake ride the surface and swim back to land.
pub fn animal_float(ctx: &mut Ctx) -> Result<()> {
    let (bot, o) = lake_pen(ctx)?;
    let mut ids = vec![];
    let t0 = Instant::now();
    for (i, kind) in ANIMALS.iter().enumerate() {
        for j in 0..2 {
            let p = [
                o[0] as f64 + 18.5,
                (Y + 1) as f64,
                o[2] as f64 + 1.5 + (i * 3 + j) as f64,
            ];
            ids.push((*kind, summon(&bot, kind, p)?.0));
        }
    }
    bot.wait_ticks(40);
    let depth: Vec<f64> = {
        let s = bot.st();
        ids.iter()
            .filter_map(|(_, id)| s.entities.get(id))
            .map(|e| (ARENA_Y + 1) as f64 - e.pos[1])
            .collect()
    };
    let mean_depth = depth.iter().sum::<f64>() / depth.len().max(1) as f64;
    let max_depth = depth.iter().cloned().fold(f64::MIN, f64::max);
    let mut left: Vec<Option<f64>> = vec![None; ids.len()];
    for _ in 0..90 {
        {
            let s = bot.st();
            for (i, (_, id)) in ids.iter().enumerate() {
                if let Some(e) = s.entities.get(id) {
                    if left[i].is_none() && e.pos[0] < (o[0] + 14) as f64 {
                        left[i] = Some(t0.elapsed().as_secs_f64());
                    }
                }
            }
        }
        if left.iter().all(|l| l.is_some()) {
            break;
        }
        bot.wait_ticks(10);
    }
    bot.wait_ticks(60);
    let (out, sunk) = {
        let s = bot.st();
        (
            ids.iter()
                .filter(|(_, id)| {
                    s.entities
                        .get(id)
                        .map(|e| !in_lake(o, e.pos))
                        .unwrap_or(false)
                })
                .count(),
            ids.iter()
                .filter(|(_, id)| {
                    s.entities
                        .get(id)
                        .map(|e| e.pos[1] < (ARENA_Y - 1) as f64)
                        .unwrap_or(true)
                })
                .count(),
        )
    };
    clean_up(ctx, &bot);
    let n_left = left.iter().filter(|l| l.is_some()).count();
    ctx.note(format!("left the lake {n_left}/{}, on land at the end {out}, sunk {sunk}; depth after 2 s mean {mean_depth:.2} max {max_depth:.2}; leave times {:?}", ids.len(), left.iter().map(|l| l.map(|v| (v * 10.0).round() / 10.0)).collect::<Vec<_>>()));
    ctx.check(sunk == 0, "an animal sank to the lake floor");
    ctx.check(
        max_depth < 1.2,
        format!("an animal sits {max_depth:.2} blocks below the surface"),
    );
    // vanilla animals mostly keep bobbing (4-5 of 8 reached the shore in 50 s); Basalt's float goal
    // swims them out on purpose, so the tolerance only bounds "clearly worse than vanilla"
    ctx.check(
        out + 1 >= 4,
        format!("only {out}/8 animals made it out of the lake"),
    );
    ctx.metric("float_left_lake", n_left as f64, 4.0);
    ctx.metric("float_on_land_end", out as f64, 4.0);
    ctx.metric("float_max_depth", (max_depth * 4.0).round() / 4.0, 0.5);
    Ok(())
}

/// A chicken dropped from 40 blocks falls slowly (flapping): fall time and mean speed vs vanilla.
pub fn chicken_fall(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    let o = ctx.arena(&bot, 8, 8, 60)?;
    bot.tp([o[0] as f64 + 1.0, (Y + 45) as f64, o[2] as f64 + 1.0])?;
    bot.fly(true);
    let h = 40.0;
    let t0 = Instant::now();
    let (id, _) = summon(
        &bot,
        "chicken",
        [o[0] as f64 + 4.5, Y as f64 + h, o[2] as f64 + 4.5],
    )?;
    let mut track: Vec<(f64, f64)> = vec![];
    for _ in 0..400 {
        bot.wait_ticks(1);
        let s = bot.st();
        let Some(e) = s.entities.get(&id) else {
            continue;
        };
        track.push((t0.elapsed().as_secs_f64(), e.pos[1]));
        if e.on_ground && e.pos[1] < Y as f64 + 0.3 {
            break;
        }
    }
    if track.is_empty() {
        bail!("chicken not tracked");
    }
    let end = track.last().copied().unwrap();
    let landed = end.1 < Y as f64 + 0.5;
    let start = track
        .iter()
        .find(|p| p.1 < Y as f64 + h - 1.0)
        .copied()
        .unwrap_or(track[0]);
    let mid: Vec<_> = track
        .iter()
        .filter(|p| p.1 < Y as f64 + h - 10.0 && p.1 > Y as f64 + 6.0)
        .collect();
    let speed = if mid.len() >= 2 {
        (mid[0].1 - mid[mid.len() - 1].1) / (mid[mid.len() - 1].0 - mid[0].0) / 20.0
    } else {
        0.0
    };
    let fall = end.0 - start.0;
    clean_up(ctx, &bot);
    ctx.note(format!(
        "landed {landed}, fell {h} blocks in {fall:.2} s, mid-fall speed {speed:.3} blocks/tick"
    ));
    ctx.check(landed, "the chicken never landed");
    ctx.check(
        fall > 4.0,
        format!("fall too fast ({fall:.2} s for {h} blocks)"),
    );
    ctx.metric("chicken_fall_s", (fall * 2.0).round() / 2.0, 1.5);
    ctx.metric("chicken_fall_speed", (speed * 100.0).round() / 100.0, 0.03);
    Ok(())
}

//! M8 Nether scenarios: dimension changes, nether terrain seen over the wire, portal frames,
//! portal travel. The portal tests build their frames in the scenario's overworld arena and remove
//! the portals they create in the Nether afterwards (the vanilla server is shared).
use super::util::*;
use crate::bot::Bot;
use crate::proto::Meta;
use crate::runner::{Ctx, ARENA_Y};
use anyhow::{bail, Result};
use std::time::{Duration, Instant};

const NETHER: &str = "minecraft:the_nether";
const OVERWORLD: &str = "minecraft:overworld";
const END: &str = "minecraft:the_end";

fn wait_dim(bot: &Bot, dim: &str, secs: u64) -> bool {
    bot.wait_until(Duration::from_secs(secs), |s| s.dim == dim)
}

/// Portal blocks of one axis inside a box (inclusive).
fn portal_count(bot: &Bot, a: [i32; 3], b: [i32; 3], axis: &str) -> i32 {
    let s = bot.st();
    let want = format!("minecraft:nether_portal[axis={axis}]");
    let mut n = 0;
    for x in a[0].min(b[0])..=a[0].max(b[0]) {
        for y in a[1].min(b[1])..=a[1].max(b[1]) {
            for z in a[2].min(b[2])..=a[2].max(b[2]) {
                if s.world.state_string(x, y, z) == want {
                    n += 1;
                }
            }
        }
    }
    n
}

/// Obsidian frame with inner size `w` x `h` whose bottom-left outer corner is `o`, along x
/// (`axis_x`) or z. `corners: false` leaves the four corners empty (vanilla doesn't need them).
fn frame(bot: &Bot, o: [i32; 3], axis_x: bool, w: i32, h: i32, corners: bool) -> Result<()> {
    let at = |i: i32, j: i32| {
        if axis_x {
            [o[0] + i, o[1] + j, o[2]]
        } else {
            [o[0], o[1] + j, o[2] + i]
        }
    };
    let (lo, hi) = if corners { (0, w + 1) } else { (1, w) };
    let cmds = [
        (at(lo, 0), at(hi, 0)),
        (at(lo, h + 1), at(hi, h + 1)),
        (at(0, 1), at(0, h)),
        (at(w + 1, 1), at(w + 1, h)),
    ];
    for (a, b) in cmds {
        bot.cmd_reply(&format!(
            "fill {} {} {} {} {} {} minecraft:obsidian",
            a[0], a[1], a[2], b[0], b[1], b[2]
        ))?;
    }
    let top = at(w + 1, h);
    if !bot.wait_until(Duration::from_secs(3), |s| {
        s.world.name(top[0], top[1], top[2]) == "obsidian"
    }) {
        bail!("frame at {o:?} not seen");
    }
    Ok(())
}

/// Inner corners (bottom-left, top-right) of a frame built by [`frame`].
fn inner(o: [i32; 3], axis_x: bool, w: i32, h: i32) -> ([i32; 3], [i32; 3]) {
    if axis_x {
        ([o[0] + 1, o[1] + 1, o[2]], [o[0] + w, o[1] + h, o[2]])
    } else {
        ([o[0], o[1] + 1, o[2] + 1], [o[0], o[1] + h, o[2] + w])
    }
}

/// `execute in <dim> run tp`: respawn into the other dimension, the dimension's layout (bedrock
/// floor at y 0 and ceiling at 127) and terrain over the wire (block counts of two columns, both
/// servers generate seed 12345's nether), and back.
pub fn execute_in(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    ctx.gamemode(&bot, "creative")?;
    bot.fly(true);
    bot.set_frozen(true);
    let r0 = bot.st().respawns;
    bot.cmd_reply("execute in minecraft:the_nether run tp @s 8.5 100 8.5")?;
    ctx.check(
        wait_dim(&bot, NETHER, 10),
        format!("never moved to the nether (dim {})", bot.st().dim),
    );
    ctx.metric("respawns_to_nether", (bot.st().respawns - r0) as f64, 0.0);
    ctx.check(
        bot.wait_until(Duration::from_secs(10), |s| {
            (s.body.pos[0] - 8.5).abs() < 0.01 && (s.body.pos[1] - 100.0).abs() < 0.01
        }),
        format!("nether position {:?}", bot.st().body.pos),
    );
    bot.wait_chunks(2, Duration::from_secs(30))?;
    for (x, z) in [(8, 8), (-21, 37)] {
        if !bot.wait_until(Duration::from_secs(20), |s| s.world.loaded(x, z)) {
            bail!("nether chunk at {x},{z} not loaded");
        }
        let names: Vec<&str> = {
            let s = bot.st();
            (0..128).map(|y| s.world.name(x, y, z)).collect()
        };
        ctx.check(
            names[0] == "bedrock" && names[127] == "bedrock",
            format!("column {x},{z}: y0 {} y127 {}", names[0], names[127]),
        );
        for b in ["netherrack", "lava", "air", "bedrock"] {
            ctx.metric(
                &format!("col_{x}_{z}_{b}"),
                names.iter().filter(|n| **n == b).count() as f64,
                2.0,
            );
        }
        // nothing above the ceiling in the nether's 256-block build range
        let above = {
            let s = bot.st();
            (128..256)
                .filter(|&y| s.world.name(x, y, z) != "air")
                .count()
        };
        ctx.metric(&format!("col_{x}_{z}_above_roof"), above as f64, 0.0);
    }
    bot.cmd_reply("execute in minecraft:overworld run tp @s 0.5 220 0.5")?;
    ctx.check(
        wait_dim(&bot, OVERWORLD, 10),
        format!("never came back to the overworld (dim {})", bot.st().dim),
    );
    ctx.metric("respawns_total", (bot.st().respawns - r0) as f64, 0.0);
    bot.set_frozen(false);
    Ok(())
}

/// `execute in minecraft:the_end run tp`: respawn into the End (layout 0..256) and seed-12345 End
/// terrain over the wire: block counts of main-island columns and of a spike column (obsidian
/// pillar with bedrock on top), nothing below the island, and back.
pub fn end_execute_in(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    ctx.gamemode(&bot, "creative")?;
    bot.fly(true);
    bot.set_frozen(true);
    let r0 = bot.st().respawns;
    bot.cmd_reply("execute in minecraft:the_end run tp @s 20.5 120 20.5")?;
    ctx.check(
        wait_dim(&bot, END, 10),
        format!("never moved to the end (dim {})", bot.st().dim),
    );
    ctx.metric("respawns_to_end", (bot.st().respawns - r0) as f64, 0.0);
    ctx.check(
        bot.wait_until(Duration::from_secs(10), |s| {
            (s.body.pos[0] - 20.5).abs() < 0.01 && (s.body.pos[1] - 120.0).abs() < 0.01
        }),
        format!("end position {:?}", bot.st().body.pos),
    );
    bot.wait_chunks(2, Duration::from_secs(30))?;
    // (42, 0): the first spike's centre for every seed; the others: main island ground
    for (x, z) in [(20, 20), (-30, 7), (42, 0), (60, -45)] {
        if !bot.wait_until(Duration::from_secs(20), |s| s.world.loaded(x, z)) {
            bail!("end chunk at {x},{z} not loaded");
        }
        let names: Vec<&str> = {
            let s = bot.st();
            (0..256).map(|y| s.world.name(x, y, z)).collect()
        };
        for b in ["end_stone", "obsidian", "bedrock", "air"] {
            ctx.metric(
                &format!("col_{x}_{z}_{b}"),
                names.iter().filter(|n| **n == b).count() as f64,
                0.0,
            );
        }
        let top = names
            .iter()
            .rposition(|n| *n != "air" && *n != "fire")
            .unwrap_or(0);
        ctx.metric(&format!("col_{x}_{z}_top"), top as f64, 0.0);
    }
    bot.cmd_reply("execute in minecraft:overworld run tp @s 0.5 220 0.5")?;
    ctx.check(
        wait_dim(&bot, OVERWORLD, 10),
        format!("never came back to the overworld (dim {})", bot.st().dim),
    );
    ctx.metric("respawns_total", (bot.st().respawns - r0) as f64, 0.0);
    bot.set_frozen(false);
    Ok(())
}

/// The 12 frame positions around a 3x3 hole centred on `c`, with the facing that points into the
/// hole (`inward`) or away from it.
fn end_ring(c: [i32; 3], inward: bool) -> Vec<([i32; 3], &'static str)> {
    let mut v = Vec::new();
    for i in -1..=1 {
        let (n, s, w, e) = if inward {
            ("south", "north", "east", "west")
        } else {
            ("north", "south", "west", "east")
        };
        v.push(([c[0] + i, c[1], c[2] - 2], n));
        v.push(([c[0] + i, c[1], c[2] + 2], s));
        v.push(([c[0] - 2, c[1], c[2] + i], w));
        v.push(([c[0] + 2, c[1], c[2] + i], e));
    }
    v
}

fn count_name(bot: &Bot, a: [i32; 3], b: [i32; 3], name: &str) -> i32 {
    let s = bot.st();
    let mut n = 0;
    for x in a[0]..=b[0] {
        for y in a[1]..=b[1] {
            for z in a[2]..=b[2] {
                if s.world.name(x, y, z) == name {
                    n += 1;
                }
            }
        }
    }
    n
}

/// End portal frames + eyes of ender, travel to the End and back: an eye used on the last empty
/// frame of an inward-facing ring fills the 3x3 hole with end_portal (survival: the eye is used
/// up); a complete ring facing outward stays empty; stepping into the portal moves to the End's
/// arrival platform (position, rotation, 5x5 obsidian at y 48 with air above); an end portal in
/// the End brings the player back to the overworld (credits game event noted).
pub fn end_portal(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    let o = ctx.arena(&bot, 16, 16, 6)?;
    ctx.stand(&bot, o[0] + 1, o[2] + 1, "creative")?;
    let y = ARENA_Y + 1;
    let rings = [
        ([o[0] + 4, y, o[2] + 4], true),
        ([o[0] + 11, y, o[2] + 11], false),
    ];
    for (ri, (c, inward)) in rings.iter().enumerate() {
        let ring = end_ring(*c, *inward);
        // all but the first frame get their eye by setblock (that alone never makes a portal)
        for (i, (p, f)) in ring.iter().enumerate() {
            bot.cmd_reply(&format!(
                "setblock {} {} {} minecraft:end_portal_frame[facing={f},eye={}]",
                p[0],
                p[1],
                p[2],
                i != 0
            ))?;
        }
        let p0 = ring[0].0;
        if !bot.wait_until(Duration::from_secs(3), |s| {
            s.world.name(p0[0], p0[1], p0[2]) == "end_portal_frame"
        }) {
            bail!("ring {ri} not seen");
        }
        ctx.gamemode(&bot, "survival")?;
        give(&bot, "ender_eye", 2)?;
        hold(&bot, "ender_eye")?;
        bot.tp([p0[0] as f64 + 0.5, y as f64, p0[2] as f64 - 1.5])?;
        bot.wait_on_ground(Duration::from_secs(3));
        bot.look_at([p0[0] as f64 + 0.5, y as f64 + 0.8, p0[2] as f64 + 0.5]);
        bot.wait_ticks(2);
        bot.place_at(p0, 1, [0.5, 0.8125, 0.5]);
        bot.wait_until(Duration::from_secs(3), |s| {
            s.world
                .state_string(p0[0], p0[1], p0[2])
                .contains("eye=true")
        });
        bot.wait_ticks(10);
        let st = bot.st().world.state_string(p0[0], p0[1], p0[2]);
        ctx.metric(
            &format!("ring{ri}_frame_eye"),
            st.contains("eye=true") as u8 as f64,
            0.0,
        );
        ctx.metric(
            &format!("ring{ri}_eyes_left"),
            bot.st().count("ender_eye") as f64,
            0.0,
        );
        let n = count_name(
            &bot,
            [c[0] - 1, y, c[2] - 1],
            [c[0] + 1, y, c[2] + 1],
            "end_portal",
        );
        ctx.metric(&format!("ring{ri}_portal_blocks"), n as f64, 0.0);
        bot.cmd_reply("clear @s minecraft:ender_eye")?;
        ctx.gamemode(&bot, "creative")?;
    }
    // into the portal of the first ring
    let c = rings[0].0;
    if bot.st().world.name(c[0], y, c[2]) != "end_portal" {
        ctx.check(false, "no portal to travel through");
        return Ok(());
    }
    let ev0 = bot.st().game_events.len();
    bot.tp([c[0] as f64 + 0.5, y as f64 + 0.1, c[2] as f64 + 0.5])?;
    ctx.check(
        wait_dim(&bot, END, 10),
        format!("never reached the end (dim {})", bot.st().dim),
    );
    bot.wait_until(Duration::from_secs(5), |s| {
        (s.body.pos[0] - 100.5).abs() < 2.0
    });
    bot.wait_ticks(5);
    let (pos, yaw, pitch) = {
        let s = bot.st();
        (s.body.pos, s.body.yaw, s.body.pitch)
    };
    for (i, k) in ["x", "y", "z"].iter().enumerate() {
        ctx.metric(&format!("arrival_{k}"), pos[i], 0.01);
    }
    ctx.metric("arrival_yaw", yaw as f64, 0.5);
    ctx.metric("arrival_pitch", pitch as f64, 0.5);
    if !bot.wait_until(Duration::from_secs(10), |s| s.world.loaded(100, 0)) {
        bail!("arrival chunk not loaded");
    }
    ctx.metric(
        "platform_obsidian",
        count_name(&bot, [98, 48, -2], [102, 48, 2], "obsidian") as f64,
        0.0,
    );
    ctx.metric(
        "platform_air",
        count_name(&bot, [98, 49, -2], [102, 51, 2], "air") as f64,
        0.0,
    );
    // back: an end portal next to the platform
    bot.cmd_reply("setblock 98 49 0 minecraft:end_portal")?;
    bot.wait_until(Duration::from_secs(3), |s| {
        s.world.name(98, 49, 0) == "end_portal"
    });
    bot.tp([98.5, 49.1, 0.5])?;
    let mut back = wait_dim(&bot, OVERWORLD, 5);
    let win = bot.st().game_events[ev0..].iter().any(|e| e.0 == 4);
    if !back && win {
        // credits: the client answers with "perform respawn"
        bot.respawn();
        back = wait_dim(&bot, OVERWORLD, 10);
    }
    ctx.check(back, format!("never came back (dim {})", bot.st().dim));
    ctx.note(format!("win-game event on leaving the end: {win}"));
    bot.cmd_reply("execute in minecraft:the_end run setblock 98 49 0 air")?;
    Ok(())
}

/// The exit portal ("podium") the dragon fight builds at the top of the main island: layout of
/// the 7x7 columns around (0, 0) (block counts per layer relative to the bedrock pillar's top,
/// the pillar top y itself) and the torches around the pillar. Vanilla shows it with the
/// portal inactive (dragon alive); Basalt builds it on first entry.
pub fn end_podium(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    ctx.gamemode(&bot, "creative")?;
    bot.fly(true);
    bot.set_frozen(true);
    bot.cmd_reply("execute in minecraft:the_end run tp @s 0.5 110 12.5")?;
    ctx.check(
        wait_dim(&bot, END, 10),
        format!("never moved to the end (dim {})", bot.st().dim),
    );
    if !bot.wait_until(Duration::from_secs(20), |s| {
        s.world.loaded(0, 0) && s.world.loaded(-4, -4) && s.world.loaded(3, 3)
    }) {
        bail!("centre chunks not loaded");
    }
    bot.wait_ticks(20);
    // the pillar: the highest bedrock in column (0, 0)
    let top = {
        let s = bot.st();
        (0..256).rev().find(|&y| s.world.name(0, y, 0) == "bedrock")
    };
    let Some(top) = top else {
        ctx.check(false, "no bedrock pillar at 0,0");
        return Ok(());
    };
    ctx.metric("pillar_top_y", top as f64, 0.0);
    let mut legend: Vec<String> = Vec::new();
    for dy in -6..=1 {
        let y = top + dy;
        let mut rows = Vec::new();
        let mut counts: std::collections::BTreeMap<String, i32> = Default::default();
        for z in -4..=4 {
            let mut row = String::new();
            for x in -4..=4 {
                let st = bot.st().world.state_string(x, y, z);
                let n = st.trim_start_matches("minecraft:").to_string();
                let base = n.split('[').next().unwrap_or("").to_string();
                *counts.entry(base.clone()).or_default() += 1;
                let i = match legend.iter().position(|l| *l == n) {
                    Some(i) => i,
                    None => {
                        legend.push(n.clone());
                        legend.len() - 1
                    }
                };
                row.push(char::from(b'a' + i as u8));
            }
            rows.push(row);
        }
        for (k, v) in &counts {
            if k != "air" {
                ctx.metric(&format!("layer{dy}_{k}"), *v as f64, 0.0);
            }
        }
        ctx.note(format!("dy {dy}: {}", rows.join(" ")));
    }
    ctx.note(format!(
        "legend: {}",
        legend
            .iter()
            .enumerate()
            .map(|(i, l)| format!("{}={l}", char::from(b'a' + i as u8)))
            .collect::<Vec<_>>()
            .join(" ")
    ));
    bot.cmd_reply("execute in minecraft:overworld run tp @s 0.5 220 0.5")?;
    wait_dim(&bot, OVERWORLD, 10);
    bot.set_frozen(false);
    Ok(())
}

/// End crystals: one on every spike top (positions + show_bottom metadata seen over the wire), an
/// end_crystal item used on obsidian spawns one on top (not on stone), and hitting it blows it up
/// (power 6: crater in the arena floor).
pub fn end_crystals(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    let o = ctx.arena(&bot, 16, 16, 8)?;
    ctx.stand(&bot, o[0] + 2, o[2] + 2, "creative")?;
    let y = ARENA_Y + 1;
    // ---- placing
    let obs = [o[0] + 8, y, o[2] + 8];
    let stone = [o[0] + 4, y, o[2] + 8];
    setblock(&bot, obs, "minecraft:obsidian")?;
    setblock(&bot, stone, "minecraft:stone")?;
    ctx.gamemode(&bot, "survival")?;
    give(&bot, "end_crystal", 2)?;
    hold(&bot, "end_crystal")?;
    bot.tp([o[0] as f64 + 6.5, y as f64, o[2] as f64 + 5.5])?;
    bot.wait_on_ground(Duration::from_secs(3));
    let t0 = Instant::now();
    for b in [stone, obs] {
        bot.look_at([b[0] as f64 + 0.5, b[1] as f64 + 1.0, b[2] as f64 + 0.5]);
        bot.wait_ticks(2);
        bot.place(b, 1);
        bot.wait_ticks(10);
    }
    let placed: Vec<(i32, [f64; 3])> = {
        let s = bot.st();
        s.entities
            .iter()
            .filter(|(_, e)| e.kind == "end_crystal" && e.added >= t0)
            .map(|(k, e)| (*k, e.pos))
            .collect()
    };
    ctx.metric("placed_crystals", placed.len() as f64, 0.0);
    ctx.metric(
        "crystals_left_in_hand",
        bot.st().count("end_crystal") as f64,
        0.0,
    );
    if let Some((id, p)) = placed.first().copied() {
        ctx.metric("placed_dx", p[0] - obs[0] as f64, 0.01);
        ctx.metric("placed_dy", p[1] - obs[1] as f64, 0.01);
        ctx.metric("placed_dz", p[2] - obs[2] as f64, 0.01);
        let bottom = bot
            .st()
            .entities
            .get(&id)
            .and_then(|e| e.meta.get(&9).cloned());
        ctx.note(format!("placed crystal meta 9 (show_bottom): {bottom:?}"));
        // ---- hit it
        bot.look_at([p[0], p[1] + 1.0, p[2]]);
        bot.wait_ticks(2);
        bot.attack(id);
        let gone = bot.wait_until(Duration::from_secs(3), |s| !s.entities.contains_key(&id));
        ctx.metric("crystal_gone_after_hit", gone as u8 as f64, 0.0);
        bot.wait_ticks(10);
        // floor blocks blown away around it (the obsidian survives)
        let mut air = 0;
        {
            let s = bot.st();
            for x in obs[0] - 6..=obs[0] + 6 {
                for z in obs[2] - 6..=obs[2] + 6 {
                    if s.world.name(x, ARENA_Y, z) == "air" {
                        air += 1;
                    }
                }
            }
        }
        ctx.metric("floor_holes", air as f64, 15.0);
        let mut box_air = 0;
        {
            let s = bot.st();
            for x in obs[0] - 6..=obs[0] + 6 {
                for z in obs[2] - 6..=obs[2] + 6 {
                    for yy in ARENA_Y - 2..ARENA_Y {
                        if s.world.name(x, yy, z) == "air" {
                            box_air += 1;
                        }
                    }
                }
            }
        }
        let (side, exps) = {
            let s = bot.st();
            (
                s.world.name(stone[0], stone[1], stone[2]).to_string(),
                s.explosions
                    .iter()
                    .map(|e| (e.pos, e.radius, e.blocks))
                    .collect::<Vec<_>>(),
            )
        };
        ctx.note(format!("air below the floor after the blast: {box_air}; side stone {side}; explosions {exps:?}"));
        ctx.metric(
            "obsidian_kept",
            (bot.st().world.name(obs[0], obs[1], obs[2]) == "obsidian") as u8 as f64,
            0.0,
        );
    }
    // control: TNT in the same geometry (obsidian on the floor) blows holes in the floor
    ctx.gamemode(&bot, "creative")?;
    bot.fly(true);
    let t = [o[0] + 3, y, o[2] + 3];
    setblock(&bot, t, "minecraft:obsidian")?;
    bot.cmd_reply(&format!(
        "summon minecraft:tnt {}.5 {} {}.5 {{fuse:0}}",
        t[0],
        t[1] + 1,
        t[2]
    ))?;
    bot.wait_ticks(10);
    let holes = {
        let s = bot.st();
        let mut n = 0;
        for x in t[0] - 3..=t[0] + 3 {
            for z in t[2] - 3..=t[2] + 3 {
                if s.world.name(x, ARENA_Y, z) == "air" {
                    n += 1;
                }
            }
        }
        n
    };
    ctx.note(format!("tnt control: {holes} floor holes"));
    // ---- spikes
    bot.cmd_reply("execute in minecraft:the_end run tp @s 0.5 125 0.5")?;
    if !wait_dim(&bot, END, 10) {
        bail!("not in the end");
    }
    bot.set_frozen(true);
    bot.wait_until(Duration::from_secs(20), |s| {
        s.entities
            .values()
            .filter(|e| e.kind == "end_crystal")
            .count()
            >= 10
    });
    let mut cs: Vec<([f64; 3], Option<Meta>)> = {
        let s = bot.st();
        s.entities
            .values()
            .filter(|e| {
                e.kind == "end_crystal" && (e.pos[0].powi(2) + e.pos[2].powi(2)).sqrt() < 60.0
            })
            .map(|e| (e.pos, e.meta.get(&9).cloned()))
            .collect()
    };
    cs.sort_by(|a, b| {
        a.0[2]
            .atan2(a.0[0])
            .partial_cmp(&b.0[2].atan2(b.0[0]))
            .unwrap()
    });
    ctx.metric("spike_crystals", cs.len() as f64, 0.0);
    for (i, (p, m)) in cs.iter().enumerate() {
        ctx.metric(&format!("spike{i}_x"), p[0], 0.01);
        ctx.metric(&format!("spike{i}_y"), p[1], 0.01);
        ctx.metric(&format!("spike{i}_z"), p[2], 0.01);
        if i == 0 {
            ctx.note(format!("spike crystal meta 9 (show_bottom): {m:?}"));
        }
    }
    bot.set_frozen(false);
    bot.cmd_reply("execute in minecraft:overworld run tp @s 0.5 220 0.5")?;
    wait_dim(&bot, OVERWORLD, 10);
    Ok(())
}

/// Obsidian frames: fire lights an empty frame (axis x and z, with and without corners, 2x3 up to
/// 21 wide; 22 wide and incomplete frames don't light), flint and steel lights one, breaking a frame
/// block removes the whole portal.
pub fn portal_frames(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    let o = ctx.arena(&bot, 34, 16, 8)?;
    ctx.stand(&bot, o[0] + 1, o[2] + 1, "creative")?;
    let y = ARENA_Y + 1;
    // (origin, axis_x, w, h, corners, expect lit)
    let frames: [([i32; 3], bool, i32, i32, bool, bool); 5] = [
        ([o[0] + 2, y, o[2] + 3], true, 2, 3, true, true),
        ([o[0] + 8, y, o[2] + 2], false, 3, 4, false, true),
        ([o[0] + 2, y, o[2] + 8], true, 21, 3, true, true),
        ([o[0] + 2, y, o[2] + 12], true, 22, 3, true, false),
        ([o[0] + 26, y, o[2] + 3], true, 2, 3, true, false),
    ];
    for (i, f) in frames.iter().enumerate() {
        frame(&bot, f.0, f.1, f.2, f.3, f.4)?;
        if i == 4 {
            // incomplete: no top-right block
            let t = [f.0[0] + f.2, f.0[1] + f.3 + 1, f.0[2]];
            bot.cmd_reply(&format!("setblock {} {} {} air", t[0], t[1], t[2]))?;
        }
        let (a, b) = inner(f.0, f.1, f.2, f.3);
        bot.cmd_reply(&format!(
            "setblock {} {} {} minecraft:fire",
            a[0], a[1], a[2]
        ))?;
        let axis = if f.1 { "x" } else { "z" };
        let want = if f.5 { f.2 * f.3 } else { 0 };
        // (portal_count takes the state lock itself: poll instead of wait_until)
        let deadline = Instant::now() + Duration::from_secs(3);
        while portal_count(&bot, a, b, axis) != want && Instant::now() < deadline {
            bot.wait_ticks(2);
        }
        if want == 0 {
            bot.wait_ticks(10);
        }
        let n = portal_count(&bot, a, b, axis);
        ctx.check(
            n == want,
            format!(
                "frame {i} ({}x{} axis {axis}): {n} portal blocks, want {want}",
                f.2, f.3
            ),
        );
        ctx.metric(&format!("frame{i}_portal_blocks"), n as f64, 0.0);
        let fire_left = bot.st().world.name(a[0], a[1], a[2]).to_string();
        ctx.metric(
            &format!("frame{i}_fire_kept"),
            (fire_left == "fire") as u8 as f64,
            0.0,
        );
    }
    // breaking one frame block of the first portal removes all of its portal blocks
    let f = frames[0];
    let (a, b) = inner(f.0, f.1, f.2, f.3);
    bot.cmd_reply(&format!(
        "setblock {} {} {} air",
        f.0[0],
        f.0[1] + 2,
        f.0[2]
    ))?;
    bot.wait_until(Duration::from_secs(3), |s| {
        s.world.name(a[0], a[1] + 1, a[2]) != "nether_portal"
    });
    let n = portal_count(&bot, a, b, "x");
    ctx.check(
        n == 0,
        format!("{n} portal blocks left after breaking the frame"),
    );
    ctx.metric("broken_portal_blocks_left", n as f64, 0.0);
    // flint and steel on the bottom of a fresh frame
    let fo = [o[0] + 14, y, o[2] + 3];
    frame(&bot, fo, true, 2, 3, true)?;
    ctx.gamemode(&bot, "survival")?;
    give(&bot, "flint_and_steel", 1)?;
    hold(&bot, "flint_and_steel")?;
    bot.tp([fo[0] as f64 + 1.5, y as f64, fo[2] as f64 - 1.5])?;
    bot.wait_on_ground(Duration::from_secs(3));
    bot.look_at([fo[0] as f64 + 1.5, y as f64 + 1.0, fo[2] as f64 + 0.5]);
    bot.wait_ticks(2);
    bot.place([fo[0] + 1, y, fo[2]], 1);
    let (a, b) = inner(fo, true, 2, 3);
    bot.wait_until(Duration::from_secs(3), |s| {
        s.world.name(b[0], b[1], b[2]) == "nether_portal"
    });
    let n = portal_count(&bot, a, b, "x");
    ctx.check(n == 6, format!("flint and steel: {n} portal blocks"));
    ctx.metric("flint_portal_blocks", n as f64, 0.0);
    let dmg = bot
        .st()
        .inv
        .iter()
        .find(|s| s.name() == "flint_and_steel")
        .map(|s| s.damage)
        .unwrap_or(-1);
    ctx.metric("flint_damage", dmg as f64, 0.0);
    // remove everything (vanilla: shared server)
    bot.cmd_reply(&format!(
        "fill {} {} {} {} {} {} air",
        o[0],
        y,
        o[2],
        o[0] + 33,
        y + 6,
        o[2] + 15
    ))?;
    Ok(())
}

/// Standing in a portal: survival travel after ~80 ticks to x/8, z/8 in the nether, where a new
/// portal (axis kept) is built; staying in the arrival portal doesn't bounce back; stepping out
/// and in again returns to the original overworld portal. Creative travels at once. Removes the
/// nether portal afterwards.
pub fn portal_travel(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    let o = ctx.arena(&bot, 14, 14, 8)?;
    ctx.stand(&bot, o[0] + 1, o[2] + 1, "creative")?;
    let y = ARENA_Y;
    // frame bottom in the floor: inner x o+5..o+6, y ARENA_Y+1..+3, z o+6
    let fo = [o[0] + 4, y, o[2] + 6];
    frame(&bot, fo, true, 2, 3, true)?;
    bot.cmd_reply(&format!(
        "setblock {} {} {} minecraft:fire",
        fo[0] + 1,
        y + 1,
        fo[2]
    ))?;
    if !bot.wait_until(Duration::from_secs(3), |s| {
        s.world.name(fo[0] + 2, y + 3, fo[2]) == "nether_portal"
    }) {
        bail!("portal did not light");
    }
    ctx.gamemode(&bot, "survival")?;
    bot.fly(false);
    // step in from the side (z - 1.5) so the timer starts when the bot enters
    bot.tp([fo[0] as f64 + 1.5, (y + 1) as f64, fo[2] as f64 - 1.5])?;
    bot.wait_on_ground(Duration::from_secs(3));
    let r0 = bot.st().respawns;
    let t0 = Instant::now();
    bot.walk_by(0.0, 2.0, Duration::from_secs(3)).ok();
    let arrived = wait_dim(&bot, NETHER, 12);
    let ticks = t0.elapsed().as_secs_f64() * 20.0;
    if !ctx.check(
        arrived,
        format!(
            "survival: no nether after {ticks:.0} ticks (dim {})",
            bot.st().dim
        ),
    ) {
        return Ok(());
    }
    ctx.note(format!(
        "survival travel after {ticks:.1} ticks (from starting the walk)"
    ));
    ctx.metric("travel_ticks_ge_70", (ticks >= 70.0) as u8 as f64, 0.0);
    ctx.metric("travel_ticks_le_110", (ticks <= 110.0) as u8 as f64, 0.0);
    bot.wait_until(Duration::from_secs(10), |s| s.world.chunks.len() > 20);
    bot.wait_on_ground(Duration::from_secs(5));
    let p = bot.st().body.pos;
    ctx.note(format!("nether arrival {p:?}"));
    // the exit portal: target x/8, z/8; record what was built around the arrival
    let (bx, by, bz) = (
        p[0].floor() as i32,
        p[1].floor() as i32,
        p[2].floor() as i32,
    );
    let portals_here = portal_count(
        &bot,
        [bx - 2, by - 1, bz - 2],
        [bx + 2, by + 3, bz + 2],
        "x",
    );
    ctx.check(
        portals_here == 6,
        format!("{portals_here} x-axis portal blocks around the nether arrival {p:?}"),
    );
    ctx.metric("nether_exit_portal_blocks", portals_here as f64, 0.0);
    // the exact spot depends on the nether terrain there, which earlier runs on the shared vanilla
    // server may have changed (first run: x and y identical to vanilla): compare loosely
    ctx.note(format!("arrival x {}", p[0])); // varies run to run on the shared vanilla world
                                             // known gap: where a new exit portal is built (vanilla 26.2 picked y 89 / z 38.5 where Basalt
                                             // picks y 113 / z 43.5 on the shared world, 2026-11-24) — noted, not compared yet
    ctx.note(format!("arrival y {} z {}", p[1], p[2]));
    ctx.metric(
        "nether_arrival_centred",
        ((p[0].fract().abs() - 0.5).abs() < 0.01 || (p[2].fract().abs() - 0.5).abs() < 0.01) as u8
            as f64,
        0.0,
    );
    let target = ((fo[0] as f64 + 1.5) / 8.0, (fo[2] as f64 + 0.5) / 8.0);
    ctx.check(
        (p[0] - target.0).abs() < 17.0 && (p[2] - target.1).abs() < 17.0,
        format!("arrival {p:?} not near x/8 z/8 {target:?}"),
    );
    // stay inside: no bounce-back while on cooldown
    bot.wait_ticks(120);
    ctx.metric(
        "bounced_while_standing",
        (bot.st().respawns - r0 - 1) as f64,
        0.0,
    );
    // step out across the portal and back in
    bot.walk_by(0.0, 1.6, Duration::from_secs(3)).ok();
    bot.wait_ticks(30);
    let t1 = Instant::now();
    bot.walk_by(0.0, -1.6, Duration::from_secs(3)).ok();
    let back = wait_dim(&bot, OVERWORLD, 12);
    let ticks_back = t1.elapsed().as_secs_f64() * 20.0;
    // remove the nether portal first (we know where it is), whatever happened
    let clean = format!("execute in minecraft:the_nether run fill {} {} {} {} {} {} air replace minecraft:nether_portal", bx - 4, by - 2, bz - 4, bx + 4, by + 5, bz + 4);
    let clean2 = format!(
        "execute in minecraft:the_nether run fill {} {} {} {} {} {} air replace minecraft:obsidian",
        bx - 4,
        by - 2,
        bz - 4,
        bx + 4,
        by + 5,
        bz + 4
    );
    if ctx.check(
        back,
        format!(
            "no way back after {ticks_back:.0} ticks (dim {})",
            bot.st().dim
        ),
    ) {
        bot.wait_on_ground(Duration::from_secs(5));
        let q = bot.st().body.pos;
        ctx.check(
            (q[0] - (fo[0] as f64 + 1.5)).abs() < 1.5
                && (q[2] - (fo[2] as f64 + 0.5)).abs() < 1.0
                && (q[1] - (y + 1) as f64).abs() < 0.5,
            format!(
                "came back at {q:?}, not in the original portal near {:?}",
                [fo[0] + 1, y + 1, fo[2]]
            ),
        );
        ctx.metric("overworld_return_x", q[0], 0.05);
        ctx.metric("overworld_return_z", q[2], 0.05);
        ctx.metric("overworld_return_y", q[1], 0.05);
    }
    bot.cmd_reply(&clean)?;
    bot.cmd_reply(&clean2)?;
    // creative: travel on the first tick inside
    ctx.gamemode(&bot, "creative")?;
    bot.fly(false);
    bot.tp([fo[0] as f64 + 1.5, (y + 1) as f64, fo[2] as f64 - 1.5])?;
    bot.wait_on_ground(Duration::from_secs(3));
    bot.wait_ticks(15);
    let t2 = Instant::now();
    bot.walk_by(0.0, 2.0, Duration::from_secs(3)).ok();
    let went = wait_dim(&bot, NETHER, 6);
    let ct = t2.elapsed().as_secs_f64() * 20.0;
    let (pp, inb) = {
        let s = bot.st();
        (
            s.body.pos,
            s.world.name(fo[0] + 1, y + 1, fo[2]).to_string(),
        )
    };
    ctx.metric(
        "creative_travel_ticks_le_80",
        (went && ct <= 80.0) as u8 as f64,
        0.0,
    );
    ctx.check(
        went && ct < 80.0,
        format!("creative travel: {went} after {ct:.0} ticks (at {pp:?}, portal block {inb})"),
    );
    if went {
        bot.wait_until(Duration::from_secs(10), |s| s.world.chunks.len() > 20);
        let p = bot.st().body.pos;
        let (bx, by, bz) = (
            p[0].floor() as i32,
            p[1].floor() as i32,
            p[2].floor() as i32,
        );
        bot.cmd_reply(&format!("execute in minecraft:the_nether run fill {} {} {} {} {} {} air replace minecraft:nether_portal", bx - 4, by - 2, bz - 4, bx + 4, by + 5, bz + 4))?;
        bot.cmd_reply(&format!("execute in minecraft:the_nether run fill {} {} {} {} {} {} air replace minecraft:obsidian", bx - 4, by - 2, bz - 4, bx + 4, by + 5, bz + 4))?;
        bot.cmd_reply("execute in minecraft:overworld run tp @s 0.5 220 0.5")?;
        wait_dim(&bot, OVERWORLD, 10);
    }
    // the overworld frame
    bot.cmd_reply(&format!(
        "fill {} {} {} {} {} {} air",
        fo[0],
        y + 1,
        fo[2],
        fo[0] + 3,
        y + 4,
        fo[2]
    ))?;
    Ok(())
}

/// A player in the nether and a block placed there survive a restart; the nether is saved in
/// vanilla's folder layout (world/dimensions/minecraft/the_nether/region).
pub fn nether_persistence(ctx: &mut Ctx) -> Result<()> {
    dim_persistence(ctx, NETHER, "the_nether", [8.5, 110.0, 8.5], [9, 112, 9])
}

/// A player + block in the End survive a restart (region in dimensions/minecraft/the_end), and the
/// exit podium is built once (its pillar top stays the highest bedrock of column 0,0).
pub fn end_persistence(ctx: &mut Ctx) -> Result<()> {
    dim_persistence(ctx, END, "the_end", [8.5, 90.0, 8.5], [9, 92, 9])?;
    let bot = Bot::connect(&ctx.addr(), "EndCheck")?;
    ctx.gamemode(&bot, "creative")?;
    bot.fly(true);
    bot.cmd_reply("execute in minecraft:the_end run tp @s 0.5 100 6.5")?;
    if !wait_dim(&bot, END, 10)
        || !bot.wait_until(Duration::from_secs(20), |s| s.world.loaded(0, 0))
    {
        bail!("second visit failed");
    }
    let tops: Vec<i32> = {
        let s = bot.st();
        (0..256)
            .filter(|&y| s.world.name(0, y, 0) == "bedrock")
            .collect()
    };
    // disc below + 4-high pillar on seed 12345's surface (y 64), as measured in m8_end_podium
    ctx.check(
        tops == (63..=67).collect::<Vec<_>>(),
        format!("pillar bedrock at {tops:?}"),
    );
    Ok(())
}

fn dim_persistence(
    ctx: &mut Ctx,
    dim: &str,
    folder: &str,
    at: [f64; 3],
    b: [i32; 3],
) -> Result<()> {
    let bot = ctx.bot()?;
    let name = bot.name.clone();
    ctx.gamemode(&bot, "creative")?;
    bot.fly(true);
    bot.cmd_reply(&format!(
        "execute in {dim} run tp @s {} {} {}",
        at[0], at[1], at[2]
    ))?;
    if !wait_dim(&bot, dim, 10) {
        bail!("not in {dim}");
    }
    bot.wait_chunks(1, Duration::from_secs(20))?;
    setblock(&bot, b, "minecraft:gold_block")?;
    bot.quit();
    std::thread::sleep(Duration::from_millis(300));
    ctx.restart()?;
    let bot = Bot::connect(&ctx.addr(), &name)?;
    ctx.check(
        wait_dim(&bot, dim, 10),
        format!("after restart in {}", bot.st().dim),
    );
    bot.wait_chunks(1, Duration::from_secs(20))?;
    let p = bot.st().pos();
    ctx.check(
        (p[0] - at[0]).abs() < 0.5 && (p[2] - at[2]).abs() < 0.5,
        format!("position after restart {p:?}"),
    );
    ctx.check(
        bot.wait_until(Duration::from_secs(5), |s| {
            s.world.name(b[0], b[1], b[2]) == "gold_block"
        }),
        format!(
            "block after restart: {}",
            bot.st().world.name(b[0], b[1], b[2])
        ),
    );
    let region = ctx
        .basalt
        .as_ref()
        .map(|s| s.args.clone())
        .unwrap_or_default();
    let dir = region
        .iter()
        .position(|a| a == "--world")
        .and_then(|i| region.get(i + 1))
        .cloned()
        .unwrap_or_default();
    let path =
        std::path::Path::new(&dir).join(format!("dimensions/minecraft/{folder}/region/r.0.0.mca"));
    ctx.check(path.exists(), format!("{} missing", path.display()));
    Ok(())
}

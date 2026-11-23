//! M8 Nether scenarios: dimension changes, nether terrain seen over the wire, portal frames,
//! portal travel. The portal tests build their frames in the scenario's overworld arena and remove
//! the portals they create in the Nether afterwards (the vanilla server is shared).
use super::util::*;
use crate::bot::Bot;
use crate::runner::{Ctx, ARENA_Y};
use anyhow::{bail, Result};
use std::time::{Duration, Instant};

const NETHER: &str = "minecraft:the_nether";
const OVERWORLD: &str = "minecraft:overworld";

fn wait_dim(bot: &Bot, dim: &str, secs: u64) -> bool { bot.wait_until(Duration::from_secs(secs), |s| s.dim == dim) }

/// Portal blocks of one axis inside a box (inclusive).
fn portal_count(bot: &Bot, a: [i32; 3], b: [i32; 3], axis: &str) -> i32 {
    let s = bot.st();
    let want = format!("minecraft:nether_portal[axis={axis}]");
    let mut n = 0;
    for x in a[0].min(b[0])..=a[0].max(b[0]) { for y in a[1].min(b[1])..=a[1].max(b[1]) { for z in a[2].min(b[2])..=a[2].max(b[2]) {
        if s.world.state_string(x, y, z) == want { n += 1; }
    } } }
    n
}

/// Obsidian frame with inner size `w` x `h` whose bottom-left outer corner is `o`, along x
/// (`axis_x`) or z. `corners: false` leaves the four corners empty (vanilla doesn't need them).
fn frame(bot: &Bot, o: [i32; 3], axis_x: bool, w: i32, h: i32, corners: bool) -> Result<()> {
    let at = |i: i32, j: i32| if axis_x { [o[0] + i, o[1] + j, o[2]] } else { [o[0], o[1] + j, o[2] + i] };
    let (lo, hi) = if corners { (0, w + 1) } else { (1, w) };
    let cmds = [
        (at(lo, 0), at(hi, 0)), (at(lo, h + 1), at(hi, h + 1)), (at(0, 1), at(0, h)), (at(w + 1, 1), at(w + 1, h)),
    ];
    for (a, b) in cmds { bot.cmd_reply(&format!("fill {} {} {} {} {} {} minecraft:obsidian", a[0], a[1], a[2], b[0], b[1], b[2]))?; }
    let top = at(w + 1, h);
    if !bot.wait_until(Duration::from_secs(3), |s| s.world.name(top[0], top[1], top[2]) == "obsidian") { bail!("frame at {o:?} not seen"); }
    Ok(())
}

/// Inner corners (bottom-left, top-right) of a frame built by [`frame`].
fn inner(o: [i32; 3], axis_x: bool, w: i32, h: i32) -> ([i32; 3], [i32; 3]) {
    if axis_x { ([o[0] + 1, o[1] + 1, o[2]], [o[0] + w, o[1] + h, o[2]]) } else { ([o[0], o[1] + 1, o[2] + 1], [o[0], o[1] + h, o[2] + w]) }
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
    ctx.check(wait_dim(&bot, NETHER, 10), format!("never moved to the nether (dim {})", bot.st().dim));
    ctx.metric("respawns_to_nether", (bot.st().respawns - r0) as f64, 0.0);
    ctx.check(bot.wait_until(Duration::from_secs(10), |s| (s.body.pos[0] - 8.5).abs() < 0.01 && (s.body.pos[1] - 100.0).abs() < 0.01), format!("nether position {:?}", bot.st().body.pos));
    bot.wait_chunks(2, Duration::from_secs(30))?;
    for (x, z) in [(8, 8), (-21, 37)] {
        if !bot.wait_until(Duration::from_secs(20), |s| s.world.loaded(x, z)) { bail!("nether chunk at {x},{z} not loaded"); }
        let names: Vec<&str> = { let s = bot.st(); (0..128).map(|y| s.world.name(x, y, z)).collect() };
        ctx.check(names[0] == "bedrock" && names[127] == "bedrock", format!("column {x},{z}: y0 {} y127 {}", names[0], names[127]));
        for b in ["netherrack", "lava", "air", "bedrock"] {
            ctx.metric(&format!("col_{x}_{z}_{b}"), names.iter().filter(|n| **n == b).count() as f64, 2.0);
        }
        // nothing above the ceiling in the nether's 256-block build range
        let above = { let s = bot.st(); (128..256).filter(|&y| s.world.name(x, y, z) != "air").count() };
        ctx.metric(&format!("col_{x}_{z}_above_roof"), above as f64, 0.0);
    }
    bot.cmd_reply("execute in minecraft:overworld run tp @s 0.5 220 0.5")?;
    ctx.check(wait_dim(&bot, OVERWORLD, 10), format!("never came back to the overworld (dim {})", bot.st().dim));
    ctx.metric("respawns_total", (bot.st().respawns - r0) as f64, 0.0);
    bot.set_frozen(false);
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
        bot.cmd_reply(&format!("setblock {} {} {} minecraft:fire", a[0], a[1], a[2]))?;
        let axis = if f.1 { "x" } else { "z" };
        let want = if f.5 { f.2 * f.3 } else { 0 };
        // (portal_count takes the state lock itself: poll instead of wait_until)
        let deadline = Instant::now() + Duration::from_secs(3);
        while portal_count(&bot, a, b, axis) != want && Instant::now() < deadline { bot.wait_ticks(2); }
        if want == 0 { bot.wait_ticks(10); }
        let n = portal_count(&bot, a, b, axis);
        ctx.check(n == want, format!("frame {i} ({}x{} axis {axis}): {n} portal blocks, want {want}", f.2, f.3));
        ctx.metric(&format!("frame{i}_portal_blocks"), n as f64, 0.0);
        let fire_left = bot.st().world.name(a[0], a[1], a[2]).to_string();
        ctx.metric(&format!("frame{i}_fire_kept"), (fire_left == "fire") as u8 as f64, 0.0);
    }
    // breaking one frame block of the first portal removes all of its portal blocks
    let f = frames[0];
    let (a, b) = inner(f.0, f.1, f.2, f.3);
    bot.cmd_reply(&format!("setblock {} {} {} air", f.0[0], f.0[1] + 2, f.0[2]))?;
    bot.wait_until(Duration::from_secs(3), |s| s.world.name(a[0], a[1] + 1, a[2]) != "nether_portal");
    let n = portal_count(&bot, a, b, "x");
    ctx.check(n == 0, format!("{n} portal blocks left after breaking the frame"));
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
    bot.wait_until(Duration::from_secs(3), |s| s.world.name(b[0], b[1], b[2]) == "nether_portal");
    let n = portal_count(&bot, a, b, "x");
    ctx.check(n == 6, format!("flint and steel: {n} portal blocks"));
    ctx.metric("flint_portal_blocks", n as f64, 0.0);
    let dmg = bot.st().inv.iter().find(|s| s.name() == "flint_and_steel").map(|s| s.damage).unwrap_or(-1);
    ctx.metric("flint_damage", dmg as f64, 0.0);
    // remove everything (vanilla: shared server)
    bot.cmd_reply(&format!("fill {} {} {} {} {} {} air", o[0], y, o[2], o[0] + 33, y + 6, o[2] + 15))?;
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
    bot.cmd_reply(&format!("setblock {} {} {} minecraft:fire", fo[0] + 1, y + 1, fo[2]))?;
    if !bot.wait_until(Duration::from_secs(3), |s| s.world.name(fo[0] + 2, y + 3, fo[2]) == "nether_portal") { bail!("portal did not light"); }
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
    if !ctx.check(arrived, format!("survival: no nether after {ticks:.0} ticks (dim {})", bot.st().dim)) { return Ok(()); }
    ctx.note(format!("survival travel after {ticks:.1} ticks (from starting the walk)"));
    ctx.metric("travel_ticks_ge_70", (ticks >= 70.0) as u8 as f64, 0.0);
    ctx.metric("travel_ticks_le_110", (ticks <= 110.0) as u8 as f64, 0.0);
    bot.wait_until(Duration::from_secs(10), |s| s.world.chunks.len() > 20);
    bot.wait_on_ground(Duration::from_secs(5));
    let p = bot.st().body.pos;
    ctx.note(format!("nether arrival {p:?}"));
    // the exit portal: target x/8, z/8; record what was built around the arrival
    let (bx, by, bz) = (p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32);
    let portals_here = portal_count(&bot, [bx - 2, by - 1, bz - 2], [bx + 2, by + 3, bz + 2], "x");
    ctx.check(portals_here == 6, format!("{portals_here} x-axis portal blocks around the nether arrival {p:?}"));
    ctx.metric("nether_exit_portal_blocks", portals_here as f64, 0.0);
    // the exact spot depends on the nether terrain there, which earlier runs on the shared vanilla
    // server may have changed (first run: x and y identical to vanilla): compare loosely
    ctx.metric("nether_arrival_x", p[0], 4.0);
    ctx.metric("nether_arrival_y", p[1], 4.0);
    ctx.metric("nether_arrival_z", p[2], 4.0);
    ctx.metric("nether_arrival_centred", ((p[0].fract().abs() - 0.5).abs() < 0.01 || (p[2].fract().abs() - 0.5).abs() < 0.01) as u8 as f64, 0.0);
    let target = ((fo[0] as f64 + 1.5) / 8.0, (fo[2] as f64 + 0.5) / 8.0);
    ctx.check((p[0] - target.0).abs() < 17.0 && (p[2] - target.1).abs() < 17.0, format!("arrival {p:?} not near x/8 z/8 {target:?}"));
    // stay inside: no bounce-back while on cooldown
    bot.wait_ticks(120);
    ctx.metric("bounced_while_standing", (bot.st().respawns - r0 - 1) as f64, 0.0);
    // step out across the portal and back in
    bot.walk_by(0.0, 1.6, Duration::from_secs(3)).ok();
    bot.wait_ticks(30);
    let t1 = Instant::now();
    bot.walk_by(0.0, -1.6, Duration::from_secs(3)).ok();
    let back = wait_dim(&bot, OVERWORLD, 12);
    let ticks_back = t1.elapsed().as_secs_f64() * 20.0;
    // remove the nether portal first (we know where it is), whatever happened
    let clean = format!("execute in minecraft:the_nether run fill {} {} {} {} {} {} air replace minecraft:nether_portal", bx - 4, by - 2, bz - 4, bx + 4, by + 5, bz + 4);
    let clean2 = format!("execute in minecraft:the_nether run fill {} {} {} {} {} {} air replace minecraft:obsidian", bx - 4, by - 2, bz - 4, bx + 4, by + 5, bz + 4);
    if ctx.check(back, format!("no way back after {ticks_back:.0} ticks (dim {})", bot.st().dim)) {
        bot.wait_on_ground(Duration::from_secs(5));
        let q = bot.st().body.pos;
        ctx.check((q[0] - (fo[0] as f64 + 1.5)).abs() < 1.5 && (q[2] - (fo[2] as f64 + 0.5)).abs() < 1.0 && (q[1] - (y + 1) as f64).abs() < 0.5,
            format!("came back at {q:?}, not in the original portal near {:?}", [fo[0] + 1, y + 1, fo[2]]));
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
    let (pp, inb) = { let s = bot.st(); (s.body.pos, s.world.name(fo[0] + 1, y + 1, fo[2]).to_string()) };
    ctx.metric("creative_travel_ticks_le_80", (went && ct <= 80.0) as u8 as f64, 0.0);
    ctx.check(went && ct < 80.0, format!("creative travel: {went} after {ct:.0} ticks (at {pp:?}, portal block {inb})"));
    if went {
        bot.wait_until(Duration::from_secs(10), |s| s.world.chunks.len() > 20);
        let p = bot.st().body.pos;
        let (bx, by, bz) = (p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32);
        bot.cmd_reply(&format!("execute in minecraft:the_nether run fill {} {} {} {} {} {} air replace minecraft:nether_portal", bx - 4, by - 2, bz - 4, bx + 4, by + 5, bz + 4))?;
        bot.cmd_reply(&format!("execute in minecraft:the_nether run fill {} {} {} {} {} {} air replace minecraft:obsidian", bx - 4, by - 2, bz - 4, bx + 4, by + 5, bz + 4))?;
        bot.cmd_reply("execute in minecraft:overworld run tp @s 0.5 220 0.5")?;
        wait_dim(&bot, OVERWORLD, 10);
    }
    // the overworld frame
    bot.cmd_reply(&format!("fill {} {} {} {} {} {} air", fo[0], y + 1, fo[2], fo[0] + 3, y + 4, fo[2]))?;
    Ok(())
}

/// A player in the nether and a block placed there survive a restart; the nether is saved in
/// vanilla's folder layout (world/dimensions/minecraft/the_nether/region).
pub fn nether_persistence(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    let name = bot.name.clone();
    ctx.gamemode(&bot, "creative")?;
    bot.fly(true);
    bot.cmd_reply("execute in minecraft:the_nether run tp @s 8.5 110 8.5")?;
    if !wait_dim(&bot, NETHER, 10) { bail!("not in the nether"); }
    bot.wait_chunks(1, Duration::from_secs(20))?;
    let b = [9, 112, 9];
    setblock(&bot, b, "minecraft:gold_block")?;
    bot.quit();
    std::thread::sleep(Duration::from_millis(300));
    ctx.restart()?;
    let bot = Bot::connect(&ctx.addr(), &name)?;
    ctx.check(wait_dim(&bot, NETHER, 10), format!("after restart in {}", bot.st().dim));
    bot.wait_chunks(1, Duration::from_secs(20))?;
    let p = bot.st().pos();
    ctx.check((p[0] - 8.5).abs() < 0.5 && (p[2] - 8.5).abs() < 0.5, format!("position after restart {p:?}"));
    ctx.check(bot.wait_until(Duration::from_secs(5), |s| s.world.name(b[0], b[1], b[2]) == "gold_block"), format!("block after restart: {}", bot.st().world.name(b[0], b[1], b[2])));
    let region = ctx.basalt.as_ref().map(|s| s.args.clone()).unwrap_or_default();
    let dir = region.iter().position(|a| a == "--world").and_then(|i| region.get(i + 1)).cloned().unwrap_or_default();
    let path = std::path::Path::new(&dir).join("dimensions/minecraft/the_nether/region/r.0.0.mca");
    ctx.check(path.exists(), format!("{} missing", path.display()));
    Ok(())
}

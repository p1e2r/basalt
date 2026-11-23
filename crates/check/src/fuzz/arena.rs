//! Per-worker arenas for the parallel fuzz harnesses.
use crate::bot::Bot;
use crate::runner::Ctx;
use anyhow::{bail, Result};
use std::time::Duration;

/// Build a clear arena for worker `w` (stone floor `sx` x `sz` at ARENA_Y, `h` blocks of air above,
/// shifted 16 blocks per worker so parallel bots don't share one) and leave the bot standing at its
/// centre in `gm`. Returns the floor origin.
pub fn worker_arena(ctx: &mut Ctx, bot: &Bot, w: usize, sx: i32, sz: i32, h: i32, gm: &str) -> Result<[i32; 3]> {
    let mut o = ctx.arena_origin();
    o[2] += w as i32 * 16;
    ctx.gamemode(bot, "creative")?;
    bot.fly(true);
    bot.tp([o[0] as f64 + sx as f64 / 2.0, (o[1] + 3) as f64, o[2] as f64 + sz as f64 / 2.0])?;
    bot.set_frozen(true);
    let ok = bot.wait_until(Duration::from_secs(30), |s| ((o[0] - 1) >> 4..=(o[0] + sx) >> 4).all(|cx| ((o[2] - 1) >> 4..=(o[2] + sz) >> 4).all(|cz| s.world.chunks.contains_key(&(cx, cz)))));
    if !ok { bail!("arena chunks did not load"); }
    bot.cmd_reply(&format!("fill {} {} {} {} {} {} air", o[0] - 1, o[1], o[2] - 1, o[0] + sx, o[1] + h, o[2] + sz))?;
    if ctx.is_vanilla() {
        bot.cmd_reply(&format!("kill @e[type=!minecraft:player,x={},y={},z={},dx={},dy={},dz={}]", o[0] - 3, o[1] - 8, o[2] - 3, sx + 6, h + 16, sz + 6))?;
    }
    bot.cmd_reply(&format!("fill {} {} {} {} {} {} stone", o[0], o[1], o[2], o[0] + sx - 1, o[1], o[2] + sz - 1))?;
    bot.set_frozen(false);
    bot.tp([o[0] as f64 + sx as f64 / 2.0, (o[1] + 1) as f64, o[2] as f64 + sz as f64 / 2.0])?;
    ctx.gamemode(bot, gm)?;
    bot.fly(false);
    if !bot.wait_on_ground(Duration::from_secs(5)) { bail!("bot not standing in its arena: {:?}", bot.st().body.pos); }
    Ok(o)
}

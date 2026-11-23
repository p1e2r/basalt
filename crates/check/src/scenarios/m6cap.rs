//! TEMP capture scenario (vanilla only): drives chest/furnace interactions so a proxy can record the wire.
use super::util::*;
use crate::runner::Ctx;
use anyhow::Result;

pub fn capture(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    let o = ctx.arena(&bot, 12, 12, 5)?;
    let y = o[1] + 1;
    ctx.stand(&bot, o[0] + 2, o[2] + 6, "survival")?;
    bot.cmd_reply("gamemode creative")?;
    give(&bot, "chest", 2)?;
    give(&bot, "iron_ore", 5)?;
    give(&bot, "coal", 3)?;
    give(&bot, "furnace", 1)?;
    bot.cmd_reply("gamemode survival")?;
    bot.wait_ticks(5);
    let c = [o[0] + 5, y, o[2] + 5];
    setblock(&bot, c, "chest[facing=south]")?;
    bot.cmd_reply(&format!("item replace block {} {} {} container.0 with minecraft:diamond 3", c[0], c[1], c[2]))?;
    bot.cmd_reply(&format!("item replace block {} {} {} container.5 with minecraft:iron_ore 64", c[0], c[1], c[2]))?;
    bot.look_at([c[0] as f64 + 0.5, c[1] as f64 + 0.5, c[2] as f64 + 0.5]); bot.wait_ticks(3);
    bot.place(c, 1); bot.wait_ticks(10);
    bot.click(0, 0, 0); bot.wait_ticks(2);
    bot.click(30, 0, 0); bot.wait_ticks(2);
    bot.close_container(); bot.wait_ticks(10);
    hold(&bot, "chest")?;
    let c2 = [c[0] + 1, y, c[2]];
    bot.tp([c2[0] as f64 + 0.5, y as f64 + 0.1, c2[2] as f64 + 2.5])?; bot.wait_ticks(5);
    bot.look_at([c2[0] as f64 + 0.5, c2[1] as f64 - 0.5, c2[2] as f64 + 0.5]); bot.wait_ticks(3);
    bot.place([c2[0], y - 1, c2[2]], 1); bot.wait_ticks(10);
    bot.look_at([c2[0] as f64 + 0.5, c2[1] as f64 + 0.5, c2[2] as f64 + 0.5]); bot.wait_ticks(3);
    bot.place(c2, 1); bot.wait_ticks(10);
    bot.click(0, 0, 0); bot.wait_ticks(2);
    bot.close_container(); bot.wait_ticks(10);
    let f = [o[0] + 8, y, o[2] + 5];
    bot.tp([f[0] as f64 + 0.5, y as f64 + 0.1, f[2] as f64 + 2.5])?; bot.wait_ticks(5);
    setblock(&bot, f, "furnace[facing=south]")?;
    bot.cmd_reply(&format!("item replace block {} {} {} container.0 with minecraft:iron_ore 5", f[0], f[1], f[2]))?;
    bot.cmd_reply(&format!("item replace block {} {} {} container.1 with minecraft:coal 3", f[0], f[1], f[2]))?;
    bot.look_at([f[0] as f64 + 0.5, f[1] as f64 + 0.5, f[2] as f64 + 0.5]); bot.wait_ticks(3);
    bot.place(f, 1); bot.wait_ticks(260);
    bot.click(2, 0, 1); bot.wait_ticks(3);
    bot.close_container(); bot.wait_ticks(10);
    bot.tp([o[0] as f64 + 100.0, y as f64 + 3.0, o[2] as f64 + 100.0])?;
    bot.wait_ticks(60);
    bot.tp([o[0] as f64 + 5.0, y as f64 + 3.0, o[2] as f64 + 5.0])?;
    bot.wait_ticks(40);
    Ok(())
}

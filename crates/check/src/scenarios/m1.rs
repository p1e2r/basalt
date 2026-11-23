//! M1/M2: protocol basics, chunks, chat, commands, movement, multiplayer, building, persistence.
use super::util::*;
use crate::bot::Bot;
use crate::runner::{Ctx, ARENA_Y};
use anyhow::{bail, Result};
use basalt_protocol::{ids, Conn, Writer, PROTOCOL_VERSION};
use std::time::{Duration, Instant};

pub fn join_spawn(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    let (eid, hp, food, pos, chunks, gm) = { let s = bot.st(); (s.eid, s.health, s.food, s.pos(), s.world.chunks.len(), s.gamemode) };
    ctx.check(eid >= 0, format!("entity id {eid}"));
    ctx.approx("health", hp as f64, 20.0, 0.0);
    ctx.approx("food", food as f64, 20.0, 0.0);
    ctx.check(chunks > 0, "no chunks at spawn");
    ctx.check(gm == 0, format!("default gamemode {gm}"));
    let st = ctx.state(&bot)?;
    ctx.approx("server health", st.health, 20.0, 0.0);
    let d: f64 = (0..3).map(|i| (st.pos[i] - pos[i]).powi(2)).sum::<f64>().sqrt();
    ctx.check(d < 1.0, format!("server pos {:?} vs bot {pos:?}", st.pos));
    // standing on something after spawning
    ctx.check(bot.wait_on_ground(Duration::from_secs(10)), format!("not on ground after spawn at {:?}", bot.st().pos()));
    ctx.note(format!("spawn {:.1} {:.1} {:.1}, {chunks} chunks", pos[0], pos[1], pos[2]));
    Ok(())
}

pub fn status_ping(ctx: &mut Ctx) -> Result<()> {
    let addr = ctx.addr();
    let (json, pong): (serde_json::Value, i64) = crate::bot::rt().block_on(async {
        let (host, port) = addr.rsplit_once(':').unwrap();
        let mut c = Conn::new(tokio::net::TcpStream::connect(&addr).await?);
        let mut w = Writer::with_id(0); w.varint(PROTOCOL_VERSION).str(host).u16(port.parse()?).varint(1); c.send(&w).await?;
        c.send(&Writer::with_id(ids::status::serverbound::STATUS_REQUEST)).await?;
        let f = c.read().await?;
        if f.id != ids::status::clientbound::STATUS_RESPONSE { bail!("unexpected status packet {}", f.id); }
        let j: serde_json::Value = serde_json::from_str(&f.reader().str()?)?;
        let mut w = Writer::with_id(ids::status::serverbound::PING_REQUEST); w.i64(0x1234_5678_9abc); c.send(&w).await?;
        let f = c.read().await?;
        anyhow::Ok((j, f.reader().i64()?))
    })?;
    ctx.check(json["version"]["protocol"] == PROTOCOL_VERSION, format!("protocol {}", json["version"]));
    ctx.check(json["version"]["name"].as_str().map(|n| n.contains("26.2")).unwrap_or(false), format!("version name {}", json["version"]["name"]));
    ctx.check(json["players"]["max"].as_i64().unwrap_or(0) > 0, "players.max");
    ctx.check(json.get("description").is_some(), "description");
    ctx.check(pong == 0x1234_5678_9abc, format!("pong {pong:x}"));
    Ok(())
}

pub fn chunk_streaming(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    bot.wait_ticks(60);
    let n0 = bot.st().world.chunks.len();
    // radius actually covered around the player
    let (cx, cz) = { let p = bot.st().pos(); ((p[0].floor() as i32) >> 4, (p[2].floor() as i32) >> 4) };
    let full = |b: &Bot, cx: i32, cz: i32, r: i32| (-r..=r).all(|dx| (-r..=r).all(|dz| b.st().world.chunks.contains_key(&(cx + dx, cz + dz))));
    ctx.check(full(&bot, cx, cz, 3), format!("chunks within 3 of {cx},{cz} missing ({n0} loaded)"));
    ctx.metric("chunks_initial", n0 as f64, 10.0);
    // fly 12 chunks east: new chunks stream in, old ones are forgotten
    ctx.gamemode(&bot, "creative")?; bot.fly(true);
    let p = bot.st().pos();
    bot.tp([p[0] + 192.0, p[1] + 20.0, p[2]])?;
    bot.set_frozen(true);
    let ok = bot.wait_until(Duration::from_secs(30), |s| { let (x, z) = ((s.pos()[0].floor() as i32) >> 4, (s.pos()[2].floor() as i32) >> 4); (-3..=3).all(|dx| (-3..=3).all(|dz| s.world.chunks.contains_key(&(x + dx, z + dz)))) });
    ctx.check(ok, "chunks around the new position did not arrive");
    bot.wait_ticks(40);
    let (n1, old) = { let s = bot.st(); (s.world.chunks.len(), s.world.chunks.contains_key(&(cx, cz))) };
    ctx.check(!old, "chunk at the old position was not forgotten");
    ctx.metric("chunks_after_move", n1 as f64, 10.0);
    ctx.note(format!("{n0} chunks initially, {n1} after moving"));
    Ok(())
}

pub fn chat_echo(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    let mark = bot.chat_mark();
    bot.chat("hello from basalt-check");
    let ok = bot.wait_until(Duration::from_secs(5), |s| s.chat[mark..].iter().any(|l| l.text.plain.contains("hello from basalt-check") && l.text.plain.contains(&s.name)));
    ctx.check(ok, format!("chat not echoed; got {:?}", bot.chat_since(mark).iter().map(|t| t.plain.clone()).collect::<Vec<_>>()));
    Ok(())
}

pub fn commands(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    let r = bot.cmd_reply("time set day")?;
    ctx.check(!r.is_empty(), "no reply to /time set day");
    give(&bot, "diamond", 3)?;
    ctx.approx("diamonds", bot.st().count("diamond") as f64, 3.0, 0.0);
    ctx.gamemode(&bot, "creative")?;
    ctx.check(bot.st().gamemode == 1, "gamemode creative");
    ctx.check(bot.wait_until(Duration::from_secs(2), |s| s.may_fly), "creative abilities (may fly) not sent");
    ctx.gamemode(&bot, "survival")?;
    bot.cmd_reply("effect give @s minecraft:speed 30 1")?;
    ctx.check(bot.wait_until(Duration::from_secs(2), |s| s.effects.get("speed").map(|e| e.0) == Some(1)), format!("speed effect missing: {:?}", bot.st().effects));
    let st = ctx.state(&bot)?;
    ctx.check(st.effects.get("speed").map(|e| e.0) == Some(1), format!("server effects {:?}", st.effects));
    ctx.check(st.count("diamond") == 3, format!("server inventory {:?}", st.inv));
    let p = bot.st().pos();
    bot.tp([p[0] + 3.0, p[1] + 1.0, p[2] - 2.0])?;
    let q = bot.st().pos();
    ctx.check((q[0] - p[0] - 3.0).abs() < 0.01 && (q[2] - p[2] + 2.0).abs() < 0.01, format!("tp landed at {q:?}"));
    Ok(())
}

pub fn movement(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    let o = ctx.arena(&bot, 20, 8, 6)?;
    // a one-block step to walk up and a ledge to fall from
    fill(&bot, [o[0] + 10, ARENA_Y + 1, o[2]], [o[0] + 13, ARENA_Y + 1, o[2] + 7], "stone")?;
    ctx.stand(&bot, o[0] + 2, o[2] + 4, "survival")?;
    let tps = bot.st().teleports.len();
    let start = bot.st().pos();
    bot.walk_to(o[0] as f64 + 16.5, o[2] as f64 + 4.5, Duration::from_secs(15))?;
    bot.jump()?;
    bot.wait_on_ground(Duration::from_secs(3));
    bot.walk_to(o[0] as f64 + 3.5, o[2] as f64 + 4.5, Duration::from_secs(15))?;
    bot.wait_ticks(10);
    let (end, tps2) = { let s = bot.st(); (s.pos(), s.teleports.len()) };
    ctx.check(tps2 == tps, format!("{} position corrections while walking", tps2 - tps));
    ctx.metric("corrections", (tps2 - tps) as f64, 0.0);
    ctx.approx("returned x", end[0], o[0] as f64 + 3.5, 0.6);
    ctx.approx("ground level", end[1], (ARENA_Y + 1) as f64, 1e-6);
    ctx.check((start[0] - end[0]).abs() < 2.0, "walk path");
    let st = ctx.state(&bot)?;
    let d: f64 = (0..3).map(|i| (st.pos[i] - end[i]).powi(2)).sum::<f64>().sqrt();
    ctx.check(d < 0.05, format!("server position {:?} != bot {end:?}", st.pos));
    ctx.metric("server_pos_error", (d * 10.0).round() / 10.0, 0.1);
    Ok(())
}

pub fn multiplayer(ctx: &mut Ctx) -> Result<()> {
    let a = ctx.bot()?;
    let o = ctx.arena(&a, 12, 12, 5)?;
    ctx.stand(&a, o[0] + 3, o[2] + 3, "survival")?;
    let b = ctx.bot()?;
    let b_uuid = crate::bot::offline_uuid(&b.name);
    b.tp([o[0] as f64 + 6.5, (ARENA_Y + 1) as f64, o[2] as f64 + 6.5])?;
    let seen = a.wait_until(Duration::from_secs(5), |s| s.entities.values().any(|e| e.uuid == b_uuid && e.kind == "player"));
    ctx.check(seen, "A never got add_entity for B");
    ctx.check(b.wait_until(Duration::from_secs(5), |s| s.entities.values().any(|e| e.kind == "player" && e.uuid == crate::bot::offline_uuid(&a.name))), "B never got add_entity for A");
    ctx.check(a.st().players.values().any(|n| *n == b.name), "B missing from A's tab list");
    // B walks; A sees it move
    b.walk_by(3.0, 0.0, Duration::from_secs(5))?;
    let bp = b.st().pos();
    let tracked = a.wait_until(Duration::from_secs(3), |s| s.entities.values().any(|e| e.uuid == b_uuid && (e.pos[0] - bp[0]).abs() < 0.3 && (e.pos[2] - bp[2]).abs() < 0.3));
    ctx.check(tracked, format!("A's view of B {:?} != B at {bp:?}", a.st().entities.values().find(|e| e.uuid == b_uuid).map(|e| e.pos)));
    let mark = a.chat_mark();
    b.chat("hi from B");
    ctx.check(a.wait_until(Duration::from_secs(3), |s| s.chat[mark..].iter().any(|l| l.text.plain.contains("hi from B"))), "A didn't get B's chat");
    // B leaves: A gets remove_entities
    let eid = a.st().entities.iter().find(|(_, e)| e.uuid == b_uuid).map(|(k, _)| *k);
    b.quit();
    if let Some(eid) = eid { ctx.check(a.wait_until(Duration::from_secs(5), |s| !s.entities.contains_key(&eid)), "B's entity not removed after leaving"); }
    Ok(())
}

pub fn creative_break_place(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    let o = ctx.arena(&bot, 10, 10, 5)?;
    ctx.stand(&bot, o[0] + 4, o[2] + 4, "creative")?;
    give(&bot, "stone", 4)?;
    hold(&bot, "stone")?;
    let t = [o[0] + 6, ARENA_Y + 1, o[2] + 4];
    bot.look_at([t[0] as f64 + 0.5, t[1] as f64, t[2] as f64 + 0.5]);
    bot.wait_ticks(2);
    bot.place([t[0], t[1] - 1, t[2]], 1);
    ctx.check(bot.wait_until(Duration::from_secs(3), |s| s.world.name(t[0], t[1], t[2]) == "stone"), "placed stone not seen by bot");
    ctx.check(ctx.block_is(&bot, t, "stone")?, "server has no stone after place");
    ctx.check(bot.st().count("stone") == 4, "creative placing consumed an item");
    bot.dig(t)?;
    ctx.check(bot.wait_until(Duration::from_secs(3), |s| s.world.name(t[0], t[1], t[2]) == "air"), "creative break not seen");
    ctx.check(ctx.block_is(&bot, t, "air")?, "server still has the block");
    bot.wait_ticks(10);
    let near = { let s = bot.st(); s.items_near(s.pos(), 6.0).len() };
    ctx.check(near == 0, "creative break dropped an item");
    Ok(())
}

pub fn orientation(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    let o = ctx.arena(&bot, 10, 10, 5)?;
    ctx.stand(&bot, o[0] + 2, o[2] + 2, "creative")?;
    for i in ["oak_log", "oak_stairs", "stone_slab"] { give(&bot, i, 4)?; }
    let y = ARENA_Y + 1;
    let base = [o[0] + 4, y, o[2] + 4];
    hold(&bot, "oak_log")?;
    let place = |against: [i32; 3], face: u8, cursor: Option<[f32; 3]>, yaw: f32| -> Result<()> {
        bot.look(yaw, 30.0); bot.wait_ticks(2);
        match cursor { Some(c) => bot.place_at(against, face, c), None => bot.place(against, face) }
        bot.wait_ticks(4); Ok(())
    };
    place([base[0], base[1] - 1, base[2]], 1, None, 0.0)?; // on top of floor -> axis y
    place(base, 5, None, 0.0)?; // east face -> axis x at base+x
    place(base, 3, None, 0.0)?; // south face -> axis z at base+z
    let checks = [([base[0], y, base[2]], "oak_log[axis=y]"), ([base[0] + 1, y, base[2]], "oak_log[axis=x]"), ([base[0], y, base[2] + 1], "oak_log[axis=z]")];
    let mut ok_all = Vec::new();
    for (p, want) in checks { let ok = ctx.block_is(&bot, p, want)?; ctx.check(ok, format!("{want} at {p:?}: bot sees {}", bot.st().world.state_string(p[0], p[1], p[2]))); ok_all.push(ok); }
    // stairs: facing follows the player's yaw (yaw 0 = looking south, -90 = east, 180 = north, 90 = west)
    hold(&bot, "oak_stairs")?;
    let s1 = [o[0] + 7, y, o[2] + 2];
    place([s1[0], s1[1] - 1, s1[2]], 1, None, 0.0)?;
    let s2 = [o[0] + 7, y, o[2] + 4];
    place([s2[0], s2[1] - 1, s2[2]], 1, None, -90.0)?;
    let f1 = ctx.block_is(&bot, s1, "oak_stairs[facing=south,half=bottom]")?;
    let f2 = ctx.block_is(&bot, s2, "oak_stairs[facing=east,half=bottom]")?;
    ctx.check(f1, format!("stairs yaw 0: {}", bot.st().world.state_string(s1[0], s1[1], s1[2])));
    ctx.check(f2, format!("stairs yaw -90: {}", bot.st().world.state_string(s2[0], s2[1], s2[2])));
    // slab: clicking the upper half of a side face gives a top slab
    hold(&bot, "stone_slab")?;
    place(base, 2, Some([0.5, 0.75, 0.0]), 0.0)?;
    let sl = [base[0], y, base[2] - 1];
    let f3 = ctx.block_is(&bot, sl, "stone_slab[type=top]")?;
    ctx.check(f3, format!("slab upper click: {}", bot.st().world.state_string(sl[0], sl[1], sl[2])));
    for (i, ok) in ok_all.iter().chain([f1, f2, f3].iter()).enumerate() { ctx.metric(&format!("orientation_{i}"), *ok as i32 as f64, 0.0); }
    Ok(())
}

pub fn persistence(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    let name = bot.name.clone();
    let o = ctx.arena(&bot, 8, 8, 4)?;
    ctx.stand(&bot, o[0] + 2, o[2] + 2, "survival")?;
    let b = [o[0] + 5, ARENA_Y + 1, o[2] + 5];
    setblock(&bot, b, "oak_log[axis=x]")?;
    give(&bot, "diamond", 5)?;
    give(&bot, "wooden_pickaxe", 1)?;
    hold(&bot, "wooden_pickaxe")?;
    let rock = [o[0] + 3, ARENA_Y + 1, o[2] + 2];
    setblock(&bot, rock, "stone")?;
    bot.dig(rock)?;
    bot.wait_until(Duration::from_secs(2), |s| s.inv[36..45].iter().any(|i| i.damage > 0));
    let pos = bot.st().pos();
    bot.quit();
    std::thread::sleep(Duration::from_millis(300));
    let t = Instant::now();
    ctx.restart()?;
    ctx.note(format!("restart {:.1}s", t.elapsed().as_secs_f64()));
    let bot = Bot::connect(&ctx.addr(), &name)?;
    bot.wait_chunks(1, Duration::from_secs(10))?;
    let p = bot.st().pos();
    ctx.check((0..3).all(|i| (p[i] - pos[i]).abs() < 0.1), format!("position after restart {p:?}, was {pos:?}"));
    let dia = bot.st().count("diamond");
    ctx.check(dia == 5, format!("diamonds after restart: {dia}"));
    let dmg = bot.st().inv.iter().find(|s| s.name() == "wooden_pickaxe").map(|s| s.damage);
    ctx.check(dmg == Some(1), format!("pickaxe damage after restart {dmg:?}"));
    ctx.check(ctx.block_is(&bot, b, "oak_log[axis=x]")?, format!("placed log after restart: {}", crate::query::basalt_block(&bot, b)?));
    ctx.check(bot.st().world.name(b[0], b[1], b[2]) == "oak_log", "bot doesn't see the log after restart");
    ctx.check(ctx.block_is(&bot, rock, "air")?, "broken block came back");
    Ok(())
}

/// Two-block blocks, connection shapes, support checks and trapdoor/shift rules, measured against vanilla (differential).
pub fn place_shapes(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    let o = ctx.arena(&bot, 14, 10, 6)?;
    ctx.stand(&bot, o[0] + 4, o[2] + 3, "creative")?;
    for i in ["stone", "oak_door", "red_bed", "lilac"] { give(&bot, i, 4)?; }
    let y = ARENA_Y + 1;
    let floor = |x: i32, z: i32| [o[0] + x, y - 1, o[2] + z];
    let at = |x: i32, z: i32| [o[0] + x, y, o[2] + z];
    let place = |item: &str, against: [i32; 3], face: u8, yaw: f32| -> Result<()> {
        hold(&bot, item)?;
        bot.look(yaw, 20.0); bot.wait_ticks(2);
        bot.place_at(against, face, [0.5, 1.0, 0.5]);
        bot.wait_ticks(4); Ok(())
    };
    let mut results: Vec<(String, bool)> = Vec::new();
    let mut want = |ctx: &mut Ctx, bot: &Bot, name: &str, p: [i32; 3], state: &str| -> Result<()> {
        let ok = ctx.block_is(bot, p, state)?;
        ctx.check(ok, format!("{name}: want {state}, server has {}", crate::query::basalt_block(bot, p).unwrap_or_default()));
        results.push((name.into(), ok)); Ok(())
    };
    // door: both halves, hinge default left, facing = look direction
    place("oak_door", floor(2, 1), 1, 0.0)?;
    want(ctx, &bot, "door lower", at(2, 1), "oak_door[facing=south,half=lower,hinge=left,open=false,powered=false]")?;
    want(ctx, &bot, "door upper", [at(2, 1)[0], y + 1, at(2, 1)[2]], "oak_door[facing=south,half=upper,hinge=left,open=false,powered=false]")?;
    // a plain click toggles the door instead of placing the held block
    hold(&bot, "stone")?; bot.place_at(at(2, 1), 3, [0.5, 0.5, 1.0]); bot.wait_ticks(4);
    want(ctx, &bot, "door opened", at(2, 1), "oak_door[facing=south,half=lower,hinge=left,open=true,powered=false]")?;
    // bed: head one block ahead
    place("red_bed", floor(5, 1), 1, 0.0)?;
    want(ctx, &bot, "bed foot", at(5, 1), "red_bed[facing=south,occupied=false,part=foot]")?;
    want(ctx, &bot, "bed head", at(5, 2), "red_bed[facing=south,occupied=false,part=head]")?;
    // tall flower
    setblock(&bot, floor(7, 1), "grass_block")?;
    place("lilac", floor(7, 1), 1, 0.0)?;
    want(ctx, &bot, "lilac lower", at(7, 1), "lilac[half=lower]")?;
    want(ctx, &bot, "lilac upper", [at(7, 1)[0], y + 1, at(7, 1)[2]], "lilac[half=upper]")?;
    bot.cmd("clear"); bot.wait_ticks(4);
    for i in ["oak_fence", "cobblestone_wall", "glass_pane", "torch", "oak_trapdoor"] { give(&bot, i, 4)?; }
    // fence / wall / pane connect to neighbours and to each other
    place("oak_fence", floor(3, 5), 1, 0.0)?;
    place("oak_fence", floor(4, 5), 1, 0.0)?;
    want(ctx, &bot, "fence west", at(3, 5), "oak_fence[east=true,north=false,south=false,waterlogged=false,west=false]")?;
    want(ctx, &bot, "fence east", at(4, 5), "oak_fence[east=false,north=false,south=false,waterlogged=false,west=true]")?;
    place("cobblestone_wall", floor(6, 5), 1, 0.0)?;
    place("cobblestone_wall", floor(7, 5), 1, 0.0)?;
    want(ctx, &bot, "wall straight", at(6, 5), "cobblestone_wall[east=low,north=none,south=none,up=true,waterlogged=false,west=none]")?;
    place("glass_pane", floor(6, 6), 1, 0.0)?;
    want(ctx, &bot, "pane next to wall", at(6, 6), "glass_pane[east=false,north=true,south=false,waterlogged=false,west=false]")?;
    // trapdoor faces the player when clicked on top of a block
    place("oak_trapdoor", floor(8, 3), 1, 0.0)?;
    want(ctx, &bot, "trapdoor faces the player", at(8, 3), "oak_trapdoor[facing=north,half=bottom,open=false,powered=false,waterlogged=false]")?;
    // torch on top of a fence post, and clicked on the side of a fence: whether each is accepted is compared with vanilla (no fixed expectation)
    place("torch", at(3, 5), 1, 0.0)?;
    let on_post = ctx.block_is(&bot, [at(3, 5)[0], y + 1, at(3, 5)[2]], "torch")?;
    ctx.metric("torch_on_fence_post", on_post as i32 as f64, 0.0);
    place("torch", at(4, 5), 5, 0.0)?;
    let side = ctx.block_is(&bot, at(5, 5), "torch")?;
    ctx.metric("torch_beside_fence", side as i32 as f64, 0.0);
    for (i, (_, ok)) in results.iter().enumerate() { ctx.metric(&format!("place_shapes_{i}"), *ok as i32 as f64, 0.0); }
    Ok(())
}

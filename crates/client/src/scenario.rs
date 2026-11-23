//! Headless scenario tests: `basalt-client --scenario <name|all> --addr HOST:PORT [--vanilla HOST:PORT]`.
//! Each scenario drives a real (offscreen) client against the server through the same input entry
//! points the window uses, asserts on client state and on rendered pixels, and prints
//! `SCENARIO m7/<name> PASS|FAIL|SKIP <details>`.
use crate::container::Kind;
use crate::menu::{Config, Screen};
use crate::net::ItemStack;
use crate::{Args, Assets, Game, GameOpts};
use anyhow::{anyhow, bail, Result};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::keyboard::KeyCode;

const W: u32 = 854;
const H: u32 = 480;

pub const ALL: &[&str] = &["edge_chunks", "inventory", "crafting_table", "death", "dig", "dig_others", "models", "interp", "sounds", "menus"];

enum Outcome { Pass(String), Skip(String) }

struct Ctx<'a> { args: &'a Args, assets: Arc<Assets>, rules: Option<Arc<basalt_game::GameData>>, run: u32 }

impl Ctx<'_> {
    fn game_at(&self, short: &str, addr: Option<&str>) -> Result<Game> {
        let name = format!("M7{short}{}", self.run % 1000);
        let cfg = Config { name: name.clone(), render_distance: 6, ..Default::default() };
        let sounds = crate::load_sounds(self.args, false);
        Game::new(None, (W, H), self.assets.clone(), self.rules.clone(), GameOpts { name, addr: addr.map(String::from), auto_respawn: false, sounds, cfg })
    }
    fn game(&self, short: &str) -> Result<Game> { self.game_at(short, Some(&self.args.addr)) }
}

pub fn run(args: &Args, assets: Arc<Assets>, name: &str) -> Result<bool> {
    let dir = std::env::temp_dir().join(format!("basalt-scenario-{}", std::process::id()));
    std::env::set_var("BASALT_CONFIG_DIR", &dir);
    let ctx = Ctx { args, assets, rules: crate::load_rules(args), run: std::process::id() };
    let list: Vec<&str> = if name == "all" { ALL.to_vec() } else { name.split(',').collect() };
    let mut ok = true;
    for s in list {
        let t = Instant::now();
        let r = match s {
            "edge_chunks" => edge_chunks(&ctx), "inventory" => inventory(&ctx), "crafting_table" => crafting_table(&ctx), "death" => death(&ctx),
            "dig" => dig(&ctx), "dig_others" => dig_others(&ctx), "models" => models(&ctx), "interp" => interp(&ctx), "sounds" => sounds(&ctx), "menus" => menus(&ctx),
            other => Err(anyhow!("unknown scenario {other} (known: {})", ALL.join(", "))),
        };
        let secs = t.elapsed().as_secs_f32();
        match r {
            Ok(Outcome::Pass(d)) => println!("SCENARIO m7/{s} PASS {d} ({secs:.1}s)"),
            Ok(Outcome::Skip(d)) => println!("SCENARIO m7/{s} SKIP {d}"),
            Err(e) => { ok = false; println!("SCENARIO m7/{s} FAIL {e:#} ({secs:.1}s)"); }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(ok)
}

// ---------------------------------------------------------------- driving helpers

fn step(g: &mut Game) -> Result<()> {
    g.update();
    if let Some(d) = &g.disconnected { if g.connected || !matches!(g.screen, Some(Screen::Connecting { .. })) { bail!("disconnected: {d}"); } }
    std::thread::sleep(Duration::from_millis(4));
    Ok(())
}
fn wait(g: &mut Game, secs: f32) -> Result<()> { let t = Instant::now(); while t.elapsed().as_secs_f32() < secs { step(g)?; } Ok(()) }
fn wait_until(g: &mut Game, secs: f32, mut f: impl FnMut(&Game) -> bool) -> Result<bool> {
    let t = Instant::now();
    while t.elapsed().as_secs_f32() < secs { if f(g) { return Ok(true); } step(g)?; }
    Ok(f(g))
}
fn settled(g: &Game) -> bool { g.spawned && g.loaded_sent && g.dirty.is_empty() && g.in_flight.is_empty() && !g.world.chunks.is_empty() }
fn join(g: &mut Game) -> Result<()> {
    if !wait_until(g, 20.0, |g| g.spawned && g.loaded_sent)? { bail!("did not spawn"); }
    wait_until(g, 15.0, settled)?;
    Ok(())
}
fn cmd(g: &mut Game, c: &str) { g.send_chat(&format!("/{c}")); }
/// Teleport and wait until the client is there with chunks meshed.
fn tp(g: &mut Game, p: [f64; 3], yaw: f32, pitch: f32) -> Result<()> {
    cmd(g, &format!("tp @s {} {} {}", p[0], p[1], p[2]));
    if !wait_until(g, 8.0, |g| (g.player.pos[0] - p[0]).abs() < 0.01 && (g.player.pos[2] - p[2]).abs() < 0.01)? { bail!("teleport to {p:?} not applied"); }
    g.player.yaw = yaw; g.player.pitch = pitch;
    wait_until(g, 10.0, |g| g.world.chunks.contains_key(&((p[0].floor() as i32) >> 4, (p[2].floor() as i32) >> 4)) && g.dirty.is_empty() && g.in_flight.is_empty())?;
    wait(g, 0.3)
}
fn look_at(g: &mut Game, t: [f64; 3]) {
    let e = g.eye(1.0);
    let (dx, dy, dz) = (t[0] - e[0], t[1] - e[1], t[2] - e[2]);
    g.player.yaw = (-dx).atan2(dz).to_degrees() as f32;
    g.player.pitch = (-(dy.atan2((dx * dx + dz * dz).sqrt())).to_degrees()) as f32;
}
fn tap(g: &mut Game, k: KeyCode, text: Option<&str>) -> Result<()> { g.key(k, true, text); step(g)?; g.key(k, false, None); step(g) }
fn click_at(g: &mut Game, x: f32, y: f32, button: u8) -> Result<()> {
    g.mouse_move(x, y); step(g)?;
    g.mouse_button(button, true); step(g)?;
    g.mouse_button(button, false); step(g)
}
fn type_text(g: &mut Game, s: &str) -> Result<()> { for c in s.chars() { let t = c.to_string(); g.key(KeyCode::KeyA, true, Some(&t)); g.key(KeyCode::KeyA, false, None); } step(g) }
fn widget_center(g: &Game, id: &str) -> Result<(f32, f32)> {
    let s = g.gui_scale();
    let w = g.widgets().into_iter().find(|w| w.id == id).ok_or_else(|| anyhow!("no widget {id} on this screen"))?;
    Ok(((w.x + w.w / 2.0) * s, (w.y + w.h / 2.0) * s))
}
fn press_widget(g: &mut Game, id: &str) -> Result<()> { let (x, y) = widget_center(g, id)?; click_at(g, x, y, 0) }
fn slot_center(g: &Game, kind: Kind, i: usize) -> (f32, f32) {
    let s = g.gui_scale(); let (ox, oy) = g.container_origin(kind); let (x, y) = kind.slot_pos(i).unwrap();
    (ox + (x + 8.0) * s, oy + (y + 8.0) * s)
}
fn container(g: &Game) -> Option<&crate::container::Open> { if let Some(Screen::Container(c)) = &g.screen { Some(c) } else { None } }
fn item_id(n: &str) -> i32 { basalt_data::registry_id(basalt_data::registries::ITEM, n).unwrap_or(-1) }
fn count_item(slots: &[ItemStack], item: i32) -> i32 { slots.iter().filter(|s| s.item == item && s.count > 0).map(|s| s.count).sum() }
fn find_slot(slots: &[ItemStack], item: i32) -> Option<usize> { slots.iter().position(|s| s.item == item && s.count > 0) }

fn capture(g: &mut Game) -> Result<Vec<u8>> { g.capture(1.0) }
fn px(img: &[u8], x: i32, y: i32) -> [u8; 3] { let x = x.clamp(0, W as i32 - 1) as usize; let y = y.clamp(0, H as i32 - 1) as usize; let i = (y * W as usize + x) * 3; [img[i], img[i + 1], img[i + 2]] }
fn diff_count(a: &[u8], b: &[u8], x0: i32, y0: i32, x1: i32, y1: i32) -> usize {
    let mut n = 0;
    for y in y0.max(0)..y1.min(H as i32) { for x in x0.max(0)..x1.min(W as i32) {
        let (p, q) = (px(a, x, y), px(b, x, y));
        if (0..3).any(|i| (p[i] as i32 - q[i] as i32).abs() > 12) { n += 1; }
    } }
    n
}
fn save(g: &Game, img: &[u8], name: &str) {
    let dir = std::env::var("BASALT_SCENARIO_SHOTS").unwrap_or_else(|_| "target/scenario-shots".into());
    let _ = std::fs::create_dir_all(&dir);
    let _ = crate::render::save_png(&format!("{dir}/{name}.png"), g.gpu.size.0, g.gpu.size.1, img);
}

// ---------------------------------------------------------------- scenarios

/// Sections at the edge of the loaded area (a horizontal neighbour missing) get meshed too.
fn edge_chunks(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("edge")?;
    join(&mut g)?;
    wait_until(&mut g, 10.0, settled)?;
    wait(&mut g, 1.0)?;
    wait_until(&mut g, 10.0, settled)?;
    let cols: std::collections::HashSet<(i32, i32)> = g.gpu.sections.keys().map(|k| (k.0, k.2)).collect();
    let loaded: Vec<(i32, i32)> = g.world.chunks.keys().copied().filter(|k| g.world.chunks[k].sections.iter().any(|s| !s.is_empty())).collect();
    let ring: Vec<&(i32, i32)> = loaded.iter().filter(|(x, z)| [(-1, 0), (1, 0), (0, -1), (0, 1)].iter().any(|(dx, dz)| !g.world.chunks.contains_key(&(x + dx, z + dz)))).collect();
    let ring_meshed = ring.iter().filter(|k| cols.contains(k)).count();
    let all_meshed = loaded.iter().filter(|k| cols.contains(k)).count();
    if ring.is_empty() { bail!("no edge columns ({} loaded)", loaded.len()); }
    if ring_meshed != ring.len() || all_meshed != loaded.len() { bail!("edge columns meshed {ring_meshed}/{}, all {all_meshed}/{}", ring.len(), loaded.len()); }
    Ok(Outcome::Pass(format!("edge columns meshed {ring_meshed}/{}, all {all_meshed}/{}", ring.len(), loaded.len())))
}

/// Inventory screen: GUI drawn from the jar, craft planks in the 2×2 grid with real clicks,
/// right-click split, drag, number-key swap, Q throw, double-click collect, close.
fn inventory(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("inv")?;
    join(&mut g)?;
    cmd(&mut g, "gamemode survival");
    tp(&mut g, [200.5, -60.0, 0.5], 0.0, 0.0)?;
    let (log, planks) = (item_id("oak_log"), item_id("oak_planks"));
    cmd(&mut g, "give @s oak_log 2");
    if !wait_until(&mut g, 5.0, |g| count_item(&g.inventory, log) == 2)? { bail!("give oak_log not received"); }
    tap(&mut g, KeyCode::KeyE, None)?;
    if container(&g).map(|c| c.kind) != Some(Kind::Inventory) { bail!("E did not open the inventory"); }
    let img = capture(&mut g)?;
    save(&g, &img, "inventory");
    // the container background (vanilla GUI grey 198,198,198) covers the panel
    let (ox, oy) = g.container_origin(Kind::Inventory); let s = g.gui_scale();
    let mut grey = 0; let mut total = 0;
    for y in (oy as i32..(oy + 166.0 * s) as i32).step_by(2) { for x in (ox as i32..(ox + 176.0 * s) as i32).step_by(2) { total += 1; if px(&img, x, y) == [198, 198, 198] { grey += 1; } } }
    if grey * 4 < total { bail!("inventory background not drawn ({grey}/{total} grey px)"); }
    // log slot → craft grid (right click puts one), deposit the rest back
    let ls = find_slot(&g.inventory, log).ok_or_else(|| anyhow!("log slot"))?;
    let p = slot_center(&g, Kind::Inventory, ls); click_at(&mut g, p.0, p.1, 0)?;
    if g.carried.item != log { bail!("pick up did not put the log on the cursor"); }
    let p = slot_center(&g, Kind::Inventory, 1); click_at(&mut g, p.0, p.1, 1)?;
    let p = slot_center(&g, Kind::Inventory, ls); click_at(&mut g, p.0, p.1, 0)?;
    if !wait_until(&mut g, 3.0, |g| container(g).map(|c| c.slots[0].item == planks && c.slots[0].count == 4).unwrap_or(false))? { bail!("server did not show oak_planks x4 in the result slot"); }
    // shift-click the result
    g.key(KeyCode::ShiftLeft, true, None);
    let p = slot_center(&g, Kind::Inventory, 0); click_at(&mut g, p.0, p.1, 0)?;
    g.key(KeyCode::ShiftLeft, false, None);
    if !wait_until(&mut g, 3.0, |g| count_item(&g.inventory[9..45], planks) == 4 && count_item(&g.inventory[9..45], log) == 1)? { bail!("crafting did not give 4 planks (inv planks {}, logs {})", count_item(&g.inventory[9..45], planks), count_item(&g.inventory[9..45], log)); }
    // right click on the planks stack: split in half
    let ps = 9 + find_slot(&g.inventory[9..45], planks).unwrap();
    let p = slot_center(&g, Kind::Inventory, ps); click_at(&mut g, p.0, p.1, 1)?;
    if g.carried.count != 2 { bail!("right-click split carried {} (want 2)", g.carried.count); }
    let p = slot_center(&g, Kind::Inventory, ps); click_at(&mut g, p.0, p.1, 0)?;
    wait(&mut g, 0.4)?;
    // drag 4 planks over three empty main slots: 1 each, 1 left on the cursor
    let p = slot_center(&g, Kind::Inventory, ps); click_at(&mut g, p.0, p.1, 0)?;
    let targets = [10usize, 11, 12];
    if targets.iter().any(|&t| g.inventory[t].count > 0) { bail!("drag target slots not empty"); }
    let p0 = slot_center(&g, Kind::Inventory, targets[0]);
    g.mouse_move(p0.0, p0.1); step(&mut g)?; g.mouse_button(0, true); step(&mut g)?;
    for &t in &targets[1..] { let p = slot_center(&g, Kind::Inventory, t); g.mouse_move(p.0, p.1); step(&mut g)?; }
    g.mouse_button(0, false); step(&mut g)?;
    wait(&mut g, 0.5)?;
    let dragged: Vec<i32> = targets.iter().map(|&t| g.inventory[t].count).collect();
    if dragged != vec![1, 1, 1] || g.carried.count != 1 { bail!("drag result {dragged:?}, carried {}", g.carried.count); }
    let p = slot_center(&g, Kind::Inventory, 10); click_at(&mut g, p.0, p.1, 0)?; // drop the last one onto slot 10 → 2
    // number key 3: swap slot 11 with hotbar slot 3 (window slot 38)
    let p = slot_center(&g, Kind::Inventory, 11); g.mouse_move(p.0, p.1); step(&mut g)?;
    tap(&mut g, KeyCode::Digit3, None)?;
    if !wait_until(&mut g, 2.0, |g| g.inventory[38].item == planks && g.inventory[11].count == 0)? { bail!("number-key swap failed"); }
    // Q over slot 12 throws it
    let p = slot_center(&g, Kind::Inventory, 12); g.mouse_move(p.0, p.1); step(&mut g)?;
    tap(&mut g, KeyCode::KeyQ, None)?;
    if !wait_until(&mut g, 2.0, |g| g.inventory[12].count == 0)? { bail!("Q did not throw"); }
    wait(&mut g, 0.4)?;
    // double click slot 10: pick up (2) then collect the rest (hotbar 38: 1) → 3 on the cursor
    let p = slot_center(&g, Kind::Inventory, 10);
    click_at(&mut g, p.0, p.1, 0)?; click_at(&mut g, p.0, p.1, 0)?;
    wait(&mut g, 0.4)?;
    if g.carried.count != 3 { bail!("double-click collect carried {} (want 3)", g.carried.count); }
    let p = slot_center(&g, Kind::Inventory, 10); click_at(&mut g, p.0, p.1, 0)?;
    wait(&mut g, 0.4)?;
    tap(&mut g, KeyCode::KeyE, None)?;
    if g.screen.is_some() { bail!("E did not close the inventory"); }
    if !wait_until(&mut g, 2.0, |g| g.inventory[10].item == planks && g.inventory[10].count == 3)? { bail!("final state: slot 10 {:?}", g.inventory[10]); }
    let thrown = g.entities.values().any(|e| crate::entity::type_name(e.kind) == "item");
    Ok(Outcome::Pass(format!("gui {grey}/{total} px; crafted 4 planks; split/drag/swap/throw/collect ok; thrown item entity seen {thrown}; clicks {}", g.clicks_sent)))
}

/// Crafting table opened by the server's open_screen: sticks from two planks.
fn crafting_table(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("ct")?;
    join(&mut g)?;
    cmd(&mut g, "gamemode survival");
    tp(&mut g, [300.5, -60.0, 0.5], 0.0, 0.0)?;
    cmd(&mut g, "setblock 300 -60 2 crafting_table");
    cmd(&mut g, "give @s oak_planks 2");
    let (planks, stick) = (item_id("oak_planks"), item_id("stick"));
    if !wait_until(&mut g, 5.0, |g| count_item(&g.inventory, planks) == 2 && basalt_data::BlockState(g.world.get(300, -60, 2) as u32).name() == "crafting_table")? { bail!("setup failed"); }
    look_at(&mut g, [300.5, -59.5, 2.0]);
    step(&mut g)?;
    g.mouse_button(1, true); step(&mut g)?; g.mouse_button(1, false);
    if !wait_until(&mut g, 3.0, |g| container(g).map(|c| c.kind == Kind::Crafting).unwrap_or(false))? { bail!("crafting screen did not open"); }
    wait(&mut g, 0.3)?;
    let img = capture(&mut g)?; save(&g, &img, "crafting_table");
    let cs = container(&g).unwrap().slots.clone();
    let ps = find_slot(&cs, planks).ok_or_else(|| anyhow!("planks not in the crafting window"))?;
    let p = slot_center(&g, Kind::Crafting, ps); click_at(&mut g, p.0, p.1, 0)?;
    let p = slot_center(&g, Kind::Crafting, 2); click_at(&mut g, p.0, p.1, 1)?;
    let p = slot_center(&g, Kind::Crafting, 5); click_at(&mut g, p.0, p.1, 1)?;
    if !wait_until(&mut g, 3.0, |g| container(g).map(|c| c.slots[0].item == stick && c.slots[0].count == 4).unwrap_or(false))? { bail!("no sticks in the result slot"); }
    g.key(KeyCode::ShiftLeft, true, None);
    let p = slot_center(&g, Kind::Crafting, 0); click_at(&mut g, p.0, p.1, 0)?;
    g.key(KeyCode::ShiftLeft, false, None);
    wait(&mut g, 0.3)?;
    tap(&mut g, KeyCode::Escape, None)?;
    if g.screen.is_some() { bail!("Esc did not close"); }
    if !wait_until(&mut g, 3.0, |g| count_item(&g.inventory, stick) == 4 && count_item(&g.inventory, planks) == 0)? { bail!("inventory after craft: sticks {}, planks {}", count_item(&g.inventory, stick), count_item(&g.inventory, planks)); }
    Ok(Outcome::Pass("crafting_table screen from open_screen, 4 sticks crafted via GUI clicks".into()))
}

/// Death screen on player_combat_kill, Respawn button → client_command, health back to 20.
fn death(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("dth")?;
    join(&mut g)?;
    cmd(&mut g, "gamemode survival");
    wait(&mut g, 0.3)?;
    cmd(&mut g, "kill @s");
    if !wait_until(&mut g, 5.0, |g| matches!(g.screen, Some(Screen::Death { .. })))? { bail!("no death screen"); }
    let msg = if let Some(Screen::Death { msg, .. }) = &g.screen { msg.clone() } else { String::new() };
    wait(&mut g, 1.1)?;
    let img = capture(&mut g)?; save(&g, &img, "death");
    let scr = g.screen.take(); let plain = capture(&mut g)?; g.screen = scr;
    // red overlay: the frame gets redder (r - g) compared with the same frame without the screen
    let redness = |im: &[u8]| { let mut s = 0i64; let mut n = 0i64; for y in (40..200).step_by(4) { for x in (20..(W as i32 - 20)).step_by(4) { let p = px(im, x, y); s += p[0] as i64 - p[1] as i64; n += 1; } } s / n };
    let (r1, r0) = (redness(&img), redness(&plain));
    if r1 < r0 + 15 { bail!("death overlay not red (r-g {r1} vs {r0} without)"); }
    if g.hp > 0.0 { bail!("health not 0 on death ({})", g.hp); }
    press_widget(&mut g, "respawn")?;
    if !wait_until(&mut g, 8.0, |g| g.screen.is_none() && g.spawned && g.hp >= 20.0)? { bail!("respawn failed (hp {}, screen open {})", g.hp, g.screen.is_some()); }
    Ok(Outcome::Pass(format!("death screen \"{msg}\", respawned with hp {}", g.hp)))
}

/// Survival digging: stone by hand and with a wooden pickaxe take the vanilla time (±2 ticks),
/// the crack overlay advances, the server accepts the break.
fn dig(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("dig")?;
    join(&mut g)?;
    cmd(&mut g, "gamemode survival");
    tp(&mut g, [400.5, -60.0, 0.5], 0.0, 0.0)?;
    let mut details = Vec::new();
    // (tool, expected ticks: wiki break times for stone — hand 7.5 s, wooden pickaxe 1.15 s)
    for (i, (tool, expect)) in [(None, 150u32), (Some("wooden_pickaxe"), 23u32)].into_iter().enumerate() {
        let b = [400, -60, 2 + i as i32 * 0];
        if let Some(t) = tool { cmd(&mut g, &format!("give @s {t}")); if !wait_until(&mut g, 3.0, |g| g.hotbar[0].item == item_id(t))? { bail!("no {t}"); } }
        cmd(&mut g, &format!("setblock {} {} {} stone", b[0], b[1], b[2]));
        if !wait_until(&mut g, 3.0, |g| basalt_data::BlockState(g.world.get(b[0], b[1], b[2]) as u32).name() == "stone")? { bail!("setblock failed"); }
        wait(&mut g, 0.3)?;
        look_at(&mut g, [b[0] as f64 + 0.5, b[1] as f64 + 0.5, b[2] as f64]);
        step(&mut g)?;
        g.last_dig = None;
        g.mouse_button(0, true);
        let mut crack_px = 0; let mut stages = Vec::new(); let mut swung = false;
        let t0 = Instant::now();
        while g.last_dig.is_none() && t0.elapsed().as_secs_f32() < expect as f32 / 20.0 + 3.0 {
            step(&mut g)?;
            swung |= g.fp_swing >= 0;
            if let Some(st) = g.dig_stage() { if stages.last() != Some(&st) { stages.push(st); } }
            if crack_px == 0 && g.dig_stage().map(|s| s >= 5).unwrap_or(false) {
                let a = capture(&mut g)?; g.hide_cracks = true; let bimg = capture(&mut g)?; g.hide_cracks = false;
                crack_px = diff_count(&a, &bimg, 0, 0, W as i32, H as i32);
                if i == 0 { save(&g, &a, "dig_crack"); }
            }
        }
        g.mouse_button(0, false);
        let Some((ticks, _)) = g.last_dig else { bail!("dig with {tool:?} did not finish") };
        if (ticks as i32 - expect as i32).abs() > 2 { bail!("dig with {tool:?} took {ticks} ticks (vanilla {expect})"); }
        if crack_px < 50 { bail!("crack overlay not visible ({crack_px} px differ)"); }
        if stages.len() < 5 { bail!("crack stages did not advance: {stages:?}"); }
        if !swung { bail!("no swing animation while digging"); }
        // the server must not put the block back
        wait(&mut g, 1.0)?;
        if g.world.get(b[0], b[1], b[2]) != 0 { bail!("server rejected the break with {tool:?}"); }
        details.push(format!("{}: {ticks}t (vanilla {expect}), stages {:?}, crack {crack_px}px", tool.unwrap_or("hand"), stages.len()));
    }
    Ok(Outcome::Pass(details.join("; ")))
}

fn vanilla_console(line: &str) -> Result<()> {
    let p = std::env::var("BASALT_VANILLA_CONSOLE").unwrap_or_else(|_| "vanilla/srv/console.fifo".into());
    let line = format!("{line}\n");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || { let r = std::fs::OpenOptions::new().write(true).open(&p).and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes())); let _ = tx.send(r.is_ok()); });
    match rx.recv_timeout(Duration::from_secs(2)) { Ok(true) => Ok(()), _ => bail!("vanilla console fifo not writable") }
}
fn vanilla_addr(c: &Ctx) -> Option<String> {
    let a = c.args.vanilla.clone()?;
    std::net::TcpStream::connect_timeout(&a.parse().ok()?, Duration::from_secs(1)).ok()?;
    Some(a)
}

/// Another player's digging shows as block_destruction cracks + swing (needs the vanilla server:
/// Basalt doesn't broadcast block_destruction yet).
fn dig_others(c: &Ctx) -> Result<Outcome> {
    let Some(addr) = vanilla_addr(c) else { return Ok(Outcome::Skip("needs --vanilla HOST:PORT (vanilla server not reachable)".into())) };
    let mut a = c.game_at("dA", Some(&addr))?;
    let mut b = c.game_at("dB", Some(&addr))?;
    join(&mut a)?; join(&mut b)?;
    vanilla_console(&format!("op {}", a.name))?;
    wait(&mut a, 1.0)?;
    let base = [5000 + (c.run % 50) as i32 * 8, 200, 5000];
    let (an, bn) = (a.name.clone(), b.name.clone());
    // drop onto the terrain in creative (no fall damage), then set up the block where A landed
    for n in [&an, &bn] { cmd(&mut a, &format!("gamemode creative {n}")); }
    cmd(&mut a, &format!("tp {an} {} {} {}", base[0] as f64 + 0.5, base[1], base[2] as f64 + 0.5));
    let t0 = Instant::now();
    let mut ground = 0;
    while t0.elapsed().as_secs_f32() < 20.0 && ground < 20 {
        step(&mut a)?; step(&mut b)?;
        if a.player.on_ground && (a.player.pos[0] - base[0] as f64 - 0.5).abs() < 0.01 { ground += 1; } else { ground = 0; }
    }
    if ground < 20 { bail!("digger did not land at the test site"); }
    let p = [a.player.pos[0].floor() as i32, a.player.pos[1].floor() as i32, a.player.pos[2].floor() as i32];
    cmd(&mut a, &format!("fill {} {} {} {} {} {} air", p[0] - 1, p[1], p[2] + 1, p[0] + 3, p[1] + 2, p[2] + 3));
    cmd(&mut a, &format!("setblock {} {} {} stone", p[0], p[1], p[2] + 2));
    let ay = a.player.pos[1]; cmd(&mut a, &format!("tp {bn} {} {} {}", p[0] as f64 + 2.5, ay, p[2] as f64 + 0.5));
    cmd(&mut a, "give @s wooden_pickaxe");
    cmd(&mut a, &format!("gamemode survival {an}"));
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f32() < 10.0 {
        step(&mut a)?; step(&mut b)?;
        if b.entities.contains_key(&a.my_eid) && basalt_data::BlockState(b.world.get(p[0], p[1], p[2] + 2) as u32).name() == "stone" && a.player.gamemode == 0 && a.hotbar[0].count > 0 { break; }
    }
    for _ in 0..40 { step(&mut a)?; step(&mut b)?; }
    look_at(&mut a, [p[0] as f64 + 0.5, p[1] as f64 + 0.5, p[2] as f64 + 2.0]);
    a.mouse_button(0, true);
    let (mut max_stage, mut swing_seen) = (-1i8, false);
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f32() < 8.0 && a.last_dig.is_none() {
        step(&mut a)?; step(&mut b)?;
        for (_, (_, st)) in &b.destroy { max_stage = max_stage.max(*st); }
        swing_seen |= b.entities.get(&a.my_eid).map(|e| e.swing >= 0).unwrap_or(false);
    }
    a.mouse_button(0, false);
    if max_stage < 3 { let t = a.target().map(|h| h.block); bail!("observer saw no block_destruction progress (max stage {max_stage}; digger at {:?} gm {} target {t:?} dig {:?} last {:?}; observer at {:?})", a.player.pos, a.player.gamemode, a.dig.as_ref().map(|d| (d.ticks, d.total)), a.last_dig, b.player.pos); }
    if !swing_seen { bail!("observer saw no swing animation"); }
    Ok(Outcome::Pass(format!("observer saw cracks up to stage {max_stage} and the swing")))
}

/// Mob/player/item models: each type draws model pixels where it stands; hurt tint, death tip-over.
fn models(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("mdl")?;
    join(&mut g)?;
    cmd(&mut g, "gamemode creative");
    let o = [500.5, -60.0, 0.5];
    tp(&mut g, o, 180.0, 15.0)?;
    // probe whether this server has /summon
    let mobs = ["zombie", "skeleton", "creeper", "spider", "cow", "pig", "sheep", "chicken"];
    let spots: Vec<[f64; 3]> = (0..mobs.len()).map(|i| [o[0] - 7.0 + (i % 4) as f64 * 4.5, o[1], o[2] - 7.0 - (i / 4) as f64 * 5.0]).collect();
    cmd(&mut g, "kill @e[type=!player]");
    wait(&mut g, 0.5)?;
    for (m, p) in mobs.iter().zip(&spots) { cmd(&mut g, &format!("summon {m} {} {} {} NoAI", p[0], p[1], p[2])); }
    let have = |g: &Game, m: &str| g.entities.values().any(|e| crate::entity::type_name(e.kind) == m);
    if !wait_until(&mut g, 4.0, |g| mobs.iter().all(|m| have(g, m)))? {
        let missing: Vec<&&str> = mobs.iter().filter(|m| !have(&g, m)).collect();
        bail!("summon did not produce {missing:?} (server needs /summon)");
    }
    // a second player in view
    let mut bot = c.game("bot")?;
    join(&mut bot)?;
    cmd(&mut bot, &format!("tp @s {} {} {}", o[0] + 2.5, o[1], o[2] - 3.0));
    for _ in 0..150 { step(&mut bot)?; step(&mut g)?; }
    wait(&mut g, 0.5)?;
    let with = capture(&mut g)?; save(&g, &with, "models");
    g.hide_entities = true; let without = capture(&mut g)?; g.hide_entities = false;
    let mut res = Vec::new();
    let mut weak = Vec::new();
    let check = |g: &Game, kind: &str, pos: [f64; 3], res: &mut Vec<String>, weak: &mut Vec<String>| {
        let e = g.entities.values().find(|e| crate::entity::type_name(e.kind) == kind && ((e.pos[0] - pos[0]).abs() < 1.0 || pos[0].is_nan()));
        let Some(e) = e else { weak.push(format!("{kind}: missing")); return };
        let (w, h, _) = crate::entity::dims(e.kind);
        let mut x0 = f32::MAX; let mut y0 = f32::MAX; let mut x1 = f32::MIN; let mut y1 = f32::MIN;
        for dx in [-1.0, 1.0] { for dz in [-1.0, 1.0] { for dy in [0.0, 1.0] {
            if let Some((sx, sy)) = g.project([e.pos[0] + dx * w as f64 / 2.0, e.pos[1] + dy * h as f64, e.pos[2] + dz * w as f64 / 2.0]) { x0 = x0.min(sx); y0 = y0.min(sy); x1 = x1.max(sx); y1 = y1.max(sy); }
        } } }
        let n = diff_count(&with, &without, x0 as i32, y0 as i32, x1 as i32 + 1, y1 as i32 + 1);
        let area = ((x1 - x0) * (y1 - y0)).max(1.0);
        let mut m = crate::mesh::MeshData::default();
        let modeled = crate::entity::build_model(&g.entity_atlas, e, 1.0, [255, 255], &mut m);
        res.push(format!("{kind} {n}px"));
        if n < 40 || (n as f32) < area * 0.08 || !modeled { weak.push(format!("{kind}: {n}px of {area:.0} (model {modeled})")); }
    };
    for (m, p) in mobs.iter().zip(&spots) { check(&g, m, *p, &mut res, &mut weak); }
    check(&g, "player", [f64::NAN; 3], &mut res, &mut weak);
    if !weak.is_empty() { bail!("weak/missing models: {}", weak.join(", ")); }
    let tag = g.player_names.values().any(|n| *n == bot.name);
    // hurt tint + death tip-over: attack the pig, then kill it
    let pig = g.entities.values().find(|e| crate::entity::type_name(e.kind) == "pig").map(|e| e.id).unwrap();
    cmd(&mut g, "gamemode survival");
    tp(&mut g, [spots[5][0], o[1], spots[5][2] + 2.0], 180.0, 30.0)?;
    let mut hurt = false;
    for _ in 0..3 { g.attack(pig); if wait_until(&mut g, 1.0, |g| g.entities.get(&pig).map(|e| e.hurt > 0).unwrap_or(false))? { hurt = true; break; } }
    cmd(&mut g, "kill @e[type=pig]");
    let died = wait_until(&mut g, 3.0, |g| g.entities.get(&pig).map(|e| e.death > 0).unwrap_or(false))?;
    if !hurt { bail!("no hurt animation on the attacked pig"); }
    if !died { bail!("no death animation on the killed pig"); }
    // dropped item: throw one from the hotbar
    cmd(&mut g, "give @s stone 3");
    wait_until(&mut g, 3.0, |g| g.hotbar[0].count > 0)?;
    g.player.pitch = 10.0; tap(&mut g, KeyCode::KeyQ, None)?;
    let got_item = wait_until(&mut g, 3.0, |g| g.entities.values().any(|e| crate::entity::type_name(e.kind) == "item" && e.item.is_some()))?;
    if !got_item { bail!("thrown item entity not seen (with its stack)"); }
    wait(&mut g, 1.0)?;
    let wi = capture(&mut g)?; g.hide_entities = true; let wo = capture(&mut g)?; g.hide_entities = false;
    save(&g, &wi, "item_entity");
    let item_px = { let e = g.entities.values().find(|e| crate::entity::type_name(e.kind) == "item").unwrap(); let p = g.project(e.pos).unwrap_or((0.0, 0.0)); diff_count(&wi, &wo, p.0 as i32 - 60, p.1 as i32 - 60, p.0 as i32 + 60, p.1 as i32 + 30) };
    if item_px < 20 { bail!("dropped item not drawn ({item_px} px)"); }
    // arrow geometry exists (no /summon arrow needed)
    let mut m = crate::mesh::MeshData::default();
    let arrow = crate::entity::Entity::new(-5, uuid::Uuid::nil(), basalt_data::registry_id(basalt_data::registries::ENTITY_TYPE, "arrow").unwrap_or(0), [0.0; 3], [0.0; 3], 0.0, 0.0, 0.0);
    if !crate::entity::build_model(&g.entity_atlas, &arrow, 1.0, [255, 255], &mut m) || m.idx.is_empty() { bail!("no arrow model"); }
    Ok(Outcome::Pass(format!("{}; nametag {tag}; hurt+death anim ok; item {item_px}px; arrow ok", res.join(" "))))
}

/// Remote entity movement is interpolated: no per-frame jumps while another player walks.
fn interp(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("ipv")?;
    let mut bot = c.game("ipb")?;
    join(&mut g)?; join(&mut bot)?;
    let o = [600.5, -60.0, 0.5];
    tp(&mut g, o, 0.0, 10.0)?;
    cmd(&mut bot, &format!("tp @s {} {} {}", o[0] - 3.0, o[1], o[2] + 4.0));
    for _ in 0..200 { step(&mut bot)?; step(&mut g)?; }
    bot.player.yaw = -90.0; // walk towards +X
    bot.keys.insert(KeyCode::KeyW);
    let Some(id) = g.entities.values().find(|e| crate::entity::type_name(e.kind) == "player").map(|e| e.id) else { bail!("other player not visible") };
    let start = g.entities[&id].pos;
    let mut last: Option<[f64; 3]> = None; let mut max_jump = 0.0f64; let mut frames = 0;
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f32() < 2.0 {
        step(&mut bot)?;
        let alpha = g.update();
        let p = g.entities[&id].render_pos(alpha as f32);
        if let Some(l) = last { max_jump = max_jump.max(((p[0] - l[0]).powi(2) + (p[2] - l[2]).powi(2)).sqrt()); }
        last = Some(p); frames += 1;
        std::thread::sleep(Duration::from_millis(4));
    }
    bot.keys.remove(&KeyCode::KeyW);
    let end = g.entities[&id].pos;
    let moved = ((end[0] - start[0]).powi(2) + (end[2] - start[2]).powi(2)).sqrt();
    if moved < 1.5 { bail!("other player moved only {moved:.2}"); }
    if max_jump > 0.12 { bail!("rendered position jumps {max_jump:.3} blocks between frames"); }
    Ok(Outcome::Pass(format!("moved {moved:.2} blocks over {frames} frames, max frame step {max_jump:.3}")))
}

/// Sound index + Ogg decoding from the asset store; play events for local place/break and server packets.
fn sounds(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("snd")?;
    if g.sounds.event_count() < 500 { return Ok(Outcome::Skip(format!("launcher asset store not found ({} events)", g.sounds.event_count()))); }
    let pcm = g.sounds.pcm("dig/stone1").ok_or_else(|| anyhow!("could not decode dig/stone1.ogg"))?;
    if pcm.samples.len() < 1000 || pcm.rate < 8000 { bail!("decoded audio too short ({} samples @ {} Hz)", pcm.samples.len(), pcm.rate); }
    join(&mut g)?;
    cmd(&mut g, "gamemode creative");
    tp(&mut g, [700.5, -60.0, 0.5], 0.0, 50.0)?;
    cmd(&mut g, "give @s stone 4");
    wait_until(&mut g, 3.0, |g| g.hotbar[0].count > 0)?;
    g.sounds.log.clear();
    g.mouse_button(1, true); step(&mut g)?; g.mouse_button(1, false);
    wait(&mut g, 0.5)?;
    let placed = g.sounds.log.iter().any(|p| p.event == "block.stone.place");
    look_at(&mut g, [700.5, -60.5, 2.5]);
    g.mouse_button(0, true); wait(&mut g, 0.2)?; g.mouse_button(0, false);
    let broke = g.sounds.log.iter().any(|p| p.event.ends_with(".break"));
    // positional attenuation: a far sound is quieter than a near one
    let me = g.player.pos;
    g.sounds.play("block.stone.break", Some([me[0] + 1.0, me[1], me[2]]), 1.0, 1.0);
    g.sounds.play("block.stone.break", Some([me[0] + 12.0, me[1], me[2]]), 1.0, 1.0);
    let n = g.sounds.log.len();
    let (near, far) = (g.sounds.log[n - 2].gain, g.sounds.log[n - 1].gain);
    // server-sent sound packet: hurt a mob (Basalt M5 sends sound packets for mob hurt)
    cmd(&mut g, "summon cow 700.5 -60 3 NoAI");
    let have_cow = wait_until(&mut g, 3.0, |g| g.entities.values().any(|e| crate::entity::type_name(e.kind) == "cow"))?;
    let mut server_sound = None;
    if have_cow {
        cmd(&mut g, "gamemode survival");
        wait(&mut g, 0.3)?;
        let cow = g.entities.values().find(|e| crate::entity::type_name(e.kind) == "cow").map(|e| e.id).unwrap();
        g.attack(cow);
        wait(&mut g, 1.0)?;
        server_sound = g.sounds.log.iter().find(|p| p.event.starts_with("entity.")).map(|p| p.event.clone());
    }
    if !placed { bail!("no block.stone.place on placing"); }
    if !broke { bail!("no break sound"); }
    if !(near > far && far >= 0.0) { bail!("attenuation wrong: near {near} far {far}"); }
    let Some(ss) = server_sound else { bail!("no server sound packet played (cow summoned: {have_cow})") };
    Ok(Outcome::Pass(format!("{} events; dig/stone1 {} samples @ {} Hz; place+break played; gain near {near:.2} far {far:.2}; server sound {ss}; output device {}", g.sounds.event_count(), pcm.samples.len(), pcm.rate, g.sounds.has_output())))
}

/// Title → Multiplayer → Direct Connection → join; Esc → pause → Options (FOV slider) → Done →
/// Disconnect → title; Add Server saved to the config and joined from the list.
fn menus(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game_at("menu", None)?;
    wait(&mut g, 0.2)?;
    if !matches!(g.screen, Some(Screen::Title)) { bail!("not on the title screen"); }
    let img = capture(&mut g)?; save(&g, &img, "title");
    // logo drawn: bright pixels near the top centre
    let logo = (0..W as i32).step_by(3).filter(|&x| { let p = px(&img, x, 60); p[0] as u32 + p[1] as u32 + p[2] as u32 > 300 }).count();
    if logo < 10 { bail!("title logo not drawn"); }
    press_widget(&mut g, "multiplayer")?;
    if !matches!(g.screen, Some(Screen::Multiplayer { .. })) { bail!("Multiplayer button did not open the server list"); }
    let clicks = g.sounds.log.iter().filter(|p| p.event == "ui.button.click").count();
    press_widget(&mut g, "direct")?;
    if !matches!(g.screen, Some(Screen::Direct { .. })) { bail!("Direct Connection screen did not open"); }
    // clear any prefilled text, type the address
    for _ in 0..64 { g.key(KeyCode::Backspace, true, None); g.key(KeyCode::Backspace, false, None); }
    type_text(&mut g, &c.args.addr)?;
    press_widget(&mut g, "join")?;
    if !wait_until(&mut g, 20.0, |g| g.spawned && g.screen.is_none())? { bail!("did not join from Direct Connection"); }
    wait_until(&mut g, 10.0, settled)?;
    tap(&mut g, KeyCode::Escape, None)?;
    if !matches!(g.screen, Some(Screen::Pause)) { bail!("Esc did not open the pause menu"); }
    let img = capture(&mut g)?; save(&g, &img, "pause");
    press_widget(&mut g, "options")?;
    if !matches!(g.screen, Some(Screen::Options { .. })) { bail!("Options did not open"); }
    let s = g.gui_scale();
    let fov = g.widgets().into_iter().find(|w| w.id == "fov").ok_or_else(|| anyhow!("no fov slider"))?;
    click_at(&mut g, (fov.x + fov.w * 0.9) * s, (fov.y + 10.0) * s, 0)?;
    let new_fov = g.cfg.fov;
    if new_fov < 95.0 { bail!("FOV slider did not change the FOV ({new_fov})"); }
    let img = capture(&mut g)?; save(&g, &img, "options");
    press_widget(&mut g, "done")?;
    if !matches!(g.screen, Some(Screen::Pause)) { bail!("Done did not return to the pause menu"); }
    let saved = Config::load();
    if (saved.fov - new_fov).abs() > 0.5 { bail!("config not saved (fov {} on disk)", saved.fov); }
    press_widget(&mut g, "disconnect")?;
    if !matches!(g.screen, Some(Screen::Title)) || g.connected { bail!("Disconnect did not return to the title screen"); }
    // Add Server → list → double-click joins
    press_widget(&mut g, "multiplayer")?;
    press_widget(&mut g, "add")?;
    for _ in 0..64 { g.key(KeyCode::Backspace, true, None); g.key(KeyCode::Backspace, false, None); }
    type_text(&mut g, "Test Server")?;
    tap(&mut g, KeyCode::Tab, None)?;
    type_text(&mut g, &c.args.addr)?;
    press_widget(&mut g, "done")?;
    let idx = g.cfg.servers.iter().position(|s| s.addr == c.args.addr && s.name == "Test Server").ok_or_else(|| anyhow!("server not added: {:?}", g.cfg.servers))?;
    if !Config::load().servers.iter().any(|s| s.name == "Test Server") { bail!("server list not saved"); }
    let img = capture(&mut g)?; save(&g, &img, "server_list");
    let (x, y) = widget_center(&g, &format!("row{idx}"))?;
    click_at(&mut g, x, y, 0)?; click_at(&mut g, x, y, 0)?;
    if !wait_until(&mut g, 20.0, |g| g.spawned && g.screen.is_none())? { bail!("did not join from the server list"); }
    Ok(Outcome::Pass(format!("title→multiplayer→direct→join, pause→options fov {new_fov}→saved, disconnect→title, add server→join; ui clicks {clicks}+")))
}

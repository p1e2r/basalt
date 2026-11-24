//! Headless scenario tests: `basalt-client --scenario <name|all> --addr HOST:PORT [--vanilla HOST:PORT]`.
//! Each scenario drives a real (offscreen) client against the server through the same input entry
//! points the window uses, asserts on client state and on rendered pixels, and prints
//! `SCENARIO M7/<name> PASS|FAIL|SKIP <details>`.
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

pub const ALL: &[&str] = &["edge_chunks", "inventory", "crafting_table", "death", "dig", "dig_others", "models", "interp", "head_pitch", "first_person_arm", "command_suggestions", "sounds", "menus", "held_item", "break_particles", "options_pages", "chest", "furnace", "server_list", "singleplayer", "falling_block", "sprint", "xp_bar", "titles", "nether", "enchanting_screen", "anvil_screen", "grindstone_screen", "smithing_screen", "brewing_screen", "font_widths", "grass_overlay", "item_drop_smooth", "pickup_anim", "chat_keys", "landing", "water_splash"];

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
    std::env::set_var("BASALT_NO_OS_CLIPBOARD", "1");
    let dir = std::env::temp_dir().join(format!("basalt-scenario-{}", std::process::id()));
    std::env::set_var("BASALT_CONFIG_DIR", &dir);
    let ctx = Ctx { args, assets, rules: crate::load_rules(args), run: std::process::id() };
    let list: Vec<&str> = if name == "all" { ALL.to_vec() } else { name.split(',').collect() };
    let mut ok = true;
    for s in list {
        let t = Instant::now();
        let r = match s {
            "edge_chunks" => edge_chunks(&ctx), "inventory" => inventory(&ctx), "crafting_table" => crafting_table(&ctx), "death" => death(&ctx),
            "dig" => dig(&ctx), "dig_others" => dig_others(&ctx), "models" => models(&ctx), "interp" => interp(&ctx), "head_pitch" => head_pitch(&ctx), "first_person_arm" => first_person_arm(&ctx), "command_suggestions" => command_suggestions(&ctx), "sounds" => sounds(&ctx), "menus" => menus(&ctx), "held_item" => held_item(&ctx), "break_particles" => break_particles(&ctx), "options_pages" => options_pages(&ctx), "server_list" => server_list(&ctx), "singleplayer" => singleplayer(&ctx), "falling_block" => falling_block(&ctx), "sprint" => sprint(&ctx), "xp_bar" => xp_bar(&ctx), "titles" => titles(&ctx), "chest" => chest(&ctx), "furnace" => furnace(&ctx), "nether" => nether(&ctx), "enchanting_screen" => enchanting_screen(&ctx), "anvil_screen" => anvil_screen(&ctx), "grindstone_screen" => grindstone_screen(&ctx), "smithing_screen" => smithing_screen(&ctx), "brewing_screen" => brewing_screen(&ctx), "font_widths" => font_widths(&ctx), "grass_overlay" => grass_overlay(&ctx), "item_drop_smooth" => item_drop_smooth(&ctx), "pickup_anim" => pickup_anim(&ctx), "chat_keys" => chat_keys(&ctx), "landing" => landing(&ctx), "water_splash" => water_splash(&ctx),
            other => Err(anyhow!("unknown scenario {other} (known: {})", ALL.join(", "))),
        };
        let secs = t.elapsed().as_secs_f32();
        match r {
            Ok(Outcome::Pass(d)) => println!("SCENARIO M7/{s} PASS {d} ({secs:.1}s)"),
            Ok(Outcome::Skip(d)) => println!("SCENARIO M7/{s} SKIP {d}"),
            Err(e) => { ok = false; println!("SCENARIO M7/{s} FAIL {e:#} ({secs:.1}s)"); }
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
    cmd(g, &format!("tp @s {:.4} {:.4} {:.4}", p[0], p[1], p[2]));
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
        let old_items: std::collections::HashSet<i32> = g.entities.keys().copied().collect();
        let had_cobble = count_item(&g.inventory, item_id("cobblestone"));
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
        // by hand stone drops nothing (vanilla: needs a pickaxe); with the pickaxe it drops cobblestone
        let cobble = item_id("cobblestone");
        // a new cobblestone item entity, or cobblestone already picked up
        let dropped = wait_until(&mut g, 1.5, |g| g.entities.iter().any(|(id, e)| !old_items.contains(id) && e.item.as_ref().map(|i| i.item == cobble).unwrap_or(false)) || count_item(&g.inventory, cobble) > had_cobble)?;
        if tool.is_none() && dropped { bail!("stone broken by hand dropped cobblestone"); }
        if tool.is_some() && !dropped { bail!("stone broken with a pickaxe dropped nothing"); }
        details.push(format!("{}: {ticks}t (vanilla {expect}), stages {:?}, crack {crack_px}px, drop {dropped}", tool.unwrap_or("hand"), stages.len()));
    }
    // creative, empty hand: instant break, no drop
    cmd(&mut g, "clear @s");
    cmd(&mut g, "gamemode creative");
    if !wait_until(&mut g, 3.0, |g| g.player.gamemode == 1 && g.hotbar.iter().all(|s| s.count == 0))? { bail!("creative / clear did not apply"); }
    let b = [400, -60, 2];
    cmd(&mut g, &format!("setblock {} {} {} stone", b[0], b[1], b[2]));
    if !wait_until(&mut g, 3.0, |g| basalt_data::BlockState(g.world.get(b[0], b[1], b[2]) as u32).name() == "stone")? { bail!("setblock failed"); }
    wait(&mut g, 0.3)?;
    look_at(&mut g, [b[0] as f64 + 0.5, b[1] as f64 + 0.5, b[2] as f64]);
    step(&mut g)?;
    wait(&mut g, 0.4)?;   // past any dig delay left from the last break
    let old_items: std::collections::HashSet<i32> = g.entities.keys().copied().collect();
    g.mouse_button(0, true); ticks(&mut g, 2)?; g.mouse_button(0, false);
    if !wait_until(&mut g, 1.0, |g| g.world.get(b[0], b[1], b[2]) == 0)? { bail!("creative empty-hand click did not break stone at once"); }
    wait(&mut g, 1.0)?;
    if g.world.get(b[0], b[1], b[2]) != 0 { bail!("server put the creative-broken block back"); }
    let cobble = item_id("cobblestone");
    if g.entities.iter().any(|(id, e)| !old_items.contains(id) && e.item.as_ref().map(|i| i.item == cobble || i.item == item_id("stone")).unwrap_or(false)) { bail!("creative break dropped an item"); }
    details.push("creative hand: instant, no drop".into());
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
    cmd(&mut a, &format!("tp {an} {:.4} {:.4} {:.4}", base[0] as f64 + 0.5, base[1], base[2] as f64 + 0.5));
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
    let ay = a.player.pos[1]; cmd(&mut a, &format!("tp {bn} {:.4} {:.4} {:.4}", p[0] as f64 + 2.5, ay, p[2] as f64 + 0.5));
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
    for (m, p) in mobs.iter().zip(&spots) { cmd(&mut g, &format!("summon {m} {} {} {} {{NoAI:1b}}", p[0], p[1], p[2])); }
    let have = |g: &Game, m: &str| g.entities.values().any(|e| crate::entity::type_name(e.kind) == m);
    if !wait_until(&mut g, 4.0, |g| mobs.iter().all(|m| have(g, m)))? {
        let missing: Vec<&&str> = mobs.iter().filter(|m| !have(&g, m)).collect();
        bail!("summon did not produce {missing:?} (server needs /summon)");
    }
    // a second player in view
    let mut bot = c.game("bot")?;
    join(&mut bot)?;
    cmd(&mut bot, &format!("tp @s {:.4} {:.4} {:.4}", o[0] + 2.5, o[1], o[2] - 3.0));
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
    // the thrown stack, not the pig's loot: pick the item entity nearest to us
    let me = g.player.pos;
    let item_px = { let e = g.entities.values().filter(|e| crate::entity::type_name(e.kind) == "item")
        .min_by(|a, b| { let d = |e: &crate::entity::Entity| (0..3).map(|k| (e.pos[k] - me[k]).powi(2)).sum::<f64>(); d(a).partial_cmp(&d(b)).unwrap() }).unwrap(); let p = g.project(e.pos).unwrap_or((0.0, 0.0)); diff_count(&wi, &wo, p.0 as i32 - 60, p.1 as i32 - 60, p.0 as i32 + 60, p.1 as i32 + 30) };
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
    cmd(&mut bot, &format!("tp @s {:.4} {:.4} {:.4}", o[0] - 3.0, o[1], o[2] + 4.0));
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

/// Regression: head pitch arrives as a *signed* byte. A mob looking up at a player above it used to
/// decode as ~310° and its head interpolated through a backflip. Samples the rendered pitch every frame.
fn head_pitch(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("hp")?;
    join(&mut g)?;
    let o = [700.5, -54.0, 700.5];
    cmd(&mut g, "time set midnight");
    cmd(&mut g, "fill 698 -55 698 702 -55 702 stone");
    wait(&mut g, 0.5)?;
    tp(&mut g, o, 0.0, 0.0)?;
    cmd(&mut g, "summon zombie 700.5 -60 700.5");
    if !wait_until(&mut g, 5.0, |g| g.entities.values().any(|e| crate::entity::type_name(e.kind) == "zombie"))? { bail!("zombie not visible"); }
    let id = g.entities.values().find(|e| crate::entity::type_name(e.kind) == "zombie").map(|e| e.id).unwrap();
    let (mut min, mut max, mut last, mut max_step, mut frames) = (f32::MAX, f32::MIN, None::<f32>, 0.0f32, 0);
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f32() < 4.0 {
        let alpha = g.update() as f32;
        if let Some(e) = g.entities.get(&id) {
            let p = e.prev_pitch + (e.pitch - e.prev_pitch) * alpha;
            min = min.min(p); max = max.max(p);
            if let Some(l) = last { max_step = max_step.max((p - l).abs()); }
            last = Some(p); frames += 1;
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    if frames < 50 { bail!("only {frames} frames with the zombie in view"); }
    if min < -90.01 || max > 90.01 { bail!("rendered head pitch left ±90°: {min:.1}..{max:.1}"); }
    if max_step > 20.0 { bail!("head pitch jumped {max_step:.1}° between frames (backflip)"); }
    if min > -10.0 { bail!("the zombie never looked up at the player above it (min pitch {min:.1}°) — scenario not exercising the bug"); }
    Ok(Outcome::Pass(format!("pitch {min:.1}..{max:.1}° over {frames} frames, max step {max_step:.1}°")))
}

/// Region of the frame the first-person arm reference covers, and its cell size (see refs/first_person_arm.txt).
const ARM_REGION: (usize, usize, usize, usize) = (612, 250, 854, 478);
const ARM_CELL: usize = 4;

/// Arm pixels = where the frame with the hand differs from the frame without it, as a 4x4-cell grid.
fn arm_cells(with: &[u8], without: &[u8]) -> Vec<Vec<bool>> {
    let (x0, y0, x1, y1) = ARM_REGION;
    let (cw, ch) = ((x1 - x0) / ARM_CELL, (y1 - y0) / ARM_CELL);
    (0..ch).map(|cy| (0..cw).map(|cx| {
        let mut n = 0;
        for y in cy * ARM_CELL..(cy + 1) * ARM_CELL { for x in cx * ARM_CELL..(cx + 1) * ARM_CELL {
            let i = ((y0 + y) * W as usize + x0 + x) * 3;
            let d: i32 = (0..3).map(|k| (with[i + k] as i32 - without[i + k] as i32).abs()).sum();
            if d > 40 { n += 1; }
        } }
        n >= ARM_CELL * ARM_CELL / 2
    }).collect()).collect()
}

fn arm_reference() -> Vec<Vec<bool>> {
    include_str!("../refs/first_person_arm.txt").lines().filter(|l| !l.starts_with("//") && !l.is_empty()).map(|l| l.chars().map(|c| c == '#').collect()).collect()
}

fn grid_iou(a: &[Vec<bool>], b: &[Vec<bool>]) -> f32 {
    let (mut i, mut u) = (0, 0);
    for (ra, rb) in a.iter().zip(b) { for (x, y) in ra.iter().zip(rb) { if *x && *y { i += 1; } if *x || *y { u += 1; } } }
    i as f32 / u.max(1) as f32
}

fn centroid_y(g: &[Vec<bool>]) -> f32 {
    let (mut s, mut n) = (0.0, 0.0);
    for (y, row) in g.iter().enumerate() { for v in row { if *v { s += y as f32; n += 1.0; } } }
    s / f32::max(n, 1.0) * ARM_CELL as f32
}

/// The empty first-person arm must sit where the real client draws it (silhouette measured from a
/// real 26.2 capture: refs/first_person_arm.txt), and swinging must move it.
fn first_person_arm(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("fpa")?;
    join(&mut g)?;
    cmd(&mut g, "time set noon");
    tp(&mut g, [820.5, -60.0, 820.5], 0.0, 0.0)?;
    wait(&mut g, 1.5)?;
    if g.hotbar[g.held as usize].count != 0 { bail!("hotbar slot is not empty; the arm is only drawn for an empty hand"); }
    g.fp_swing = -1;
    let with = capture(&mut g)?;
    g.hide_hand = true; let without = capture(&mut g)?; g.hide_hand = false;
    let cells = arm_cells(&with, &without);
    let n: usize = cells.iter().map(|r| r.iter().filter(|v| **v).count()).sum();
    if n < 150 { bail!("arm barely drawn: {n} cells"); }
    let iou = grid_iou(&cells, &arm_reference());
    save(&g, &with, "first_person_arm");
    if iou < 0.70 { bail!("arm silhouette IoU {iou:.2} vs the real client (need 0.70); {n} cells"); }
    // swing: the arm must move (vanilla raises it forward and up)
    g.fp_swing = 3;
    let swung = capture(&mut g)?;
    let sc = arm_cells(&swung, &without);
    let dy = (centroid_y(&sc) - centroid_y(&cells)).abs();
    if dy < 3.0 { bail!("swing barely moves the arm (centroid shift {dy:.1}px)"); }
    Ok(Outcome::Pass(format!("silhouette IoU {iou:.2} vs real client, {n} cells, swing shifts it {dy:.0}px")))
}

/// Chat-box command suggestions: the server's tree, local suggestions, Tab completion/cycling,
/// ask_server answers, the popup being drawn.
fn command_suggestions(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("cs")?;
    join(&mut g)?;
    if !wait_until(&mut g, 5.0, |g| g.cmd_tree.is_some())? { bail!("no commands packet received"); }
    let nodes = g.cmd_tree.as_ref().map(|t| t.nodes.len()).unwrap_or(0);
    if nodes < 50 { bail!("command tree has only {nodes} nodes"); }
    wait(&mut g, 0.5)?;
    let sug = |g: &Game| g.sug.list.clone();
    tap(&mut g, KeyCode::KeyT, None)?;
    type_text(&mut g, "/gam")?;
    if !sug(&g).contains(&"gamemode".to_string()) { bail!("'/gam' did not suggest gamemode: {:?}", sug(&g)); }
    // the popup is actually drawn (compare with the list hidden)
    let shown = capture(&mut g)?;
    let saved = std::mem::take(&mut g.sug.list); let hidden = capture(&mut g)?; g.sug.list = saved;
    let popup_px = diff_count(&shown, &hidden, 0, 300, 300, 470);
    if popup_px < 100 { bail!("suggestion popup not drawn ({popup_px} px)"); }
    save(&g, &shown, "command_suggestions");
    tap(&mut g, KeyCode::Tab, None)?;
    if g.chat_input.as_deref() != Some("/gamemode") { bail!("Tab gave {:?}, expected /gamemode", g.chat_input); }
    type_text(&mut g, " c")?;
    if sug(&g) != vec!["creative".to_string()] { bail!("'/gamemode c' suggested {:?}", sug(&g)); }
    tap(&mut g, KeyCode::Tab, None)?;
    if g.chat_input.as_deref() != Some("/gamemode creative") { bail!("Tab gave {:?}", g.chat_input); }
    // cycling through a list with repeated Tab
    g.chat_input = Some("/gamemode ".into()); g.update_suggestions();
    tap(&mut g, KeyCode::Tab, None)?; tap(&mut g, KeyCode::Tab, None)?;
    if g.chat_input.as_deref() != Some("/gamemode creative") { bail!("second Tab gave {:?}, expected the 2nd suggestion (creative)", g.chat_input); }
    // entity selectors (give takes players only: no @e) and registry items
    g.chat_input = Some("/give @".into()); g.update_suggestions();
    if !sug(&g).contains(&"@s".to_string()) || sug(&g).contains(&"@e".to_string()) { bail!("'/give @' suggested {:?}", sug(&g)); }
    g.chat_input = Some("/give @s sto".into()); g.update_suggestions();
    if !sug(&g).contains(&"minecraft:stone".to_string()) { bail!("'/give @s sto' suggested {:?}", &sug(&g)[..sug(&g).len().min(5)]); }
    g.chat_input = Some("/summon zomb".into()); g.update_suggestions();
    if !sug(&g).contains(&"minecraft:zombie".to_string()) { bail!("'/summon zomb' suggested {:?}", sug(&g)); }
    // ask_server: the server answers /time set <marker>
    g.chat_input = Some("/time set n".into()); g.update_suggestions();
    if !wait_until(&mut g, 3.0, |g| g.sug.list.contains(&"night".to_string()))? { bail!("server suggestions for '/time set n' never arrived: {:?}", sug(&g)); }
    g.chat_input = Some("/zzz".into()); g.update_suggestions();
    if !sug(&g).is_empty() { bail!("'/zzz' suggested {:?}", sug(&g)); }
    tap(&mut g, KeyCode::Escape, None)?;
    Ok(Outcome::Pass(format!("{nodes}-node tree; local, Tab/cycle, selectors, registries and server (ask_server) suggestions; popup {popup_px}px")))
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
    cmd(&mut g, "summon cow 700.5 -60 3 {NoAI:1b}");
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
    let logo = (0..W as i32).step_by(3).filter(|&x| { let p = px(&img, x, 100); p[0] as u32 + p[1] as u32 + p[2] as u32 > 300 }).count();
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

/// Right-click the block at `b` (placed in front of the player) and wait for the screen kind.
fn open_block(g: &mut Game, b: [i32; 3], kind: Kind) -> Result<()> {
    look_at(g, [b[0] as f64 + 0.5, b[1] as f64 + 0.5, b[2] as f64 + 0.5]);
    step(g)?;
    g.mouse_button(1, true); step(g)?; g.mouse_button(1, false);
    if !wait_until(g, 4.0, |g| container(g).map(|c| c.kind == kind && c.slots.len() == kind.slot_count()).unwrap_or(false))? { bail!("screen {kind:?} did not open"); }
    wait(g, 0.3)
}

/// Chest window: open_screen (3 rows), contents, left/right/shift clicks with the real container_click format;
/// the result is checked against the server by closing and reopening.
fn chest(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("chest")?;
    join(&mut g)?;
    cmd(&mut g, "gamemode survival");
    tp(&mut g, [320.5, -60.0, 0.5], 0.0, 0.0)?;
    cmd(&mut g, "setblock 320 -60 2 chest[facing=north]");
    cmd(&mut g, "item replace block 320 -60 2 container.0 with diamond 5");
    cmd(&mut g, "item replace block 320 -60 2 container.1 with iron_ore 10");
    cmd(&mut g, "give @s cobblestone 7");
    let (dia, iron, cobble) = (item_id("diamond"), item_id("iron_ore"), item_id("cobblestone"));
    if !wait_until(&mut g, 5.0, |g| count_item(&g.inventory, cobble) == 7 && basalt_data::BlockState(g.world.get(320, -60, 2) as u32).name() == "chest")? { bail!("setup failed"); }
    wait(&mut g, 0.3)?;
    let kind = Kind::Generic(3);
    open_block(&mut g, [320, -60, 2], kind)?;
    let img = capture(&mut g)?; save(&g, &img, "chest");
    {
        let cs = &container(&g).unwrap().slots;
        if cs[0].item != dia || cs[0].count != 5 || cs[1].item != iron || cs[1].count != 10 { bail!("chest contents on open: slot0 {:?} slot1 {:?}", cs[0], cs[1]); }
        if count_item(&cs[27..], cobble) != 7 { bail!("inventory part has {} cobblestone", count_item(&cs[27..], cobble)); }
        if container(&g).unwrap().title != crate::lang::tr("container.chest", &[]) { bail!("title {:?}", container(&g).unwrap().title); }
    }
    // pick the diamonds up and put them in slot 3
    let p = slot_center(&g, kind, 0); click_at(&mut g, p.0, p.1, 0)?;
    let p = slot_center(&g, kind, 3); click_at(&mut g, p.0, p.1, 0)?;
    // shift-click the iron ore: moves into the inventory
    g.key(KeyCode::ShiftLeft, true, None);
    let p = slot_center(&g, kind, 1); click_at(&mut g, p.0, p.1, 0)?;
    g.key(KeyCode::ShiftLeft, false, None);
    // right-click takes half of the diamonds, a second right-click places them one by one
    let p = slot_center(&g, kind, 3); click_at(&mut g, p.0, p.1, 1)?;
    let p = slot_center(&g, kind, 5); click_at(&mut g, p.0, p.1, 0)?;
    if !wait_until(&mut g, 3.0, |g| container(g).map(|c| c.slots[3].count == 2 && c.slots[5].count == 3 && c.slots[0].count == 0 && c.slots[1].count == 0 && count_item(&c.slots[27..], iron) == 10).unwrap_or(false))? {
        let cs = &container(&g).unwrap().slots;
        bail!("after clicks: slot3 {:?} slot5 {:?} slot0 {:?} slot1 {:?} iron in inventory {}", cs[3], cs[5], cs[0], cs[1], count_item(&cs[27..], iron));
    }
    tap(&mut g, KeyCode::Escape, None)?;
    if g.screen.is_some() { bail!("Esc did not close the chest"); }
    wait(&mut g, 0.5)?;
    // reopen: the server must have the same contents
    open_block(&mut g, [320, -60, 2], kind)?;
    let cs = container(&g).unwrap().slots.clone();
    if cs[3].item != dia || cs[3].count != 2 || cs[5].item != dia || cs[5].count != 3 || cs[0].count != 0 || cs[1].count != 0 { bail!("server contents after reopening: slot3 {:?} slot5 {:?}", cs[3], cs[5]); }
    if count_item(&cs[27..], iron) != 10 || count_item(&cs[27..], cobble) != 7 { bail!("inventory after reopening: iron {} cobble {}", count_item(&cs[27..], iron), count_item(&cs[27..], cobble)); }
    tap(&mut g, KeyCode::Escape, None)?;
    Ok(Outcome::Pass("chest screen (3 rows): contents from the server, pickup/place/half/shift clicks, state identical after reopening".into()))
}

/// Furnace window: slots + the flame and arrow progress sprites driven by container_set_data, taking the result.
fn furnace(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("furn")?;
    join(&mut g)?;
    cmd(&mut g, "gamemode survival");
    tp(&mut g, [340.5, -60.0, 0.5], 0.0, 0.0)?;
    cmd(&mut g, "setblock 340 -60 2 furnace[facing=north]");
    let (ore, coal, ingot) = (item_id("iron_ore"), item_id("coal"), item_id("iron_ingot"));
    if !wait_until(&mut g, 5.0, |g| basalt_data::BlockState(g.world.get(340, -60, 2) as u32).name() == "furnace")? { bail!("setup failed"); }
    let kind = Kind::Furnace(0);
    // idle window as the reference picture
    open_block(&mut g, [340, -60, 2], kind)?;
    let idle = capture(&mut g)?; save(&g, &idle, "furnace_idle");
    tap(&mut g, KeyCode::Escape, None)?;
    wait(&mut g, 0.3)?;
    cmd(&mut g, "item replace block 340 -60 2 container.0 with iron_ore 2");
    cmd(&mut g, "item replace block 340 -60 2 container.1 with coal 2");
    wait(&mut g, 1.0)?;
    open_block(&mut g, [340, -60, 2], kind)?;
    {
        let cs = &container(&g).unwrap().slots;
        if cs[0].item != ore || cs[1].item != coal { bail!("furnace slots on open: {:?} {:?}", cs[0], cs[1]); }
    }
    if !wait_until(&mut g, 4.0, |g| container(g).map(|c| c.props[1] == 1600 && c.props[3] == 200 && c.props[2] >= 20).unwrap_or(false))? { bail!("furnace properties {:?}", container(&g).map(|c| c.props)); }
    let img = capture(&mut g)?; save(&g, &img, "furnace_cooking");
    let (s, (ox, oy)) = (g.gui_scale(), g.container_origin(kind));
    let rect = |x: f32, y: f32, w: f32, h: f32| ((ox + x * s) as i32, (oy + y * s) as i32, (ox + (x + w) * s) as i32, (oy + (y + h) * s) as i32);
    let (f0, f1, f2, f3) = rect(56.0, 36.0, 14.0, 14.0);
    let flame = diff_count(&idle, &img, f0, f1, f2, f3);
    let (a0, a1, a2, a3) = rect(79.0, 34.0, 24.0, 16.0);
    let arrow = diff_count(&idle, &img, a0, a1, a2, a3);
    // the flame is lit (pixel count scales with the gui scale squared), the arrow partly filled
    let (min_flame, min_arrow) = ((s * s * 20.0) as usize, (s * s * 6.0) as usize); // at ~10% progress the arrow is only a few texels wide
    if flame < min_flame { bail!("flame sprite not drawn: {flame} differing pixels (< {min_flame})"); }
    if arrow < min_arrow { bail!("arrow sprite not drawn: {arrow} differing pixels (< {min_arrow})"); }
    // the arrow keeps growing
    let p0 = container(&g).unwrap().props[2];
    wait(&mut g, 1.0)?;
    let p1 = container(&g).unwrap().props[2];
    if p1 <= p0 { bail!("cook progress did not advance: {p0} -> {p1}"); }
    let img2 = capture(&mut g)?;
    let arrow2 = diff_count(&idle, &img2, a0, a1, a2, a3);
    if arrow2 <= arrow { bail!("arrow did not grow: {arrow} -> {arrow2} pixels"); }
    // wait for the first ingot and take it with a shift-click
    if !wait_until(&mut g, 14.0, |g| container(g).map(|c| c.slots[2].item == ingot).unwrap_or(false))? { bail!("no ingot in the result slot after 14 s (props {:?})", container(&g).map(|c| c.props)); }
    g.key(KeyCode::ShiftLeft, true, None);
    let p = slot_center(&g, kind, 2); click_at(&mut g, p.0, p.1, 0)?;
    g.key(KeyCode::ShiftLeft, false, None);
    if !wait_until(&mut g, 3.0, |g| container(g).map(|c| c.slots[2].count == 0 && count_item(&c.slots[3..], ingot) == 1).unwrap_or(false))? { bail!("ingot not moved to the inventory"); }
    // the input and fuel slots follow the server: one ore smelted, one coal burning, one left
    let cs = container(&g).unwrap().slots.clone();
    if cs[0].count != 1 || cs[1].count != 1 { bail!("input {:?} fuel {:?} after the first ingot (one coal was lit, one is left)", cs[0], cs[1]); }
    tap(&mut g, KeyCode::Escape, None)?;
    Ok(Outcome::Pass(format!("furnace screen: flame {flame} + arrow {arrow}->{arrow2} px vs idle, progress {p0}->{p1}, ingot taken by shift-click")))
}

/// Held item in first person (block model and an extruded flat item), the lower/raise animation on
/// switching, and the item name shown above the hotbar (vanilla client recording, 2026-11-23).
fn held_item(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("held")?;
    join(&mut g)?;
    cmd(&mut g, "gamemode survival");
    cmd(&mut g, "clear @s");
    cmd(&mut g, "time set noon");
    tp(&mut g, [860.5, -60.0, 860.5], 0.0, 20.0)?;
    let mut details = Vec::new();
    for item in ["oak_planks", "torch"] {
        cmd(&mut g, "clear @s");
        cmd(&mut g, &format!("give @s {item}"));
        if !wait_until(&mut g, 3.0, |g| g.hotbar[0].item == item_id(item) && g.hotbar[0].count > 0)? { bail!("no {item}"); }
        let want = crate::lang::item_name(item_id(item));
        if !wait_until(&mut g, 0.3, |g| g.held_name.1 > 0 && g.held_name.0 == want)? { bail!("item name not shown for {item}: {:?}", g.held_name); }
        if g.equip.0 >= 1.0 { bail!("no equip animation after selecting {item}"); }
        wait(&mut g, 0.5)?;
        if g.equip.0 < 1.0 { bail!("equip animation did not finish for {item}"); }
        g.fp_swing = -1;
        let with = capture(&mut g)?;
        g.hide_hand = true; let without = capture(&mut g)?; g.hide_hand = false;
        // the item sits in the lower right quarter of the view
        let (w, h) = (W as i32, H as i32);
        let inside = diff_count(&with, &without, w / 2, h / 2, w, h);
        let outside = diff_count(&with, &without, 0, 0, w / 2, h / 2);
        save(&g, &with, &format!("held_{item}"));
        if inside < 800 { bail!("{item} barely drawn in the hand ({inside} px)"); }
        if outside > inside / 10 { bail!("{item} drawn outside the lower right ({outside} px)"); }
        // the name fades out after about 2 s
        wait(&mut g, 2.3)?;
        if g.held_name.1 > 0 { bail!("item name still shown after 2.3 s"); }
        details.push(format!("{item} {inside}px"));
    }
    Ok(Outcome::Pass(details.join(", ")))
}

/// Breaking a block bursts chips of its texture that fall and vanish; digging sprays a few.
fn break_particles(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("parts")?;
    join(&mut g)?;
    cmd(&mut g, "gamemode survival");
    cmd(&mut g, "clear @s");
    tp(&mut g, [900.5, -60.0, 900.5], 0.0, 0.0)?;
    let b = [900, -60, 902];
    cmd(&mut g, &format!("setblock {} {} {} dirt", b[0], b[1], b[2]));
    if !wait_until(&mut g, 3.0, |g| basalt_data::BlockState(g.world.get(b[0], b[1], b[2]) as u32).name() == "dirt")? { bail!("setblock failed"); }
    wait(&mut g, 0.3)?;
    look_at(&mut g, [b[0] as f64 + 0.5, b[1] as f64 + 0.5, b[2] as f64]);
    step(&mut g)?;
    g.last_dig = None;
    g.mouse_button(0, true);
    let (mut dig_max, mut after) = (0usize, 0usize);
    let t0 = Instant::now();
    while g.last_dig.is_none() && t0.elapsed().as_secs_f32() < 4.0 { step(&mut g)?; dig_max = dig_max.max(g.particles.len()); }
    g.mouse_button(0, false);
    if g.last_dig.is_none() { bail!("dirt did not break"); }
    step(&mut g)?;
    after = after.max(g.particles.len());
    let img = capture(&mut g)?; save(&g, &img, "break_particles");
    if dig_max < 3 { bail!("digging sprayed {dig_max} particles"); }
    if after < 50 { bail!("break burst only {after} particles (vanilla: 4x4x4)"); }
    wait(&mut g, 2.5)?;
    if !g.particles.is_empty() { bail!("{} particles still alive after 2.5 s", g.particles.len()); }
    Ok(Outcome::Pass(format!("{dig_max} while digging, {after} on break, all gone after 2.5 s")))
}

/// Landing audio and dust: the server sends the fall sounds to other players only (measured on
/// vanilla), so the client plays small_fall / big_fall, the landed-on block's fall sound and, with the
/// damage event, the hurt sound itself, and throws block dust. 10 blocks: big_fall; 5: small_fall; 3: nothing.
fn landing(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("land")?;
    join(&mut g)?;
    cmd(&mut g, "gamemode creative");
    tp(&mut g, [1000.5, -60.0, 1020.5], 0.0, 0.0)?;
    wait(&mut g, 1.0)?;
    cmd(&mut g, "gamemode survival");
    wait(&mut g, 0.5)?;
    let mut notes = Vec::new();
    for (h, want) in [(3.0, None), (5.0, Some("entity.player.small_fall")), (10.0, Some("entity.player.big_fall"))] {
        cmd(&mut g, "effect give @s instant_health 1 5");
        wait(&mut g, 1.5)?;
        g.sounds.log.clear();
        let parts0 = g.particles.len();
        cmd(&mut g, &format!("tp @s 1000.5 {} 1020.5", -60.0 + h));
        wait(&mut g, 0.3)?;
        if !wait_until(&mut g, 4.0, |g| g.player.on_ground)? { bail!("{h}-block fall: never landed"); }
        let mut dust = 0;
        for _ in 0..10 { step(&mut g)?; dust = dust.max(g.particles.len().saturating_sub(parts0)); }
        wait(&mut g, 0.5)?;
        let ev: Vec<String> = g.sounds.log.iter().map(|p| p.event.clone()).collect();
        let fall_block = ev.iter().any(|e| e.starts_with("block.") && e.ends_with(".fall"));
        match want {
            None => { if fall_block || ev.iter().any(|e| e.contains("_fall")) { bail!("{h}-block fall played landing sounds {ev:?}"); } }
            Some(w) => {
                if !ev.iter().any(|e| e == w) { bail!("{h}-block fall: no {w} in {ev:?}"); }
                if !fall_block { bail!("{h}-block fall: no block fall sound in {ev:?}"); }
                if !ev.iter().any(|e| e == "entity.player.hurt") { bail!("{h}-block fall: no hurt sound in {ev:?}"); }
                if dust == 0 { bail!("{h}-block fall: no landing dust"); }
            }
        }
        notes.push(format!("{h}: {} sounds, {dust} dust", ev.len()));
    }
    Ok(Outcome::Pass(notes.join("; ")))
}

/// Falling into a pool: the splash sound (normal from 3 blocks, high-speed from 14) with ~13 bubbles +
/// ~13 drops, the drops gone after a second, the underwater enter sound; leaving it plays the exit sound.
fn water_splash(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("splash")?;
    join(&mut g)?;
    cmd(&mut g, "gamemode creative");
    tp(&mut g, [1000.5, -58.0, 1060.5], 0.0, 0.0)?;
    cmd(&mut g, "fill 997 -61 1057 1003 -50 1063 stone");
    cmd(&mut g, "fill 998 -60 1058 1002 -53 1062 water");
    cmd(&mut g, "fill 998 -52 1058 1002 -30 1062 air");
    wait(&mut g, 1.0)?;
    cmd(&mut g, "gamemode survival");
    wait(&mut g, 0.5)?;
    let mut notes = Vec::new();
    for (h, want) in [(3.0, "entity.player.splash"), (20.0, "entity.player.splash.high_speed")] {
        tp(&mut g, [1000.5, -52.0 + 8.0, 1060.5], 0.0, 0.0)?;
        wait(&mut g, 2.0)?;
        g.sounds.log.clear();
        let p0 = g.particles.len();
        cmd(&mut g, &format!("tp @s 1000.5 {} 1060.5", -52.0 + h));
        let mut peak = 0;
        if !wait_until(&mut g, 4.0, |g| g.player.pos[1] > -52.0 + h - 1.0)? { bail!("tp above the pool not applied"); }
        wait_until(&mut g, 2.0, |g| !g.player.in_water)?;
        if !wait_until(&mut g, 5.0, |g| g.player.in_water)? { bail!("never fell into the water: pos {:?} gm {} flying {} block {}", g.player.pos, g.player.gamemode, g.player.flying, g.world.get(1000, -53, 1060)); }
        for _ in 0..5 { step(&mut g)?; peak = peak.max(g.particles.len().saturating_sub(p0)); }
        wait(&mut g, 0.4)?;
        let ev: Vec<String> = g.sounds.log.iter().map(|p| p.event.clone()).collect();
        if !ev.iter().any(|e| e == want) { bail!("{h}-block fall into water: no {want} in {ev:?}"); }
        if peak < 20 { bail!("{h}-block fall into water: only {peak} splash particles"); }
        notes.push(format!("{h}: {want}, {peak} particles"));
    }
    wait(&mut g, 2.5)?;
    if g.particles.len() > 5 { bail!("splash particles did not die out: {}", g.particles.len()); }
    Ok(Outcome::Pass(notes.join("; ")))
}

/// Chat box keys like vanilla: Up/Down through the session history (Down past the newest brings
/// the draft back), Left/Right/Home/End with Shift selection, Ctrl word jumps, Backspace/Delete,
/// Ctrl+A/C/X/V, Tab completion, PageUp/PageDown scrolling of the open chat.
fn chat_keys(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("chat")?;
    join(&mut g)?;
    let press = |g: &mut Game, k: KeyCode| { g.key(k, true, None); g.key(k, false, None); };
    let chord = |g: &mut Game, m: KeyCode, k: KeyCode| { g.key(m, true, None); g.key(k, true, None); g.key(k, false, None); g.key(m, false, None); };
    let typed = |g: &mut Game, t: &str| { for ch in t.chars() { let s = ch.to_string(); g.key(KeyCode::KeyA, true, Some(&s)); g.key(KeyCode::KeyA, false, None); } };
    let open = |g: &mut Game| { g.key(KeyCode::KeyT, true, None); g.key(KeyCode::KeyT, false, None); };
    // send three things
    for m in ["hello one", "/time query daytime", "hello three"] { open(&mut g); typed(&mut g, m); press(&mut g, KeyCode::Enter); step(&mut g)?; }
    if g.chat_hist != ["hello one", "/time query daytime", "hello three"] { bail!("history {:?}", g.chat_hist); }
    // history: Up x2 -> the command, Down -> newest, Down -> the draft
    open(&mut g); typed(&mut g, "draft");
    press(&mut g, KeyCode::ArrowUp);
    if g.chat_input.as_deref() != Some("hello three") { bail!("Up: {:?}", g.chat_input); }
    press(&mut g, KeyCode::ArrowUp);
    if g.chat_input.as_deref() != Some("/time query daytime") { bail!("Up Up: {:?}", g.chat_input); }
    press(&mut g, KeyCode::ArrowDown); press(&mut g, KeyCode::ArrowDown);
    if g.chat_input.as_deref() != Some("draft") { bail!("Down past the newest should restore the draft, got {:?}", g.chat_input); }
    // editing: Home, insert, End; Shift+Left x3 then Ctrl+X, Ctrl+V at the start
    g.chat_input = Some("abc def".into()); g.chat_cursor = None; g.chat_sel = None;
    press(&mut g, KeyCode::Home); typed(&mut g, "X");
    if g.chat_input.as_deref() != Some("Xabc def") { bail!("Home+type: {:?}", g.chat_input); }
    press(&mut g, KeyCode::End);
    for _ in 0..3 { chord(&mut g, KeyCode::ShiftLeft, KeyCode::ArrowLeft); }
    chord(&mut g, KeyCode::ControlLeft, KeyCode::KeyX);
    if g.chat_input.as_deref() != Some("Xabc ") { bail!("Shift+Left x3, Ctrl+X: {:?}", g.chat_input); }
    press(&mut g, KeyCode::Home); chord(&mut g, KeyCode::ControlLeft, KeyCode::KeyV);
    if g.chat_input.as_deref() != Some("defXabc ") { bail!("Ctrl+V at the start: {:?}", g.chat_input); }
    // Ctrl+Right jumps a word, Backspace deletes before the cursor, Delete after
    press(&mut g, KeyCode::Home); chord(&mut g, KeyCode::ControlLeft, KeyCode::ArrowRight); press(&mut g, KeyCode::Backspace);
    if g.chat_input.as_deref() != Some("defXabc") { bail!("Ctrl+Right then Backspace: {:?}", g.chat_input); }
    press(&mut g, KeyCode::Home); press(&mut g, KeyCode::Delete);
    if g.chat_input.as_deref() != Some("efXabc") { bail!("Delete at the start: {:?}", g.chat_input); }
    // Ctrl+A then typing replaces everything; Ctrl+C keeps the text
    chord(&mut g, KeyCode::ControlLeft, KeyCode::KeyA); chord(&mut g, KeyCode::ControlLeft, KeyCode::KeyC);
    if g.chat_input.as_deref() != Some("efXabc") { bail!("Ctrl+C changed the text: {:?}", g.chat_input); }
    typed(&mut g, "z");
    if g.chat_input.as_deref() != Some("z") { bail!("typing over Ctrl+A: {:?}", g.chat_input); }
    // Tab completion still works
    g.chat_input = Some("/gamem".into()); g.chat_cursor = None; g.update_suggestions();
    wait_until(&mut g, 2.0, |g| !g.sug.list.is_empty())?;
    press(&mut g, KeyCode::Tab);
    if g.chat_input.as_deref() != Some("/gamemode") { bail!("Tab: {:?}", g.chat_input); }
    press(&mut g, KeyCode::Escape);
    // PageUp/PageDown scroll the open chat (needs more lines than fit)
    for i in 0..30 { g.chat.push((format!("line {i}"), Instant::now())); }
    open(&mut g); press(&mut g, KeyCode::PageUp);
    if g.chat_scroll == 0 { bail!("PageUp did not scroll"); }
    press(&mut g, KeyCode::PageDown);
    if g.chat_scroll != 0 { bail!("PageDown did not scroll back ({})", g.chat_scroll); }
    press(&mut g, KeyCode::Escape);
    Ok(Outcome::Pass("history (incl. draft), Home/End, Shift selection, Ctrl word jumps, Backspace/Delete, Ctrl+A/C/X/V, Tab, PageUp/PageDown".into()))
}

/// Picking an item up: take_item_entity makes the item fly into the player over 3 ticks (a copy is
/// drawn after the server removed the entity), the pickup sound plays, and the hotbar slot that
/// received it pops for 5 ticks.
fn pickup_anim(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("pick")?;
    join(&mut g)?;
    cmd(&mut g, "gamemode creative");
    tp(&mut g, [980.5, -60.0, 980.5], 0.0, 30.0)?;
    wait(&mut g, 1.0)?;
    cmd(&mut g, "gamemode survival");
    cmd(&mut g, "clear @s");
    wait(&mut g, 0.3)?;
    let b = [980, -60, 981];
    cmd(&mut g, &format!("setblock {} {} {} dirt", b[0], b[1], b[2]));
    if !wait_until(&mut g, 3.0, |g| basalt_data::BlockState(g.world.get(b[0], b[1], b[2]) as u32).name() == "dirt")? { bail!("setblock dirt failed"); }
    look_at(&mut g, [b[0] as f64 + 0.5, b[1] as f64 + 0.5, b[2] as f64 + 0.5]);
    step(&mut g)?;
    g.mouse_button(0, true);
    let broke = wait_until(&mut g, 3.0, |g| basalt_data::BlockState(g.world.get(b[0], b[1], b[2]) as u32).is_air())?;
    g.mouse_button(0, false);
    if !broke { bail!("dirt did not break"); }
    let sounds0 = g.sounds.log.len();
    if !wait_until(&mut g, 3.0, |g| !g.pickups.is_empty())? { bail!("no pickup animation started (take_item_entity)"); }
    // follow the flying copy: it must move towards the player
    let me = g.player.pos;
    let dist = |g: &Game| g.pickups.first().map(|(e, _, age)| { let t = (*age as f64 / 3.0).min(1.0); let p: Vec<f64> = (0..3).map(|i| e.pos[i] + ([me[0], me[1] + 0.5, me[2]][i] - e.pos[i]) * t).collect(); ((p[0] - me[0]).powi(2) + (p[1] - me[1] - 0.5).powi(2) + (p[2] - me[2]).powi(2)).sqrt() }).unwrap_or(-1.0);
    let d0 = dist(&g);
    ticks(&mut g, 1)?;
    let d1 = dist(&g);
    if d1 >= 0.0 && d1 >= d0 { bail!("pickup copy not moving towards the player ({d0:.2} -> {d1:.2})"); }
    let popped = wait_until(&mut g, 1.0, |g| g.pop.iter().any(|p| *p > 0))?;
    if !popped { bail!("hotbar slot did not pop after the pickup"); }
    let slot = g.pop.iter().position(|p| *p > 0).unwrap_or(0);
    wait(&mut g, 0.5)?;
    if !g.pickups.is_empty() { bail!("pickup animation still running after 0.5 s (3 ticks long)"); }
    if g.pop.iter().any(|p| *p > 0) { bail!("pop timer still running after 0.5 s (5 ticks)"); }
    let heard = g.sounds.log[sounds0.min(g.sounds.log.len())..].iter().any(|p| p.event == "entity.item.pickup");
    if !heard { bail!("no entity.item.pickup sound"); }
    let got = g.hotbar[slot].item == item_id("dirt");
    if !got { bail!("popped slot {slot} doesn't hold the dirt"); }
    Ok(Outcome::Pass(format!("item flew in ({d0:.2} -> {d1:.2} from the player), slot {slot} popped, pickup sound played")))
}

/// Dropped items must not teleport: a Q-drop and a block drop are followed every tick for 1.5 s;
/// the item starts where add_entity put it and never moves more in one tick than its own speed
/// plus 0.1 (a jump to a server sync position after hanging still was the bug).
fn item_drop_smooth(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("drop")?;
    join(&mut g)?;
    cmd(&mut g, "gamemode creative");
    tp(&mut g, [960.5, -60.0, 960.5], 0.0, 10.0)?;
    wait(&mut g, 1.0)?;
    cmd(&mut g, "gamemode survival");
    cmd(&mut g, "clear @s");
    cmd(&mut g, "give @s cobblestone 4");
    wait(&mut g, 0.5)?;
    let snapshot = |g: &Game| -> std::collections::HashSet<i32> { g.entities.keys().copied().collect() };
    let follow = |g: &mut Game, label: &str, before: std::collections::HashSet<i32>| -> Result<String> {
        let mut id = None;
        for _ in 0..40 {
            if let Some(e) = g.entities.values().find(|e| crate::entity::type_name(e.kind) == "item" && !before.contains(&e.id)) { id = Some(e.id); break; }
            step(g)?;
        }
        let Some(id) = id else { bail!("{label}: no item entity appeared") };
        let (mut last, mut worst) = (g.entities[&id].pos, 0.0f64);
        let spawn = g.entities[&id].server_pos;
        let d0: f64 = (0..3).map(|i| (last[i] - spawn[i]).powi(2)).sum::<f64>().sqrt();
        if d0 > 0.35 { bail!("{label}: item appeared {d0:.2} from its spawn position"); }
        for t in 0..30 {
            let speed = { let e = &g.entities[&id]; (e.vel[0] * e.vel[0] + e.vel[1] * e.vel[1] + e.vel[2] * e.vel[2]).sqrt() };
            step(g)?;
            let Some(e) = g.entities.get(&id) else { break };
            let d: f64 = (0..3).map(|i| (e.pos[i] - last[i]).powi(2)).sum::<f64>().sqrt();
            let jump = d - speed;
            worst = worst.max(jump);
            if jump > 0.1 { bail!("{label}: tick {t}: moved {d:.3} with speed {speed:.3} (jump {jump:.3})"); }
            last = e.pos;
        }
        Ok(format!("{label} worst excess {worst:.3}"))
    };
    let before = snapshot(&g);
    tap(&mut g, KeyCode::KeyQ, None)?;
    let a = follow(&mut g, "Q-drop", before)?;
    let b = [961, -60, 963];
    cmd(&mut g, &format!("setblock {} {} {} dirt", b[0], b[1], b[2]));
    if !wait_until(&mut g, 3.0, |g| basalt_data::BlockState(g.world.get(b[0], b[1], b[2]) as u32).name() == "dirt")? { bail!("setblock dirt failed"); }
    // survival digging by hand (dirt: 15 ticks), so the drop takes the block-break path
    look_at(&mut g, [b[0] as f64 + 0.5, b[1] as f64 + 0.5, b[2] as f64]);
    step(&mut g)?;
    let before = snapshot(&g);
    g.mouse_button(0, true);
    let broke = wait_until(&mut g, 3.0, |g| basalt_data::BlockState(g.world.get(b[0], b[1], b[2]) as u32).is_air())?;
    g.mouse_button(0, false);
    if !broke { bail!("dirt did not break"); }
    let bd = follow(&mut g, "block drop", before)?;
    Ok(Outcome::Pass(format!("{a}; {bd}")))
}

/// Grass side flicker (z-fighting): the grass_block side overlay is a second, coplanar quad in the
/// cutout layer over the opaque side. It must have exactly the base quad's vertex positions (the
/// chunk shader marks the clip position @invariant so both pipelines produce the same depth, and
/// LessEqual lets the later overlay win). Short grass / ferns must be cutout, not translucent
/// (translucent sorting would make them flicker).
fn grass_overlay(c: &Ctx) -> Result<Outcome> {
    use crate::mesh::{self, QuadTrace, SectionMesh, Snapshot};
    let a = &c.assets;
    let biome = crate::assets::Biome { name: "scene".into(), grass: 0x91BD59, foliage: 0x77AB2F, dry_foliage: 0xA0784D, water: 0x3F76E4, sky: 0x78A7FF, fog: 0xC0D8FF };
    let mesh_one = |state: &str| -> Result<Vec<QuadTrace>> {
        let st = basalt_data::BlockState::parse(state).ok_or_else(|| anyhow!("unknown {state}"))?;
        let mut blocks = Box::new([0u16; 5832]);
        blocks[mesh::sidx(8, 8, 8)] = st.0 as u16;
        let light = Box::new([0xF0u8; 5832]);
        let snap = Snapshot { origin: [0, 56, 0], blocks, light, biomes: [0; 64] };
        let mut out = SectionMesh::default();
        let mut trace = Vec::new();
        mesh::mesh_block(a, std::slice::from_ref(&biome), &snap, 8, 8, 8, &mut out, Some(&mut trace));
        Ok(trace)
    };
    let q = mesh_one("grass_block[snowy=false]")?;
    let overlays: Vec<&QuadTrace> = q.iter().filter(|t| t.layer == crate::assets::LAYER_CUTOUT).collect();
    if overlays.len() != 4 { bail!("grass_block: {} cutout overlay quads, want 4 (one per side)", overlays.len()); }
    for o in &overlays {
        let base = q.iter().find(|b| b.layer == crate::assets::LAYER_OPAQUE && b.dir == o.dir);
        let Some(b) = base else { bail!("grass_block: no opaque side quad under the overlay on face {}", o.dir) };
        let mut bp: Vec<[f32; 3]> = b.verts.iter().map(|v| v.pos).collect();
        let mut op: Vec<[f32; 3]> = o.verts.iter().map(|v| v.pos).collect();
        bp.sort_by(|x, y| x.partial_cmp(y).unwrap());
        op.sort_by(|x, y| x.partial_cmp(y).unwrap());
        if bp != op { bail!("grass_block face {}: overlay vertices {op:?} differ from the side {bp:?} (z-fighting)", o.dir); }
    }
    let mut notes = vec!["grass_block: 4 overlay quads exactly coplanar with the sides".to_string()];
    for plant in ["short_grass", "fern", "tall_grass[half=lower]"] {
        let q = mesh_one(plant)?;
        if q.iter().any(|t| t.layer == crate::assets::LAYER_TRANSLUCENT) { bail!("{plant} has translucent quads (sorting flicker)"); }
        notes.push(format!("{plant}: {} cutout quads", q.len()));
    }
    for other in ["grass_block[snowy=true]", "mycelium", "podzol", "dirt_path"] {
        let q = mesh_one(other)?;
        let cut = q.iter().filter(|t| t.layer == crate::assets::LAYER_CUTOUT).count();
        notes.push(format!("{other}: {} quads, {cut} cutout", q.len()));
    }
    let shader = include_str!("shader.wgsl");
    if !shader.contains("@builtin(position) @invariant clip") { bail!("chunk shader position is not @invariant"); }
    Ok(Outcome::Pass(notes.join("; ")))
}

/// Per-glyph advances like vanilla's bitmap provider (rightmost opaque column + 1, +1 spacing,
/// space 4) on the ascii page — and every ASCII glyph present, so a full UI atlas can't silently
/// fall back to a fixed advance again ("Si ngl epl ayer"). Expected widths computed from the
/// jar's font/ascii.png with that rule (2026-11-24).
fn font_widths(c: &Ctx) -> Result<Outcome> {
    let g = c.game_at("font", None)?;
    for ch in (0x21u8..0x7f).map(|b| b as char) {
        if !g.ui.has_glyph(ch) { bail!("no glyph for {ch:?}: the ascii font page is missing from the UI atlas"); }
    }
    let want = [("i", 2.0), ("l", 3.0), ("t", 4.0), ("!", 2.0), (".", 2.0), ("W", 6.0), (" ", 4.0), ("Singleplayer", 62.0), ("Multiplayer", 54.0), ("Options...", 42.0), ("Minecraft Realms", 84.0), ("Quit Game", 46.0)];
    let mut bad = Vec::new();
    for (t, w) in want {
        let got = g.ui.text_width(t, 1.0);
        if (got - w).abs() > 0.01 { bad.push(format!("{t:?} {got} (want {w})")); }
    }
    if !bad.is_empty() { bail!("text widths differ from vanilla: {}", bad.join(", ")); }
    Ok(Outcome::Pass(format!("{} widths match vanilla's bitmap rule; all 94 ASCII glyphs present", want.len())))
}

/// Title + pause + every Options page renders and works: icons/tooltips, the vanilla options
/// layout, toggles/cycles/sliders stored in the config, category volumes applied, scrolling lists.
fn options_pages(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game_at("opts", None)?;
    wait(&mut g, 0.2)?;
    // non-ASCII text uses the jar's accented / nonlatin font pages
    for ch in ['ñ', 'é', 'Ж', 'λ'] { if !g.ui.has_glyph(ch) { bail!("no glyph for {ch:?} (font pages not loaded)"); } }
    let img = capture(&mut g)?; save(&g, &img, "title_full");
    press_widget(&mut g, "options")?;
    if !matches!(g.screen, Some(Screen::Options { .. })) { bail!("Options did not open from the title"); }
    let img = capture(&mut g)?; save(&g, &img, "options_main");
    let mut seen = Vec::new();
    for (id, name) in [("page:skin", "skin"), ("page:sounds", "sounds"), ("page:video", "video"), ("page:controls", "controls"), ("page:language", "language"),
        ("page:chat", "chat"), ("page:packs", "packs"), ("page:accessibility", "accessibility"), ("page:credits", "credits"), ("page:online", "online")] {
        press_widget(&mut g, id)?;
        if !matches!(g.screen, Some(Screen::Page { .. })) { bail!("{id} did not open a page"); }
        let n = g.widgets().len();
        if n < 3 { bail!("{name} page has only {n} widgets"); }
        let img = capture(&mut g)?; save(&g, &img, &format!("page_{name}"));
        seen.push(format!("{name}:{n}"));
        press_widget(&mut g, "done")?;
        if !matches!(g.screen, Some(Screen::Options { .. })) { bail!("Done on {name} did not return to Options"); }
    }
    // a toggle persists
    press_widget(&mut g, "page:skin")?;
    let before = crate::options::value(&g.cfg, &crate::options::opts(crate::options::Page::Skin)[0]);
    press_widget(&mut g, "opt:cape")?;
    let after = crate::options::value(&g.cfg, &crate::options::opts(crate::options::Page::Skin)[0]);
    if before == after { bail!("Cape toggle did not change"); }
    if Config::load().extra.get("cape").copied() != Some(after) { bail!("Cape toggle not saved"); }
    press_widget(&mut g, "opt:cape")?;
    press_widget(&mut g, "done")?;
    // a category slider changes the gain of that category's sounds
    press_widget(&mut g, "page:sounds")?;
    let blocks = g.widgets().into_iter().find(|w| w.id == "opt:vol.block").ok_or_else(|| anyhow!("no Blocks volume slider"))?;
    let s = g.gui_scale();
    click_at(&mut g, (blocks.x + 4.0) * s, (blocks.y + 10.0) * s, 0)?;
    g.mouse_button(0, false);
    let vol = g.sounds.categories.get("block").copied().unwrap_or(1.0);
    if vol > 0.05 { bail!("Blocks volume slider at the left edge left the category at {vol}"); }
    set_opt_back(&mut g, "vol.block");
    press_widget(&mut g, "done")?;
    // the video page scrolls
    press_widget(&mut g, "page:video")?;
    let first = g.widgets().iter().filter(|w| w.id.starts_with("opt:")).map(|w| w.id.clone()).next();
    g.scroll(-5.0);
    let after_scroll = g.widgets().iter().filter(|w| w.id.starts_with("opt:")).map(|w| w.id.clone()).next();
    if first == after_scroll { bail!("video settings list did not scroll"); }
    press_widget(&mut g, "done")?;
    Ok(Outcome::Pass(format!("pages {}; toggle saved; block volume applied; video list scrolls", seen.join(" "))))
}

fn set_opt_back(g: &mut Game, id: &str) { g.set_opt(id, 1.0); }

/// Multiplayer list pings: MOTD / players / ping bars for a live server, the error line for a
/// closed port, the vanilla server's status when reachable, Refresh re-pings.
fn server_list(c: &Ctx) -> Result<Outcome> {
    use crate::menu::ServerEntry;
    use crate::status::PingState;
    let mut g = c.game_at("slist", None)?;
    wait(&mut g, 0.2)?;
    let dead = "127.0.0.1:25798".to_string();
    g.cfg.servers = vec![ServerEntry { name: "Basalt test".into(), addr: c.args.addr.clone() }, ServerEntry { name: "Closed port".into(), addr: dead.clone() }];
    let van = vanilla_addr(c);
    if let Some(v) = &van { g.cfg.servers.push(ServerEntry { name: "Vanilla".into(), addr: v.clone() }); }
    press_widget(&mut g, "multiplayer")?;
    let done = |g: &Game| g.cfg.servers.iter().all(|s| !matches!(g.pings.get(&s.addr), Some(PingState::Pinging(_)) | None));
    if !wait_until(&mut g, 8.0, done)? { bail!("pings did not finish"); }
    let img = capture(&mut g)?; save(&g, &img, "server_list_pings");
    let Some(PingState::Done(st)) = g.pings.get(&c.args.addr) else { bail!("Basalt server ping failed: {:?}", g.pings.get(&c.args.addr).map(|p| matches!(p, PingState::Failed(_)))) };
    if st.protocol != basalt_protocol::PROTOCOL_VERSION as i64 || st.max <= 0 { bail!("bad status: protocol {} max {}", st.protocol, st.max); }
    let basalt = format!("{}/{} players, {} ms, motd {:?}", st.online, st.max, st.ping_ms, st.motd.iter().map(|l| l.iter().map(|r| r.0.as_str()).collect::<String>()).collect::<Vec<_>>());
    if !matches!(g.pings.get(&dead), Some(PingState::Failed(_))) { bail!("closed port did not fail"); }
    let vanilla = match van.as_ref().and_then(|v| g.pings.get(v)) { Some(PingState::Done(st)) => format!("vanilla {} {}/{}", st.version, st.online, st.max), Some(_) => bail!("vanilla server ping failed"), None => "vanilla skipped".into() };
    let rounds = g.ping_rounds;
    press_widget(&mut g, "refresh")?;
    if g.ping_rounds != rounds + 1 { bail!("Refresh did not re-ping"); }
    if !wait_until(&mut g, 8.0, done)? { bail!("refresh pings did not finish"); }
    Ok(Outcome::Pass(format!("{basalt}; closed port fails; {vanilla}; refresh ok")))
}

/// Singleplayer through the integrated server: create a world (superflat), play, change a block,
/// save and quit (icon taken, server stopped), play again (block kept), rename, back up, delete.
fn singleplayer(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game_at("sp", None)?;
    wait(&mut g, 0.2)?;
    for w in crate::worlds::list() { let _ = crate::worlds::delete(&w.dir); }
    press_widget(&mut g, "singleplayer")?;
    if !matches!(g.screen, Some(Screen::CreateWorld { .. })) { bail!("no worlds should open Create New World"); }
    for _ in 0..20 { tap(&mut g, KeyCode::Backspace, None)?; }
    type_text(&mut g, "Test World")?;
    press_widget(&mut g, "worldtype")?;
    let img = capture(&mut g)?; save(&g, &img, "sp_create");
    press_widget(&mut g, "create")?;
    let t0 = Instant::now();
    if !wait_until(&mut g, 60.0, |g| g.spawned && g.screen.is_none())? { bail!("did not join the integrated server (screen {:?})", g.screen.as_ref().map(|_| ())); }
    let join = t0.elapsed().as_secs_f32();
    wait_until(&mut g, 10.0, settled)?;
    let p = g.player.pos;
    let b = [p[0].floor() as i32 + 2, p[1].floor() as i32, p[2].floor() as i32];
    cmd(&mut g, &format!("setblock {} {} {} gold_block", b[0], b[1], b[2]));
    if !wait_until(&mut g, 3.0, |g| basalt_data::BlockState(g.world.get(b[0], b[1], b[2]) as u32).name() == "gold_block")? { bail!("setblock in singleplayer failed"); }
    tap(&mut g, KeyCode::Escape, None)?;
    if !g.widgets().iter().any(|w| w.id == "lan") { bail!("singleplayer pause menu has no Open to LAN"); }
    let img = capture(&mut g)?; save(&g, &img, "sp_pause");
    press_widget(&mut g, "disconnect")?;
    if g.integrated.is_some() || g.connected || !matches!(g.screen, Some(Screen::Title)) { bail!("Save and Quit did not stop the integrated server and return to the title"); }
    let worlds = crate::worlds::list();
    let Some(w) = worlds.first() else { bail!("world not listed after quitting") };
    if w.meta.name != "Test World" || !w.meta.flat { bail!("world meta wrong: {:?}", w.meta); }
    if !w.dir.join("icon.png").exists() { bail!("no world icon written"); }
    // play again: the block is still there
    press_widget(&mut g, "singleplayer")?;
    if !matches!(g.screen, Some(Screen::Worlds { .. })) { bail!("world list did not open"); }
    let img = capture(&mut g)?; save(&g, &img, "sp_list");
    press_widget(&mut g, "world0")?;
    press_widget(&mut g, "play")?;
    if !wait_until(&mut g, 60.0, |g| g.spawned && g.screen.is_none())? { bail!("did not rejoin the world"); }
    wait_until(&mut g, 10.0, settled)?;
    if !wait_until(&mut g, 5.0, |g| basalt_data::BlockState(g.world.get(b[0], b[1], b[2]) as u32).name() == "gold_block")? { bail!("block not saved across restart"); }
    tap(&mut g, KeyCode::Escape, None)?;
    press_widget(&mut g, "disconnect")?;
    // rename, back up, delete
    press_widget(&mut g, "singleplayer")?;
    press_widget(&mut g, "world0")?;
    press_widget(&mut g, "edit")?;
    for _ in 0..20 { tap(&mut g, KeyCode::Backspace, None)?; }
    type_text(&mut g, "Renamed")?;
    press_widget(&mut g, "backup")?;
    press_widget(&mut g, "save")?;
    if crate::worlds::list().first().map(|w| w.meta.name.as_str()) != Some("Renamed") { bail!("rename not saved"); }
    if std::fs::read_dir(crate::worlds::backups_dir()).map(|d| d.count()).unwrap_or(0) == 0 { bail!("no backup made"); }
    press_widget(&mut g, "world0")?;
    press_widget(&mut g, "delete")?;
    press_widget(&mut g, "confirm")?;
    if !crate::worlds::list().is_empty() { bail!("world not deleted"); }
    Ok(Outcome::Pass(format!("created + joined in {join:.1}s, block kept across save/quit/rejoin, icon written, rename/backup/delete ok")))
}

/// A falling sand block is drawn with the sand model while it falls, then becomes the block.
fn falling_block(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("fall")?;
    join(&mut g)?;
    cmd(&mut g, "gamemode creative");
    cmd(&mut g, "time set noon");
    tp(&mut g, [940.5, -60.0, 940.5], 0.0, -20.0)?;
    wait(&mut g, 1.0)?;
    let b = [940, -48, 945];
    cmd(&mut g, &format!("setblock {} {} {} sand", b[0], b[1], b[2]));
    let sand = basalt_data::BlockState::from_name("sand").map(|s| s.0 as i32).unwrap_or(-1);
    if !wait_until(&mut g, 3.0, |g| g.entities.values().any(|e| crate::entity::type_name(e.kind) == "falling_block" && e.data == sand))? { bail!("no falling_block entity with the sand state"); }
    let p = g.entities.values().find(|e| crate::entity::type_name(e.kind) == "falling_block").map(|e| e.pos).unwrap_or([0.0; 3]);
    look_at(&mut g, [p[0], p[1] - 1.0, p[2]]);
    step(&mut g)?;
    let with = capture(&mut g)?;
    g.hide_entities = true; let without = capture(&mut g)?; g.hide_entities = false;
    let px = diff_count(&with, &without, 0, 0, W as i32, H as i32);
    save(&g, &with, "falling_block");
    if px < 100 { bail!("falling block barely drawn ({px} px)"); }
    if !wait_until(&mut g, 4.0, |g| basalt_data::BlockState(g.world.get(b[0], -60, b[2]) as u32).name() == "sand")? { bail!("sand did not land"); }
    Ok(Outcome::Pass(format!("falling sand drawn ({px} px), landed as a block")))
}

fn hold_key(g: &mut Game, k: KeyCode, down: bool) { g.key(k, down, None); }
fn ticks(g: &mut Game, n: u64) -> Result<()> { let t = g.ticks; while g.ticks < t + n { step(g)?; } Ok(()) }

/// Sprint triggers and stops like the vanilla client (packet order checked against the real
/// client by `basalt-check client-input`): double-tap forward, the sprint key in hold and toggle
/// mode, a sprint outliving the key, stops on releasing forward / hunger <= 6 / a wall / item use;
/// sneaking does not stop it; double-tap jump toggles flying in creative.
fn sprint(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("sprint")?;
    join(&mut g)?;
    cmd(&mut g, "gamemode survival");
    cmd(&mut g, "clear @s");
    tp(&mut g, [980.5, -60.0, 980.5], 0.0, 0.0)?;
    wait(&mut g, 1.0)?;
    let w = KeyCode::KeyW;
    // double tap within 7 ticks
    hold_key(&mut g, w, true); ticks(&mut g, 2)?; hold_key(&mut g, w, false); ticks(&mut g, 2)?; hold_key(&mut g, w, true); ticks(&mut g, 2)?;
    if !g.sprinting { bail!("double-tap forward did not start sprinting"); }
    hold_key(&mut g, KeyCode::ShiftLeft, true); ticks(&mut g, 3)?;
    if !g.sprinting { bail!("sneaking ended the sprint (vanilla 26.2 keeps it)"); }
    hold_key(&mut g, KeyCode::ShiftLeft, false);
    hold_key(&mut g, w, false); ticks(&mut g, 2)?;
    if g.sprinting { bail!("releasing forward did not stop sprinting"); }
    // a slow double tap (more than 7 ticks apart) does not sprint
    hold_key(&mut g, w, true); ticks(&mut g, 1)?; hold_key(&mut g, w, false); ticks(&mut g, 9)?; hold_key(&mut g, w, true); ticks(&mut g, 2)?;
    if g.sprinting { bail!("double tap 9 ticks apart started sprinting"); }
    // sprint key (hold mode): the sprint outlives the key
    hold_key(&mut g, KeyCode::ControlLeft, true); ticks(&mut g, 2)?;
    if !g.sprinting { bail!("sprint key did not start sprinting"); }
    hold_key(&mut g, KeyCode::ControlLeft, false); ticks(&mut g, 3)?;
    if !g.sprinting { bail!("releasing the sprint key stopped the sprint"); }
    // hunger
    g.food = 6; ticks(&mut g, 2)?;
    if g.sprinting { bail!("food 6 did not stop sprinting"); }
    hold_key(&mut g, KeyCode::ControlLeft, true); ticks(&mut g, 2)?;
    if g.sprinting { bail!("sprinted with food 6"); }
    hold_key(&mut g, KeyCode::ControlLeft, false);
    g.food = 20; ticks(&mut g, 2)?;
    // a wall ahead stops it
    hold_key(&mut g, KeyCode::KeyW, false);
    tp(&mut g, [980.5, -60.0, 980.5], 0.0, 0.0)?;
    hold_key(&mut g, KeyCode::KeyW, true);
    cmd(&mut g, "fill 979 -60 983 982 -58 983 stone");
    wait_until(&mut g, 3.0, |g| g.world.get(980, -60, 983) != 0)?;
    hold_key(&mut g, KeyCode::ControlLeft, true); ticks(&mut g, 1)?; hold_key(&mut g, KeyCode::ControlLeft, false);
    let hit = wait_until(&mut g, 3.0, |g| g.player.h_collide)?;
    ticks(&mut g, 2)?;
    if !hit || g.sprinting { bail!("running into a wall did not stop sprinting (collided {hit})"); }
    hold_key(&mut g, w, false); ticks(&mut g, 2)?;
    // toggle mode: a tap of the key latches sprinting on
    g.cfg.extra.insert("toggle_sprint".into(), 0.0);
    tp(&mut g, [990.5, -60.0, 980.5], 0.0, 0.0)?;
    hold_key(&mut g, KeyCode::ControlLeft, true); ticks(&mut g, 1)?; hold_key(&mut g, KeyCode::ControlLeft, false); ticks(&mut g, 1)?;
    hold_key(&mut g, w, true); ticks(&mut g, 2)?;
    if !g.sprinting { bail!("toggle mode: tapping the sprint key did not latch sprinting"); }
    hold_key(&mut g, w, false); ticks(&mut g, 2)?;
    g.cfg.extra.remove("toggle_sprint");
    // creative: double-tap jump toggles flying
    cmd(&mut g, "gamemode creative");
    wait_until(&mut g, 3.0, |g| g.player.gamemode == 1)?;
    let before = g.player.flying;
    hold_key(&mut g, KeyCode::Space, true); ticks(&mut g, 1)?; hold_key(&mut g, KeyCode::Space, false); ticks(&mut g, 2)?;
    hold_key(&mut g, KeyCode::Space, true); ticks(&mut g, 1)?; hold_key(&mut g, KeyCode::Space, false); ticks(&mut g, 1)?;
    if g.player.flying == before { bail!("double-tap jump did not toggle flying"); }
    Ok(Outcome::Pass("double-tap, slow double-tap, key hold/outlive/toggle, food 6, wall, sneak keeps sprint, double-tap fly".into()))
}

/// Respawn into the Nether (`execute in minecraft:the_nether run tp`): the world is rebuilt with the
/// nether layout (y 0..256), entities of the old dimension are gone, there is no blue sky (the
/// background is the biome's fog colour), and back in the overworld the sky is blue again.
fn nether(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("nth")?;
    join(&mut g)?;
    cmd(&mut g, "gamemode creative");
    cmd(&mut g, "summon minecraft:pig ~2 ~ ~");
    wait(&mut g, 1.0)?;
    let ents_before = g.entities.len();
    cmd(&mut g, "execute in minecraft:the_nether run tp @s 8.5 100 8.5");
    if !wait_until(&mut g, 10.0, |g| g.world.min_y == 0 && g.world.nsections() == 16)? { bail!("no nether layout (min_y {}, sections {})", g.world.min_y, g.world.nsections()); }
    if !wait_until(&mut g, 30.0, settled)? { bail!("nether chunks never settled ({} loaded)", g.world.chunks.len()); }
    let dim = g.dim.as_ref().map(|d| d.name.clone()).unwrap_or_default();
    if dim != "the_nether" { bail!("dimension type {dim}"); }
    // the overworld pig is gone (entities are cleared on respawn)
    if ents_before > 0 && g.entities.len() >= ents_before { bail!("{} entities kept from the overworld", g.entities.len()); }
    // look up: no sky, the frame top is the fog colour (reddish, never blue)
    tp(&mut g, [8.5, 100.0, 8.5], 0.0, -10.0)?;
    wait(&mut g, 1.0)?;
    let img = capture(&mut g)?; save(&g, &img, "nether");
    let top = px(&img, W as i32 / 2, 4);
    if top[2] > top[0] { bail!("nether top pixel {top:?} is blue"); }
    cmd(&mut g, "execute in minecraft:overworld run tp @s 0.5 -50 0.5");
    if !wait_until(&mut g, 10.0, |g| g.world.min_y == -64)? { bail!("no overworld layout after going back"); }
    if !wait_until(&mut g, 30.0, settled)? { bail!("overworld chunks never settled"); }
    cmd(&mut g, "time set noon");
    tp(&mut g, [0.5, -50.0, 0.5], 0.0, -30.0)?;
    wait(&mut g, 1.0)?;
    let img2 = capture(&mut g)?; save(&g, &img2, "nether_back");
    let sky = px(&img2, W as i32 / 2, 4);
    if sky[2] <= sky[0] { bail!("overworld sky {sky:?} not blue after returning"); }
    Ok(Outcome::Pass(format!("nether top {top:?}, overworld sky {sky:?}, entities {ents_before} -> cleared")))
}

/// set_experience drives the XP bar and the green level number above the hotbar.
fn xp_bar(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("xpbar")?;
    join(&mut g)?;
    cmd(&mut g, "gamemode survival");
    cmd(&mut g, "xp set @s 0 levels");
    cmd(&mut g, "xp set @s 0 points");
    wait(&mut g, 0.5)?;
    let before = capture(&mut g)?;
    cmd(&mut g, "xp add @s 20 points");
    if !wait_until(&mut g, 3.0, |g| g.xp.1 == 2)? { bail!("level after 20 points: {:?} (vanilla 2)", g.xp); }
    wait(&mut g, 0.2)?;
    let after = capture(&mut g)?;
    save(&g, &after, "xp_bar");
    let s = g.gui_scale() as i32;
    let (w, h) = (W as i32, H as i32);
    let bar = diff_count(&before, &after, w / 2 - 91 * s, h - 29 * s, w / 2 + 91 * s, h - 24 * s);
    let num = diff_count(&before, &after, w / 2 - 10 * s, h - 37 * s, w / 2 + 10 * s, h - 28 * s);
    if bar < 20 { bail!("XP bar fill not drawn ({bar} px)"); }
    if num < 10 { bail!("level number not drawn ({num} px)"); }
    Ok(Outcome::Pass(format!("level {} progress {:.2}; bar {bar}px, number {num}px", g.xp.1, g.xp.0)))
}

/// /title title|subtitle|actionbar draw big centred text and the action bar line, then fade out.
fn titles(c: &Ctx) -> Result<Outcome> {
    let mut g = c.game("titles")?;
    join(&mut g)?;
    cmd(&mut g, "title @s times 0 20 0");
    wait(&mut g, 0.3)?;
    let before = capture(&mut g)?;
    cmd(&mut g, "title @s subtitle \"small\"");
    cmd(&mut g, "title @s title {text:\"Big\",color:\"gold\"}");
    cmd(&mut g, "title @s actionbar \"bar\"");
    if !wait_until(&mut g, 3.0, |g| crate::lang::strip_codes(&g.title.0) == "Big" && crate::lang::strip_codes(&g.actionbar.0) == "bar")? { bail!("title/actionbar not received: {:?} {:?}", g.title.0, g.actionbar.0); }
    wait(&mut g, 0.2)?;
    let shown = capture(&mut g)?;
    save(&g, &shown, "titles");
    let (w, h) = (W as i32, H as i32);
    let mid = diff_count(&before, &shown, w / 4, h / 4, 3 * w / 4, h / 2 + h / 8);
    if mid < 200 { bail!("title text not drawn ({mid} px)"); }
    // {color:"gold"} -> (255,170,0)
    let gold = (0..W as i32).step_by(2).flat_map(|x| (H as i32 / 4..H as i32 / 2).step_by(2).map(move |y| (x, y))).filter(|&(x, y)| { let p = px(&shown, x, y); p[0] > 220 && (150..=190).contains(&p[1]) && p[2] < 40 }).count();
    if gold < 20 { bail!("title not drawn in gold ({gold} px)"); }
    if !wait_until(&mut g, 3.0, |g| g.title.2 < 0)? { bail!("title did not time out after times 0 20 0"); }
    Ok(Outcome::Pass(format!("title drawn ({mid} px), gone after its stay time")))
}

// ---------------------------------------------------------------- crafting stations (enchanting, anvil, grindstone, smithing, brewing)
// These open the screen through an injected open_screen + container_set_content + container_set_data
// (no server needed: the server side of the stations is tested by the check harness) and read the packets the
// client queues back through a private channel.

type Sent = tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>;

fn stack(item: &str, n: i32) -> ItemStack { ItemStack { count: n, item: item_id(item), ..Default::default() } }

/// Client with no connection showing `menu`: container contents `cs` (slot, stack), the player's xp level, and set_data props.
fn station(c: &Ctx, short: &str, menu: &str, title: &str, cs: &[(usize, ItemStack)], props: &[(i16, i16)], level: i32) -> Result<(Game, Sent)> {
    use crate::net::Event;
    let mut g = c.game_at(short, None)?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    g.out = tx;
    g.enchantments = ["protection", "sharpness", "unbreaking", "efficiency"].iter().map(|n| format!("minecraft:{n}")).collect();
    g.player.gamemode = 0;
    g.handle(Event::Experience { progress: 0.0, level, total: 0 });
    let id = basalt_data::registry_id(basalt_data::registries::MENU, menu).ok_or_else(|| anyhow!("no menu {menu}"))?;
    g.handle(Event::OpenScreen { window: 7, menu: id, title: title.to_string() });
    let kind = container(&g).ok_or_else(|| anyhow!("screen did not open"))?.kind;
    if kind == Kind::Other { bail!("menu {menu} fell back to Kind::Other"); }
    let mut items = vec![ItemStack::default(); kind.slot_count()];
    for (i, s) in cs { items[*i] = s.clone(); }
    g.handle(Event::Content { window: 7, state: 1, items, carried: ItemStack::default() });
    for &(i, v) in props { g.handle(Event::ContainerData { window: 7, id: i, value: v }); }
    wait(&mut g, 0.3)?;
    Ok((g, rx))
}
fn drain(rx: &mut Sent) -> Vec<Vec<u8>> { let mut v = Vec::new(); while let Ok(b) = rx.try_recv() { v.push(b); } v }
fn packets_named(rx: &mut Sent, name: &str) -> Vec<Vec<u8>> {
    drain(rx).into_iter().filter(|b| basalt_protocol::Reader::new(b).varint().map(|id| basalt_protocol::ids::play::serverbound::name(id).ends_with(name)).unwrap_or(false)).collect()
}
fn region_of(g: &Game, kind: Kind, x: f32, y: f32, w: f32, h: f32) -> (i32, i32, i32, i32) {
    let (s, (ox, oy)) = (g.gui_scale(), g.container_origin(kind));
    ((ox + x * s) as i32, (oy + y * s) as i32, (ox + (x + w) * s) as i32, (oy + (y + h) * s) as i32)
}

fn enchanting_screen(c: &Ctx) -> Result<Outcome> {
    let kind = Kind::Enchant;
    let cs = [(0, stack("diamond_sword", 1)), (1, stack("lapis_lazuli", 2))];
    // reference: no offers
    let (mut g0, _) = station(c, "en0", "enchantment", "Enchant", &cs, &[], 30)?;
    let idle = capture(&mut g0)?; save(&g0, &idle, "enchant_idle");
    // offers: 5 / 15 / 30 levels with clues sharpness II, unbreaking I, efficiency III; the player has 20 levels and 2 lapis
    let props = [(0, 5), (1, 15), (2, 30), (3, 1234), (4, 1), (5, 2), (6, 3), (7, 2), (8, 1), (9, 3)];
    let (mut g, mut rx) = station(c, "en1", "enchantment", "Enchant", &cs, &props, 20)?;
    let img = capture(&mut g)?; save(&g, &img, "enchant_offers");
    for i in 0..3 {
        let (x0, y0, x1, y1) = region_of(&g, kind, 60.0, 14.0 + 19.0 * i as f32, 108.0, 19.0);
        if diff_count(&idle, &img, x0, y0, x1, y1) < 100 { bail!("offer button {i} not drawn differently from the idle table"); }
    }
    // hover tooltip
    let (x, y) = { let (a, b, d, e) = region_of(&g, kind, 60.0, 14.0, 108.0, 19.0); ((a + d) / 2, (b + e) / 2) };
    g.mouse_move(x as f32, y as f32); step(&mut g)?;
    let tip = capture(&mut g)?; save(&g, &tip, "enchant_tooltip");
    if diff_count(&img, &tip, x + 8, y - 40, x + 260, y + 10) < 200 { bail!("no tooltip while hovering an offer"); }
    // click: button 1 is affordable (15 levels... 20 >= 15, 2 lapis >= 2), button 2 is not (30 levels, 3 lapis), button 0 is
    let _ = drain(&mut rx);
    for i in [0usize, 1, 2] { let (a, b, d, e) = region_of(&g, kind, 60.0, 14.0 + 19.0 * i as f32, 108.0, 19.0); click_at(&mut g, ((a + d) / 2) as f32, ((b + e) / 2) as f32, 0)?; }
    let sent: Vec<(i32, i32)> = packets_named(&mut rx, "container_button_click").iter().map(|b| { let mut r = basalt_protocol::Reader::new(b); r.varint().unwrap(); (r.varint().unwrap(), r.varint().unwrap()) }).collect();
    if sent != vec![(7, 0), (7, 1)] { bail!("button click packets (window, button) = {sent:?}, expected [(7,0),(7,1)] (button 2 needs 30 levels and 3 lapis)"); }
    Ok(Outcome::Pass("enchanting screen: three offer buttons with level sprites + cost, tooltip on hover, enabled buttons send container_button_click (disabled ones do not)".into()))
}

fn anvil_screen(c: &Ctx) -> Result<Outcome> {
    let kind = Kind::Anvil;
    let (mut g0, _) = station(c, "an0", "anvil", "Repair & Name", &[], &[], 10)?;
    let idle = capture(&mut g0)?; save(&g0, &idle, "anvil_idle");
    let cs = [(0, stack("diamond_sword", 1)), (1, stack("diamond", 1)), (2, stack("diamond_sword", 1))];
    let (mut g, mut rx) = station(c, "an1", "anvil", "Repair & Name", &cs, &[(0, 7)], 10)?;
    let img = capture(&mut g)?; save(&g, &img, "anvil_cost");
    // the text field shows the item name, the cost line is drawn, the field is editable
    if container(&g).unwrap().text != crate::lang::item_name(item_id("diamond_sword")) { bail!("name field = {:?}", container(&g).unwrap().text); }
    let (x0, y0, x1, y1) = region_of(&g, kind, 100.0, 66.0, 70.0, 14.0);
    if diff_count(&idle, &img, x0, y0, x1, y1) < 40 { bail!("cost line not drawn"); }
    // typing renames: backspace the old name away, type a new one
    let _ = drain(&mut rx);
    for _ in 0..30 { tap(&mut g, KeyCode::Backspace, None)?; }
    type_text(&mut g, "Excalibur")?;
    if container(&g).unwrap().text != "Excalibur" { bail!("field text after typing = {:?}", container(&g).unwrap().text); }
    let names: Vec<String> = packets_named(&mut rx, "rename_item").iter().map(|b| { let mut r = basalt_protocol::Reader::new(b); r.varint().unwrap(); r.str().unwrap() }).collect();
    if names.last().map(|s| s.as_str()) != Some("Excalibur") || names.len() < 9 { bail!("rename_item packets: {names:?}"); }
    // typing must not have closed the screen (E would close it when the field is not focused)
    type_text(&mut g, "e")?;
    tap(&mut g, KeyCode::KeyE, Some("e"))?;
    if container(&g).is_none() { bail!("E closed the screen while typing in the name field"); }
    let typed = capture(&mut g)?; save(&g, &typed, "anvil_typed");
    // too expensive: the cost line changes to a message
    g.handle(crate::net::Event::ContainerData { window: 7, id: 0, value: 45 });
    wait(&mut g, 0.2)?;
    let exp = capture(&mut g)?; save(&g, &exp, "anvil_expensive");
    if diff_count(&img, &exp, x0 - 40, y0, x1, y1) < 40 { bail!("too-expensive state not drawn differently"); }
    // not editable without an input item: keys do not type
    let (mut g2, mut rx2) = station(c, "an2", "anvil", "Repair & Name", &[], &[], 10)?;
    type_text(&mut g2, "abc")?;
    if !container(&g2).unwrap().text.is_empty() || !packets_named(&mut rx2, "rename_item").is_empty() { bail!("typed into a disabled name field"); }
    // clicking the result sends a container_click for slot 2
    let _ = drain(&mut rx);
    let p = slot_center(&g, kind, 2); click_at(&mut g, p.0, p.1, 0)?;
    let clicks = packets_named(&mut rx, "container_click");
    let slot = clicks.first().map(|b| { let mut r = basalt_protocol::Reader::new(b); r.varint().unwrap(); r.varint().unwrap(); r.varint().unwrap(); r.i16().unwrap() });
    if slot != Some(2) { bail!("result click sent slot {slot:?}"); }
    Ok(Outcome::Pass("anvil screen: name field (default item name, typing sends rename_item, E does not close), cost / too-expensive line, disabled field without input, result click".into()))
}

fn grindstone_screen(c: &Ctx) -> Result<Outcome> {
    let kind = Kind::Grindstone;
    let (mut g0, _) = station(c, "gr0", "grindstone", "Repair & Disenchant", &[], &[], 0)?;
    let idle = capture(&mut g0)?; save(&g0, &idle, "grindstone_idle");
    // inputs without a result: the error cross
    let (mut ge, _) = station(c, "gr1", "grindstone", "Repair & Disenchant", &[(0, stack("diamond_sword", 1))], &[], 0)?;
    let err = capture(&mut ge)?; save(&ge, &err, "grindstone_error");
    let (x0, y0, x1, y1) = region_of(&ge, kind, 92.0, 31.0, 28.0, 21.0);
    if diff_count(&idle, &err, x0, y0, x1, y1) < 100 { bail!("error sprite not drawn"); }
    // with a result: no cross, items drawn, the result is clicked
    let cs = [(0, stack("diamond_sword", 1)), (2, stack("diamond_sword", 1))];
    let (mut g, mut rx) = station(c, "gr2", "grindstone", "Repair & Disenchant", &cs, &[], 0)?;
    let img = capture(&mut g)?; save(&g, &img, "grindstone_result");
    let ((a0, b0, a1, b1), (r0, s0, r1, s1)) = (region_of(&g, kind, 129.0, 34.0, 16.0, 16.0), (x0, y0, x1, y1));
    if diff_count(&idle, &img, a0, b0, a1, b1) < 100 { bail!("result item not drawn"); }
    if diff_count(&idle, &img, r0 + 2, s0 + 2, r1 - 2, s1 - 2) > 20 { bail!("error sprite drawn although there is a result"); }
    let _ = drain(&mut rx);
    g.key(KeyCode::ShiftLeft, true, None);
    let p = slot_center(&g, kind, 2); click_at(&mut g, p.0, p.1, 0)?;
    g.key(KeyCode::ShiftLeft, false, None);
    let clicks = packets_named(&mut rx, "container_click");
    let first = clicks.first().map(|b| { let mut r = basalt_protocol::Reader::new(b); r.varint().unwrap(); r.varint().unwrap(); r.varint().unwrap(); (r.i16().unwrap(), r.i8().unwrap(), r.varint().unwrap()) });
    if first != Some((2, 0, 1)) { bail!("shift-click on the result sent (slot, button, mode) = {first:?}"); }
    Ok(Outcome::Pass("grindstone screen: slots, error cross without a result, result item, shift-click sends the click for slot 2".into()))
}

fn smithing_screen(c: &Ctx) -> Result<Outcome> {
    let kind = Kind::Smithing;
    let (mut g0, _) = station(c, "sm0", "smithing", "Upgrade Gear", &[], &[], 0)?;
    let idle = capture(&mut g0)?; save(&g0, &idle, "smithing_idle");
    let cs = [(0, stack("netherite_upgrade_smithing_template", 1)), (1, stack("diamond_sword", 1)), (2, stack("netherite_ingot", 1))];
    let (mut ge, _) = station(c, "sm1", "smithing", "Upgrade Gear", &cs, &[], 0)?;
    let err = capture(&mut ge)?; save(&ge, &err, "smithing_error");
    let (x0, y0, x1, y1) = region_of(&ge, kind, 65.0, 46.0, 28.0, 21.0);
    if diff_count(&idle, &err, x0, y0, x1, y1) < 100 { bail!("error sprite not drawn"); }
    let mut cs2 = cs.to_vec(); cs2.push((3, stack("netherite_sword", 1)));
    let (mut g, mut rx) = station(c, "sm2", "smithing", "Upgrade Gear", &cs2, &[], 0)?;
    let img = capture(&mut g)?; save(&g, &img, "smithing_result");
    let (a0, b0, a1, b1) = region_of(&g, kind, 98.0, 48.0, 16.0, 16.0);
    if diff_count(&idle, &img, a0, b0, a1, b1) < 100 { bail!("result item not drawn"); }
    if diff_count(&err, &img, x0 + 2, y0 + 2, x1 - 2, y1 - 2) < 20 { bail!("error cross still drawn with a result"); }
    // taking the result: left click on slot 3 (take-only, the server decides, nothing predicted)
    let _ = drain(&mut rx);
    let p = slot_center(&g, kind, 3); click_at(&mut g, p.0, p.1, 0)?;
    let slot = packets_named(&mut rx, "container_click").first().map(|b| { let mut r = basalt_protocol::Reader::new(b); r.varint().unwrap(); r.varint().unwrap(); r.varint().unwrap(); r.i16().unwrap() });
    if slot != Some(3) { bail!("result click sent slot {slot:?}"); }
    if container(&g).unwrap().slots[3].count != 1 { bail!("result slot was changed locally"); }
    Ok(Outcome::Pass("smithing screen: four slots (template/base/addition/result), ghost icons, error cross, result take-only click".into()))
}

fn brewing_screen(c: &Ctx) -> Result<Outcome> {
    let kind = Kind::Brewing;
    let cs = [(0, stack("potion", 1)), (1, stack("potion", 1)), (2, stack("potion", 1)), (3, stack("nether_wart", 1)), (4, stack("blaze_powder", 1))];
    let (mut g0, _) = station(c, "br0", "brewing_stand", "Brewing Stand", &cs, &[], 0)?;
    let idle = capture(&mut g0)?; save(&g0, &idle, "brewing_idle");
    let (mut g, mut rx) = station(c, "br1", "brewing_stand", "Brewing Stand", &cs, &[(0, 300), (1, 15)], 0)?;
    let img = capture(&mut g)?; save(&g, &img, "brewing_active");
    let region = |x, y, w, h| region_of(&g, kind, x, y, w, h);
    let (f0, f1, f2, f3) = region(60.0, 44.0, 18.0, 4.0);
    let fuel = diff_count(&idle, &img, f0, f1, f2, f3);
    if fuel < 20 { bail!("fuel bar not drawn ({fuel} px)"); }
    let (a0, a1, a2, a3) = region(97.0, 16.0, 9.0, 28.0);
    let arrow = diff_count(&idle, &img, a0, a1, a2, a3);
    if arrow < 20 { bail!("brew progress arrow not drawn ({arrow} px)"); }
    let (b0, b1, b2, b3) = region(63.0, 14.0, 12.0, 29.0);
    if diff_count(&idle, &img, b0, b1, b2, b3) < 5 { bail!("bubbles not drawn"); }
    // the arrow grows as the brew time counts down
    g.handle(crate::net::Event::ContainerData { window: 7, id: 0, value: 100 });
    wait(&mut g, 0.2)?;
    let img2 = capture(&mut g)?; save(&g, &img2, "brewing_late");
    let arrow2 = diff_count(&idle, &img2, a0, a1, a2, a3);
    if arrow2 <= arrow { bail!("progress arrow did not grow: {arrow} -> {arrow2}"); }
    // fuel 0: the bar disappears again
    g.handle(crate::net::Event::ContainerData { window: 7, id: 1, value: 0 });
    wait(&mut g, 0.2)?;
    let img3 = capture(&mut g)?;
    if diff_count(&idle, &img3, f0, f1, f2, f3) > 4 { bail!("fuel bar still drawn with no fuel"); }
    // clicking the fuel slot with blaze powder on the cursor goes through the normal click path
    let _ = drain(&mut rx);
    let p = slot_center(&g, kind, 4); click_at(&mut g, p.0, p.1, 0)?;
    let slot = packets_named(&mut rx, "container_click").first().map(|b| { let mut r = basalt_protocol::Reader::new(b); r.varint().unwrap(); r.varint().unwrap(); r.varint().unwrap(); r.i16().unwrap() });
    if slot != Some(4) { bail!("fuel slot click sent slot {slot:?}"); }
    Ok(Outcome::Pass(format!("brewing screen: fuel bar ({fuel} px), progress arrow {arrow}->{arrow2} px, bubbles, fuel slot click")))
}

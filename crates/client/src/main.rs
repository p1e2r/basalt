//! Basalt client: joins a 26.2 server, renders the world with wgpu.
//!
//! Usage: basalt-client [--addr host:port | host:port] [--name N] [--jar client.jar] [--assets DIR --asset-index ID]
//!                      [--screenshot out.png --wait SECS] [--look yaw,pitch] [--cmd 'tp @s 0 100 0']...
//!                      [--scenario NAME|all] [--auto-respawn]
//! Without an address (windowed) it starts at the title screen.
mod assets;
mod board;
mod assetgrid;
mod bench;
mod container;
mod entity;
mod lang;
mod menu;
mod mesh;
mod meshdump;
mod net;
mod options;
mod particles;
mod player;
mod render;
mod scenario;
mod script;
mod anim;
mod sound;
mod status;
mod suggest;
mod ui;
mod world;
mod worlds;

use anyhow::{Context, Result};
use assets::{Assets, Biome};
use basalt_protocol::{ids::play::serverbound as sb, Writer};
use glam::{Mat4, Vec3};
use mesh::{MeshData, SectionMesh, Snapshot, Vertex};
use menu::Screen;
use net::{Event, ItemStack};
use player::{Input, Player};
use render::{Frame, Gpu, Tex, Uniforms};
use std::collections::{HashMap, HashSet};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use ui::UiAtlas;
use world::World;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

pub struct Args {
    data: String, addr: String, addr_given: bool, name: String, jar: String, screenshot: Option<String>, wait: f32, size: (u32, u32), pos: Option<[f64; 3]>, look: Option<(f32, f32)>,
    cmds: Vec<String>, walk: f32, chat: Vec<String>, click: Option<char>, clicks: Vec<(i16, i8, i32)>, hold: f32, later: Vec<(f32, String)>, attack: u32, attack_every: f32,
    scenario: Option<String>, vanilla: Option<String>, assets: Option<String>, asset_index: Option<String>, auto_respawn: bool,
    // visual parity / asset grid / benchmark
    time: Option<i64>, fov: Option<f32>, render_distance: Option<u32>, gui_scale: Option<u32>, no_gui: bool, options: Option<String>,
    asset_grid: Option<String>, grid_items: bool, allowlist: String, bench: Option<String>, seconds: f32, bench_mode: String,
    mesh_dump: Option<String>, mesh_out: String, anim_eval: Option<String>, frames: Option<String>, fps: f32,
    script: Option<String>, script_log: Option<String>,
}

fn parse_args() -> Args {
    let mut a = Args { data: "data".into(), addr: "127.0.0.1:25566".into(), addr_given: false, name: "Basalt".into(), jar: basalt_paths::client_jar().to_string_lossy().into_owned(),
        screenshot: None, wait: 8.0, size: (1280, 720), pos: None, look: None, cmds: Vec::new(), walk: 0.0, chat: Vec::new(), click: None, clicks: Vec::new(), hold: 1.0, later: Vec::new(), attack: 0, attack_every: 0.7,
        scenario: None, vanilla: None, assets: None, asset_index: None, auto_respawn: false,
        time: None, fov: None, render_distance: None, gui_scale: None, no_gui: false, options: None, asset_grid: None, grid_items: false, allowlist: "tools/asset-allowlist.txt".into(), mesh_dump: None, mesh_out: "target/mesh-dump".into(), anim_eval: None, frames: None, fps: 20.0, script: None, script_log: None,
        bench: None, seconds: 20.0, bench_mode: "rotate".into() };
    let mut it = std::env::args().skip(1);
    let triple = |s: String| -> Option<[f64; 3]> { let v: Vec<f64> = s.split(',').filter_map(|p| p.parse().ok()).collect(); (v.len() == 3).then(|| [v[0], v[1], v[2]]) };
    while let Some(k) = it.next() {
        let mut v = || it.next().unwrap_or_default();
        match k.as_str() {
            "--data" => a.data = v(),
            "--addr" => { a.addr = v(); a.addr_given = true; }
            "--name" => a.name = v(), "--jar" => a.jar = v(),
            "--assets" => a.assets = Some(v()), "--asset-index" => a.asset_index = Some(v()),
            "--screenshot" => a.screenshot = Some(v()), "--wait" => a.wait = v().parse().unwrap_or(8.0),
            "--size" => { let s = v(); if let Some((w, h)) = s.split_once('x') { a.size = (w.parse().unwrap_or(1280), h.parse().unwrap_or(720)); } }
            "--pos" => a.pos = triple(v()),
            "--cmd" => a.cmds.push(v()),
            "--chat" => a.chat.push(v()),
            "--click" => a.click = v().chars().next(),
            // "--later 10:tp @s 0 0 0": run a command N seconds after joining
            "--later" => { let v = v(); if let Some((t, c)) = v.split_once(':') { a.later.push((t.parse().unwrap_or(0.0), c.to_string())); } }
            "--hold" => a.hold = v().parse().unwrap_or(1.0),
            // test hook: attack the nearest mob N times (every --attack-every seconds, from 3 s after joining)
            "--attack" => a.attack = v().parse().unwrap_or(1),
            "--attack-every" => a.attack_every = v().parse().unwrap_or(0.7),
            // container clicks in the player inventory: "slot:button:mode,..."
            "--clicks" => a.clicks = v().split(',').filter_map(|c| { let p: Vec<&str> = c.split(':').collect(); Some((p.first()?.parse().ok()?, p.get(1).unwrap_or(&"0").parse().ok()?, p.get(2).unwrap_or(&"0").parse().ok()?)) }).collect(),
            "--walk" => a.walk = v().parse().unwrap_or(0.0),
            "--look" => { let s = v(); if let Some((y, p)) = s.split_once(',') { a.look = Some((y.parse().unwrap_or(0.0), p.parse().unwrap_or(0.0))); } }
            "--scenario" => a.scenario = Some(v()),
            // scripted input (test hook, see script.rs): JSON-lines ops as in tools/oracle-mod/SCHEMA.md; serverbound packets go to --script-log
            "--script" => a.script = Some(v()), "--script-log" => a.script_log = Some(v()),
            "--vanilla" => a.vanilla = Some(v()),
            "--auto-respawn" => a.auto_respawn = true,
            "--time" => a.time = v().parse().ok(),
            "--fov" => a.fov = v().parse().ok(),
            "--render-distance" => a.render_distance = v().parse().ok(),
            "--gui-scale" => a.gui_scale = v().parse().ok(),
            "--no-gui" => a.no_gui = true,
            "--options" => a.options = Some(v()),
            "--asset-grid" => a.asset_grid = Some(v()),
            "--mesh-dump" => a.mesh_dump = Some(v()), "--out" => a.mesh_out = v(),
            // animation comparer (anim.rs): oracle bone dump in, Basalt part poses out (--out)
            "--anim-eval" => a.anim_eval = Some(v()),
            // script mode: dump rendered frames (PNG) to DIR at --fps
            "--frames" => a.frames = Some(v()), "--fps" => a.fps = v().parse().unwrap_or(20.0),
            "--blocks" => a.grid_items = false, "--items" => a.grid_items = true,
            "--allowlist" => a.allowlist = v(),
            "--bench" => a.bench = Some(v()),
            "--seconds" => a.seconds = v().parse().unwrap_or(20.0),
            "--bench-mode" => a.bench_mode = v(),
            other if !other.starts_with("--") => { a.addr = other.to_string(); a.addr_given = true; }
            other => eprintln!("unknown argument {other}"),
        }
    }
    a
}

// ---------------------------------------------------------------- meshing workers

struct Mesher { jobs: mpsc::Sender<((i32, i32, i32), Snapshot)>, done: mpsc::Receiver<((i32, i32, i32), SectionMesh, Duration)> }

impl Mesher {
    fn new(assets: Arc<Assets>, biomes: Arc<Mutex<Arc<Vec<Biome>>>>) -> Mesher {
        let (jtx, jrx) = mpsc::channel::<((i32, i32, i32), Snapshot)>();
        let (dtx, drx) = mpsc::channel();
        let jrx = Arc::new(Mutex::new(jrx));
        let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).saturating_sub(2).clamp(1, 8);
        for i in 0..n {
            let (jrx, dtx, assets, biomes) = (jrx.clone(), dtx.clone(), assets.clone(), biomes.clone());
            std::thread::Builder::new().name(format!("mesh{i}")).spawn(move || loop {
                let job = jrx.lock().unwrap().recv();
                let Ok((key, snap)) = job else { return };
                let b = biomes.lock().unwrap().clone();
                let t = Instant::now();
                let m = mesh::mesh_section(&assets, &b, &snap);
                if dtx.send((key, m, t.elapsed())).is_err() { return; }
            }).unwrap();
        }
        Mesher { jobs: jtx, done: drx }
    }
}

fn snapshot(w: &World, cx: i32, sy: i32, cz: i32) -> Snapshot {
    let origin = [cx * 16, w.min_y + sy * 16, cz * 16];
    let mut blocks = Box::new([0u16; 5832]);
    let mut light = Box::new([0u8; 5832]);
    for y in -1..17 { for z in -1..17 { for x in -1..17 {
        let (wx, wy, wz) = (origin[0] + x, origin[1] + y, origin[2] + z);
        let i = mesh::sidx(x, y, z);
        blocks[i] = w.get(wx, wy, wz);
        let (s, b) = w.light(wx, wy, wz);
        light[i] = s << 4 | b;
    } } }
    let biomes = w.chunks.get(&(cx, cz)).map(|c| c.sections[sy as usize].biomes).unwrap_or([0; 64]);
    Snapshot { origin, blocks, light, biomes }
}

/// How long a dirty section waits for its horizontal neighbours before it is meshed anyway
/// (faces toward unloaded chunks are then drawn, like the edge of vanilla's view).
const EDGE_MESH_DELAY: Duration = Duration::from_millis(400);

// ---------------------------------------------------------------- game state

/// Measurements for --bench.
#[derive(Default)]
pub struct Stats { pub mesh_ms: Vec<f32>, pub sections_meshed: u64, pub chunk_arrived: HashMap<(i32, i32), Instant>, pub chunk_latency_ms: Vec<f32> }

pub struct Dig { pub block: [i32; 3], pub face: u8, pub ticks: u32, pub total: u32 }

/// Chat-box suggestion popup state.
#[derive(Default)]
pub struct Sug {
    pub list: Vec<String>,
    pub sel: usize,
    /// byte offset (after the slash) of the word the suggestions replace
    pub anchor: usize,
    /// text the list was computed for; after a Tab completion the list is kept so Tab keeps cycling
    for_text: String,
    cycling: bool,
    pub query_id: i32,
}

pub struct Game {
    gpu: Gpu,
    assets: Arc<Assets>,
    rules: Option<Arc<basalt_game::GameData>>,
    ui: UiAtlas,
    entity_atlas: entity::EntityAtlas,
    biomes: Arc<Mutex<Arc<Vec<Biome>>>>,
    dim_types: Vec<String>,
    /// the current dimension type (None before login)
    pub dim: Option<assets::DimensionType>,
    world: World,
    player: Player,
    mesher: Mesher,
    dirty: HashMap<(i32, i32, i32), Instant>,
    in_flight: HashSet<(i32, i32, i32)>,
    events: mpsc::Receiver<Event>,
    out: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    entities: HashMap<i32, entity::Entity>,
    player_names: HashMap<uuid::Uuid, String>,
    /// scoreboard objectives / scores / display slots and boss bars
    board: board::Board,
    my_eid: i32,
    name: String,
    chat: Vec<(String, Instant)>,
    chat_input: Option<String>,
    /// server command tree (clientbound `commands`) and the chat box's suggestion popup
    cmd_tree: Option<suggest::Tree>,
    sug: Sug,
    hp: f32, pub food: i32,
    /// experience bar progress 0..1 and level (set_experience)
    pub xp: (f32, i32),
    /// enchantment registry names in the server's synced order (clue ids in the enchanting screen)
    pub enchantments: Vec<String>,
    /// title / subtitle with ticks since shown and fade in/stay/out; action bar text + ticks left
    pub title: (String, String, i32, [i32; 3]), pub actionbar: (String, i32),
    hotbar: [ItemStack; 9], held: u8,
    inventory: Vec<ItemStack>,
    inv_state: i32,
    carried: ItemStack,
    day_time: i64,
    spawned: bool,
    loaded_sent: bool,
    connected: bool,
    keys: HashSet<KeyCode>,
    mouse: [bool; 2],
    mouse_pos: (f32, f32),
    slider_drag: Option<String>,
    last_tick: Instant,
    last_sent: ([f64; 3], f32, f32, bool),
    ticks_since_pos: u32,
    last_input_flags: u8,
    seq: i32,
    dig: Option<Dig>,
    dig_delay: u32,
    /// (ticks taken, block) of the last survival break the client finished
    last_dig: Option<(u32, [i32; 3])>,
    /// other players' block_destruction progress: eid → (block, stage)
    destroy: HashMap<i32, ([i32; 3], i8)>,
    fp_swing: i32,
    /// sprint state last reported with player_command
    sprint_sent: bool,
    /// vanilla-style sprint: current state, double-tap window (ticks), forward/sprint keys last tick,
    /// toggle-mode latch, and the tick of the last jump press (double-tap fly)
    pub sprinting: bool, sprint_trigger: u32, fwd_was: bool, sprint_key_was: bool, sprint_latch: bool, jump_tick: Option<u64>,
    pub particles: particles::Particles,
    /// first-person equip progress (now, previous tick) and the (slot, item) it was raised for
    pub equip: (f32, f32), equip_for: i32,
    /// first-person: item drawn in the hand (switches once the hand is lowered), ticks since the
    /// last attack (attack strength), view-bob walk distance and amplitude (now, previous tick),
    /// own hurt animation ticks left (10 -> 0)
    shown_item: i32, attack_ticks: u32, walk_dist: (f32, f32), view_bob: (f32, f32), self_hurt: u32,
    /// item name shown above the hotbar after selecting a different item, and its remaining ticks
    pub held_name: (String, i32),
    /// title screen splash line and when the menus started (panorama spin, splash pulse)
    pub splash: String, pub menu_t0: Instant,
    /// server list pings by address, their result channel, and favicon slots by address
    pub pings: HashMap<String, status::PingState>,
    ping_chan: (mpsc::Sender<status::PingResult>, mpsc::Receiver<status::PingResult>),
    pub icon_slots: HashMap<String, usize>,
    pub ping_rounds: u32,
    /// singleplayer: the world list, the integrated server and the world being played
    pub worlds: Vec<worlds::WorldEntry>,
    pub integrated: Option<worlds::Integrated>,
    pub sp_world: Option<worlds::WorldEntry>,
    step_dist: f64,
    last_place: Instant,
    debug: bool,
    disconnected: Option<String>,
    death_msg: Option<String>,
    respawn_at: Option<Instant>,
    auto_respawn: bool,
    screen: Option<Screen>,
    cfg: menu::Config,
    sounds: sound::Sounds,
    last_addr: String,
    quit: bool,
    fullscreen_changed: bool,
    hide_entities: bool,
    hide_cracks: bool,
    /// F1: no HUD, no hand
    pub(crate) hide_gui: bool,
    /// test hook: draw the HUD but not the first-person arm
    pub hide_hand: bool,
    /// fixed time of day for deterministic captures (overrides the server)
    pub(crate) fixed_time: Option<i64>,
    last_chunk_at: Instant,
    stats: Stats,
    clicks_sent: u32,
    pub ticks: u64,
    frames: (u32, Instant, f32),
}

pub(crate) fn send(out: &tokio::sync::mpsc::UnboundedSender<Vec<u8>>, w: Writer) { script::log_packet(&w.buf); let _ = out.send(w.buf); }

pub struct GameOpts { pub name: String, pub addr: Option<String>, pub auto_respawn: bool, pub sounds: sound::Sounds, pub cfg: menu::Config }

impl Game {
    pub fn new(gpu_window: Option<Arc<Window>>, size: (u32, u32), assets: Arc<Assets>, rules: Option<Arc<basalt_game::GameData>>, o: GameOpts) -> Result<Game> {
        let ui = UiAtlas::build(&assets);
        let entity_atlas = entity::EntityAtlas::build(&assets);
        let gpu = Gpu::new(gpu_window, size, &assets, (&ui.rgba, ui.w, ui.h), (&entity_atlas.rgba, entity_atlas.w, entity_atlas.h))?;
        let biomes = Arc::new(Mutex::new(Arc::new(Vec::new())));
        let mesher = Mesher::new(assets.clone(), biomes.clone());
        let (_tx, rx) = mpsc::channel();
        let (otx, _orx) = tokio::sync::mpsc::unbounded_channel();
        let mut sounds = o.sounds; sounds.master = o.cfg.master_volume;
        let mut g = Game {
            gpu, assets, rules, ui, entity_atlas, biomes, dim_types: Vec::new(), dim: None, world: World::new(-64, 384), player: Player::new(), mesher,
            dirty: HashMap::new(), in_flight: HashSet::new(), events: rx, out: otx, entities: HashMap::new(), player_names: HashMap::new(), board: Default::default(), my_eid: 0, name: o.name,
            chat: Vec::new(), chat_input: None, cmd_tree: None, sug: Sug::default(), hp: 20.0, food: 20, xp: (0.0, 0), enchantments: Vec::new(), title: (String::new(), String::new(), -1, [10, 70, 20]), actionbar: (String::new(), 0), hotbar: Default::default(), held: 0, inventory: vec![ItemStack::default(); 46], inv_state: 0, carried: ItemStack::default(), day_time: 6000,
            spawned: false, loaded_sent: false, connected: false, keys: HashSet::new(), mouse: [false; 2], mouse_pos: (0.0, 0.0), slider_drag: None, last_tick: Instant::now(),
            last_sent: ([0.0; 3], 0.0, 0.0, false), ticks_since_pos: 0, last_input_flags: 0, seq: 0, dig: None, dig_delay: 0, last_dig: None, destroy: HashMap::new(), fp_swing: -1, sprint_sent: false, sprinting: false, sprint_trigger: 0, fwd_was: false, sprint_key_was: false, sprint_latch: false, jump_tick: None, particles: Default::default(), equip: (1.0, 1.0), equip_for: -1, shown_item: -1, attack_ticks: 100, walk_dist: (0.0, 0.0), view_bob: (0.0, 0.0), self_hurt: 0, held_name: (String::new(), 0), splash: String::new(), menu_t0: Instant::now(), pings: HashMap::new(), ping_chan: mpsc::channel(), icon_slots: HashMap::new(), ping_rounds: 0, worlds: Vec::new(), integrated: None, sp_world: None, step_dist: 0.0,
            last_place: Instant::now(), debug: false, disconnected: None, death_msg: None, respawn_at: None, auto_respawn: o.auto_respawn,
            screen: Some(Screen::Title), cfg: o.cfg, sounds, last_addr: String::new(), quit: false, fullscreen_changed: false, hide_entities: false, hide_cracks: false, hide_gui: false, hide_hand: false, fixed_time: None, last_chunk_at: Instant::now(), stats: Stats::default(), clicks_sent: 0, ticks: 0,
            frames: (0, Instant::now(), 0.0),
        };
        // title panorama (launcher asset store; the jar only has a placeholder) and a random splash
        let faces: Vec<assets::Image> = (0..6).filter_map(|i| g.sounds.read_object(&format!("minecraft/textures/gui/title/background/panorama_{i}.png")).and_then(|b| assets::decode_png(&b).ok())).collect();
        if faces.len() == 6 { g.gpu.set_panorama(&faces.iter().map(|f| (f.w, f.h, f.rgba.as_slice())).collect::<Vec<_>>()); }
        else { log::warn!("title panorama not in the launcher asset store ({} of 6 faces); menus use a plain dark backdrop", faces.len()); }
        if !g.assets.splashes.is_empty() {
            let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos() as usize).unwrap_or(0);
            g.splash = g.assets.splashes[n % g.assets.splashes.len()].clone();
        }
        if g.cfg.language != "en_us" && !g.cfg.language.is_empty() { let l = g.cfg.language.clone(); g.set_language(&l); }
        for (k, v) in g.cfg.extra.clone() { if let Some(cat) = k.strip_prefix("vol.") { g.sounds.set_category(cat, v); } }
        if g.cfg.extra.get("hide_splash").copied().unwrap_or(0.0) > 0.5 { g.splash.clear(); }
        if let Some(a) = o.addr { g.connect(&a); }
        Ok(g)
    }

    /// Open a connection (replacing any current one) and show the connecting screen.
    pub fn connect(&mut self, addr: &str) {
        self.disconnect();
        let (tx, rx) = mpsc::channel();
        let (otx, orx) = tokio::sync::mpsc::unbounded_channel();
        net::spawn(net::Options { addr: addr.to_string(), name: self.name.clone(), view_distance: self.cfg.render_distance.clamp(2, 32) as i8 }, tx, orx);
        self.events = rx; self.out = otx;
        self.last_addr = addr.to_string();
        self.disconnected = None;
        self.connected = true;
        self.screen = Some(Screen::Connecting { addr: addr.to_string(), error: None });
    }

    /// Drop the connection (the net thread ends when its outbound channel closes) and clear the world.
    pub fn disconnect(&mut self) {
        let (_tx, rx) = mpsc::channel();
        let (otx, _orx) = tokio::sync::mpsc::unbounded_channel();
        self.events = rx; self.out = otx;
        self.connected = false; self.spawned = false; self.loaded_sent = false;
        self.world = World::new(-64, 384); self.gpu.sections.clear(); self.dirty.clear(); self.entities.clear(); self.destroy.clear();
        self.inventory = vec![ItemStack::default(); 46]; self.hotbar = Default::default(); self.carried = ItemStack::default(); self.dig = None;
        self.chat.clear(); self.chat_input = None;
    }

    fn mark_chunk(&mut self, cx: i32, cz: i32) {
        let now = Instant::now();
        for dx in -1..=1 { for dz in -1..=1 {
            let k = (cx + dx, cz + dz);
            if !self.world.chunks.contains_key(&k) { continue; }
            for sy in 0..self.world.nsections() as i32 { self.dirty.entry((k.0, sy, k.1)).or_insert(now); }
        } }
    }
    fn mark_block(&mut self, x: i32, y: i32, z: i32) {
        let sy = (y - self.world.min_y) >> 4;
        let now = Instant::now() - EDGE_MESH_DELAY;
        for dx in -1..=1 { for dy in -1..=1 { for dz in -1..=1 {
            let (cx, csy, cz) = ((x + dx) >> 4, ((y + dy - self.world.min_y) >> 4), (z + dz) >> 4);
            if csy >= 0 && csy < self.world.nsections() as i32 && (csy - sy).abs() <= 1 { self.dirty.entry((cx, csy, cz)).or_insert(now); }
        } } }
    }

    fn reset_world(&mut self, dim_type: i32) {
        let name = self.dim_types.get(dim_type as usize).cloned().unwrap_or_default();
        let (min_y, height) = self.assets.dimension_types.get(&name).map(|d| (d.min_y, d.height)).unwrap_or((-64, 384));
        self.world = World::new(min_y, height);
        self.dim = self.assets.dimension_types.get(&name).cloned();
        self.gpu.sections.clear(); self.dirty.clear(); self.entities.clear(); self.destroy.clear();
    }

    fn open_container(&self) -> Option<&container::Open> { if let Some(Screen::Container(c)) = &self.screen { Some(c) } else { None } }

    fn handle(&mut self, ev: Event) {
        match ev {
            Event::Registries { biomes, dimension_types, enchantments } => {
                self.enchantments = enchantments;
                *self.biomes.lock().unwrap() = Arc::new(self.assets.biomes(&biomes));
                self.dim_types = dimension_types;
            }
            Event::Login { entity_id, dimension_type, gamemode, .. } => {
                self.my_eid = entity_id; self.player.gamemode = gamemode; self.player.flying = gamemode == 3;
                self.reset_world(dimension_type);
            }
            Event::Respawn { dimension_type, gamemode, .. } => {
                self.player.gamemode = gamemode; self.reset_world(dimension_type); self.spawned = false;
                if matches!(self.screen, Some(Screen::Death { .. })) { self.screen = None; }
            }
            Event::Chunk { cx, cz, raw } => match net::decode_chunk(&raw, self.world.nsections()) {
                Ok(c) => { self.world.chunks.insert((cx, cz), c); self.mark_chunk(cx, cz); self.last_chunk_at = Instant::now(); self.stats.chunk_arrived.entry((cx, cz)).or_insert_with(Instant::now); }
                Err(e) => log::warn!("chunk {cx},{cz}: {e:#}"),
            },
            Event::Light { cx, cz, raw } => {
                let n = self.world.nsections();
                if let Some(c) = self.world.chunks.get_mut(&(cx, cz)) {
                    if let Ok((sky, block, top)) = world::Chunk::decode_light(&mut basalt_protocol::Reader::new(&raw), n) {
                        for (i, s) in sky.into_iter().enumerate() { if s.is_some() { c.sky[i] = s; } }
                        for (i, b) in block.into_iter().enumerate() { if b.is_some() { c.block[i] = b; } }
                        c.sky_open_from = c.sky_open_from.max(top);
                    }
                    self.mark_chunk(cx, cz);
                }
            }
            Event::Unload { cx, cz } => {
                self.world.chunks.remove(&(cx, cz));
                for sy in 0..self.world.nsections() as i32 { self.gpu.sections.remove(&(cx, sy, cz)); self.dirty.remove(&(cx, sy, cz)); }
            }
            Event::Block { x, y, z, state } => { if self.world.set(x, y, z, state) { self.mark_block(x, y, z); } }
            Event::Rotate { yaw, ry, pitch, rp } => {
                let p = &mut self.player;
                p.yaw = if ry { p.yaw + yaw } else { yaw };
                p.pitch = (if rp { p.pitch + pitch } else { pitch }).clamp(-90.0, 90.0);
            }
            Event::Teleport { id, pos, vel, yaw, pitch, flags } => {
                let p = &mut self.player;
                for i in 0..3 { p.pos[i] = if flags >> i & 1 == 1 { p.pos[i] + pos[i] } else { pos[i] }; }
                p.yaw = if flags & 8 != 0 { p.yaw + yaw } else { yaw };
                p.pitch = if flags & 16 != 0 { p.pitch + pitch } else { pitch };
                for i in 0..3 { p.vel[i] = if flags >> (5 + i) & 1 == 1 { p.vel[i] + vel[i] } else { vel[i] }; }
                p.prev_pos = p.pos;
                let mut w = Writer::with_id(sb::ACCEPT_TELEPORTATION); w.varint(id); send(&self.out, w);
                self.send_move(true);
                self.spawned = true;
                if matches!(self.screen, Some(Screen::Connecting { .. })) { self.screen = None; }
            }
            Event::Chat(s) => { println!("[chat] {}", lang::strip_codes(&s)); self.chat.push((s, Instant::now())); if self.chat.len() > 100 { self.chat.remove(0); } }
            Event::Board(u) => self.board.apply(u),
            Event::Health { hp, food } => { self.hp = hp; self.food = food; }
            Event::Experience { progress, level, total: _ } => { self.xp = (progress, level); }
            Event::Title { kind: 0, text } => { self.title.0 = text; self.title.2 = 0; }
            Event::Title { kind: 1, text } => { self.title.1 = text; }
            Event::Title { text, .. } => { self.actionbar = (text, 60); }
            Event::TitleTimes(t) => { self.title.3 = t; }
            Event::ClearTitles { reset } => { self.title.0.clear(); self.title.1.clear(); self.title.2 = -1; if reset { self.title.3 = [10, 70, 20]; } }
            Event::GameMode(g) => { self.player.gamemode = g; self.player.flying = g == 3 || (g == 1 && self.player.flying); }
            Event::Time { day, .. } => self.day_time = day,
            Event::HeldSlot(s) => self.held = s.min(8),
            Event::Slot { window, state, slot, item } => {
                if window == -1 && slot == -1 { self.carried = item; return; }
                // set_player_inventory numbering: 0-8 hotbar, 9-35 main, 36-39 armor (feet..head), 40 offhand
                let (window, slot) = if window == -2 { (0, match slot { 0..=8 => 36 + slot, 9..=35 => slot, 36..=39 => 8 - (slot - 36), 40 => 45, _ => -1 }) } else { (window, slot) };
                if window == 0 && (0..46).contains(&slot) {
                    self.inventory[slot as usize] = item.clone();
                    if (36..45).contains(&slot) { self.hotbar[(slot - 36) as usize] = item.clone(); }
                    if state != 0 { self.inv_state = state; }
                }
                if let Some(Screen::Container(c)) = &mut self.screen {
                    if c.window == window && slot >= 0 && (slot as usize) < c.slots.len() { c.slots[slot as usize] = item.clone(); c.state = state; }
                    // the player's own inventory part of another window mirrors window 0
                    if window == 0 && c.window != 0 && (9..45).contains(&slot) { let i = c.kind.player_start() + slot as usize - 9; if i < c.slots.len() { c.slots[i] = item; } }
                }
            }
            Event::Content { window, state, items, carried } => {
                if window == 0 {
                    for (i, it) in items.iter().enumerate().take(46) { self.inventory[i] = it.clone(); }
                    for i in 0..9 { self.hotbar[i] = self.inventory[36 + i].clone(); }
                    self.inv_state = state;
                }
                if let Some(Screen::Container(c)) = &mut self.screen {
                    if c.window == window { c.slots = items.clone(); c.slots.resize(c.kind.slot_count().max(items.len()), ItemStack::default()); c.state = state; }
                    if c.window != 0 && window != c.window && window == 0 { let ps = c.kind.player_start(); for i in 0..36 { if ps + i < c.slots.len() { c.slots[ps + i] = self.inventory[9 + i].clone(); } } }
                    if c.window == window { self.carried = carried; }
                } else if window == 0 { self.carried = carried; }
                // keep the hotbar in sync when the server sends another window's content
                if window != 0 { if let Some(Screen::Container(c)) = &self.screen { if c.window == window { let ps = c.kind.player_start(); for i in 0..9 { if let Some(it) = c.slots.get(ps + 27 + i) { self.hotbar[i] = it.clone(); self.inventory[36 + i] = it.clone(); } } } } }
            }
            Event::Cursor(item) => self.carried = item,
            Event::OpenScreen { window, menu, title } => {
                let kind = container::Kind::from_menu(menu);
                let mut slots = vec![ItemStack::default(); kind.slot_count()];
                let ps = kind.player_start();
                for i in 0..36 { if ps + i < slots.len() { slots[ps + i] = self.inventory[9 + i].clone(); } }
                self.chat_input = None; self.mouse = [false; 2];
                self.screen = Some(Screen::Container(container::Open { window, kind, title, slots, state: 0, drag: None, last_click: None, hover: None, props: [0; 10], text: String::new(), text_item: -1 }));
            }
            Event::ContainerData { window, id, value } => { if let Some(Screen::Container(c)) = &mut self.screen { if c.window == window && (0..10).contains(&id) { c.props[id as usize] = value; } } }
            Event::CloseScreen(w) => { if self.open_container().map(|c| c.window == w).unwrap_or(false) { self.screen = None; self.carried = ItemStack::default(); } }
            Event::AddEntity { id, uuid, kind, pos, vel, yaw, pitch, head_yaw, data } => { let mut e = entity::Entity::new(id, uuid, kind, pos, vel, yaw, pitch, head_yaw); e.data = data; self.entities.insert(id, e); }
            Event::MoveEntity { id, delta, rot, on_ground } => { if let Some(e) = self.entities.get_mut(&id) { let p = [e.server_pos[0] + delta[0], e.server_pos[1] + delta[1], e.server_pos[2] + delta[2]]; e.move_to(p, rot); e.on_ground = on_ground; } }
            Event::TeleportEntity { id, pos, yaw, pitch, on_ground } => { if let Some(e) = self.entities.get_mut(&id) { e.move_to(pos, Some((yaw, pitch))); e.on_ground = on_ground; } }
            Event::EntityMotion { id, vel } => { if id == self.my_eid { self.player.vel = vel; } else if let Some(e) = self.entities.get_mut(&id) { e.vel = vel; } }
            Event::EntityData { id, entries } => { if let Some(e) = self.entities.get_mut(&id) { e.apply_meta(&entries); } }
            Event::HeadYaw { id, yaw } => { if let Some(e) = self.entities.get_mut(&id) { e.set_head(yaw); } }
            Event::Animate { id, action } => { if action == 0 || action == 3 { if let Some(e) = self.entities.get_mut(&id) { e.swing = 0; } } }
            Event::Hurt { id } => { if id == self.my_eid { self.self_hurt = 10; } else if let Some(e) = self.entities.get_mut(&id) { e.hurt = 10; } }
            Event::EntityEvent { id, status } => { if let Some(e) = self.entities.get_mut(&id) { match status { 3 => { if e.death == 0 { e.death = 1; } } 2 => e.hurt = 10, _ => {} } } }
            Event::TakeItem { item, collector } => {
                if collector == self.my_eid { let p = self.player.pos; self.sounds.play("entity.item.pickup", Some(p), 0.2, 1.4); }
                let _ = item;
            }
            Event::RemoveEntities(v) => { for id in v { self.entities.remove(&id); self.destroy.remove(&id); } }
            Event::BlockDestruction { id, pos, stage } => { if (0..10).contains(&stage) { self.destroy.insert(id, (pos, stage)); } else { self.destroy.remove(&id); } }
            Event::Sound { event, pos, entity, volume, pitch, .. } => {
                let p = pos.or_else(|| entity.and_then(|id| if id == self.my_eid { Some(self.player.pos) } else { self.entities.get(&id).map(|e| e.pos) }));
                self.sounds.play(&event, p, volume, pitch);
            }
            Event::LevelEvent { event, pos, data } => {
                if event == 2001 {
                    self.block_sound(data as u16, "break", [pos[0] as f64 + 0.5, pos[1] as f64 + 0.5, pos[2] as f64 + 0.5]);
                    let tint = self.particle_tint(data as u16, pos);
                    self.particles.block_break(&self.assets, data as u16, pos, tint);
                }
            }
            Event::PlayerName { uuid, name } => { self.player_names.insert(uuid, name); }
            Event::Commands(b) => match suggest::Tree::parse(&b) { Ok(t) => self.cmd_tree = Some(t), Err(e) => log::warn!("commands packet: {e:#}") },
            Event::Suggestions { id, start, len: _, list } => {
                // answer to our ask_server query: only if it is still for the text being typed
                if id == self.sug.query_id && !list.is_empty() {
                    let anchor = (start as usize).saturating_sub(1);
                    if anchor == self.sug.anchor { for s in list { if !self.sug.list.contains(&s) { self.sug.list.push(s); } } self.sug.list.sort(); }
                }
            }
            Event::Velocity(v) => self.player.vel = v,
            Event::Died { player, msg } => {
                if player != self.my_eid && self.my_eid != 0 { return; }
                println!("[death] {msg}");
                self.chat.push((msg.clone(), Instant::now()));
                self.death_msg = Some(msg.clone());
                self.mouse = [false; 2]; self.dig = None; self.chat_input = None;
                if self.auto_respawn { self.respawn_at = Some(Instant::now() + Duration::from_secs(2)); }
                else { self.screen = Some(Screen::Death { msg, at: Instant::now() }); }
            }
            Event::Disconnected(m) => {
                eprintln!("disconnected: {m}");
                self.chat.push((format!("Disconnected: {m}"), Instant::now()));
                self.disconnected = Some(m.clone());
                if self.connected { self.disconnect(); self.screen = Some(Screen::Connecting { addr: self.last_addr.clone(), error: Some(m) }); }
            }
        }
    }

    fn respawn(&mut self) {
        let mut w = Writer::with_id(sb::CLIENT_COMMAND); w.varint(0); send(&self.out, w);
        self.spawned = false;
        if matches!(self.screen, Some(Screen::Death { .. })) { self.screen = None; }
    }

    fn block_sound(&mut self, state: u16, kind: &str, at: [f64; 3]) {
        if state == 0 { return; }
        let g = sound::block_group(basalt_data::BlockState(state as u32).name());
        let (v, p) = match kind { "break" | "place" => (1.0, 0.8), "step" => (0.15, 1.0), _ => (0.25, 0.5) };
        self.sounds.play(&format!("block.{g}.{kind}"), Some(at), v, p);
    }

    fn pump(&mut self) {
        while let Ok((addr, r)) = self.ping_chan.1.try_recv() {
            let st = match r {
                Ok(mut st) => {
                    if let Some(icon) = st.icon.take().filter(|i| i.w == 64 && i.h == 64) {
                        let n = self.icon_slots.len();
                        let slot = *self.icon_slots.entry(addr.clone()).or_insert(n);
                        self.gpu.set_icon(slot, &icon.rgba);
                        st.icon = Some(icon);
                    }
                    status::PingState::Done(st)
                }
                Err(e) => status::PingState::Failed(format!("{e:#}")),
            };
            self.pings.insert(addr, st);
        }
        let start = Instant::now();
        while let Ok(ev) = self.events.try_recv() { self.handle(ev); if start.elapsed() > Duration::from_millis(30) { break; } }
        while let Ok((k, m, dt)) = self.mesher.done.try_recv() {
            self.in_flight.remove(&k);
            self.stats.mesh_ms.push(dt.as_secs_f32() * 1000.0); self.stats.sections_meshed += 1;
            if self.world.chunks.contains_key(&(k.0, k.2)) {
                self.gpu.set_section(k, &m);
                if let Some(t) = self.stats.chunk_arrived.remove(&(k.0, k.2)) { self.stats.chunk_latency_ms.push(t.elapsed().as_secs_f32() * 1000.0); }
            }
        }
        // queue dirty sections, nearest first: those with all horizontal neighbours loaded at once,
        // edge sections (a neighbour missing) after a short wait; they are re-meshed when it arrives
        if self.in_flight.len() < 64 && !self.dirty.is_empty() {
            let (pcx, pcz) = ((self.player.pos[0].floor() as i32) >> 4, (self.player.pos[2].floor() as i32) >> 4);
            let mut ready: Vec<(i32, i32, i32)> = self.dirty.iter().filter(|&(&(cx, sy, cz), t)| {
                !self.in_flight.contains(&(cx, sy, cz)) && (t.elapsed() >= EDGE_MESH_DELAY || [(-1, 0), (1, 0), (0, -1), (0, 1)].iter().all(|(dx, dz)| self.world.chunks.contains_key(&(cx + dx, cz + dz))))
            }).map(|(k, _)| *k).collect();
            ready.sort_by_key(|&(cx, _, cz)| (cx - pcx).pow(2) + (cz - pcz).pow(2));
            for k in ready.into_iter().take(256) {
                self.dirty.remove(&k);
                let Some(c) = self.world.chunks.get(&(k.0, k.2)) else { continue };
                if c.sections[k.1 as usize].is_empty() { self.gpu.sections.remove(&k); continue; }
                self.in_flight.insert(k);
                let _ = self.mesher.jobs.send((k, snapshot(&self.world, k.0, k.1, k.2)));
            }
        }
    }

    fn send_move(&mut self, force: bool) {
        let p = &self.player;
        let flags = p.on_ground as u8 | (p.h_collide as u8) << 1;
        let moved = (0..3).any(|i| (p.pos[i] - self.last_sent.0[i]).abs() > 1e-4) || self.ticks_since_pos >= 20;
        let rotated = p.yaw != self.last_sent.1 || p.pitch != self.last_sent.2;
        let w = if force || (moved && rotated) {
            let mut w = Writer::with_id(sb::MOVE_PLAYER_POS_ROT); w.f64(p.pos[0]).f64(p.pos[1]).f64(p.pos[2]).f32(p.yaw).f32(p.pitch).u8(flags); w
        } else if moved {
            let mut w = Writer::with_id(sb::MOVE_PLAYER_POS); w.f64(p.pos[0]).f64(p.pos[1]).f64(p.pos[2]).u8(flags); w
        } else if rotated {
            let mut w = Writer::with_id(sb::MOVE_PLAYER_ROT); w.f32(p.yaw).f32(p.pitch).u8(flags); w
        } else if p.on_ground != self.last_sent.3 {
            let mut w = Writer::with_id(sb::MOVE_PLAYER_STATUS_ONLY); w.u8(flags); w
        } else { self.ticks_since_pos += 1; return };
        if force || moved { self.ticks_since_pos = 0; } else { self.ticks_since_pos += 1; }
        self.last_sent = (p.pos, p.yaw, p.pitch, p.on_ground);
        send(&self.out, w);
    }

    fn input(&self) -> Input {
        if self.chat_input.is_some() || self.screen.is_some() { return Input::default(); }
        let k = |c: KeyCode| self.keys.contains(&c);
        Input {
            forward: k(KeyCode::KeyW) as i32 as f32 - k(KeyCode::KeyS) as i32 as f32,
            strafe: k(KeyCode::KeyA) as i32 as f32 - k(KeyCode::KeyD) as i32 as f32,
            jump: k(KeyCode::Space), sneak: k(KeyCode::ShiftLeft), sprint: self.sprinting,
        }
    }

    /// Item use that stops / prevents sprinting (eating, drinking, drawing a bow, blocking, ...).
    fn using_item(&self) -> bool {
        if !self.mouse[1] { return false; }
        let h = &self.hotbar[self.held as usize];
        if h.count <= 0 { return false; }
        let name = basalt_data::registries::ITEM.get(h.item as usize).copied().unwrap_or("");
        self.rules.as_ref().and_then(|r| r.item(h.item)).map(|i| i.food.is_some()).unwrap_or(false)
            || matches!(name, "bow" | "crossbow" | "shield" | "trident" | "spyglass" | "potion" | "milk_bucket" | "honey_bottle" | "goat_horn" | "brush")
    }

    /// Vanilla sprint rules, once per tick (behaviour measured against the real client with the
    /// oracle input channel, 2026-11-23): double-tap forward within 7 ticks, or the sprint key
    /// (hold or toggle per options) starts it; once started it lasts until forward is released,
    /// food drops to 6 or less (survival), the player runs into a wall, uses an item or is
    /// in water. Sneaking prevents starting one but does not end it.
    fn update_sprint(&mut self) {
        let in_ui = self.chat_input.is_some() || self.screen.is_some();
        let k = |c: KeyCode| !in_ui && self.keys.contains(&c);
        let fwd = k(KeyCode::KeyW) && !k(KeyCode::KeyS);
        let sneak = k(KeyCode::ShiftLeft);
        let key = k(KeyCode::ControlLeft) || k(KeyCode::KeyR);
        let toggle = self.cfg.extra.get("toggle_sprint").map(|v| *v < 0.5).unwrap_or(false);
        if toggle && key && !self.sprint_key_was { self.sprint_latch = !self.sprint_latch; }
        let wants_key = if toggle { self.sprint_latch } else { key };
        let creative = self.player.gamemode == 1 || self.player.gamemode == 3 || self.player.flying;
        let can = fwd && !sneak && (self.food > 6 || creative) && !self.using_item() && !self.player.h_collide && !self.player.in_water;
        if self.sprint_trigger > 0 { self.sprint_trigger -= 1; }
        if !self.sprinting {
            let pressed = fwd && !self.fwd_was;
            if pressed && self.sprint_trigger > 0 && can { self.sprinting = true; }
            else if pressed { self.sprint_trigger = 7; }
            if wants_key && can { self.sprinting = true; }
        } else if !fwd || (self.food <= 6 && !creative) || self.using_item() || self.player.h_collide || self.player.in_water {
            // (sneaking does not end a sprint in 26.2: the real client keeps sending sprint until forward is released)
            self.sprinting = false;
        }
        self.fwd_was = fwd; self.sprint_key_was = key;
    }

    fn eye(&self, alpha: f64) -> [f64; 3] {
        let p = &self.player;
        let l = |i: usize| p.prev_pos[i] + (p.pos[i] - p.prev_pos[i]) * alpha;
        [l(0), l(1) + p.eye_height(), l(2)]
    }
    fn look_dir(&self) -> [f64; 3] {
        let (y, p) = ((self.player.yaw as f64).to_radians(), (self.player.pitch as f64).to_radians());
        [-y.sin() * p.cos(), -p.sin(), y.cos() * p.cos()]
    }

    fn tick(&mut self) {
        self.ticks += 1;
        let lerping: std::collections::HashSet<i32> = self.entities.values().filter(|e| e.steps > 0).map(|e| e.id).collect();
        for e in self.entities.values_mut() { e.tick(); }
        // items and XP orbs move on their own between the server's (every 20 ticks) syncs, like the
        // vanilla client: without this they hang still after spawning, then jump to the synced spot
        for e in self.entities.values_mut() {
            let k = entity::type_name(e.kind);
            // a tick spent lerping to a server position is that tick's movement
            if lerping.contains(&e.id) || !matches!(k, "item" | "experience_orb") { continue; }
            e.vel[1] -= if k == "item" { 0.04 } else { 0.03 };
            let h = 0.125;
            let bb = [e.pos[0] - h, e.pos[1], e.pos[2] - h, e.pos[0] + h, e.pos[1] + 0.25, e.pos[2] + h];
            let m = player::move_box(&self.assets, &self.world, bb, e.vel);
            let ground = e.vel[1] < 0.0 && m[1] > e.vel[1] + 1e-7;
            for (i, mi) in m.iter().enumerate() { e.pos[i] += mi; if (mi - e.vel[i]).abs() > 1e-7 { e.vel[i] = 0.0; } }
            let slip = if ground {
                let below = self.world.get(e.pos[0].floor() as i32, (e.pos[1] - 0.99).floor() as i32, e.pos[2].floor() as i32);
                match basalt_data::BlockState(below as u32).name() { "ice" | "packed_ice" | "frosted_ice" => 0.98, "blue_ice" => 0.989, "slime_block" => 0.8, _ => 0.6 }
            } else { 1.0 };
            e.vel[0] *= slip * 0.98; e.vel[2] *= slip * 0.98; e.vel[1] *= 0.98;
            e.on_ground = ground;
            e.target = e.pos;
        }
        if self.fp_swing >= 0 { self.fp_swing += 1; if self.fp_swing >= 6 { self.fp_swing = -1; } }
        if !self.particles.is_empty() { self.particles.tick(&self.assets, &self.world); }
        // equip height (measured on the vanilla client, oracle "fp" recorder 2026-11-23): moves at most
        // 0.4/tick towards (attack strength)^3, or towards 0 while a different item is still drawn;
        // the drawn item switches once the hand is down (< 0.1)
        let h = &self.hotbar[self.held as usize];
        let now = if h.count > 0 { h.item } else { -1 };
        if now != self.equip_for {
            self.equip_for = now;
            self.held_name = if now >= 0 { (crate::lang::item_name(now), 40) } else { (String::new(), 0) };
        }
        if self.ticks <= 1 { self.shown_item = now; }
        self.attack_ticks = self.attack_ticks.saturating_add(1);
        let strength = (self.attack_ticks as f32 / attack_cooldown(&self.assets, now)).min(1.0);
        let target = if now != self.shown_item { 0.0 } else { strength.powi(3) };
        self.equip.1 = self.equip.0;
        self.equip.0 += (target - self.equip.0).clamp(-0.4, 0.4);
        if self.equip.0 < 0.1 { self.shown_item = now; }
        if self.self_hurt > 0 { self.self_hurt -= 1; }
        if self.held_name.1 > 0 { self.held_name.1 -= 1; }
        if self.title.2 >= 0 { self.title.2 += 1; let [a, b, c] = self.title.3; if self.title.2 > a + b + c { self.title.2 = -1; self.title.0.clear(); self.title.1.clear(); } }
        if self.actionbar.1 > 0 { self.actionbar.1 -= 1; }
        if self.respawn_at.map(|t| Instant::now() >= t).unwrap_or(false) { self.respawn_at = None; self.respawn(); }
        if !self.spawned || matches!(self.screen, Some(Screen::Death { .. })) { return; }
        if !self.loaded_sent && !self.world.chunks.is_empty() {
            send(&self.out, Writer::with_id(sb::PLAYER_LOADED)); self.loaded_sent = true;
        }
        self.update_sprint();
        let inp = self.input();
        let flags = (inp.forward > 0.0) as u8 | ((inp.forward < 0.0) as u8) << 1 | ((inp.strafe > 0.0) as u8) << 2 | ((inp.strafe < 0.0) as u8) << 3
            | (inp.jump as u8) << 4 | (inp.sneak as u8) << 5 | (self.sprint_key_was as u8) << 6;   // the sprint bit is the key, not the state (oracle client-input)
        if flags != self.last_input_flags { let mut w = Writer::with_id(sb::PLAYER_INPUT); w.u8(flags); send(&self.out, w); self.last_input_flags = flags; }
        // sprinting also sends player_command 1 (start) / 2 (stop), like the vanilla client (oracle client-input, 2026-11-23)
        let sprinting = self.sprinting;
        if sprinting != self.sprint_sent {
            self.sprint_sent = sprinting;
            let mut w = Writer::with_id(sb::PLAYER_COMMAND); w.varint(self.my_eid).varint(if sprinting { 1 } else { 2 }).varint(0); send(&self.out, w);
        }
        let before = self.player.pos;
        self.player.tick(&self.assets, &self.world, inp);
        // view bobbing (vanilla client, oracle "fp" recorder 2026-11-23): the walk distance grows by
        // 0.6 x horizontal distance; the amplitude eases (x0.4/tick) to min(0.1, horizontal speed)
        // on the ground and to 0 in the air
        {
            let hs = ((self.player.pos[0] - before[0]).powi(2) + (self.player.pos[2] - before[2]).powi(2)).sqrt() as f32;
            self.walk_dist.1 = self.walk_dist.0; self.walk_dist.0 += hs * 0.6;
            let target = if self.player.on_ground && !self.player.flying { hs.min(0.1) } else { 0.0 };
            self.view_bob.1 = self.view_bob.0; self.view_bob.0 += (target - self.view_bob.0) * 0.4;
        }
        // footsteps
        if self.player.on_ground && !self.player.flying && !self.player.sneaking {
            self.step_dist += ((self.player.pos[0] - before[0]).powi(2) + (self.player.pos[2] - before[2]).powi(2)).sqrt();
            if self.step_dist > 1.6 {
                self.step_dist = 0.0;
                let p = self.player.pos;
                let below = self.world.get(p[0].floor() as i32, (p[1] - 0.2).floor() as i32, p[2].floor() as i32);
                self.block_sound(below, "step", [p[0], p[1], p[2]]);
            }
        }
        self.send_move(false);
        self.tick_dig();
        send(&self.out, Writer::with_id(sb::CLIENT_TICK_END));
    }

    fn target(&self) -> Option<player::Hit> {
        player::raycast(&self.assets, &self.world, self.eye(1.0), self.look_dir(), if self.player.gamemode == 1 { 5.0 } else { 4.5 })
    }

    fn player_action(&mut self, status: i32, b: [i32; 3], face: u8) {
        self.seq += 1;
        let mut w = Writer::with_id(sb::PLAYER_ACTION); w.varint(status).position(b[0], b[1], b[2]).u8(face).varint(self.seq); send(&self.out, w);
    }

    /// Nearest entity whose box the look ray hits within reach, if closer than the block target.
    fn target_entity(&self) -> Option<(i32, f64)> {
        let (o, d) = (self.eye(1.0), self.look_dir());
        let reach: f64 = if self.player.gamemode == 1 { 5.0 } else { 3.0 };
        let block = self.target().map(|h| { let c = [h.block[0] as f64 + 0.5, h.block[1] as f64 + 0.5, h.block[2] as f64 + 0.5]; ((c[0] - o[0]).powi(2) + (c[1] - o[1]).powi(2) + (c[2] - o[2]).powi(2)).sqrt() - 0.5 }).unwrap_or(f64::MAX);
        let mut best: Option<(i32, f64)> = None;
        for (id, e) in &self.entities {
            if *id == self.my_eid || !entity::is_living(e.kind) || e.death > 0 { continue; }
            let (w, h, _) = entity::dims(e.kind);
            let hw = w as f64 / 2.0;
            let bb = [e.pos[0] - hw, e.pos[1], e.pos[2] - hw, e.pos[0] + hw, e.pos[1] + h as f64, e.pos[2] + hw];
            let (mut t0, mut t1) = (0.0f64, reach);
            let mut ok = true;
            for a in 0..3 {
                if d[a].abs() < 1e-12 { if o[a] < bb[a] || o[a] > bb[a + 3] { ok = false; break; } continue; }
                let (mut x, mut y) = ((bb[a] - o[a]) / d[a], (bb[a + 3] - o[a]) / d[a]);
                if x > y { std::mem::swap(&mut x, &mut y); }
                t0 = t0.max(x); t1 = t1.min(y);
                if t0 > t1 { ok = false; break; }
            }
            if ok && t0 < block && best.map(|b| t0 < b.1).unwrap_or(true) { best = Some((*id, t0)); }
        }
        best
    }

    /// Attack packet (varint entity id) + swing, as the real client sends on left click.
    fn attack(&mut self, id: i32) {
        let mut w = Writer::with_id(sb::ATTACK); w.varint(id); send(&self.out, w);
        self.swing_arm();
        self.attack_ticks = 0;
    }
    /// View-space camera shake: hurt roll and walking bob (vanilla client, oracle "fp" recorder
    /// 2026-11-23). Applied between the view and projection matrices (and so to the hand as well).
    fn camera_bob(&self, alpha: f32) -> Mat4 {
        let mut m = Mat4::IDENTITY;
        if self.self_hurt > 0 {
            // roll -14° x sin(((hurtTime - partial) / 10)^4 pi), hurtTime 10 -> 0
            let f = ((self.self_hurt as f32 - alpha) / 10.0).max(0.0);
            m *= Mat4::from_rotation_z((-14.0 * (f.powi(4) * std::f32::consts::PI).sin()).to_radians());
        }
        if self.cfg.extra.get("view_bobbing").copied().unwrap_or(1.0) > 0.5 && self.player.gamemode != 3 {
            let (w0, w1) = (self.walk_dist.1, self.walk_dist.0);
            let w = -(w1 + (w1 - w0) * alpha);
            let b = self.view_bob.1 + (self.view_bob.0 - self.view_bob.1) * alpha;
            let pi = std::f32::consts::PI;
            m *= Mat4::from_translation(Vec3::new((w * pi).sin() * b * 0.5, -(w * pi).cos().abs() * b, 0.0))
                * Mat4::from_rotation_z(((w * pi).sin() * b * 3.0).to_radians())
                * Mat4::from_rotation_x(((w * pi - 0.2).cos().abs() * b * 5.0).to_radians());
        }
        m
    }

    fn swing_arm(&mut self) {
        let mut w = Writer::with_id(sb::SWING); w.varint(0); send(&self.out, w);
        self.fp_swing = 0;
    }

    /// Test hook: face and attack the nearest non-player living entity within `range`.
    fn attack_nearest(&mut self, range: f64) -> bool {
        let eye = self.eye(1.0);
        let skip = |k: i32| !entity::is_living(k) || entity::type_name(k) == "player";
        let Some((id, pos, h)) = self.entities.iter().filter(|(id, e)| **id != self.my_eid && !skip(e.kind) && e.death == 0)
            .map(|(id, e)| (*id, e.pos, entity::dims(e.kind).1 as f64))
            .min_by(|a, b| { let d = |p: [f64; 3]| (p[0] - eye[0]).powi(2) + (p[2] - eye[2]).powi(2); d(a.1).partial_cmp(&d(b.1)).unwrap() }) else { return false };
        let (dx, dy, dz) = (pos[0] - eye[0], pos[1] + h * 0.5 - eye[1], pos[2] - eye[2]);
        if (dx * dx + dy * dy + dz * dz).sqrt() > range { return false; }
        self.player.yaw = (-dx).atan2(dz).to_degrees() as f32;
        self.player.pitch = (-(dy.atan2((dx * dx + dz * dz).sqrt())).to_degrees()) as f32;
        self.attack(id);
        true
    }

    /// Tick-based digging like the vanilla client: progress accumulates per tick, the finish packet
    /// goes out on the tick it reaches 1, then a 5-tick delay before the next block.
    fn tick_dig(&mut self) {
        if self.dig_delay > 0 { self.dig_delay -= 1; }
        if !self.mouse[0] || self.chat_input.is_some() || self.screen.is_some() {
            if let Some(d) = self.dig.take() { self.player_action(1, d.block, 0); }   // vanilla aborts with face DOWN (oracle client-dig)
            return;
        }
        let hit = self.target();
        let same = matches!((&self.dig, &hit), (Some(d), Some(h)) if d.block == h.block);
        if same {
            let d = self.dig.as_mut().unwrap();
            d.ticks += 1;
            let (b, f, ticks, total) = (d.block, d.face, d.ticks, d.total);
            if ticks % 4 == 0 { let st = self.world.get(b[0], b[1], b[2]); self.block_sound(st, "hit", [b[0] as f64 + 0.5, b[1] as f64 + 0.5, b[2] as f64 + 0.5]); }
            // vanilla sends a swing every tick while digging; the animation only restarts once half done
            if self.fp_swing < 0 || self.fp_swing >= 3 { self.swing_arm(); } else { let mut w = Writer::with_id(sb::SWING); w.varint(0); send(&self.out, w); }
            { let st = self.world.get(b[0], b[1], b[2]); let tint = self.particle_tint(st, b); self.particles.block_hit(&self.assets, st, b, f, tint); }
            if ticks >= total {
                self.player_action(2, b, f);
                self.break_local(b);
                self.last_dig = Some((ticks, b));
                self.dig = None; self.dig_delay = 5;
            }
            return;
        }
        if let Some(d) = self.dig.take() { self.player_action(1, d.block, 0); }   // vanilla aborts with face DOWN (oracle client-dig)
        let Some(h) = hit else { return };
        // between blocks (5-tick delay) the vanilla client keeps swinging every tick (oracle client-dig)
        if self.dig_delay > 0 { let mut w = Writer::with_id(sb::SWING); w.varint(0); send(&self.out, w); if self.fp_swing < 0 || self.fp_swing >= 3 { self.fp_swing = 0; } return; }
        self.player_action(0, h.block, h.face);
        self.swing_arm();
        if self.player.gamemode == 1 {
            // creative: instant break, no abort/finish follows
            self.break_local(h.block);
            self.dig_delay = 5;
        } else {
            let total = self.dig_ticks(h.block);
            if total <= 1 { self.break_local(h.block); self.last_dig = Some((total, h.block)); self.dig_delay = 5; }
            else { self.dig = Some(Dig { block: h.block, face: h.face, ticks: 0, total }); }
        }
    }

    /// Colour block particles take: the block's biome tint, except grass blocks (their chips are dirt).
    fn particle_tint(&self, st: u16, b: [i32; 3]) -> [u8; 3] {
        use assets::Tint;
        let biomes = self.biomes.lock().unwrap().clone();
        let bi = biomes.get(self.world.biome(b[0], b[1], b[2]) as usize);
        let c = match self.assets.info(st).tint {
            Tint::None => return [255; 3],
            _ if basalt_data::BlockState(st as u32).name() == "grass_block" => return [255; 3],
            Tint::Grass => bi.map(|b| b.grass).unwrap_or(0x79C05A), Tint::Foliage => bi.map(|b| b.foliage).unwrap_or(0x59AE30),
            Tint::Water => bi.map(|b| b.water).unwrap_or(0x3F76E4), Tint::Fixed(c) => c, Tint::DryFoliage => bi.map(|b| b.dry_foliage).unwrap_or(0x7B5334),
        };
        [(c >> 16) as u8, (c >> 8) as u8, c as u8]
    }

    fn break_local(&mut self, b: [i32; 3]) {
        let st = self.world.get(b[0], b[1], b[2]);
        self.block_sound(st, "break", [b[0] as f64 + 0.5, b[1] as f64 + 0.5, b[2] as f64 + 0.5]);
        let tint = self.particle_tint(st, b);
        self.particles.block_break(&self.assets, st, b, tint);
        self.world.set(b[0], b[1], b[2], 0); self.mark_block(b[0], b[1], b[2]);
    }

    /// Survival break time in ticks for the block with the held item (vanilla formula via basalt-game).
    fn dig_ticks(&self, b: [i32; 3]) -> u32 {
        let state = self.world.get(b[0], b[1], b[2]) as u32;
        let held = self.hotbar[self.held as usize].clone();
        let held = (held.count > 0).then_some(held.item);
        let eye = self.eye(1.0);
        let in_water = self.assets.info(self.world.get(eye[0].floor() as i32, eye[1].floor() as i32, eye[2].floor() as i32)).fluid == 1;
        match &self.rules {
            Some(r) => r.dig_ticks(state, held, self.player.on_ground, in_water).unwrap_or(u32::MAX),
            None => 24,
        }
    }

    fn use_item(&mut self) {
        self.last_place = Instant::now();
        let Some(h) = self.target() else {
            self.seq += 1; let mut w = Writer::with_id(sb::USE_ITEM); w.varint(0).varint(self.seq).f32(self.player.yaw).f32(self.player.pitch); send(&self.out, w); return;
        };
        self.seq += 1;
        let c = [(h.point[0] - h.block[0] as f64) as f32, (h.point[1] - h.block[1] as f64) as f32, (h.point[2] - h.block[2] as f64) as f32];
        let mut w = Writer::with_id(sb::USE_ITEM_ON);
        w.varint(0).position(h.block[0], h.block[1], h.block[2]).varint(h.face as i32).f32(c[0]).f32(c[1]).f32(c[2]).bool(false).bool(false).varint(self.seq);
        send(&self.out, w);
        self.swing_arm();
        // place sound for block items (the server sends it only to other players)
        let held = self.hotbar[self.held as usize].clone();
        if held.count > 0 {
            let name = basalt_data::registries::ITEM.get(held.item as usize).copied().unwrap_or("");
            if let Some(st) = basalt_data::BlockState::from_name(name) {
                let d = assets::DIR_VEC[h.face as usize];
                self.block_sound(st.0 as u16, "place", [h.block[0] as f64 + 0.5 + d[0] as f64, h.block[1] as f64 + 0.5 + d[1] as f64, h.block[2] as f64 + 0.5 + d[2] as f64]);
            }
        }
    }

    fn send_chat(&mut self, msg: &str) {
        if msg.is_empty() { return; }
        if let Some(cmd) = msg.strip_prefix('/') {
            let mut w = Writer::with_id(sb::CHAT_COMMAND); w.str(cmd); send(&self.out, w);
        } else {
            let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
            let mut w = Writer::with_id(sb::CHAT);
            // message, timestamp, salt, no signature, last-seen offset 0, 20-bit acknowledged set, checksum
            w.str(msg).i64(ts).i64(0).bool(false).varint(0).bytes(&[0, 0, 0]).u8(1);
            send(&self.out, w);
        }
    }

    fn set_held(&mut self, s: u8) { self.held = s; let mut w = Writer::with_id(sb::SET_CARRIED_ITEM); w.i16(s as i16); send(&self.out, w); }

    // ------------------------------------------------------------ input (shared by the window and scenarios)

    /// Whether the mouse should be captured for looking around.
    pub fn wants_grab(&self) -> bool { self.screen.is_none() && self.chat_input.is_none() && self.connected && self.spawned }

    /// Recompute the popup for the current chat text (and ask the server where the tree says so).
    fn update_suggestions(&mut self) {
        let Some(text) = self.chat_input.clone() else { self.sug = Sug { query_id: self.sug.query_id, ..Default::default() }; return };
        if self.sug.cycling && text == self.sug.for_text { return; }
        self.sug.cycling = false;
        self.sug.list.clear(); self.sug.sel = 0; self.sug.for_text = text.clone();
        let Some(tree) = &self.cmd_tree else { return };
        let players: Vec<String> = self.player_names.values().cloned().collect();
        let Some(o) = suggest::suggest(tree, &text, &players) else { return };
        self.sug.anchor = o.anchor; self.sug.list = o.list;
        if o.ask_server {
            self.sug.query_id += 1;
            let mut w = Writer::with_id(sb::COMMAND_SUGGESTION); w.varint(self.sug.query_id).str(&text); send(&self.out, w);
        }
    }

    /// Tab: put the selected suggestion into the box; pressing it again cycles through the list.
    fn complete_suggestion(&mut self, step: isize) {
        if self.sug.list.is_empty() { return; }
        let n = self.sug.list.len();
        let sel = if self.sug.cycling { (self.sug.sel as isize + step).rem_euclid(n as isize) as usize } else { self.sug.sel };
        let Some(buf) = &mut self.chat_input else { return };
        let cut = 1 + self.sug.anchor;
        if cut > buf.len() { return; }
        buf.truncate(cut); buf.push_str(&self.sug.list[sel]);
        self.sug.sel = sel; self.sug.cycling = true; self.sug.for_text = buf.clone();
    }

    pub fn key(&mut self, code: KeyCode, pressed: bool, text: Option<&str>) {
        if pressed { self.keys.insert(code); } else { self.keys.remove(&code); }
        if !pressed { return; }
        if let Some(buf) = &mut self.chat_input {
            match code {
                KeyCode::Escape => self.chat_input = None,
                KeyCode::Enter | KeyCode::NumpadEnter => { let m = self.chat_input.take().unwrap(); self.send_chat(&m); }
                KeyCode::Backspace => { buf.pop(); }
                KeyCode::Tab => { let back = self.keys.contains(&KeyCode::ShiftLeft); self.complete_suggestion(if back { -1 } else { 1 }); return; }
                KeyCode::ArrowDown | KeyCode::ArrowUp if !self.sug.list.is_empty() => {
                    let n = self.sug.list.len() as isize; let d = if code == KeyCode::ArrowDown { 1 } else { -1 };
                    self.sug.sel = (self.sug.sel as isize + d).rem_euclid(n) as usize; return;
                }
                _ => if let Some(t) = text { for c in t.chars() { if !c.is_control() && buf.len() < 256 { buf.push(c); } } },
            }
            self.update_suggestions();
            return;
        }
        if code == KeyCode::F11 { self.cfg.fullscreen = !self.cfg.fullscreen; self.cfg.save(); self.fullscreen_changed = true; return; }
        match &self.screen {
            Some(Screen::Container(_)) => { self.container_key(code, text); return; }
            Some(_) => { self.menu_key(code, text); return; }
            None => {}
        }
        match code {
            KeyCode::KeyT => { self.chat_input = Some(String::new()); self.update_suggestions(); }
            KeyCode::Slash => { self.chat_input = Some("/".into()); self.update_suggestions(); }
            KeyCode::F3 => self.debug = !self.debug,
            KeyCode::KeyE => self.open_inventory(),
            KeyCode::KeyQ => {
                // drop one (ctrl: the whole stack) from the selected hotbar slot
                let all = self.keys.contains(&KeyCode::ControlLeft) || self.keys.contains(&KeyCode::SuperLeft);
                if self.hotbar[self.held as usize].count > 0 {
                    self.player_action(if all { 3 } else { 4 }, [0, 0, 0], 0);
                    let h = &mut self.hotbar[self.held as usize]; h.count = if all { 0 } else { h.count - 1 };
                    self.swing_arm();
                }
            }
            KeyCode::KeyF => self.player_action(6, [0, 0, 0], 0),
            KeyCode::Space if self.player.gamemode == 1 => {
                // double-tap jump within 7 ticks toggles flight (vanilla's window)
                if self.jump_tick.map(|t| self.ticks.saturating_sub(t) < 7).unwrap_or(false) {
                    self.player.flying = !self.player.flying; self.jump_tick = None;
                    let mut w = Writer::with_id(sb::PLAYER_ABILITIES); w.u8(if self.player.flying { 2 } else { 0 }); send(&self.out, w);
                } else { self.jump_tick = Some(self.ticks); }
            }
            KeyCode::Escape => { if self.connected { self.screen = Some(Screen::Pause); self.mouse = [false; 2]; } }
            _ => {
                let digits = [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4, KeyCode::Digit5, KeyCode::Digit6, KeyCode::Digit7, KeyCode::Digit8, KeyCode::Digit9];
                if let Some(i) = digits.iter().position(|d| *d == code) { self.set_held(i as u8); }
            }
        }
    }

    /// Mouse button: 0 left, 1 right, 2 middle.
    pub fn mouse_button(&mut self, button: u8, pressed: bool) {
        match &self.screen {
            Some(Screen::Container(_)) => { self.container_mouse(button, pressed); return; }
            Some(_) => { self.menu_mouse(button, pressed); return; }
            None => {}
        }
        if self.chat_input.is_some() { return; }
        match button {
            0 => { if pressed { if let Some((id, _)) = self.target_entity() { self.attack(id); } else {
                // attacking the air swings too and resets the attack strength (vanilla client, oracle "fp" recorder)
                if self.screen.is_none() && self.target().is_none() { self.swing_arm(); self.attack_ticks = 0; }
                self.mouse[0] = true; } } else { self.mouse[0] = false; } }
            1 => { self.mouse[1] = pressed; if pressed { self.use_item(); } }
            _ => {}
        }
    }
    /// Absolute cursor position in window pixels.
    pub fn mouse_move(&mut self, x: f32, y: f32) {
        self.mouse_pos = (x, y);
        match &self.screen { Some(Screen::Container(_)) => self.container_motion(), Some(_) => self.menu_motion(), None => {} }
    }
    pub fn mouse_look(&mut self, dx: f64, dy: f64) {
        let f = self.cfg.mouse_factor();
        self.player.yaw += dx as f32 * f;
        self.player.pitch = (self.player.pitch + dy as f32 * f).clamp(-90.0, 90.0);
    }
    pub fn scroll(&mut self, dy: f32) {
        if self.screen.is_some() { self.menu_scroll(dy); return; }
        if dy.abs() < 0.5 { return; }
        let s = (self.held as i32 - dy.signum() as i32).rem_euclid(9) as u8; self.set_held(s);
    }

    // ------------------------------------------------------------ drawing

    fn atlas_uv(&self, name: &str) -> [f32; 4] {
        let a = &self.assets.atlas;
        let r = a.rects.get(name).or(a.rects.get("missing")).map(|r| r.0).unwrap_or_default();
        [(r.x as f32 + 0.5) / a.w as f32, (r.y as f32 + 0.5) / a.h as f32, (r.x + r.w) as f32 / a.w as f32 - 0.5 / a.w as f32, (r.y + r.h) as f32 / a.h as f32 - 0.5 / a.h as f32]
    }

    fn cuboid(m: &mut MeshData, lo: [f32; 3], hi: [f32; 3], uv: [f32; 4], color: [u8; 4], light: u8) {
        const FACES: [[usize; 12]; 6] = [
            [0, 0, 1, 0, 0, 0, 1, 0, 0, 1, 0, 1], [0, 1, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0],
            [1, 1, 0, 1, 0, 0, 0, 0, 0, 0, 1, 0], [0, 1, 1, 0, 0, 1, 1, 0, 1, 1, 1, 1],
            [0, 1, 0, 0, 0, 0, 0, 0, 1, 0, 1, 1], [1, 1, 1, 1, 0, 1, 1, 0, 0, 1, 1, 0],
        ];
        for f in FACES {
            let b = m.verts.len() as u32;
            for i in 0..4 {
                let p = [if f[i * 3] == 1 { hi[0] } else { lo[0] }, if f[i * 3 + 1] == 1 { hi[1] } else { lo[1] }, if f[i * 3 + 2] == 1 { hi[2] } else { lo[2] }];
                let uvc = [[uv[0], uv[1]], [uv[0], uv[3]], [uv[2], uv[3]], [uv[2], uv[1]]][i];
                m.verts.push(Vertex { pos: p, uv: uvc, color, light: [light, light, 0, 0] });
            }
            m.idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
        }
    }

    /// Crack overlay on a block: its model faces re-textured with destroy_stage_N, pushed out a hair.
    fn crack(&self, m: &mut MeshData, b: [i32; 3], stage: i8) {
        let Some(uv) = self.assets.tex_uv(&format!("block/destroy_stage_{}", stage.clamp(0, 9))) else { return };
        let st = self.world.get(b[0], b[1], b[2]);
        if st == 0 { return; }
        let model = self.assets.model(st);
        let mut quads: Vec<([[f32; 3]; 4], u8)> = model.quads.iter().map(|q| (q.pos, q.dir)).collect();
        if quads.is_empty() {
            let mut cube = MeshData::default();
            Self::cuboid(&mut cube, [0.0; 3], [1.0; 3], [0.0; 4], [0; 4], 0);
            for (i, f) in cube.verts.chunks(4).enumerate() { quads.push(([f[0].pos, f[1].pos, f[2].pos, f[3].pos], [0u8, 1, 2, 3, 4, 5][i])); }
        }
        let (sl, bl) = self.world.light(b[0], b[1] + 1, b[2]);
        for (pos, dir) in quads {
            let d = assets::DIR_VEC[dir as usize % 6];
            let base = m.verts.len() as u32;
            for p in pos {
                let (u, v) = match dir { 0 | 1 => (p[0], p[2]), 2 | 3 => (p[0], 1.0 - p[1]), _ => (p[2], 1.0 - p[1]) };
                let (u, v) = (u.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
                m.verts.push(Vertex {
                    pos: [b[0] as f32 + p[0] + d[0] as f32 * 0.002, b[1] as f32 + p[1] + d[1] as f32 * 0.002, b[2] as f32 + p[2] + d[2] as f32 * 0.002],
                    uv: [uv[0] + (uv[2] - uv[0]) * u, uv[1] + (uv[3] - uv[1]) * v], color: [255, 255, 255, 230], light: [sl.max(bl) * 17, bl * 17, 0, 0],
                });
            }
            m.idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }

    /// Current local dig stage (0..9), if any.
    pub fn dig_stage(&self) -> Option<i8> {
        let d = self.dig.as_ref()?;
        let s = (d.ticks as f32 / d.total.max(1) as f32 * 10.0) as i32 - 1;
        (s >= 0).then_some(s.min(9) as i8)
    }

    fn build_hud(&self, ui: &mut MeshData, items: &mut MeshData, deco: &mut MeshData, vp: &Mat4, alpha: f32) {
        let (w, h) = (self.gpu.size.0 as f32, self.gpu.size.1 as f32);
        let s = self.gui_scale();
        let a = &self.ui;
        let in_game = self.connected && self.spawned;
        if in_game {
            // name tags of other players (screen-space, scaled by distance)
            let eye = self.eye(alpha as f64);
            for e in self.entities.values() {
                if entity::type_name(e.kind) != "player" || e.id == self.my_eid || self.hide_entities { continue; }
                let Some(name) = self.player_names.get(&e.uuid) else { continue };
                let p = e.render_pos(alpha);
                let d = ((p[0] - eye[0]).powi(2) + (p[1] - eye[1]).powi(2) + (p[2] - eye[2]).powi(2)).sqrt();
                if d > 64.0 { continue; }
                let c = *vp * glam::Vec4::new((p[0] - eye[0]) as f32, (p[1] + 2.05 - eye[1]) as f32, (p[2] - eye[2]) as f32, 1.0);
                if c.w <= 0.1 { continue; }
                let (sx, sy) = ((c.x / c.w * 0.5 + 0.5) * w, (0.5 - c.y / c.w * 0.5) * h);
                let ts = (h / 240.0 * 6.0 / d.max(1.0) as f32).clamp(0.5, 3.0);
                let tw = a.text_width(name, ts);
                a.rect(ui, sx - tw / 2.0 - ts, sy - ts, tw + 2.0 * ts, 10.0 * ts, [0, 0, 0, 64]);
                a.text_ex(ui, name, sx - tw / 2.0, sy, ts, [255; 4], false);
            }
            // crosshair
            if let Some(r) = a.rects.get("crosshair") { a.sprite(ui, "crosshair", ((w - r[2] as f32 * s) / 2.0).floor(), ((h - r[3] as f32 * s) / 2.0).floor(), s); }
            // hotbar
            let hx = ((w - 182.0 * s) / 2.0).floor(); let hy = h - 22.0 * s;
            a.sprite(ui, "hotbar", hx, hy, s);
            a.sprite(ui, "hotbar_selection", hx - s + self.held as f32 * 20.0 * s, hy - s, s);
            for (i, it) in self.hotbar.iter().enumerate() {
                if it.count <= 0 { continue; }
                let (x, y) = (hx + (3.0 + i as f32 * 20.0) * s, hy + 3.0 * s);
                ui::item_icon(&self.assets, items, it.item, x, y, s);
                ui::item_decorations(a, deco, it, self.max_damage(it.item), x, y, s);
            }
            // title (4x) and subtitle (2x) centred, fading in/out; action bar above the hotbar
            if self.title.2 >= 0 && self.screen.is_none() {
                let [fi, st, fo] = self.title.3;
                let t = self.title.2 as f32 + alpha;
                let k = if t < fi as f32 { t / fi.max(1) as f32 } else if t > (fi + st) as f32 { 1.0 - (t - (fi + st) as f32) / fo.max(1) as f32 } else { 1.0 };
                let al = (k.clamp(0.0, 1.0) * 255.0) as u8;
                if al > 8 {
                    let (cx, cy) = (w / 2.0, h / 2.0);
                    let tw = a.text_width(&self.title.0, s * 4.0);
                    a.text(deco, &self.title.0, (cx - tw / 2.0).floor(), cy - 10.0 * 4.0 * s, s * 4.0, [255, 255, 255, al]);
                    let sw = a.text_width(&self.title.1, s * 2.0);
                    a.text(deco, &self.title.1, (cx - sw / 2.0).floor(), cy + 5.0 * 2.0 * s, s * 2.0, [255, 255, 255, al]);
                }
            }
            if self.actionbar.1 > 0 && !self.actionbar.0.is_empty() {
                let al = ((self.actionbar.1 as f32 - alpha) * 256.0 / 20.0).clamp(0.0, 255.0) as u8;
                let tw = a.text_width(&self.actionbar.0, s);
                a.text(deco, &self.actionbar.0, ((w - tw) / 2.0).floor(), h - 68.0 * s, s, [255, 255, 255, al]);
            }
            // name of a newly selected item above the hotbar: 2 s, fading over the last half second
            if self.held_name.1 > 0 && !self.held_name.0.is_empty() && self.screen.is_none() {
                let alpha = (self.held_name.1 * 256 / 10).min(255) as u8;
                let ty = h - (if self.player.gamemode == 1 || self.player.gamemode == 3 { 45.0 } else { 59.0 }) * s;
                let tw = a.text_width(&self.held_name.0, s);
                a.text(deco, &self.held_name.0, ((w - tw) / 2.0).floor(), ty, s, [255, 255, 255, alpha]);
            }
            if !self.hide_gui { self.board.draw_bars(a, ui, w, h, s); self.board.draw_sidebar(a, ui, w, h, s); }
            // hearts + food (survival / adventure)
            if self.player.gamemode == 0 || self.player.gamemode == 2 {
                // experience bar and level, as laid out by vanilla: 182x5 bar 29 above the bottom,
                // level number in green with a black outline above it
                a.sprite(ui, "experience_bar_background", hx, h - 29.0 * s, s);
                let fill = (self.xp.0.clamp(0.0, 1.0) * 183.0).floor();
                if fill > 0.0 { a.region(ui, "experience_bar_progress", hx, h - 29.0 * s, 0.0, 0.0, fill, 5.0, s); }
                if self.xp.1 > 0 {
                    let t = self.xp.1.to_string();
                    let tx = ((w / s - a.text_width(&t, 1.0)) / 2.0).floor();
                    let ty = h / s - 35.0;
                    for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] { a.text_ex(ui, &t, (tx + dx) * s, (ty + dy) * s, s, [0, 0, 0, 255], false); }
                    a.text_ex(ui, &t, tx * s, ty * s, s, [128, 255, 32, 255], false);
                }
                let y = h - 39.0 * s;
                for i in 0..10 {
                    let x = hx + i as f32 * 8.0 * s;
                    a.sprite(ui, "heart/container", x, y, s);
                    let v = self.hp - i as f32 * 2.0;
                    if v >= 2.0 { a.sprite(ui, "heart/full", x, y, s); } else if v >= 1.0 { a.sprite(ui, "heart/half", x, y, s); }
                    let fx = hx + 182.0 * s - (i as f32 + 1.0) * 8.0 * s - s;
                    a.sprite(ui, "food_empty", fx, y, s);
                    let f = self.food - i * 2;
                    if f >= 2 { a.sprite(ui, "food_full", fx, y, s); } else if f >= 1 { a.sprite(ui, "food_half", fx, y, s); }
                }
            }
        }
        // chat
        let open = self.chat_input.is_some();
        let mut y = h - 40.0 * s;
        if let Some(buf) = &self.chat_input {
            a.rect(ui, 2.0 * s, h - 14.0 * s, w - 4.0 * s, 12.0 * s, [0, 0, 0, 128]);
            a.text(ui, &format!("{buf}_"), 4.0 * s, h - 12.0 * s, s, [255; 4]);
            // suggestion popup above the input line, anchored at the word being replaced
            if !self.sug.list.is_empty() && buf.starts_with('/') {
                const ROWS: usize = 10;
                let (lo, hi) = suggest::window(self.sug.list.len(), self.sug.sel, ROWS);
                let x0 = 4.0 * s + a.text_width(&buf[..(1 + self.sug.anchor).min(buf.len())], s) - s;
                let wmax = self.sug.list[lo..hi].iter().map(|t| a.text_width(t, s)).fold(0.0f32, f32::max) + 2.0 * s;
                let top = h - 14.0 * s - (hi - lo) as f32 * 12.0 * s;
                a.rect(ui, x0, top, wmax, (hi - lo) as f32 * 12.0 * s, [0, 0, 0, 200]);
                for (row, idx) in (lo..hi).enumerate() {
                    let col = if idx == self.sug.sel { [255, 255, 85, 255] } else { [170, 170, 170, 255] };
                    a.text(ui, &self.sug.list[idx], x0 + s, top + row as f32 * 12.0 * s + 2.0 * s, s, col);
                }
            }
        }
        if in_game {
            for (msg, t) in self.chat.iter().rev().take(if open { 20 } else { 10 }) {
                let age = t.elapsed().as_secs_f32();
                if !open && age > 10.0 { break; }
                let alpha = if open { 255 } else { ((10.0 - age).min(1.0) * 255.0) as u8 };
                a.rect(ui, 2.0 * s, y - s, 320.0 * s, 9.0 * s, [0, 0, 0, alpha / 2]);
                a.text(ui, msg, 3.0 * s, y, s, [255, 255, 255, alpha]);
                y -= 9.0 * s;
            }
            // debug
            let p = &self.player;
            let mut lines = vec![format!("Basalt 26.2  {:.0} fps", self.frames.2)];
            if self.debug {
                lines.push(format!("XYZ: {:.3} / {:.3} / {:.3}", p.pos[0], p.pos[1], p.pos[2]));
                lines.push(format!("Facing: yaw {:.1} pitch {:.1}", p.yaw.rem_euclid(360.0), p.pitch));
                lines.push(format!("Chunks: {} loaded, {} sections drawn, {} pending", self.world.chunks.len(), self.gpu.sections.len(), self.dirty.len() + self.in_flight.len()));
                lines.push(format!("Entities: {}  Time: {}", self.entities.len(), self.day_time.rem_euclid(24000)));
                if let Some(h) = self.target() { lines.push(format!("Target: {} @ {:?}", basalt_data::BlockState(self.world.get(h.block[0], h.block[1], h.block[2]) as u32).name(), h.block)); }
            }
            for (i, l) in lines.iter().enumerate() { a.text(ui, l, 2.0 * s, 2.0 * s + i as f32 * 10.0 * s, s, [255; 4]); }
        }
    }

    /// View-projection matrix (camera at the origin, world coordinates relative to the eye).
    pub fn view_proj(&self) -> Mat4 {
        let (w, h) = (self.gpu.size.0 as f32, self.gpu.size.1 as f32);
        let d = self.look_dir();
        let view = Mat4::look_to_rh(Vec3::ZERO, Vec3::new(d[0] as f32, d[1] as f32, d[2] as f32), Vec3::Y);
        Mat4::perspective_rh(self.cfg.fov.to_radians(), w / h.max(1.0), 0.05, 1024.0) * view
    }
    /// Screen position (pixels) of a world point, if in front of the camera.
    pub fn project(&self, p: [f64; 3]) -> Option<(f32, f32)> {
        let e = self.eye(1.0);
        let c = self.view_proj() * glam::Vec4::new((p[0] - e[0]) as f32, (p[1] - e[1]) as f32, (p[2] - e[2]) as f32, 1.0);
        if c.w <= 0.05 { return None; }
        Some(((c.x / c.w * 0.5 + 0.5) * self.gpu.size.0 as f32, (0.5 - c.y / c.w * 0.5) * self.gpu.size.1 as f32))
    }

    fn frame(&self, alpha: f64) -> FrameData {
        let (w, h) = (self.gpu.size.0 as f32, self.gpu.size.1 as f32);
        let eye = self.eye(alpha);
        let d = self.look_dir();
        let view = Mat4::look_to_rh(Vec3::ZERO, Vec3::new(d[0] as f32, d[1] as f32, d[2] as f32), Vec3::Y);
        let proj = Mat4::perspective_rh(self.cfg.fov.to_radians(), w / h.max(1.0), 0.05, 1024.0);
        let vp = proj * if self.connected && self.spawned { self.camera_bob(alpha as f32) } else { Mat4::IDENTITY } * view;
        let render_dist: f32 = self.cfg.render_distance.clamp(2, 32) as f32 * 16.0;
        let biomes = self.biomes.lock().unwrap().clone();
        let bi = self.world.biome(eye[0].floor() as i32, eye[1].floor() as i32, eye[2].floor() as i32) as usize;
        let (sky, fog) = biomes.get(bi).map(|b| (b.sky, b.fog)).unwrap_or((0x78A7FF, 0xC0D8FF));
        let day_time = self.fixed_time.unwrap_or(self.day_time);
        let theta = (day_time.rem_euclid(24000) as f32 / 24000.0) * std::f32::consts::TAU;
        let sun = [theta.cos(), theta.sin(), 0.0];
        let daylight = (0.5 + 2.0 * theta.sin()).clamp(0.2, 1.0);
        let col = |c: u32, k: f32| [(c >> 16 & 255) as f32 / 255.0 * k, (c >> 8 & 255) as f32 / 255.0 * k, (c & 255) as f32 / 255.0 * k, 1.0];
        let skyless = self.dim.as_ref().is_some_and(|d| !d.has_skylight);
        // shader flag: ambient light >= 0 means "no sky" (sky = fog colour, no sun, no sky light)
        let ambient = if skyless { self.dim.as_ref().map(|d| d.ambient_light).unwrap_or(0.0) } else { -1.0 };
        let daylight = if skyless { 1.0 } else { daylight };
        let dk = 0.05 + 0.95 * daylight;
        let underwater = self.assets.info(self.world.get(eye[0].floor() as i32, eye[1].floor() as i32, eye[2].floor() as i32)).fluid == 1;
        let (mut fog_start, mut fog_end, fogc) = if std::env::var("BASALT_NOFOG").is_ok() { (1e6, 1e6 + 1.0, col(fog, dk)) } else if underwater { (0.0, 24.0, col(0x0A2A6A, dk)) } else { (render_dist - (render_dist * 0.1).clamp(4.0, 64.0), render_dist, col(fog, dk)) };
        // sky-less dimensions (the nether): environment fog distances of the dimension type, no sky
        if let (false, Some((a, b))) = (underwater || std::env::var("BASALT_NOFOG").is_ok(), self.dim.as_ref().filter(|d| !d.has_skylight).and_then(|d| d.fog)) { fog_start = a.min(render_dist); fog_end = b.min(render_dist); }
        let sky = if skyless { fog } else { sky };
        let u = Uniforms {
            vp: vp.to_cols_array_2d(), inv_vp: vp.inverse().to_cols_array_2d(),
            cam: [eye[0] as f32, eye[1] as f32, eye[2] as f32, 0.0], fog_color: fogc, sky_color: col(sky, dk),
            params: [fog_start, fog_end, daylight, day_time as f32], screen: [w, h, underwater as u8 as f32, ambient], sun: [sun[0], sun[1], sun[2], 0.0],
        };
        let cam = Vec3::new(eye[0] as f32, eye[1] as f32, eye[2] as f32);
        let mut fd = FrameData { u, cam, vp, render_dist, extra: MeshData::default(), skinned: MeshData::default(), items3d: MeshData::default(), overlay: MeshData::default(), hand_skin: MeshData::default(), hand_items: MeshData::default(), layers: Vec::new(), backdrop: self.backdrop(), blur: self.cfg.extra.get("menu_blur").copied().unwrap_or(5.0) };
        let in_world = self.connected && self.spawned;
        if in_world {
            // block outline
            let dark = self.atlas_uv("block/black_concrete");
            if self.screen.is_none() {
                if let Some(hit) = self.target() {
                    let t = 0.004;
                    for b in &hit.boxes {
                        let lo = [b[0] as f32 - t, b[1] as f32 - t, b[2] as f32 - t]; let hi = [b[3] as f32 + t, b[4] as f32 + t, b[5] as f32 + t];
                        for ax in 0..3 { for c1 in 0..2 { for c2 in 0..2 {
                            let (a1, a2) = match ax { 0 => (1, 2), 1 => (0, 2), _ => (0, 1) };
                            let mut l = lo; let mut hh = hi;
                            let v1 = if c1 == 0 { lo[a1] } else { hi[a1] }; let v2 = if c2 == 0 { lo[a2] } else { hi[a2] };
                            l[a1] = v1 - t; hh[a1] = v1 + t; l[a2] = v2 - t; hh[a2] = v2 + t;
                            Self::cuboid(&mut fd.extra, l, hh, dark, [30, 30, 30, 255], 255);
                        } } }
                    }
                }
            }
            // block-breaking cracks: own dig + other players' block_destruction
            if !self.hide_cracks {
                if let (Some(d), Some(st)) = (&self.dig, self.dig_stage()) { self.crack(&mut fd.overlay, d.block, st); }
                for (id, (b, st)) in &self.destroy { if *id != self.my_eid { self.crack(&mut fd.overlay, *b, *st); } }
            }
            // entities
            let wool = self.atlas_uv("block/white_wool");
            let a = alpha as f32;
            if !self.hide_entities {
                for (id, e) in &self.entities {
                    if *id == self.my_eid { continue; }
                    let p = e.render_pos(a);
                    let (sl, bl) = self.world.light(p[0].floor() as i32, (p[1] + 0.5).floor() as i32, p[2].floor() as i32);
                    let light = [sl * 17, bl * 17];
                    let name = entity::type_name(e.kind);
                    if name == "item" { entity::build_item(&self.assets, e, a, light, &mut fd.items3d); continue; }
                    if name == "falling_block" { entity::build_falling_block(&self.assets, e, a, light, &mut fd.items3d); continue; }
                    if name == "experience_orb" { entity::build_orb(&self.entity_atlas, e, a, self.player.yaw, self.player.pitch, light, &mut fd.skinned); continue; }
                    if !entity::build_model(&self.entity_atlas, e, a, light, &mut fd.skinned) {
                        let (bw, bh, color) = entity::dims(e.kind);
                        let hw = bw / 2.0;
                        let p = [p[0] as f32, p[1] as f32, p[2] as f32];
                        Self::cuboid(&mut fd.extra, [p[0] - hw, p[1], p[2] - hw], [p[0] + hw, p[1] + bh, p[2] + hw], wool, color, (sl.max(bl) * 17).max(40));
                    }
                }
            }
            self.particles.build(&self.world, self.player.yaw, self.player.pitch, a, &mut fd.items3d);
            // first-person arm, or the held item in its place
            if self.screen.is_none() && !self.hide_gui && self.player.gamemode != 3 && !self.hide_hand && std::env::var("BASALT_NOHAND").is_err() {
                let sw = if self.fp_swing >= 0 { (self.fp_swing as f32 + a) / 6.0 } else { 0.0 };
                let (sl, bl) = self.world.light(eye[0].floor() as i32, eye[1].floor() as i32, eye[2].floor() as i32);
                let hand = entity::hand_space(cam, self.player.yaw, self.player.pitch);
                let equip = (self.equip.1 + (self.equip.0 - self.equip.1) * a).clamp(0.0, 1.0);
                if self.shown_item < 0 {
                    entity::first_person_arm(&self.entity_atlas, &net::offline_uuid(&self.name), hand, sw, equip, [sl * 17, bl * 17], &mut fd.hand_skin);
                } else {
                    entity::first_person_item(&self.assets, self.shown_item, hand, sw, equip, [sl * 17, bl * 17], &mut fd.hand_items);
                }
            }
        }
        // 2D: HUD, then the open screen
        let (mut ui, mut items, mut deco) = (MeshData::default(), MeshData::default(), MeshData::default());
        if !self.hide_gui { self.build_hud(&mut ui, &mut items, &mut deco, &vp, alpha as f32); }
        fd.layers.push((ui, Tex::Ui)); fd.layers.push((items, Tex::Blocks)); fd.layers.push((deco, Tex::Ui));
        match &self.screen {
            Some(Screen::Container(c)) => {
                let (mut bg, mut ents, mut it, mut top, mut top_items) = (MeshData::default(), MeshData::default(), MeshData::default(), MeshData::default(), MeshData::default());
                self.draw_container(c, &mut bg, &mut ents, &mut it, &mut top, &mut top_items);
                fd.layers.push((bg, Tex::Ui)); fd.layers.push((ents, Tex::Entities)); fd.layers.push((it, Tex::Blocks)); fd.layers.push((top, Tex::Ui)); fd.layers.push((top_items, Tex::Blocks));
            }
            Some(_) => {
                let (mut m, mut icons) = (MeshData::default(), MeshData::default());
                self.draw_menu(&mut m); self.draw_menu_icons(&mut icons);
                fd.layers.push((m, Tex::Ui)); fd.layers.push((icons, Tex::Icons));
            }
            None => {}
        }
        fd
    }

    /// Start the integrated server for a world; `poll_integrated` connects once it listens.
    pub(crate) fn play_world(&mut self, mut w: worlds::WorldEntry) {
        self.leave_world();
        w.meta.last_played = worlds::now();
        let _ = worlds::save_meta(&w.dir, &w.meta);
        match worlds::Integrated::start(&w) {
            Ok(i) => { self.integrated = Some(i); self.sp_world = Some(w); self.screen = Some(Screen::Message { text: lang::tr("selectWorld.data_read", &[]) }); }
            Err(e) => { self.screen = Some(Screen::Connecting { addr: String::new(), error: Some(format!("{e:#}")) }); }
        }
    }

    pub(crate) fn poll_integrated(&mut self) {
        if self.connected { return; }
        let Some(i) = self.integrated.as_mut() else { return };
        match i.poll() {
            Ok(Some(port)) => { self.connect(&format!("127.0.0.1:{port}")); }
            Ok(None) => {}
            Err(e) => { self.integrated = None; self.sp_world = None; self.screen = Some(Screen::Connecting { addr: String::new(), error: Some(format!("{e:#}")) }); }
        }
    }

    /// Leave the current world: for singleplayer take the world icon, then save and stop the
    /// integrated server ("Saving world").
    pub(crate) fn leave_world(&mut self) {
        if let Some(w) = self.sp_world.take() {
            if self.spawned { self.save_world_icon(&w); }
            self.screen = Some(Screen::Message { text: lang::tr("menu.savingLevel", &[]) });
            let _ = self.render(1.0, None);
            self.disconnect();
            if let Some(i) = self.integrated.take() { i.stop(); }
        } else {
            self.disconnect();
        }
    }

    /// 64x64 centre crop of the current view (no GUI) as <world>/icon.png, like vanilla's world icon.
    fn save_world_icon(&mut self, w: &worlds::WorldEntry) {
        let (screen, hide) = (self.screen.take(), self.hide_gui);
        self.hide_gui = true;
        let shot = self.capture(1.0);
        self.screen = screen; self.hide_gui = hide;
        let Ok(rgb) = shot else { return };
        let (sw, sh) = (self.gpu.size.0 as usize, self.gpu.size.1 as usize);
        let side = sw.min(sh);
        let (ox, oy) = ((sw - side) / 2, (sh - side) / 2);
        let mut out = vec![0u8; 64 * 64 * 4];
        for y in 0..64 { for x in 0..64 {
            let (px, py) = (ox + x * side / 64, oy + y * side / 64);
            let i = (py * sw + px) * 3;
            out[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4].copy_from_slice(&[rgb[i], rgb[i + 1], rgb[i + 2], 255]);
        } }
        if let Ok(f) = std::fs::File::create(w.dir.join("icon.png")) {
            let mut e = png::Encoder::new(std::io::BufWriter::new(f), 64, 64);
            e.set_color(png::ColorType::Rgba); e.set_depth(png::BitDepth::Eight);
            if let Ok(mut wr) = e.write_header() { let _ = wr.write_image_data(&out); }
        }
        self.icon_slots.remove(&format!("world:{}", w.folder));
    }

    /// Load world icons into icon slots (when the world list opens).
    pub(crate) fn load_world_icons(&mut self) {
        for w in self.worlds.clone() {
            let key = format!("world:{}", w.folder);
            if self.icon_slots.contains_key(&key) { continue; }
            let Some(img) = std::fs::read(w.dir.join("icon.png")).ok().and_then(|b| assets::decode_png(&b).ok()).filter(|i| i.w == 64 && i.h == 64) else { continue };
            let n = self.icon_slots.len();
            if n >= 32 { break; }
            self.icon_slots.insert(key, n);
            self.gpu.set_icon(n, &img.rgba);
        }
    }

    pub(crate) fn ping_chan_tx(&self) -> mpsc::Sender<status::PingResult> { self.ping_chan.0.clone() }

    /// Behind menus: the blurred world in game, the blurred spinning panorama elsewhere.
    fn backdrop(&self) -> render::Backdrop {
        match &self.screen {
            None | Some(Screen::Container(_)) | Some(Screen::Death { .. }) => render::Backdrop::None,
            Some(_) if self.connected && self.spawned => render::Backdrop::Blur,
            // spin rate and tilt by eye against a vanilla 26.2 recording
            Some(_) => render::Backdrop::Panorama { yaw: self.menu_t0.elapsed().as_secs_f32() * 1.2, pitch: 10.0 },
        }
    }

    fn render(&mut self, alpha: f64, screenshot: Option<&str>) -> Result<()> {
        let fd = self.frame(alpha);
        let f = fd.as_frame(self.world.min_y as f32);
        match screenshot { Some(p) => self.gpu.screenshot(&f, p), None => self.gpu.render(&f) }
    }
    /// Render offscreen and return RGB pixels (scenarios).
    pub fn capture(&mut self, alpha: f64) -> Result<Vec<u8>> {
        let fd = self.frame(alpha);
        let f = fd.as_frame(self.world.min_y as f32);
        self.gpu.capture(&f)
    }

    /// Advance simulation to `now`; returns interpolation alpha.
    fn update(&mut self) -> f64 {
        self.sync_anvil_text();
        self.poll_integrated();
        self.pump();
        let mut n = 0;
        while self.last_tick.elapsed() >= Duration::from_millis(50) && n < 10 { self.last_tick += Duration::from_millis(50); self.tick(); n += 1; }
        if n == 10 { self.last_tick = Instant::now(); }
        self.frames.0 += 1;
        if self.frames.1.elapsed() >= Duration::from_secs(1) { self.frames.2 = self.frames.0 as f32 / self.frames.1.elapsed().as_secs_f32(); self.frames = (0, Instant::now(), self.frames.2); }
        let e = self.eye(1.0);
        self.sounds.listener = (e, self.player.yaw);
        (self.last_tick.elapsed().as_secs_f64() / 0.05).min(1.0)
    }
}

pub struct FrameData { u: Uniforms, cam: Vec3, vp: Mat4, render_dist: f32, extra: MeshData, skinned: MeshData, items3d: MeshData, overlay: MeshData, hand_skin: MeshData, hand_items: MeshData, layers: Vec<(MeshData, Tex)>, backdrop: render::Backdrop, blur: f32 }
impl FrameData {
    fn as_frame(&self, min_y: f32) -> Frame<'_> {
        Frame { uniforms: self.u, cam: self.cam, vp: self.vp, render_dist: self.render_dist + 32.0, min_y, extra: &self.extra, skinned: &self.skinned, items3d: &self.items3d, overlay: &self.overlay, hand_skin: &self.hand_skin, hand_items: &self.hand_items, ui_layers: &self.layers, hud_layers: 3, backdrop: self.backdrop, blur: self.blur }
    }
}

// ---------------------------------------------------------------- windowed app

struct App { args: Args, assets: Arc<Assets>, window: Option<Arc<Window>>, game: Option<Game>, grabbed: bool }

impl App {
    fn grab(&mut self, on: bool) {
        let Some(w) = &self.window else { return };
        if on { let _ = w.set_cursor_grab(CursorGrabMode::Locked).or_else(|_| w.set_cursor_grab(CursorGrabMode::Confined)); } else { let _ = w.set_cursor_grab(CursorGrabMode::None); }
        w.set_cursor_visible(!on);
        self.grabbed = on;
    }
    fn sync(&mut self, el: &ActiveEventLoop) {
        let Some(g) = &mut self.game else { return };
        if g.quit { el.exit(); return; }
        if g.fullscreen_changed {
            g.fullscreen_changed = false;
            if let Some(w) = &self.window { w.set_fullscreen(if g.cfg.fullscreen { Some(winit::window::Fullscreen::Borderless(None)) } else { None }); }
        }
        let want = g.wants_grab();
        if want != self.grabbed { self.grab(want); }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() { return; }
        let attrs = Window::default_attributes().with_title("Basalt").with_inner_size(winit::dpi::PhysicalSize::new(self.args.size.0, self.args.size.1));
        let w = Arc::new(el.create_window(attrs).expect("create window"));
        let size = w.inner_size();
        let cfg = menu::Config::load();
        if cfg.fullscreen { w.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None))); }
        let name = if self.args.name != "Basalt" { self.args.name.clone() } else { cfg.name.clone() };
        let opts = GameOpts { name, addr: self.args.addr_given.then(|| self.args.addr.clone()), auto_respawn: self.args.auto_respawn, sounds: load_sounds(&self.args, true), cfg };
        match Game::new(Some(w.clone()), (size.width, size.height), self.assets.clone(), load_rules(&self.args), opts) {
            Ok(g) => { self.game = Some(g); self.window = Some(w); }
            Err(e) => { eprintln!("failed to start renderer: {e:#}"); el.exit(); }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(g) = &mut self.game else { return };
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(s) => g.gpu.resize(s.width, s.height),
            WindowEvent::Focused(false) => { g.keys.clear(); g.mouse = [false; 2]; if g.wants_grab() { g.screen = Some(Screen::Pause); } }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key { g.key(code, event.state == ElementState::Pressed, event.text.as_deref()); }
            }
            WindowEvent::CursorMoved { position, .. } => g.mouse_move(position.x as f32, position.y as f32),
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                let b = match button { MouseButton::Left => 0, MouseButton::Right => 1, MouseButton::Middle => 2, _ => 9 };
                g.mouse_button(b, pressed);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta { winit::event::MouseScrollDelta::LineDelta(_, y) => y, winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 / 20.0 };
                g.scroll(dy);
            }
            WindowEvent::RedrawRequested => {
                let alpha = g.update();
                if g.mouse[1] && g.screen.is_none() && g.last_place.elapsed() > Duration::from_millis(200) { g.use_item(); }
                if let Err(e) = g.render(alpha, None) { log::error!("render: {e:#}"); }
            }
            _ => {}
        }
        self.sync(el);
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: winit::event::DeviceId, event: DeviceEvent) {
        if !self.grabbed { return; }
        if let (Some(g), DeviceEvent::MouseMotion { delta }) = (&mut self.game, event) { g.mouse_look(delta.0, delta.1); }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) { if let Some(w) = &self.window { w.request_redraw(); } }
}

// ---------------------------------------------------------------- headless screenshot

fn run_screenshot(args: &Args, assets: Arc<Assets>, out: &str) -> Result<()> {
    let rules = load_rules(args);
    let cfg = config_from_args(args);
    let mut g = Game::new(None, args.size, assets, rules, GameOpts { name: args.name.clone(), addr: Some(args.addr.clone()), auto_respawn: true, sounds: load_sounds(args, false), cfg })?;
    g.fixed_time = args.time; g.hide_gui = args.no_gui;
    let start = Instant::now();
    let mut placed = false;
    let mut walked_from: Option<Instant> = None;
    let mut clicked = false;
    let mut clicked_at: Option<Instant> = None;
    let mut next_click = 0usize;
    let mut placed_at: Option<Instant> = None;
    let mut done_later: Vec<String> = Vec::new();
    let mut attacks_done = 0u32;
    while start.elapsed().as_secs_f32() < args.wait {
        g.update();
        if g.spawned && !placed && g.loaded_sent {
            for c in &args.cmds { g.send_chat(&format!("/{}", c.trim_start_matches('/'))); }
            for c in &args.chat { g.send_chat(c); }
            if let Some(p) = args.pos { g.player.pos = p; g.player.prev_pos = p; g.player.flying = true; }
            placed = true;
        }
        if placed { if let Some((y, p)) = args.look { g.player.yaw = y; g.player.pitch = p; } }
        if placed && start.elapsed().as_secs_f32() > 3.0 && !clicked {
            // after the world has settled: l = break the targeted block, r = use/place on it
            match args.click { Some('l') => { g.mouse[0] = true; clicked = true; } Some('r') => { g.use_item(); clicked = true; } _ => {} }
        }
        if clicked && (start.elapsed().as_secs_f32() > args.wait - 1.0 || clicked_at.get_or_insert(Instant::now()).elapsed().as_secs_f32() > args.hold) { g.mouse[0] = false; }
        if let Some(t0) = placed_at {
            let el = t0.elapsed().as_secs_f32();
            if attacks_done < args.attack && el > 3.0 + attacks_done as f32 * args.attack_every {
                if g.attack_nearest(6.0) { attacks_done += 1; println!("attack {attacks_done}"); } else if el > 3.0 + (attacks_done as f32 + 3.0) * args.attack_every { attacks_done += 1; println!("attack {attacks_done}: no target"); }
            }
        }
        if let Some(t0) = placed_at { for (t, c) in &args.later { if !done_later.contains(c) && t0.elapsed().as_secs_f32() >= *t { g.send_chat(&format!("/{c}")); done_later.push(c.clone()); } } }
        if placed && placed_at.is_none() { placed_at = Some(Instant::now()); }
        if placed && start.elapsed().as_secs_f32() > 4.0 + next_click as f32 * 0.3 && next_click < args.clicks.len() {
            let (slot, button, mode) = args.clicks[next_click]; next_click += 1;
            let mut w = Writer::with_id(sb::CONTAINER_CLICK); w.varint(0).varint(0).i16(slot).i8(button).varint(mode).varint(0).bool(false); send(&g.out, w);
        }
        if placed && args.walk > 0.0 {
            let walking = start.elapsed().as_secs_f32() < args.wait - 1.0 && walked_from.get_or_insert(Instant::now()).elapsed().as_secs_f32() < args.walk;
            if walking { g.keys.insert(KeyCode::KeyW); if g.player.h_collide { g.keys.insert(KeyCode::Space); } else { g.keys.remove(&KeyCode::Space); } }
            else { g.keys.remove(&KeyCode::KeyW); g.keys.remove(&KeyCode::Space); }
        }
        if let Some(d) = &g.disconnected { anyhow::bail!("disconnected: {d}"); }
        std::thread::sleep(Duration::from_millis(5));
    }
    // wait until every chunk the server sends has arrived (none for 1.5 s) and is meshed
    let t = Instant::now();
    while (!g.in_flight.is_empty() || !g.dirty.is_empty() || g.last_chunk_at.elapsed() < Duration::from_millis(1500)) && t.elapsed() < Duration::from_secs(30) { g.update(); if let Some((y, p)) = args.look { g.player.yaw = y; g.player.pitch = p; } std::thread::sleep(Duration::from_millis(5)); }
    g.debug = !args.no_gui;
    g.render(1.0, Some(out))?;
    if std::env::var("BASALT_DIAG").is_ok() {
        let ks: Vec<_> = g.world.chunks.keys().copied().collect();
        let (minx, maxx) = (ks.iter().map(|k| k.0).min().unwrap_or(0), ks.iter().map(|k| k.0).max().unwrap_or(0));
        let (minz, maxz) = (ks.iter().map(|k| k.1).min().unwrap_or(0), ks.iter().map(|k| k.1).max().unwrap_or(0));
        let mut cols: HashSet<(i32, i32)> = HashSet::new(); for k in g.gpu.sections.keys() { cols.insert((k.0, k.2)); }
        println!("diag: chunk x {minx}..{maxx} z {minz}..{maxz}; dirty {}; in_flight {}; columns with meshes {}", g.dirty.len(), g.in_flight.len(), cols.len());
        let mut grid = String::new();
        for z in minz..=maxz { for x in minx..=maxx { grid.push(if !g.world.chunks.contains_key(&(x, z)) { ' ' } else if cols.contains(&(x, z)) { '#' } else { '.' }); } grid.push('\n'); }
        println!("{grid}");
    }
    let p = &g.player;
    let inv: Vec<String> = g.inventory.iter().enumerate().filter(|(_, s)| s.count > 0).map(|(i, s)| format!("{i}:{}x{}{}", basalt_data::registries::ITEM.get(s.item as usize).unwrap_or(&"?"), s.count, if s.damage > 0 { format!("(dmg {})", s.damage) } else { String::new() })).collect();
    let mut kinds: Vec<String> = g.entities.iter().filter(|(id, _)| **id != g.my_eid).map(|(_, e)| format!("{}@{:.1},{:.1},{:.1}/y{:.0}/h{:.0}", entity::type_name(e.kind), e.pos[0], e.pos[1], e.pos[2], e.yaw, e.head_yaw)).collect();
    kinds.sort();
    println!("entity list: {}", kinds.join(" "));
    println!("state: hp {} food {} gamemode {} inv [{}]", g.hp, g.food, g.player.gamemode, inv.join(" "));
    println!("screenshot {out}: pos {:.2},{:.2},{:.2} chunks {} sections {} entities {}", p.pos[0], p.pos[1], p.pos[2], g.world.chunks.len(), g.gpu.sections.len(), g.entities.len());
    Ok(())
}

/// Options for deterministic captures: defaults, then the vanilla client's options.txt (--options), then flags.
pub fn config_from_args(args: &Args) -> menu::Config {
    let mut c = menu::Config { name: args.name.clone(), ..Default::default() };
    if let Some(p) = &args.options {
        match std::fs::read_to_string(p) {
            Ok(s) => for line in s.lines() {
                let Some((k, v)) = line.split_once(':') else { continue };
                match k {
                    // vanilla stores FOV normalised: 70 + v·40
                    "fov" => if let Ok(f) = v.parse::<f32>() { c.fov = 70.0 + f * 40.0; },
                    "renderDistance" => if let Ok(r) = v.parse() { c.render_distance = r; },
                    "guiScale" => if let Ok(g) = v.parse() { c.gui_scale = g; },
                    _ => {}
                }
            },
            Err(e) => log::warn!("options file {p}: {e}"),
        }
    }
    if let Some(f) = args.fov { c.fov = f; }
    if let Some(r) = args.render_distance { c.render_distance = r; }
    if let Some(g) = args.gui_scale { c.gui_scale = g; }
    c
}

fn load_rules(args: &Args) -> Option<Arc<basalt_game::GameData>> {
    match basalt_game::GameData::load(std::path::Path::new(&args.data)) {
        Ok(d) => Some(Arc::new(d)),
        Err(e) => { log::warn!("no game rules from {} ({e:#}); survival dig times will be approximate", args.data); None }
    }
}

fn load_sounds(args: &Args, audio_out: bool) -> sound::Sounds {
    let (dir, idx) = sound::default_assets();
    let dir = args.assets.clone().map(std::path::PathBuf::from).unwrap_or(dir);
    sound::Sounds::load(dir, args.asset_index.clone().or(idx), audio_out && std::env::var("BASALT_NOSOUND").is_err())
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn")).init();
    let args = parse_args();
    let _ = worlds::DATA_DIR.set(args.data.clone());
    let t = Instant::now();
    let assets = Arc::new(Assets::load(&args.jar).with_context(|| format!("loading assets from {} (pass --jar path/to/26.2.jar)", args.jar))?);
    log::info!("assets loaded in {:.2?}: {} models, atlas {}x{}", t.elapsed(), assets.models.len(), assets.atlas.w, assets.atlas.h);
    if let Some(req) = args.mesh_dump.clone() { return meshdump::run(&assets, &req, &args.mesh_out); }
    if let Some(req) = args.anim_eval.clone() { return anim::run(&assets, &req, &args.mesh_out); }
    if let Some(out) = args.asset_grid.clone() { let ok = assetgrid::run(&args, assets, &out)?; std::process::exit(if ok { 0 } else { 1 }); }
    if let Some(out) = args.bench.clone() { return bench::run(&args, assets, &out); }
    if let Some(file) = args.script.clone() { return script::run(&args, assets, &file); }
    if let Some(name) = args.scenario.clone() { let ok = scenario::run(&args, assets, &name)?; std::process::exit(if ok { 0 } else { 1 }); }
    if let Some(out) = args.screenshot.clone() { return run_screenshot(&args, assets, &out); }
    let el = EventLoop::new()?;
    el.set_control_flow(ControlFlow::Poll);
    let mut app = App { args, assets, window: None, game: None, grabbed: false };
    el.run_app(&mut app)?;
    Ok(())
}

/// Ticks for the attack strength to recharge with `item` in hand: 20 / attack speed (attack speeds
/// of the tool classes from the Minecraft Wiki; everything else 4.0 like the empty hand).
fn attack_cooldown(a: &Assets, item: i32) -> f32 {
    let _ = a;
    let n = if item >= 0 { basalt_data::registries::ITEM.get(item as usize).copied().unwrap_or("") } else { "" };
    let speed = if n.ends_with("_sword") { 1.6 } else if n.ends_with("_pickaxe") { 1.2 } else if n.ends_with("_shovel") { 1.0 }
        else if n.ends_with("_axe") { 0.9 } else if n.ends_with("_hoe") { 2.0 } else if n == "trident" { 1.1 } else if n == "mace" { 0.6 } else { 4.0 };
    20.0 / speed
}

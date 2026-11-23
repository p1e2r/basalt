//! Basalt client: joins a 26.2 server, renders the world with wgpu.
//!
//! Usage: basalt-client [--addr host:port] [--name N] [--jar client.jar]
//!                      [--screenshot out.png --wait SECS] [--look yaw,pitch] [--cmd 'tp @s 0 100 0']...
mod assets;
mod mesh;
mod net;
mod player;
mod render;
mod ui;
mod world;

use anyhow::{Context, Result};
use assets::{Assets, Biome};
use basalt_protocol::{ids::play::serverbound as sb, Writer};
use glam::{Mat4, Vec3};
use mesh::{MeshData, SectionMesh, Snapshot, Vertex};
use net::{Event, ItemStack};
use player::{Input, Player};
use render::{Frame, Gpu, Uniforms};
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

struct Args { data: String, addr: String, name: String, jar: String, screenshot: Option<String>, wait: f32, size: (u32, u32), pos: Option<[f64; 3]>, look: Option<(f32, f32)>, cmds: Vec<String>, walk: f32, chat: Vec<String>, click: Option<char>, clicks: Vec<(i16, i8, i32)>, hold: f32, later: Vec<(f32, String)>, attack: u32, attack_every: f32 }

fn parse_args() -> Args {
    let mut a = Args { data: "data".into(), addr: "127.0.0.1:25566".into(), name: "Basalt".into(), jar: std::env::var("BASALT_JAR").unwrap_or_else(|_| "vanilla/client.jar".into()),
        screenshot: None, wait: 8.0, size: (1280, 720), pos: None, look: None, cmds: Vec::new(), walk: 0.0, chat: Vec::new(), click: None, clicks: Vec::new(), hold: 1.0, later: Vec::new(), attack: 0, attack_every: 0.7 };
    let mut it = std::env::args().skip(1);
    let triple = |s: String| -> Option<[f64; 3]> { let v: Vec<f64> = s.split(',').filter_map(|p| p.parse().ok()).collect(); (v.len() == 3).then(|| [v[0], v[1], v[2]]) };
    while let Some(k) = it.next() {
        let mut v = || it.next().unwrap_or_default();
        match k.as_str() {
            "--data" => a.data = v(),
            "--addr" => a.addr = v(), "--name" => a.name = v(), "--jar" => a.jar = v(),
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
            other if !other.starts_with("--") => a.addr = other.to_string(),
            other => eprintln!("unknown argument {other}"),
        }
    }
    a
}

// ---------------------------------------------------------------- meshing workers

struct Mesher { jobs: mpsc::Sender<((i32, i32, i32), Snapshot)>, done: mpsc::Receiver<((i32, i32, i32), SectionMesh)> }

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
                if dtx.send((key, mesh::mesh_section(&assets, &b, &snap))).is_err() { return; }
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

// ---------------------------------------------------------------- game state

#[derive(Clone)]
struct Entity { pos: [f64; 3], yaw: f32, kind: i32 }

/// Box width, height and colour per entity type (vanilla hitbox sizes, from public docs).
fn entity_look(kind: i32) -> (f32, f32, [u8; 4]) {
    let name = basalt_data::registries::ENTITY_TYPE.get(kind as usize).copied().unwrap_or("");
    match name {
        "player" => (0.6, 1.8, [200, 170, 140, 255]),
        "zombie" => (0.6, 1.95, [60, 140, 70, 255]),
        "skeleton" => (0.6, 1.99, [220, 220, 220, 255]),
        "creeper" => (0.6, 1.7, [40, 200, 40, 255]),
        "spider" => (1.4, 0.9, [70, 50, 50, 255]),
        "cow" => (0.9, 1.4, [90, 60, 40, 255]),
        "pig" => (0.9, 0.9, [240, 160, 170, 255]),
        "sheep" => (0.9, 1.3, [245, 245, 245, 255]),
        "chicken" => (0.4, 0.7, [255, 255, 230, 255]),
        "item" | "experience_orb" => (0.25, 0.25, [230, 200, 80, 255]),
        "arrow" => (0.5, 0.5, [120, 90, 60, 255]),
        _ => (0.6, 0.9, [200, 170, 140, 255]),
    }
}

struct Game {
    gpu: Gpu,
    assets: Arc<Assets>,
    rules: Option<Arc<basalt_game::GameData>>,
    ui: UiAtlas,
    biomes: Arc<Mutex<Arc<Vec<Biome>>>>,
    dim_types: Vec<String>,
    world: World,
    player: Player,
    mesher: Mesher,
    dirty: HashSet<(i32, i32, i32)>,
    in_flight: HashSet<(i32, i32, i32)>,
    events: mpsc::Receiver<Event>,
    out: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    entities: HashMap<i32, Entity>,
    my_eid: i32,
    chat: Vec<(String, Instant)>,
    chat_input: Option<String>,
    hp: f32, food: i32,
    hotbar: [ItemStack; 9], held: u8,
    inventory: Vec<ItemStack>,
    day_time: i64,
    spawned: bool,
    loaded_sent: bool,
    keys: HashSet<KeyCode>,
    mouse: [bool; 2],
    last_tick: Instant,
    last_sent: ([f64; 3], f32, f32, bool),
    ticks_since_pos: u32,
    last_input_flags: u8,
    seq: i32,
    dig: Option<([i32; 3], u8, Instant)>,
    last_place: Instant,
    last_space: Option<Instant>,
    debug: bool,
    disconnected: Option<String>,
    died: Option<Instant>,
    frames: (u32, Instant, f32),
}

fn send(out: &tokio::sync::mpsc::UnboundedSender<Vec<u8>>, w: Writer) { let _ = out.send(w.buf); }

impl Game {
    fn new(gpu_window: Option<Arc<Window>>, size: (u32, u32), assets: Arc<Assets>, rules: Option<Arc<basalt_game::GameData>>, args: &Args) -> Result<Game> {
        let ui = UiAtlas::build(&assets);
        let skin = assets.skin.as_ref().map(|s| (s.rgba.as_slice(), s.w, s.h)).unwrap_or((&[255; 4], 1, 1));
        let gpu = Gpu::new(gpu_window, size, &assets, (&ui.rgba, ui.w, ui.h), skin)?;
        let biomes = Arc::new(Mutex::new(Arc::new(Vec::new())));
        let mesher = Mesher::new(assets.clone(), biomes.clone());
        let (tx, rx) = mpsc::channel();
        let (otx, orx) = tokio::sync::mpsc::unbounded_channel();
        net::spawn(net::Options { addr: args.addr.clone(), name: args.name.clone() }, tx, orx);
        Ok(Game {
            gpu, assets, rules, ui, biomes, dim_types: Vec::new(), world: World::new(-64, 384), player: Player::new(), mesher,
            dirty: HashSet::new(), in_flight: HashSet::new(), events: rx, out: otx, entities: HashMap::new(), my_eid: 0,
            chat: Vec::new(), chat_input: None, hp: 20.0, food: 20, hotbar: Default::default(), held: 0, inventory: vec![ItemStack::default(); 46], day_time: 6000,
            spawned: false, loaded_sent: false, keys: HashSet::new(), mouse: [false; 2], last_tick: Instant::now(),
            last_sent: ([0.0; 3], 0.0, 0.0, false), ticks_since_pos: 0, last_input_flags: 0, seq: 0, dig: None,
            last_place: Instant::now(), last_space: None, debug: false, disconnected: None, died: None, frames: (0, Instant::now(), 0.0),
        })
    }

    fn mark_chunk(&mut self, cx: i32, cz: i32) {
        for dx in -1..=1 { for dz in -1..=1 {
            let k = (cx + dx, cz + dz);
            if !self.world.chunks.contains_key(&k) { continue; }
            for sy in 0..self.world.nsections() as i32 { self.dirty.insert((k.0, sy, k.1)); }
        } }
    }
    fn mark_block(&mut self, x: i32, y: i32, z: i32) {
        let sy = (y - self.world.min_y) >> 4;
        for dx in -1..=1 { for dy in -1..=1 { for dz in -1..=1 {
            let (cx, csy, cz) = ((x + dx) >> 4, ((y + dy - self.world.min_y) >> 4), (z + dz) >> 4);
            if csy >= 0 && csy < self.world.nsections() as i32 && (csy - sy).abs() <= 1 { self.dirty.insert((cx, csy, cz)); }
        } } }
    }

    fn reset_world(&mut self, dim_type: i32) {
        let name = self.dim_types.get(dim_type as usize).cloned().unwrap_or_default();
        let (min_y, height) = self.assets.dimension_types.get(&name).map(|d| (d.min_y, d.height)).unwrap_or((-64, 384));
        self.world = World::new(min_y, height);
        self.gpu.sections.clear(); self.dirty.clear(); self.entities.clear();
    }

    fn handle(&mut self, ev: Event) {
        match ev {
            Event::Registries { biomes, dimension_types } => {
                *self.biomes.lock().unwrap() = Arc::new(self.assets.biomes(&biomes));
                self.dim_types = dimension_types;
            }
            Event::Login { entity_id, dimension_type, gamemode, .. } => {
                self.my_eid = entity_id; self.player.gamemode = gamemode; self.player.flying = gamemode == 3;
                self.reset_world(dimension_type);
            }
            Event::Respawn { dimension_type, gamemode, .. } => { self.player.gamemode = gamemode; self.reset_world(dimension_type); self.spawned = false; }
            Event::Chunk { cx, cz, raw } => match net::decode_chunk(&raw, self.world.nsections()) {
                Ok(c) => { self.world.chunks.insert((cx, cz), c); self.mark_chunk(cx, cz); }
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
            }
            Event::Chat(s) => { println!("[chat] {s}"); self.chat.push((s, Instant::now())); if self.chat.len() > 100 { self.chat.remove(0); } }
            Event::Health { hp, food } => { self.hp = hp; self.food = food; }
            Event::GameMode(g) => { self.player.gamemode = g; self.player.flying = g == 3 || (g == 1 && self.player.flying); }
            Event::Time { day, .. } => self.day_time = day,
            Event::HeldSlot(s) => self.held = s.min(8),
            Event::Slot { window, slot, item } => {
                if window == 0 && (0..46).contains(&slot) { self.inventory[slot as usize] = item.clone(); }
                if window == 0 && (36..45).contains(&slot) { self.hotbar[(slot - 36) as usize] = item.clone(); }
                if window == -2 && (0..9).contains(&slot) { self.hotbar[slot as usize] = item; }
            }
            Event::Inventory { items } => { for (i, it) in items.iter().enumerate().take(46) { self.inventory[i] = it.clone(); } for i in 0..9 { if let Some(it) = items.get(36 + i) { self.hotbar[i] = it.clone(); } } }
            Event::AddEntity { id, kind, pos, yaw, .. } => { self.entities.insert(id, Entity { pos, yaw, kind }); }
            Event::MoveEntity { id, delta, rot } => { if let Some(e) = self.entities.get_mut(&id) { for i in 0..3 { e.pos[i] += delta[i]; } if let Some((y, _)) = rot { e.yaw = y; } } }
            Event::TeleportEntity { id, pos, yaw } => { if let Some(e) = self.entities.get_mut(&id) { e.pos = pos; e.yaw = yaw; } }
            Event::RemoveEntities(v) => { for id in v { self.entities.remove(&id); } }
            Event::PlayerName { .. } => {}
            Event::Velocity(v) => self.player.vel = v,
            Event::EntityMotion { id, vel } => { if id == self.my_eid { self.player.vel = vel; } }
            Event::Died(m) => { println!("[death] {m}"); self.chat.push((m, Instant::now())); self.died = Some(Instant::now()); }
            Event::Disconnected(m) => { eprintln!("disconnected: {m}"); self.chat.push((format!("Disconnected: {m}"), Instant::now())); self.disconnected = Some(m); }
        }
    }

    fn pump(&mut self) {
        let start = Instant::now();
        while let Ok(ev) = self.events.try_recv() { self.handle(ev); if start.elapsed() > Duration::from_millis(30) { break; } }
        while let Ok((k, m)) = self.mesher.done.try_recv() { self.in_flight.remove(&k); if self.world.chunks.contains_key(&(k.0, k.2)) { self.gpu.set_section(k, &m); } }
        // queue dirty sections whose horizontal neighbours are loaded, nearest first
        if self.in_flight.len() < 64 && !self.dirty.is_empty() {
            let (pcx, pcz) = ((self.player.pos[0].floor() as i32) >> 4, (self.player.pos[2].floor() as i32) >> 4);
            let mut ready: Vec<(i32, i32, i32)> = self.dirty.iter().copied().filter(|&(cx, _, cz)| {
                !self.in_flight.contains(&(cx, 0, cz)) && [(-1, 0), (1, 0), (0, -1), (0, 1)].iter().all(|(dx, dz)| self.world.chunks.contains_key(&(cx + dx, cz + dz)))
            }).collect();
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
        if self.chat_input.is_some() { return Input::default(); }
        let k = |c: KeyCode| self.keys.contains(&c);
        Input {
            forward: k(KeyCode::KeyW) as i32 as f32 - k(KeyCode::KeyS) as i32 as f32,
            strafe: k(KeyCode::KeyA) as i32 as f32 - k(KeyCode::KeyD) as i32 as f32,
            jump: k(KeyCode::Space), sneak: k(KeyCode::ShiftLeft), sprint: k(KeyCode::ControlLeft) || k(KeyCode::KeyR),
        }
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
        // no death screen yet: respawn automatically after a moment
        if self.died.map(|t| t.elapsed() > Duration::from_secs(2)).unwrap_or(false) {
            self.died = None; self.spawned = false;
            let mut w = Writer::with_id(sb::CLIENT_COMMAND); w.varint(0); send(&self.out, w);
        }
        if !self.spawned || self.died.is_some() { return; }
        if !self.loaded_sent && !self.world.chunks.is_empty() {
            send(&self.out, Writer::with_id(sb::PLAYER_LOADED)); self.loaded_sent = true;
        }
        let inp = self.input();
        let flags = (inp.forward > 0.0) as u8 | ((inp.forward < 0.0) as u8) << 1 | ((inp.strafe > 0.0) as u8) << 2 | ((inp.strafe < 0.0) as u8) << 3
            | (inp.jump as u8) << 4 | (inp.sneak as u8) << 5 | (inp.sprint as u8) << 6;
        if flags != self.last_input_flags { let mut w = Writer::with_id(sb::PLAYER_INPUT); w.u8(flags); send(&self.out, w); self.last_input_flags = flags; }
        self.player.tick(&self.assets, &self.world, inp);
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
            if *id == self.my_eid { continue; }
            let (w, h, _) = entity_look(e.kind);
            if basalt_data::registries::ENTITY_TYPE.get(e.kind as usize).map(|n| matches!(*n, "item" | "arrow" | "experience_orb")).unwrap_or(false) { continue; }
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
        let mut w = Writer::with_id(sb::SWING); w.varint(0); send(&self.out, w);
    }

    /// Test hook: face and attack the nearest non-player living entity within `range`.
    fn attack_nearest(&mut self, range: f64) -> bool {
        let eye = self.eye(1.0);
        let skip = |k: i32| basalt_data::registries::ENTITY_TYPE.get(k as usize).map(|n| matches!(*n, "item" | "arrow" | "experience_orb" | "player")).unwrap_or(true);
        let Some((id, pos, h)) = self.entities.iter().filter(|(id, e)| **id != self.my_eid && !skip(e.kind))
            .map(|(id, e)| (*id, e.pos, entity_look(e.kind).1 as f64))
            .min_by(|a, b| { let d = |p: [f64; 3]| (p[0] - eye[0]).powi(2) + (p[2] - eye[2]).powi(2); d(a.1).partial_cmp(&d(b.1)).unwrap() }) else { return false };
        let (dx, dy, dz) = (pos[0] - eye[0], pos[1] + h * 0.5 - eye[1], pos[2] - eye[2]);
        if (dx * dx + dy * dy + dz * dz).sqrt() > range { return false; }
        self.player.yaw = (-dx).atan2(dz).to_degrees() as f32;
        self.player.pitch = (-(dy.atan2((dx * dx + dz * dz).sqrt())).to_degrees()) as f32;
        self.attack(id);
        true
    }

    fn tick_dig(&mut self) {
        if !self.mouse[0] || self.chat_input.is_some() {
            if let Some((b, f, _)) = self.dig.take() { self.player_action(1, b, f); }
            return;
        }
        let hit = self.target();
        match (&self.dig, hit) {
            (Some((b, _, t0)), Some(h)) if *b == h.block => {
                if self.player.gamemode != 1 && t0.elapsed() >= self.dig_time(h.block) {
                    let (b, f) = (*b, h.face); self.player_action(2, b, f); self.dig = None;
                    self.world.set(b[0], b[1], b[2], 0); self.mark_block(b[0], b[1], b[2]);
                }
            }
            (_, Some(h)) => {
                if let Some((b, f, _)) = self.dig.take() { self.player_action(1, b, f); }
                self.player_action(0, h.block, h.face);
                let mut w = Writer::with_id(sb::SWING); w.varint(0); send(&self.out, w);
                if self.player.gamemode == 1 {
                    // creative: instant break, no abort/finish follows; one block per click
                    let b = h.block; self.world.set(b[0], b[1], b[2], 0); self.mark_block(b[0], b[1], b[2]);
                    self.mouse[0] = false;
                } else { self.dig = Some((h.block, h.face, Instant::now())); }
            }
            (Some(_), None) => { if let Some((b, f, _)) = self.dig.take() { self.player_action(1, b, f); } }
            (None, None) => {}
        }
    }

    /// Survival break time for the targeted block with the held item (vanilla formula via basalt-game).
    fn dig_time(&self, b: [i32; 3]) -> Duration {
        let state = self.world.get(b[0], b[1], b[2]) as u32;
        let held = self.hotbar[self.held as usize].clone();
        let held = (held.count > 0).then_some(held.item);
        let eye = self.eye(1.0);
        let in_water = self.assets.info(self.world.get(eye[0].floor() as i32, eye[1].floor() as i32, eye[2].floor() as i32)).fluid == 1;
        match &self.rules {
            Some(r) => match r.dig_ticks(state, held, self.player.on_ground, in_water) { Some(t) => Duration::from_millis(t as u64 * 50 + 50), None => Duration::from_secs(3600) },
            None => Duration::from_millis(1200),
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
        let mut w = Writer::with_id(sb::SWING); w.varint(0); send(&self.out, w);
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

    fn on_key(&mut self, code: KeyCode, pressed: bool, text: Option<&str>) -> bool {
        if let Some(buf) = &mut self.chat_input {
            if !pressed { return false; }
            match code {
                KeyCode::Escape => self.chat_input = None,
                KeyCode::Enter | KeyCode::NumpadEnter => { let m = self.chat_input.take().unwrap(); self.send_chat(&m); }
                KeyCode::Backspace => { buf.pop(); }
                _ => if let Some(t) = text { for c in t.chars() { if !c.is_control() && buf.len() < 256 { buf.push(c); } } },
            }
            return false;
        }
        if pressed { self.keys.insert(code); } else { self.keys.remove(&code); }
        if !pressed { return false; }
        match code {
            KeyCode::KeyT => self.chat_input = Some(String::new()),
            KeyCode::Slash => self.chat_input = Some("/".into()),
            KeyCode::F3 => self.debug = !self.debug,
            KeyCode::Space if self.player.gamemode == 1 => {
                if self.last_space.map(|t| t.elapsed() < Duration::from_millis(300)).unwrap_or(false) {
                    self.player.flying = !self.player.flying; self.last_space = None;
                    let mut w = Writer::with_id(sb::PLAYER_ABILITIES); w.u8(if self.player.flying { 2 } else { 0 }); send(&self.out, w);
                } else { self.last_space = Some(Instant::now()); }
            }
            KeyCode::Escape => return true,
            _ => {
                let digits = [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4, KeyCode::Digit5, KeyCode::Digit6, KeyCode::Digit7, KeyCode::Digit8, KeyCode::Digit9];
                if let Some(i) = digits.iter().position(|d| *d == code) { self.set_held(i as u8); }
            }
        }
        false
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

    fn build_hud(&self, ui: &mut MeshData, items: &mut MeshData) {
        let (w, h) = (self.gpu.size.0 as f32, self.gpu.size.1 as f32);
        let s = (h / 240.0).floor().clamp(1.0, 4.0);
        let a = &self.ui;
        // crosshair
        if let Some(r) = a.rects.get("crosshair") { a.sprite(ui, "crosshair", ((w - r[2] as f32 * s) / 2.0).floor(), ((h - r[3] as f32 * s) / 2.0).floor(), s); }
        else { a.rect(ui, w / 2.0 - s, h / 2.0 - 5.0 * s, 2.0 * s, 10.0 * s, [255; 4]); a.rect(ui, w / 2.0 - 5.0 * s, h / 2.0 - s, 10.0 * s, 2.0 * s, [255; 4]); }
        // hotbar
        let hx = (w - 182.0 * s) / 2.0; let hy = h - 22.0 * s;
        a.sprite(ui, "hotbar", hx, hy, s);
        a.sprite(ui, "hotbar_selection", hx - s + self.held as f32 * 20.0 * s, hy - s, s);
        for (i, it) in self.hotbar.iter().enumerate() {
            if it.count <= 0 { continue; }
            let (x, y) = (hx + (3.0 + i as f32 * 20.0) * s, hy + 3.0 * s);
            if let Some(uv) = self.assets.icon_uv(it.item) { a.quad(items, x, y, 16.0 * s, 16.0 * s, uv, [255; 4]); }
            if it.count > 1 { let t = it.count.to_string(); a.text(ui, &t, x + 17.0 * s - a.text_width(&t, s), y + 9.0 * s, s, [255; 4]); }
        }
        // hearts + food (survival / adventure)
        if self.player.gamemode == 0 || self.player.gamemode == 2 {
            let y = hy - 10.0 * s;
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
        // chat
        let open = self.chat_input.is_some();
        let mut y = h - 40.0 * s;
        if let Some(buf) = &self.chat_input {
            a.rect(ui, 2.0 * s, h - 14.0 * s, w - 4.0 * s, 12.0 * s, [0, 0, 0, 128]);
            a.text(ui, &format!("{buf}_"), 4.0 * s, h - 12.0 * s, s, [255; 4]);
        }
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
        if let Some(d) = &self.disconnected { lines.push(format!("Disconnected: {d}")); }
        for (i, l) in lines.iter().enumerate() { a.text(ui, l, 2.0 * s, 2.0 * s + i as f32 * 10.0 * s, s, [255; 4]); }
    }

    fn frame_data(&self, alpha: f64) -> (Uniforms, Vec3, Mat4, MeshData, MeshData, MeshData, MeshData) {
        let (w, h) = (self.gpu.size.0 as f32, self.gpu.size.1 as f32);
        let eye = self.eye(alpha);
        let d = self.look_dir();
        let view = Mat4::look_to_rh(Vec3::ZERO, Vec3::new(d[0] as f32, d[1] as f32, d[2] as f32), Vec3::Y);
        let proj = Mat4::perspective_rh(70f32.to_radians(), w / h.max(1.0), 0.05, 1024.0);
        let vp = proj * view;
        let render_dist: f32 = 10.0 * 16.0;
        let biomes = self.biomes.lock().unwrap().clone();
        let bi = self.world.biome(eye[0].floor() as i32, eye[1].floor() as i32, eye[2].floor() as i32) as usize;
        let (sky, fog) = biomes.get(bi).map(|b| (b.sky, b.fog)).unwrap_or((0x78A7FF, 0xC0D8FF));
        let theta = (self.day_time.rem_euclid(24000) as f32 / 24000.0) * std::f32::consts::TAU;
        let sun = [theta.cos(), theta.sin(), 0.0];
        let daylight = (0.5 + 2.0 * theta.sin()).clamp(0.2, 1.0);
        let col = |c: u32, k: f32| [(c >> 16 & 255) as f32 / 255.0 * k, (c >> 8 & 255) as f32 / 255.0 * k, (c & 255) as f32 / 255.0 * k, 1.0];
        let dk = 0.05 + 0.95 * daylight;
        let underwater = self.assets.info(self.world.get(eye[0].floor() as i32, eye[1].floor() as i32, eye[2].floor() as i32)).fluid == 1;
        let (fog_start, fog_end, fogc) = if std::env::var("BASALT_NOFOG").is_ok() { (1e6, 1e6 + 1.0, col(fog, dk)) } else if underwater { (0.0, 24.0, col(0x0A2A6A, dk)) } else { (render_dist - (render_dist * 0.1).clamp(4.0, 64.0), render_dist, col(fog, dk)) };
        let u = Uniforms {
            vp: vp.to_cols_array_2d(), inv_vp: vp.inverse().to_cols_array_2d(),
            cam: [eye[0] as f32, eye[1] as f32, eye[2] as f32, 0.0], fog_color: fogc, sky_color: col(sky, dk),
            params: [fog_start, fog_end, daylight, self.day_time as f32], screen: [w, h, underwater as u8 as f32, 0.0], sun: [sun[0], sun[1], sun[2], 0.0],
        };
        let cam = Vec3::new(eye[0] as f32, eye[1] as f32, eye[2] as f32);
        // block outline + other entities
        let mut extra = MeshData::default();
        let dark = self.atlas_uv("block/black_concrete");
        if let Some(hit) = self.target() {
            let t = 0.004;
            for b in &hit.boxes {
                let lo = [b[0] as f32 - t, b[1] as f32 - t, b[2] as f32 - t]; let hi = [b[3] as f32 + t, b[4] as f32 + t, b[5] as f32 + t];
                for ax in 0..3 { for c1 in 0..2 { for c2 in 0..2 {
                    let (a1, a2) = match ax { 0 => (1, 2), 1 => (0, 2), _ => (0, 1) };
                    let mut l = lo; let mut hh = hi;
                    let v1 = if c1 == 0 { lo[a1] } else { hi[a1] }; let v2 = if c2 == 0 { lo[a2] } else { hi[a2] };
                    l[a1] = v1 - t; hh[a1] = v1 + t; l[a2] = v2 - t; hh[a2] = v2 + t;
                    Self::cuboid(&mut extra, l, hh, dark, [30, 30, 30, 255], 255);
                } } }
            }
        }
        let wool = self.atlas_uv("block/white_wool");
        for (id, e) in &self.entities {
            if *id == self.my_eid { continue; }
            let (w, hh, color) = entity_look(e.kind);
            let hw = w / 2.0;
            let (sl, bl) = self.world.light(e.pos[0].floor() as i32, e.pos[1].floor() as i32 + 1, e.pos[2].floor() as i32);
            let lv = (sl.max(bl) * 17).max(40);
            let p = [e.pos[0] as f32, e.pos[1] as f32, e.pos[2] as f32];
            Self::cuboid(&mut extra, [p[0] - hw, p[1], p[2] - hw], [p[0] + hw, p[1] + hh, p[2] + hw], wool, color, lv);
        }
        let (mut ui, mut items) = (MeshData::default(), MeshData::default());
        self.build_hud(&mut ui, &mut items);
        (u, cam, vp, extra, MeshData::default(), ui, items)
    }

    fn render(&mut self, alpha: f64, screenshot: Option<&str>) -> Result<()> {
        let (u, cam, vp, extra, skinned, ui, items) = self.frame_data(alpha);
        let f = Frame { uniforms: u, cam, vp, render_dist: 12.0 * 16.0, min_y: self.world.min_y as f32, extra: &extra, skinned: &skinned, ui: &ui, ui_items: &items };
        match screenshot { Some(p) => self.gpu.screenshot(&f, p), None => self.gpu.render(&f) }
    }

    /// Advance simulation to `now`; returns interpolation alpha.
    fn update(&mut self) -> f64 {
        self.pump();
        let mut n = 0;
        while self.last_tick.elapsed() >= Duration::from_millis(50) && n < 10 { self.last_tick += Duration::from_millis(50); self.tick(); n += 1; }
        if n == 10 { self.last_tick = Instant::now(); }
        self.frames.0 += 1;
        if self.frames.1.elapsed() >= Duration::from_secs(1) { self.frames.2 = self.frames.0 as f32 / self.frames.1.elapsed().as_secs_f32(); self.frames = (0, Instant::now(), self.frames.2); }
        (self.last_tick.elapsed().as_secs_f64() / 0.05).min(1.0)
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
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() { return; }
        let attrs = Window::default_attributes().with_title("Basalt").with_inner_size(winit::dpi::PhysicalSize::new(self.args.size.0, self.args.size.1));
        let w = Arc::new(el.create_window(attrs).expect("create window"));
        let size = w.inner_size();
        match Game::new(Some(w.clone()), (size.width, size.height), self.assets.clone(), load_rules(&self.args), &self.args) {
            Ok(g) => { self.game = Some(g); self.window = Some(w); }
            Err(e) => { eprintln!("failed to start renderer: {e:#}"); el.exit(); }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(g) = &mut self.game else { return };
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(s) => g.gpu.resize(s.width, s.height),
            WindowEvent::Focused(false) => { g.keys.clear(); g.mouse = [false; 2]; self.grab(false); }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    let was_chat = g.chat_input.is_some();
                    let esc = g.on_key(code, event.state == ElementState::Pressed, event.text.as_deref());
                    let chat_now = self.game.as_ref().map(|g| g.chat_input.is_some()).unwrap_or(false);
                    if esc || (chat_now && !was_chat) { self.grab(false); }
                    if was_chat && !chat_now { self.grab(true); }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                if !self.grabbed { if pressed && g.chat_input.is_none() { self.grab(true); } return; }
                match button {
                    MouseButton::Left => { if pressed { if let Some((id, _)) = g.target_entity() { g.attack(id); } else { g.mouse[0] = true; } } else { g.mouse[0] = false; } }
                    MouseButton::Right => { g.mouse[1] = pressed; if pressed { g.use_item(); } }
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta { winit::event::MouseScrollDelta::LineDelta(_, y) => y, winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 / 20.0 };
                if dy.abs() >= 0.5 { let s = (g.held as i32 - dy.signum() as i32).rem_euclid(9) as u8; g.set_held(s); }
            }
            WindowEvent::RedrawRequested => {
                let alpha = g.update();
                if g.mouse[1] && g.last_place.elapsed() > Duration::from_millis(200) { g.use_item(); }
                if let Err(e) = g.render(alpha, None) { log::error!("render: {e:#}"); }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: winit::event::DeviceId, event: DeviceEvent) {
        if !self.grabbed { return; }
        if let (Some(g), DeviceEvent::MouseMotion { delta }) = (&mut self.game, event) {
            g.player.yaw += delta.0 as f32 * 0.15;
            g.player.pitch = (g.player.pitch + delta.1 as f32 * 0.15).clamp(-90.0, 90.0);
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) { if let Some(w) = &self.window { w.request_redraw(); } }
}

// ---------------------------------------------------------------- headless screenshot

fn run_screenshot(args: &Args, assets: Arc<Assets>, out: &str) -> Result<()> {
    let rules = load_rules(args);
    let mut g = Game::new(None, args.size, assets, rules, args)?;
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
    // let outstanding meshes land
    let t = Instant::now();
    while (!g.in_flight.is_empty() || !g.dirty.is_empty()) && t.elapsed() < Duration::from_secs(10) { g.pump(); std::thread::sleep(Duration::from_millis(5)); }
    g.debug = true;
    g.render(1.0, Some(out))?;
    if std::env::var("BASALT_DIAG").is_ok() {
        let ks: Vec<_> = g.world.chunks.keys().copied().collect();
        let (minx, maxx) = (ks.iter().map(|k| k.0).min().unwrap_or(0), ks.iter().map(|k| k.0).max().unwrap_or(0));
        let (minz, maxz) = (ks.iter().map(|k| k.1).min().unwrap_or(0), ks.iter().map(|k| k.1).max().unwrap_or(0));
        let ready = g.dirty.iter().filter(|&&(cx, _, cz)| [(-1, 0), (1, 0), (0, -1), (0, 1)].iter().all(|(dx, dz)| g.world.chunks.contains_key(&(cx + dx, cz + dz)))).count();
        let mut cols: HashSet<(i32, i32)> = HashSet::new(); for k in g.gpu.sections.keys() { cols.insert((k.0, k.2)); }
        println!("diag: chunk x {minx}..{maxx} z {minz}..{maxz}; dirty {} (ready {ready}); in_flight {}; columns with meshes {}", g.dirty.len(), g.in_flight.len(), cols.len());
        let mut grid = String::new();
        for z in minz..=maxz { for x in minx..=maxx { grid.push(if !g.world.chunks.contains_key(&(x, z)) { ' ' } else if cols.contains(&(x, z)) { '#' } else { '.' }); } grid.push('\n'); }
        println!("{grid}");
    }
    let p = &g.player;
    let inv: Vec<String> = g.inventory.iter().enumerate().filter(|(_, s)| s.count > 0).map(|(i, s)| format!("{i}:{}x{}{}", basalt_data::registries::ITEM.get(s.item as usize).unwrap_or(&"?"), s.count, if s.damage > 0 { format!("(dmg {})", s.damage) } else { String::new() })).collect();
    let mut kinds: Vec<String> = g.entities.iter().filter(|(id, _)| **id != g.my_eid).map(|(_, e)| format!("{}@{:.1},{:.1},{:.1}", basalt_data::registries::ENTITY_TYPE.get(e.kind as usize).unwrap_or(&"?"), e.pos[0], e.pos[1], e.pos[2])).collect();
    kinds.sort();
    println!("entity list: {}", kinds.join(" "));
    println!("state: hp {} food {} gamemode {} inv [{}]", g.hp, g.food, g.player.gamemode, inv.join(" "));
    println!("screenshot {out}: pos {:.2},{:.2},{:.2} chunks {} sections {} entities {}", p.pos[0], p.pos[1], p.pos[2], g.world.chunks.len(), g.gpu.sections.len(), g.entities.len());
    Ok(())
}

fn load_rules(args: &Args) -> Option<Arc<basalt_game::GameData>> {
    match basalt_game::GameData::load(std::path::Path::new(&args.data)) {
        Ok(d) => Some(Arc::new(d)),
        Err(e) => { log::warn!("no game rules from {} ({e:#}); survival dig times will be approximate", args.data); None }
    }
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn")).init();
    let args = parse_args();
    let t = Instant::now();
    let assets = Arc::new(Assets::load(&args.jar).with_context(|| format!("loading assets from {} (pass --jar path/to/26.2.jar)", args.jar))?);
    log::info!("assets loaded in {:.2?}: {} models, atlas {}x{}", t.elapsed(), assets.models.len(), assets.atlas.w, assets.atlas.h);
    if let Some(out) = args.screenshot.clone() { return run_screenshot(&args, assets, &out); }
    let el = EventLoop::new()?;
    el.set_control_flow(ControlFlow::Poll);
    let mut app = App { args, assets, window: None, game: None, grabbed: false };
    el.run_app(&mut app)?;
    Ok(())
}

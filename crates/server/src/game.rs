//! Game thread: owns the world and all players; runs at 20 TPS.
use crate::conn::Shared;
use crate::gen::Generator;
use anyhow::{anyhow, Result};
use basalt_data::{registries as reg, registry_id, BlockState};
use basalt_protocol::{ids::play::clientbound as cb, ids::play::serverbound as sb, text, Reader, Writer};
use basalt_world::World;
use std::collections::{HashMap, HashSet};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

pub enum ToGame {
    Join { eid: i32, uuid: Uuid, name: String, out: UnboundedSender<Vec<u8>>, view: u8 },
    Packet { eid: i32, id: i32, data: Vec<u8> },
    Leave { eid: i32 },
}

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Stack { pub item: i32, pub count: i32 }

struct Player {
    eid: i32,
    uuid: Uuid,
    name: String,
    out: UnboundedSender<Vec<u8>>,
    view: i32,
    pos: [f64; 3],
    yaw: f32,
    pitch: f32,
    on_ground: bool,
    gamemode: u8,
    flying: bool,
    inv: [Stack; 46],
    held: usize,
    loaded: HashSet<(i32, i32)>,
    center: (i32, i32),
    pending_tp: Option<i32>,
    next_tp: i32,
    batch_outstanding: bool,
    chunks_per_tick: f32,
    keepalive: Option<(i64, Instant)>,
    last_keepalive: Instant,
    last_sent_pos: ([f64; 3], f32, f32),
    sneaking: bool,
}

impl Player {
    fn send(&self, w: Writer) { let _ = self.out.send(w.buf); }
    fn chunk(&self) -> (i32, i32) { ((self.pos[0].floor() as i32) >> 4, (self.pos[2].floor() as i32) >> 4) }
    fn sees_chunk(&self, c: (i32, i32)) -> bool { self.loaded.contains(&c) }
}

pub struct Game {
    sh: Arc<Shared>,
    world: World,
    gen: Box<dyn Generator>,
    players: HashMap<i32, Player>,
    tick: u64,
    day_time: i64,
    spawn: [f64; 3],
    player_type: i32,
    dir: Option<std::path::PathBuf>,
}

fn angle(deg: f32) -> u8 { ((deg / 360.0 * 256.0).round() as i32 & 0xFF) as u8 }

/// Default block state for a block name ("stone" or "minecraft:stone").
pub fn block_default(name: &str) -> Option<u32> {
    let n = name.trim_start_matches("minecraft:");
    basalt_data::blocks::BLOCKS.iter().find(|b| b.name == n).map(|b| b.default_state)
}

fn with_prop(state: u32, key: &str, val: &str) -> u32 {
    let st = BlockState(state); let b = st.block();
    // state index = sum(prop_value_index * stride), last property varies fastest
    let mut stride = 1u32; let mut idx = state - b.first_state;
    let mut out = b.first_state;
    let mut parts: Vec<u32> = Vec::new();
    for (_, vals) in b.props.iter().rev() { parts.push(idx % vals.len() as u32); idx /= vals.len() as u32; }
    parts.reverse();
    for (i, (k, vals)) in b.props.iter().enumerate() { if *k == key { if let Some(p) = vals.iter().position(|v| *v == val) { parts[i] = p as u32; } } }
    for (i, (_, vals)) in b.props.iter().enumerate().rev() { out += parts[i] * stride; stride *= vals.len() as u32; }
    out
}

const FACE: [[i32; 3]; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];

fn horizontal_facing(yaw: f32) -> &'static str {
    match (((yaw.rem_euclid(360.0) + 45.0) / 90.0) as i32) & 3 { 0 => "south", 1 => "west", 2 => "north", _ => "east" }
}
fn opposite(f: &str) -> &'static str { match f { "south" => "north", "north" => "south", "east" => "west", "west" => "east", "up" => "down", _ => "up" } }

fn replaceable(state: u32) -> bool {
    if state == 0 { return true; }
    matches!(BlockState(state).name(), "air" | "cave_air" | "void_air" | "water" | "lava" | "short_grass" | "fern" | "dead_bush" | "seagrass" | "tall_seagrass" | "snow" | "vine" | "fire" | "structure_void" | "light" | "tall_grass" | "large_fern" | "bubble_column")
}

impl Game {
    pub fn new(sh: Arc<Shared>, gen: Box<dyn Generator>) -> Game {
        let dir = sh.cfg.world_dir.clone();
        let world = World::new(dir.as_ref().map(|d| d.join("region")));
        let player_type = registry_id(reg::ENTITY_TYPE, "player").unwrap_or(0);
        let mut g = Game { sh, world, gen, players: HashMap::new(), tick: 0, day_time: 1000, spawn: [0.5, 0.0, 0.5], player_type, dir };
        g.load_level();
        let y = g.surface_y(0, 0);
        g.spawn = [0.5, y as f64, 0.5];
        g
    }

    fn load_level(&mut self) {
        let Some(d) = &self.dir else { return };
        if let Ok(s) = std::fs::read_to_string(d.join("basalt.json")) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) { self.day_time = v["day_time"].as_i64().unwrap_or(1000); }
        }
    }
    fn save_level(&mut self) {
        if let Err(e) = self.world.save_all() { log::error!("saving world: {e:#}"); }
        let Some(d) = &self.dir else { return };
        let _ = std::fs::create_dir_all(d.join("players"));
        let _ = std::fs::write(d.join("basalt.json"), serde_json::json!({ "day_time": self.day_time, "seed": self.sh.cfg.seed }).to_string());
        for p in self.players.values() { self.save_player(p); }
    }
    fn save_player(&self, p: &Player) {
        let Some(d) = &self.dir else { return };
        let inv: Vec<_> = p.inv.iter().enumerate().filter(|(_, s)| s.count > 0).map(|(i, s)| serde_json::json!([i, s.item, s.count])).collect();
        let j = serde_json::json!({ "pos": p.pos, "yaw": p.yaw, "pitch": p.pitch, "gamemode": p.gamemode, "held": p.held, "inv": inv, "flying": p.flying });
        let _ = std::fs::create_dir_all(d.join("players"));
        let _ = std::fs::write(d.join("players").join(format!("{}.json", p.uuid)), j.to_string());
    }
    fn load_player(&self, p: &mut Player) {
        let Some(d) = &self.dir else { return };
        let Ok(s) = std::fs::read_to_string(d.join("players").join(format!("{}.json", p.uuid))) else { return };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) else { return };
        if let Some(a) = v["pos"].as_array() { if a.len() == 3 { p.pos = [a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0), a[2].as_f64().unwrap_or(0.0)]; } }
        p.yaw = v["yaw"].as_f64().unwrap_or(0.0) as f32; p.pitch = v["pitch"].as_f64().unwrap_or(0.0) as f32;
        p.gamemode = v["gamemode"].as_u64().unwrap_or(p.gamemode as u64) as u8; p.held = v["held"].as_u64().unwrap_or(0).min(8) as usize;
        p.flying = v["flying"].as_bool().unwrap_or(false);
        for e in v["inv"].as_array().into_iter().flatten() {
            if let (Some(i), Some(item), Some(count)) = (e[0].as_u64(), e[1].as_i64(), e[2].as_i64()) { if (i as usize) < 46 { p.inv[i as usize] = Stack { item: item as i32, count: count as i32 }; } }
        }
    }

    // ------------------------------------------------------------ world helpers

    fn ensure_chunk(&mut self, cx: i32, cz: i32) {
        if self.world.chunk(cx, cz).is_some() { return; }
        match self.world.load(cx, cz) {
            Ok(true) => return,
            Ok(false) => {}
            Err(e) => log::warn!("loading chunk {cx},{cz}: {e:#}; regenerating"),
        }
        let c = self.gen.generate(cx, cz);
        self.world.insert(c);
        self.world.light_chunk(cx, cz);
    }

    fn surface_y(&mut self, x: i32, z: i32) -> i32 {
        self.ensure_chunk(x >> 4, z >> 4);
        let mut y = basalt_world::MIN_Y + basalt_world::HEIGHT - 1;
        while y > basalt_world::MIN_Y && self.world.get_block(x, y, z) == 0 { y -= 1; }
        y + 1
    }

    fn chunk_packet(&self, cx: i32, cz: i32) -> Option<Writer> {
        let c = self.world.chunk(cx, cz)?;
        let mut w = Writer::with_id(cb::LEVEL_CHUNK_WITH_LIGHT); w.i32(cx).i32(cz);
        c.write_network(&mut w.buf);
        Some(w)
    }

    fn broadcast(&self, except: Option<i32>, make: impl Fn() -> Writer) {
        for p in self.players.values() { if Some(p.eid) != except { p.send(make()); } }
    }
    fn broadcast_chunk(&self, c: (i32, i32), except: Option<i32>, make: impl Fn() -> Writer) {
        for p in self.players.values() { if Some(p.eid) != except && p.sees_chunk(c) { p.send(make()); } }
    }
    fn system_chat(&self, msg: basalt_nbt::Tag) {
        log::info!("[chat] {}", text::to_plain(&msg));
        self.broadcast(None, || { let mut w = Writer::with_id(cb::SYSTEM_CHAT); w.nbt(&msg).bool(false); w });
    }
    fn tell(&self, eid: i32, msg: basalt_nbt::Tag) {
        if let Some(p) = self.players.get(&eid) { let mut w = Writer::with_id(cb::SYSTEM_CHAT); w.nbt(&msg).bool(false); p.send(w); }
    }

    /// Change a block, notify viewers and resend light for affected chunks.
    fn set_block(&mut self, x: i32, y: i32, z: i32, state: u32) {
        let changed = self.world.set_block(x, y, z, state);
        self.broadcast_chunk((x >> 4, z >> 4), None, || { let mut w = Writer::with_id(cb::BLOCK_UPDATE); w.position(x, y, z).varint(state as i32); w });
        for (cx, cz) in changed {
            let Some(c) = self.world.chunk(cx, cz) else { continue };
            let mut body = Vec::new(); c.write_light(&mut body);
            self.broadcast_chunk((cx, cz), None, || { let mut w = Writer::with_id(cb::LIGHT_UPDATE); w.varint(cx).varint(cz).bytes(&body); w });
        }
    }

    // ------------------------------------------------------------ packets to players

    fn info_add(p: &Player) -> Writer {
        let mut w = Writer::with_id(cb::PLAYER_INFO_UPDATE);
        w.u8(0xff).varint(1).uuid(p.uuid).str(&p.name).varint(0).bool(false).varint(p.gamemode as i32).bool(true).varint(0).bool(false).varint(0).bool(true);
        w
    }
    fn add_entity(&self, p: &Player) -> Writer {
        let mut w = Writer::with_id(cb::ADD_ENTITY);
        w.varint(p.eid).uuid(p.uuid).varint(self.player_type).f64(p.pos[0]).f64(p.pos[1]).f64(p.pos[2]).u8(0).angle(p.pitch).angle(p.yaw).angle(p.yaw).varint(0);
        w
    }
    fn sync_entity(p: &Player) -> Writer {
        let mut w = Writer::with_id(cb::ENTITY_POSITION_SYNC);
        w.varint(p.eid).f64(p.pos[0]).f64(p.pos[1]).f64(p.pos[2]).f64(0.0).f64(0.0).f64(0.0).f32(p.yaw).f32(p.pitch).bool(p.on_ground);
        w
    }

    fn send_inventory(p: &Player) {
        let mut w = Writer::with_id(cb::CONTAINER_SET_CONTENT); w.varint(0).varint(0).varint(46);
        for s in &p.inv { write_slot(&mut w, *s); }
        write_slot(&mut w, Stack::default());
        p.send(w);
    }
    fn send_slot(p: &Player, slot: usize) {
        let mut w = Writer::with_id(cb::CONTAINER_SET_SLOT); w.varint(0).varint(0).i16(slot as i16); write_slot(&mut w, p.inv[slot]); p.send(w);
    }
    fn send_abilities(p: &Player) {
        let (inv, can_fly, instabuild) = match p.gamemode { 1 => (true, true, true), 3 => (true, true, false), _ => (false, false, false) };
        let flags = inv as u8 | (p.flying as u8) << 1 | (can_fly as u8) << 2 | (instabuild as u8) << 3;
        let mut w = Writer::with_id(cb::PLAYER_ABILITIES); w.u8(flags).f32(0.05).f32(0.1); p.send(w);
    }
    fn teleport(p: &mut Player) {
        p.next_tp += 1; p.pending_tp = Some(p.next_tp);
        let mut w = Writer::with_id(cb::PLAYER_POSITION);
        w.varint(p.next_tp).f64(p.pos[0]).f64(p.pos[1]).f64(p.pos[2]).f64(0.0).f64(0.0).f64(0.0).f32(p.yaw).f32(p.pitch).i32(0);
        p.send(w);
    }
    fn set_time_packet(&self) -> Writer {
        let mut w = Writer::with_id(cb::SET_TIME); w.i64(self.tick as i64).varint(2);
        for clock in 0..2 { w.varint(clock).varlong(self.day_time).f32(0.0).f32(1.0); }
        w
    }

    // ------------------------------------------------------------ join / leave

    fn join(&mut self, eid: i32, uuid: Uuid, name: String, out: UnboundedSender<Vec<u8>>, view: u8) {
        if let Some(old) = self.players.values().find(|p| p.uuid == uuid).map(|p| p.eid) {
            if let Some(p) = self.players.get(&old) { let mut w = Writer::with_id(cb::DISCONNECT); w.nbt(&text::plain("You logged in from another location")); p.send(w); let _ = p.out.send(Vec::new()); }
            self.leave(old);
        }
        let mut p = Player {
            eid, uuid, name: name.clone(), out, view: (view as i32).min(self.sh.cfg.view_distance as i32), pos: self.spawn, yaw: 0.0, pitch: 0.0, on_ground: false,
            gamemode: self.sh.cfg.gamemode, flying: false, inv: [Stack::default(); 46], held: 0, loaded: HashSet::new(), center: (i32::MAX, i32::MAX),
            pending_tp: None, next_tp: 0, batch_outstanding: false, chunks_per_tick: 9.0, keepalive: None, last_keepalive: Instant::now(),
            last_sent_pos: ([0.0; 3], 0.0, 0.0), sneaking: false,
        };
        self.load_player(&mut p);
        let dims = self.sh.registries.entries("minecraft:dimension_type");
        let dim_type = dims.iter().position(|d| d == "minecraft:overworld").unwrap_or(0) as i32;
        let mut w = Writer::with_id(cb::LOGIN);
        w.i32(eid).bool(false).varint(1).str("minecraft:overworld").varint(self.sh.cfg.max_players as i32).varint(p.view).varint(p.view)
            .bool(false).bool(true).bool(false).varint(dim_type).str("minecraft:overworld").i64(self.sh.cfg.seed.wrapping_mul(0x9E3779B97F4A7C15u64 as i64))
            .u8(p.gamemode).i8(-1).bool(false).bool(false).bool(false).varint(0).varint(63).bool(false).u8(0);
        p.send(w);
        let mut w = Writer::with_id(cb::CHANGE_DIFFICULTY); w.u8(1).bool(false); p.send(w);
        Self::send_abilities(&p);
        let mut w = Writer::with_id(cb::SET_HELD_SLOT); w.varint(p.held as i32); p.send(w);
        let mut w = Writer::with_id(cb::UPDATE_RECIPES); w.varint(0).varint(0); p.send(w);
        p.send(commands_packet());
        Self::teleport(&mut p);
        let mut w = Writer::with_id(cb::SERVER_DATA); w.nbt(&text::plain(&self.sh.cfg.motd)).bool(false); p.send(w);
        // tab list: everyone to the newcomer, newcomer to everyone
        for o in self.players.values() { p.send(Self::info_add(o)); }
        p.send(Self::info_add(&p));
        let mut w = Writer::with_id(cb::INITIALIZE_BORDER);
        w.f64(0.0).f64(0.0).f64(59999968.0).f64(59999968.0).varlong(0).varint(29999984).varint(5).varint(300); p.send(w);
        p.send(self.set_time_packet());
        let mut w = Writer::with_id(cb::SET_DEFAULT_SPAWN_POSITION);
        w.str("minecraft:overworld").position(self.spawn[0].floor() as i32, self.spawn[1] as i32, self.spawn[2].floor() as i32).f32(0.0).f32(0.0); p.send(w);
        let mut w = Writer::with_id(cb::GAME_EVENT); w.u8(13).f32(0.0); p.send(w);
        let mut w = Writer::with_id(cb::TICKING_STATE); w.f32(20.0).bool(false); p.send(w);
        let mut w = Writer::with_id(cb::TICKING_STEP); w.varint(0); p.send(w);
        Self::send_inventory(&p);
        let mut w = Writer::with_id(cb::SET_HEALTH); w.f32(20.0).varint(20).f32(5.0); p.send(w);
        let mut w = Writer::with_id(cb::SET_EXPERIENCE); w.f32(0.0).varint(0).varint(0); p.send(w);
        self.broadcast(None, || Self::info_add(&p));
        let add = self.add_entity(&p);
        for o in self.players.values() { o.send(Writer { buf: add.buf.clone() }); p.send(self.add_entity(o)); }
        log::info!("{name} joined (eid {eid}) at {:.1} {:.1} {:.1}", p.pos[0], p.pos[1], p.pos[2]);
        self.players.insert(eid, p);
        self.system_chat(text::colored(&format!("{name} joined the game"), "yellow"));
        self.stream_chunks(eid);
    }

    fn leave(&mut self, eid: i32) {
        let Some(p) = self.players.remove(&eid) else { return };
        self.save_player(&p);
        log::info!("{} left", p.name);
        self.broadcast(None, || { let mut w = Writer::with_id(cb::REMOVE_ENTITIES); w.varint(1).varint(eid); w });
        self.broadcast(None, || { let mut w = Writer::with_id(cb::PLAYER_INFO_REMOVE); w.varint(1).uuid(p.uuid); w });
        self.system_chat(text::colored(&format!("{} left the game", p.name), "yellow"));
    }

    // ------------------------------------------------------------ chunk streaming

    fn stream_chunks(&mut self, eid: i32) {
        let Some(p) = self.players.get(&eid) else { return };
        let (cx, cz) = p.chunk(); let view = p.view;
        let mut want: Vec<(i32, i32)> = Vec::new();
        for dx in -view..=view { for dz in -view..=view { if dx * dx + dz * dz <= view * view + view { want.push((cx + dx, cz + dz)); } } }
        let want_set: HashSet<_> = want.iter().copied().collect();
        let to_unload: Vec<_> = p.loaded.iter().copied().filter(|c| !want_set.contains(c)).collect();
        let center_changed = p.center != (cx, cz);
        let outstanding = p.batch_outstanding; let budget = p.chunks_per_tick.clamp(1.0, 64.0).ceil() as usize;
        let mut missing: Vec<_> = want.into_iter().filter(|c| !p.loaded.contains(c)).collect();
        missing.sort_by_key(|(x, z)| (x - cx).pow(2) + (z - cz).pow(2));
        let p = self.players.get_mut(&eid).unwrap();
        if center_changed { p.center = (cx, cz); let mut w = Writer::with_id(cb::SET_CHUNK_CACHE_CENTER); w.varint(cx).varint(cz); p.send(w); }
        for c in &to_unload { p.loaded.remove(c); let mut w = Writer::with_id(cb::FORGET_LEVEL_CHUNK); w.i32(c.1).i32(c.0); p.send(w); }
        if outstanding || missing.is_empty() { return; }
        let batch: Vec<_> = missing.into_iter().take(budget).collect();
        for &(x, z) in &batch { self.ensure_chunk(x, z); }
        let packets: Vec<Writer> = batch.iter().filter_map(|&(x, z)| self.chunk_packet(x, z)).collect();
        let p = self.players.get_mut(&eid).unwrap();
        p.send(Writer::with_id(cb::CHUNK_BATCH_START));
        let n = packets.len();
        for w in packets { p.send(w); }
        for c in batch { p.loaded.insert(c); }
        let mut w = Writer::with_id(cb::CHUNK_BATCH_FINISHED); w.varint(n as i32); p.send(w);
        p.batch_outstanding = true;
    }

    // ------------------------------------------------------------ inbound packets

    fn packet(&mut self, eid: i32, id: i32, data: &[u8]) -> Result<()> {
        let mut r = Reader::new(data);
        match id {
            sb::KEEP_ALIVE => { let v = r.i64()?; if let Some(p) = self.players.get_mut(&eid) { if p.keepalive.map(|k| k.0) == Some(v) { p.keepalive = None; } } }
            sb::ACCEPT_TELEPORTATION => { let t = r.varint()?; if let Some(p) = self.players.get_mut(&eid) { if p.pending_tp == Some(t) { p.pending_tp = None; } } }
            sb::CHUNK_BATCH_RECEIVED => { let rate = r.f32()?; if let Some(p) = self.players.get_mut(&eid) { p.batch_outstanding = false; if rate.is_finite() && rate > 0.0 { p.chunks_per_tick = rate; } } }
            sb::MOVE_PLAYER_POS | sb::MOVE_PLAYER_POS_ROT | sb::MOVE_PLAYER_ROT | sb::MOVE_PLAYER_STATUS_ONLY => {
                let has_pos = id == sb::MOVE_PLAYER_POS || id == sb::MOVE_PLAYER_POS_ROT;
                let has_rot = id == sb::MOVE_PLAYER_ROT || id == sb::MOVE_PLAYER_POS_ROT;
                let pos = if has_pos { Some([r.f64()?, r.f64()?, r.f64()?]) } else { None };
                let rot = if has_rot { Some((r.f32()?, r.f32()?)) } else { None };
                let flags = r.u8()?;
                let Some(p) = self.players.get_mut(&eid) else { return Ok(()) };
                if p.pending_tp.is_some() { return Ok(()); } // ignore until the client confirms our teleport
                if let Some(np) = pos {
                    if np.iter().any(|v| !v.is_finite() || v.abs() > 3.0e7) { return Err(anyhow!("invalid position")); }
                    let d2: f64 = (0..3).map(|i| (np[i] - p.pos[i]).powi(2)).sum();
                    if d2 > 100.0 * 100.0 { log::warn!("{} moved too quickly ({:.1})", p.name, d2.sqrt()); Self::teleport(p); return Ok(()); }
                    p.pos = np;
                }
                if let Some((y, pi)) = rot { if y.is_finite() && pi.is_finite() { p.yaw = y; p.pitch = pi.clamp(-90.0, 90.0); } }
                p.on_ground = flags & 1 != 0;
                if pos.is_some() && p.chunk() != p.center { self.stream_chunks(eid); }
            }
            sb::PLAYER_ABILITIES => { let f = r.u8()?; if let Some(p) = self.players.get_mut(&eid) { p.flying = f & 2 != 0 && matches!(p.gamemode, 1 | 3); } }
            sb::PLAYER_COMMAND => { let _id = r.varint()?; let _action = r.varint()?; }
            sb::PLAYER_INPUT => { let f = r.u8()?; if let Some(p) = self.players.get_mut(&eid) { p.sneaking = f & 32 != 0; } }
            sb::SET_CARRIED_ITEM => { let s = r.i16()?; if let Some(p) = self.players.get_mut(&eid) { if (0..9).contains(&s) { p.held = s as usize; } } }
            sb::SWING => {
                let hand = r.varint()?;
                self.broadcast(Some(eid), || { let mut w = Writer::with_id(cb::ANIMATE); w.varint(eid).u8(if hand == 0 { 0 } else { 3 }); w });
            }
            sb::SET_CREATIVE_MODE_SLOT => {
                let slot = r.i16()?; let st = read_slot(&mut r)?;
                if let Some(p) = self.players.get_mut(&eid) { if p.gamemode == 1 && (0..46).contains(&slot) { p.inv[slot as usize] = st; } }
            }
            sb::PLAYER_ACTION => {
                let status = r.varint()?; let (x, y, z) = r.position()?; let _face = r.u8()?; let seq = r.varint()?;
                self.dig(eid, status, [x, y, z]);
                if let Some(p) = self.players.get(&eid) { let mut w = Writer::with_id(cb::BLOCK_CHANGED_ACK); w.varint(seq); p.send(w); }
            }
            sb::USE_ITEM_ON => {
                let hand = r.varint()?; let (x, y, z) = r.position()?; let face = r.varint()?;
                let cursor = [r.f32()?, r.f32()?, r.f32()?]; let _inside = r.bool()?; let _border = r.bool()?; let seq = r.varint()?;
                if hand == 0 { self.place(eid, [x, y, z], face.clamp(0, 5) as usize, cursor); }
                if let Some(p) = self.players.get(&eid) { let mut w = Writer::with_id(cb::BLOCK_CHANGED_ACK); w.varint(seq); p.send(w); }
            }
            sb::USE_ITEM => { let _hand = r.varint()?; let seq = r.varint()?; if let Some(p) = self.players.get(&eid) { let mut w = Writer::with_id(cb::BLOCK_CHANGED_ACK); w.varint(seq); p.send(w); } }
            sb::CHAT => {
                let msg = r.str()?;
                if msg.len() > 256 || msg.chars().any(|c| c.is_control()) { return Err(anyhow!("illegal chat")); }
                let name = self.players.get(&eid).map(|p| p.name.clone()).unwrap_or_default();
                self.system_chat(text::plain(&format!("<{name}> {msg}")));
            }
            sb::CHAT_COMMAND | sb::CHAT_COMMAND_SIGNED => { let cmd = r.str()?; self.command(eid, &cmd); }
            sb::CLIENT_INFORMATION => { let _l = r.str()?; let v = r.i8()?; if let Some(p) = self.players.get_mut(&eid) { p.view = (v.clamp(2, 32) as i32).min(self.sh.cfg.view_distance as i32); } self.stream_chunks(eid); }
            _ => {}
        }
        Ok(())
    }

    fn give(&mut self, eid: i32, item: i32, mut count: i32) {
        let Some(p) = self.players.get_mut(&eid) else { return };
        let max = 64;
        let order: Vec<usize> = (36..45).chain(9..36).collect();
        for &i in &order { if count == 0 { break; } let s = &mut p.inv[i]; if s.count > 0 && s.item == item && s.count < max { let n = (max - s.count).min(count); s.count += n; count -= n; Self::send_slot(p, i); } }
        for &i in &order { if count == 0 { break; } let s = &mut p.inv[i]; if s.count == 0 { let n = max.min(count); *s = Stack { item, count: n }; count -= n; Self::send_slot(p, i); } }
    }

    fn dig(&mut self, eid: i32, status: i32, b: [i32; 3]) {
        let Some(p) = self.players.get(&eid) else { return };
        let eye = [p.pos[0], p.pos[1] + 1.62, p.pos[2]];
        let d2: f64 = (0..3).map(|i| (b[i] as f64 + 0.5 - eye[i]).powi(2)).sum();
        if d2 > 8.0 * 8.0 { return; }
        let creative = p.gamemode == 1;
        let breaks = (creative && status == 0) || (!creative && p.gamemode != 2 && status == 2);
        if !breaks { return; }
        let old = self.world.get_block(b[0], b[1], b[2]);
        if old == 0 || BlockState(old).name() == "bedrock" && !creative { return; }
        self.set_block(b[0], b[1], b[2], 0);
        // world event 2001: block break particles + sound for others
        self.broadcast_chunk((b[0] >> 4, b[2] >> 4), Some(eid), || { let mut w = Writer::with_id(cb::LEVEL_EVENT); w.i32(2001).position(b[0], b[1], b[2]).i32(old as i32).bool(false); w });
        if !creative {
            // simplification: drops go straight into the inventory
            if let Some(item) = registry_id(reg::ITEM, BlockState(old).name()) { self.give(eid, item, 1); }
        }
    }

    fn place(&mut self, eid: i32, b: [i32; 3], face: usize, cursor: [f32; 3]) {
        let Some(p) = self.players.get(&eid) else { return };
        if p.gamemode == 3 || p.gamemode == 2 { return; }
        let slot = 36 + p.held; let st = p.inv[slot];
        if st.count <= 0 { return; }
        let Some(item_name) = reg::ITEM.get(st.item as usize) else { return };
        let Some(mut state) = block_default(item_name).or_else(|| block_for_item(item_name)) else { return };
        let clicked = self.world.get_block(b[0], b[1], b[2]);
        let t = if replaceable(clicked) && BlockState(clicked).name() != "water" { b } else { [b[0] + FACE[face][0], b[1] + FACE[face][1], b[2] + FACE[face][2]] };
        if !replaceable(self.world.get_block(t[0], t[1], t[2])) { return; }
        if t[1] < basalt_world::MIN_Y || t[1] >= basalt_world::MIN_Y + basalt_world::HEIGHT { return; }
        // don't place inside a player
        for o in self.players.values() {
            let (lo, hi) = ([o.pos[0] - 0.3, o.pos[1], o.pos[2] - 0.3], [o.pos[0] + 0.3, o.pos[1] + 1.8, o.pos[2] + 0.3]);
            if (0..3).all(|i| (t[i] as f64) < hi[i] && (t[i] as f64 + 1.0) > lo[i]) && BlockState(state).block().kind != "flower" { return; }
        }
        // orientation for common block shapes
        let blk = BlockState(state).block();
        let has = |k: &str| blk.props.iter().any(|(n, _)| *n == k);
        let look = horizontal_facing(p.yaw);
        let face_dir = ["down", "up", "north", "south", "west", "east"][face];
        if has("axis") { state = with_prop(state, "axis", match face { 0 | 1 => "y", 2 | 3 => "z", _ => "x" }); }
        if has("facing") {
            let vertical_ok = blk.props.iter().find(|(n, _)| *n == "facing").map(|(_, v)| v.contains(&"up")).unwrap_or(false);
            let f = if vertical_ok && matches!(blk.kind, "piston" | "dispenser" | "observer" | "barrel" | "end_rod" | "lightning_rod" | "amethyst_cluster") { face_dir }
                else if blk.kind == "wall_torch" || blk.name.ends_with("wall_sign") || blk.kind == "ladder" { face_dir }
                else if matches!(blk.kind, "stair" | "door" | "trapdoor" | "fence_gate" | "bed" | "button" | "lever") { look } else { opposite(look) };
            state = with_prop(state, "facing", f);
        }
        if has("half") && blk.kind == "stair" || blk.kind == "trapdoor" { state = with_prop(state, "half", if face == 0 || (face > 1 && cursor[1] > 0.5) { "top" } else { "bottom" }); }
        if blk.kind == "slab" { state = with_prop(state, "type", if face == 0 || (face > 1 && cursor[1] > 0.5) { "top" } else { "bottom" }); }
        // torches on walls become wall torches
        if face >= 2 && matches!(blk.name, "torch" | "soul_torch" | "redstone_torch" | "copper_torch") {
            if let Some(ws) = block_default(&blk.name.replace("torch", "wall_torch")) { state = with_prop(ws, "facing", face_dir); }
        }
        if face < 2 && matches!(blk.name, "torch" | "soul_torch" | "redstone_torch" | "copper_torch") && face == 0 { return; }
        if BlockState(self.world.get_block(t[0], t[1], t[2])).name() == "water" && has("waterlogged") { state = with_prop(state, "waterlogged", "true"); }
        self.set_block(t[0], t[1], t[2], state);
        if blk.kind == "door" || blk.name.ends_with("_door") {
            if replaceable(self.world.get_block(t[0], t[1] + 1, t[2])) { self.set_block(t[0], t[1] + 1, t[2], with_prop(state, "half", "upper")); }
        }
        let p = self.players.get_mut(&eid).unwrap();
        if p.gamemode != 1 { p.inv[slot].count -= 1; if p.inv[slot].count <= 0 { p.inv[slot] = Stack::default(); } Self::send_slot(p, slot); }
    }

    fn command(&mut self, eid: i32, cmd: &str) {
        let name = self.players.get(&eid).map(|p| p.name.clone()).unwrap_or_default();
        log::info!("{name} issued /{cmd}");
        let mut args: Vec<&str> = cmd.split_whitespace().collect();
        // accept vanilla's leading target (`/give @s stone`, `/tp Alice 0 80 0`) — commands act on the sender
        let is_target = |s: &str| s.starts_with('@') || self.players.values().any(|p| p.name.eq_ignore_ascii_case(s));
        let targeted = match args.first().copied() { Some("give") => args.len() >= 3, Some("tp" | "teleport") => args.len() == 5, Some("gamemode" | "gm") => false, _ => false };
        if targeted && is_target(args[1]) { args.remove(1); }
        let reply = |g: &Game, s: String| g.tell(eid, text::plain(&s));
        match args.first().copied().unwrap_or("") {
            "gamemode" | "gm" => {
                let gm = match args.get(1).copied().unwrap_or("") { "survival" | "s" | "0" => 0, "creative" | "c" | "1" => 1, "adventure" | "a" | "2" => 2, "spectator" | "sp" | "3" => 3, _ => { reply(self, "Usage: /gamemode <survival|creative|adventure|spectator>".into()); return; } };
                let p = self.players.get_mut(&eid).unwrap();
                p.gamemode = gm; p.flying = gm == 3 || (gm == 1 && p.flying);
                let mut w = Writer::with_id(cb::GAME_EVENT); w.u8(3).f32(gm as f32); p.send(w);
                Self::send_abilities(p);
                let uuid = p.uuid;
                self.broadcast(None, || { let mut w = Writer::with_id(cb::PLAYER_INFO_UPDATE); w.u8(4).varint(1).uuid(uuid).varint(gm as i32); w });
                reply(self, format!("Set game mode to {}", ["Survival", "Creative", "Adventure", "Spectator"][gm as usize]));
            }
            "tp" | "teleport" => {
                let target_pos = if args.len() >= 4 {
                    let cur = self.players[&eid].pos;
                    let parse = |s: &str, c: f64| -> Option<f64> { if let Some(r) = s.strip_prefix('~') { Some(c + if r.is_empty() { 0.0 } else { r.parse().ok()? }) } else { s.parse().ok() } };
                    match (parse(args[1], cur[0]), parse(args[2], cur[1]), parse(args[3], cur[2])) { (Some(x), Some(y), Some(z)) => Some([x, y, z]), _ => None }
                } else if args.len() == 2 { self.players.values().find(|p| p.name.eq_ignore_ascii_case(args[1])).map(|p| p.pos) } else { None };
                match target_pos {
                    Some(t) => { let p = self.players.get_mut(&eid).unwrap(); p.pos = t; Self::teleport(p); self.stream_chunks(eid); reply(self, format!("Teleported to {:.1} {:.1} {:.1}", t[0], t[1], t[2])); }
                    None => reply(self, "Usage: /tp <x> <y> <z> | /tp <player>".into()),
                }
            }
            "give" => {
                let (item, count) = (args.get(1).copied().unwrap_or(""), args.get(2).and_then(|c| c.parse().ok()).unwrap_or(1));
                match registry_id(reg::ITEM, item) { Some(id) => { self.give(eid, id, count); reply(self, format!("Gave {count} {item}")); } None => reply(self, format!("Unknown item {item}")) }
            }
            "time" => {
                let v = match args.get(2).copied().or(args.get(1).copied()) { Some("day") => Some(1000), Some("noon") => Some(6000), Some("night") => Some(13000), Some("midnight") => Some(18000), Some(n) => n.parse().ok(), None => None };
                match v { Some(t) => { self.day_time = t; let w = self.set_time_packet(); self.broadcast(None, || Writer { buf: w.buf.clone() }); reply(self, format!("Set the time to {t}")); } None => reply(self, "Usage: /time set <day|night|ticks>".into()) }
            }
            "say" => { let m = args[1..].join(" "); self.system_chat(text::plain(&format!("[{name}] {m}"))); }
            "list" => { let names: Vec<_> = self.players.values().map(|p| p.name.clone()).collect(); reply(self, format!("{} online: {}", names.len(), names.join(", "))); }
            "spawn" => { let s = self.spawn; let p = self.players.get_mut(&eid).unwrap(); p.pos = s; Self::teleport(p); self.stream_chunks(eid); }
            "save" | "save-all" => { self.save_level(); reply(self, "Saved the world".into()); }
            "help" | "" => reply(self, "Commands: /gamemode /tp /give /time /say /list /spawn /save".into()),
            other => reply(self, format!("Unknown command: {other}")),
        }
    }

    // ------------------------------------------------------------ tick

    fn tick(&mut self) {
        self.tick += 1;
        self.day_time += 1;
        let ids: Vec<i32> = self.players.keys().copied().collect();
        for &eid in &ids { self.stream_chunks(eid); }
        // movement broadcast
        let mut moves = Vec::new();
        for p in self.players.values_mut() {
            if p.pos != p.last_sent_pos.0 || p.yaw != p.last_sent_pos.1 || p.pitch != p.last_sent_pos.2 {
                p.last_sent_pos = (p.pos, p.yaw, p.pitch);
                moves.push((p.eid, Self::sync_entity(p), p.yaw));
            }
        }
        for (eid, w, yaw) in moves {
            for o in self.players.values() { if o.eid != eid { o.send(Writer { buf: w.buf.clone() }); let mut h = Writer::with_id(cb::ROTATE_HEAD); h.varint(eid).u8(angle(yaw)); o.send(h); } }
        }
        // keepalive
        let mut kick = Vec::new();
        for p in self.players.values_mut() {
            if let Some((_, t)) = p.keepalive { if t.elapsed() > Duration::from_secs(30) { kick.push(p.eid); continue; } }
            if p.keepalive.is_none() && p.last_keepalive.elapsed() > Duration::from_secs(15) {
                let v = self.tick as i64; p.keepalive = Some((v, Instant::now())); p.last_keepalive = Instant::now();
                let mut w = Writer::with_id(cb::KEEP_ALIVE); w.i64(v); p.send(w);
            }
        }
        for eid in kick { if let Some(p) = self.players.get(&eid) { let mut w = Writer::with_id(cb::DISCONNECT); w.nbt(&text::plain("Timed out")); p.send(w); let _ = p.out.send(Vec::new()); } self.leave(eid); }
        if self.tick % 20 == 0 { let w = self.set_time_packet(); self.broadcast(None, || Writer { buf: w.buf.clone() }); }
        if self.tick % (20 * 60) == 0 { self.save_level(); }
        // unload chunks nobody can see
        if self.tick % 100 == 0 {
            let seen: HashSet<(i32, i32)> = self.players.values().flat_map(|p| p.loaded.iter().copied()).collect();
            for c in self.world.loaded_chunks() { if !seen.contains(&c) && (c.0.abs() > 2 || c.1.abs() > 2) { let _ = self.world.unload(c.0, c.1); } }
        }
    }

    pub fn run(mut self, rx: mpsc::Receiver<ToGame>, stop: Arc<std::sync::atomic::AtomicBool>) {
        let mut next = Instant::now();
        loop {
            if stop.load(std::sync::atomic::Ordering::Relaxed) { break; }
            let deadline = next + Duration::from_millis(50);
            loop {
                let now = Instant::now();
                if now >= deadline { break; }
                match rx.recv_timeout(deadline - now) {
                    Ok(ToGame::Join { eid, uuid, name, out, view }) => self.join(eid, uuid, name, out, view),
                    Ok(ToGame::Packet { eid, id, data }) => {
                        if let Err(e) = self.packet(eid, id, &data) {
                            log::warn!("bad packet {} from {eid}: {e:#}", sb::name(id));
                            if let Some(p) = self.players.get(&eid) { let mut w = Writer::with_id(cb::DISCONNECT); w.nbt(&text::plain(&format!("Bad packet: {e}"))); p.send(w); let _ = p.out.send(Vec::new()); }
                        }
                    }
                    Ok(ToGame::Leave { eid }) => self.leave(eid),
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
            next = deadline;
            if Instant::now() > next + Duration::from_secs(2) { log::warn!("can't keep up; skipping ticks"); next = Instant::now(); }
            self.tick();
        }
        log::info!("saving world");
        for p in self.players.values() { let mut w = Writer::with_id(cb::DISCONNECT); w.nbt(&text::plain("Server closed")); p.send(w); }
        self.save_level();
    }
}

fn write_slot(w: &mut Writer, s: Stack) {
    if s.count <= 0 { w.varint(0); } else { w.varint(s.count).varint(s.item).varint(0).varint(0); }
}

/// Creative / untrusted slot: count, item, then length-prefixed component patches (ignored).
fn read_slot(r: &mut Reader) -> Result<Stack> {
    let count = r.varint()?;
    if count <= 0 { return Ok(Stack::default()); }
    let item = r.varint()?;
    let (add, rem) = (r.varint()?, r.varint()?);
    for _ in 0..add { let _ty = r.varint()?; let len = r.varint()?; r.take(len.max(0) as usize)?; }
    for _ in 0..rem { r.varint()?; }
    Ok(Stack { item, count: count.min(99) })
}

/// Items whose block has a different name.
fn block_for_item(item: &str) -> Option<u32> {
    let b = match item {
        "redstone" => "redstone_wire", "string" => "tripwire", "wheat_seeds" => "wheat", "carrot" => "carrots", "potato" => "potatoes",
        "beetroot_seeds" => "beetroots", "melon_seeds" => "melon_stem", "pumpkin_seeds" => "pumpkin_stem", "sweet_berries" => "sweet_berry_bush",
        "glow_berries" => "cave_vines", "cocoa_beans" => "cocoa", "water_bucket" => "water", "lava_bucket" => "lava", "powder_snow_bucket" => "powder_snow",
        _ => return None,
    };
    block_default(b)
}

/// Minimal brigadier tree: each command is a literal with an optional greedy string argument.
fn commands_packet() -> Writer {
    let cmds = ["gamemode", "gm", "tp", "teleport", "give", "time", "say", "list", "spawn", "save", "save-all", "help"];
    let greedy = registry_id(reg::COMMAND_ARGUMENT_TYPE, "brigadier:string").unwrap_or(5);
    let n = 1 + cmds.len() * 2;
    let mut w = Writer::with_id(cb::COMMANDS); w.varint(n as i32);
    // root
    w.u8(0).varint(cmds.len() as i32); for i in 0..cmds.len() { w.varint(1 + i as i32 * 2); }
    for (i, c) in cmds.iter().enumerate() {
        // literal (executable) with one child
        w.u8(0x01 | 0x04).varint(1).varint(2 + i as i32 * 2).str(c);
        // argument "args": greedy string, executable
        w.u8(0x02 | 0x04).varint(0).str("args").varint(greedy).varint(2);
    }
    w.varint(0);
    w
}

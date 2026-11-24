//! Network thread: login → configuration → play. Decodes packets into events
//! for the main thread and writes outbound packets queued by it.
use crate::world::Chunk;
use anyhow::{anyhow, bail, Result};
use basalt_protocol::{ids, Conn, Reader, Writer, PROTOCOL_VERSION};
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use tokio::net::TcpStream;
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use ids::play::clientbound as cb;
use ids::play::serverbound as sb;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ItemStack { pub count: i32, pub item: i32, pub damage: i32, pub ench: Vec<(i32, i32)> }

impl ItemStack {
    pub fn is_empty(&self) -> bool { self.count <= 0 }
    /// Same item and components (stackable together).
    pub fn same_item(&self, o: &ItemStack) -> bool { self.item == o.item && self.damage == o.damage && self.ench == o.ench }
    pub fn with_count(&self, n: i32) -> ItemStack { if n <= 0 { ItemStack::default() } else { ItemStack { count: n, ..self.clone() } } }
}

/// A decoded entity metadata value (only the serializers we understand).
#[derive(Debug, Clone)]
pub enum Meta { Byte(u8), Int(i32), Float(f32), Bool(bool), Item(ItemStack), Other }

#[allow(dead_code)] // some decoded fields are kept for debugging / future use
pub enum Event {
    Registries { biomes: Vec<String>, dimension_types: Vec<String>, enchantments: Vec<String> },
    Login { entity_id: i32, dimension_type: i32, dimension: String, gamemode: u8 },
    Respawn { dimension_type: i32, dimension: String, gamemode: u8 },
    Chunk { cx: i32, cz: i32, raw: Vec<u8> },
    Light { cx: i32, cz: i32, raw: Vec<u8> },
    Unload { cx: i32, cz: i32 },
    Block { x: i32, y: i32, z: i32, state: u16 },
    Teleport { id: i32, pos: [f64; 3], vel: [f64; 3], yaw: f32, pitch: f32, flags: i32 },
    /// player_rotation (/rotate): yaw, yaw relative, pitch, pitch relative
    Rotate { yaw: f32, ry: bool, pitch: f32, rp: bool },
    Chat(String),
    /// scoreboard / boss bar packets
    Board(crate::board::Update),
    Health { hp: f32, food: i32 },
    GameMode(u8),
    Time { game: i64, day: i64 },
    HeldSlot(u8),
    Slot { window: i32, state: i32, slot: i32, item: ItemStack },
    Content { window: i32, state: i32, items: Vec<ItemStack>, carried: ItemStack },
    Cursor(ItemStack),
    OpenScreen { window: i32, menu: i32, title: String },
    CloseScreen(i32),
    ContainerData { window: i32, id: i16, value: i16 },
    Experience { progress: f32, level: i32, total: i32 },
    /// kind 0 title, 1 subtitle, 2 action bar
    Title { kind: u8, text: String },
    TitleTimes([i32; 3]),
    ClearTitles { reset: bool },
    AddEntity { id: i32, uuid: Uuid, kind: i32, pos: [f64; 3], vel: [f64; 3], yaw: f32, pitch: f32, head_yaw: f32, data: i32 },
    MoveEntity { id: i32, delta: [f64; 3], rot: Option<(f32, f32)>, on_ground: bool },
    TeleportEntity { id: i32, pos: [f64; 3], yaw: f32, pitch: f32, on_ground: bool },
    EntityMotion { id: i32, vel: [f64; 3] },
    EntityData { id: i32, entries: Vec<(u8, Meta)> },
    HeadYaw { id: i32, yaw: f32 },
    Animate { id: i32, action: u8 },
    Hurt { id: i32 },
    EntityEvent { id: i32, status: u8 },
    TakeItem { item: i32, collector: i32 },
    RemoveEntities(Vec<i32>),
    BlockDestruction { id: i32, pos: [i32; 3], stage: i8 },
    Sound { event: String, pos: Option<[f64; 3]>, entity: Option<i32>, volume: f32, pitch: f32, category: i32 },
    LevelEvent { event: i32, pos: [i32; 3], data: i32 },
    PlayerName { uuid: Uuid, name: String },
    Velocity([f64; 3]),
    Died { player: i32, msg: String },
    Commands(Vec<u8>),
    Suggestions { id: i32, start: i32, len: i32, list: Vec<String> },
    Disconnected(String),
}

pub struct Options { pub addr: String, pub name: String, pub view_distance: i8 }

pub fn offline_uuid(name: &str) -> Uuid {
    use md5::{Digest, Md5};
    let mut h = Md5::new(); h.update(format!("OfflinePlayer:{name}").as_bytes());
    let mut b: [u8; 16] = h.finalize().into();
    b[6] = b[6] & 0x0f | 0x30; b[8] = b[8] & 0x3f | 0x80;
    Uuid::from_bytes(b)
}

pub fn spawn(opts: Options, tx: Sender<Event>, out: UnboundedReceiver<Vec<u8>>) {
    std::thread::Builder::new().name("net".into()).spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let tx2 = tx.clone();
        if let Err(e) = rt.block_on(run(opts, tx, out)) { let _ = tx2.send(Event::Disconnected(format!("{e:#}"))); }
    }).unwrap();
}

async fn run(opts: Options, tx: Sender<Event>, mut out: UnboundedReceiver<Vec<u8>>) -> Result<()> {
    let (host, port) = opts.addr.rsplit_once(':').map(|(h, p)| (h.to_string(), p.parse().unwrap_or(25565))).unwrap_or((opts.addr.clone(), 25565));
    let stream = tokio::time::timeout(std::time::Duration::from_secs(10), TcpStream::connect((host.as_str(), port))).await.map_err(|_| anyhow!("connection timed out"))??;
    let mut c = Conn::new(stream);
    let mut w = Writer::with_id(0); w.varint(PROTOCOL_VERSION).str(&host).u16(port).varint(2); c.send(&w).await?;
    let mut w = Writer::with_id(ids::login::serverbound::HELLO); w.str(&opts.name).uuid(offline_uuid(&opts.name)); c.send(&w).await?;

    // ---- login ----
    loop {
        let f = c.read().await?; let mut r = f.reader();
        match f.id {
            ids::login::clientbound::LOGIN_COMPRESSION => { let t = r.varint()?; c.set_threshold(t); }
            ids::login::clientbound::LOGIN_FINISHED => { c.send(&Writer::with_id(ids::login::serverbound::LOGIN_ACKNOWLEDGED)).await?; break; }
            ids::login::clientbound::LOGIN_DISCONNECT => bail!("disconnected: {}", r.str()?),
            ids::login::clientbound::HELLO => bail!("server is in online mode (encryption/auth not supported yet)"),
            ids::login::clientbound::CUSTOM_QUERY => { let id = r.varint()?; let mut w = Writer::with_id(ids::login::serverbound::CUSTOM_QUERY_ANSWER); w.varint(id).bool(false); c.send(&w).await?; }
            _ => {}
        }
    }
    let mut players: HashMap<Uuid, String> = HashMap::new();
    'session: loop {
        // ---- configuration ----
        // same order as the real client: brand, then client information
        let mut w = Writer::with_id(ids::configuration::serverbound::CUSTOM_PAYLOAD); w.str("minecraft:brand").str("basalt"); c.send(&w).await?;
        let mut w = Writer::with_id(ids::configuration::serverbound::CLIENT_INFORMATION);
        w.str("en_us").i8(opts.view_distance).varint(0).bool(true).u8(0x7f).varint(1).bool(false).bool(true).varint(0); c.send(&w).await?;
        let (mut biomes, mut dims, mut enchs) = (Vec::new(), Vec::new(), Vec::new());
        loop {
            let f = c.read().await?; let mut r = f.reader();
            use ids::configuration::clientbound as cc; use ids::configuration::serverbound as cs;
            match f.id {
                cc::SELECT_KNOWN_PACKS => { let mut w = Writer::with_id(cs::SELECT_KNOWN_PACKS); w.bytes(&f.data); c.send(&w).await?; }
                cc::REGISTRY_DATA => {
                    let reg = r.str()?; let n = r.varint()?; let mut names = Vec::new();
                    for _ in 0..n { names.push(r.str()?); if r.bool()? { r.nbt()?; } }
                    match reg.as_str() { "minecraft:worldgen/biome" => biomes = names, "minecraft:dimension_type" => dims = names, "minecraft:enchantment" => enchs = names, _ => {} }
                }
                cc::KEEP_ALIVE => { let v = r.i64()?; let mut w = Writer::with_id(cs::KEEP_ALIVE); w.i64(v); c.send(&w).await?; }
                cc::PING => { let v = r.i32()?; let mut w = Writer::with_id(cs::PONG); w.i32(v); c.send(&w).await?; }
                cc::DISCONNECT => bail!("kicked: {}", crate::lang::text(&r.nbt()?)),
                cc::FINISH_CONFIGURATION => { c.send(&Writer::with_id(cs::FINISH_CONFIGURATION)).await?; break; }
                _ => {}
            }
        }
        tx.send(Event::Registries { biomes, dimension_types: dims, enchantments: enchs }).ok();

        // ---- play ----
        loop {
            tokio::select! {
                o = out.recv() => { match o { Some(buf) => { c.w.send(&Writer { buf }).await?; } None => return Ok(()) } }
                f = c.r.read() => {
                    let f = f?;
                    match f.id {
                        cb::KEEP_ALIVE => { let v = f.reader().i64()?; let mut w = Writer::with_id(sb::KEEP_ALIVE); w.i64(v); c.w.send(&w).await?; }
                        cb::PING => { let v = f.reader().i32()?; let mut w = Writer::with_id(sb::PONG); w.i32(v); c.w.send(&w).await?; }
                        cb::CHUNK_BATCH_FINISHED => { let mut w = Writer::with_id(sb::CHUNK_BATCH_RECEIVED); w.f32(16.0); c.w.send(&w).await?; }
                        cb::START_CONFIGURATION => { c.w.send(&Writer::with_id(sb::CONFIGURATION_ACKNOWLEDGED)).await?; continue 'session; }
                        cb::DISCONNECT => { let m = crate::lang::text(&f.reader().nbt()?); bail!("kicked: {m}") }
                        _ => {
                            if let Err(e) = handle_play(f.id, &f.data, &tx, &mut players) {
                                log::warn!("failed to decode {}: {e}", cb::name(f.id));
                            }
                        }
                    }
                }
            }
        }
    }
}

pub fn read_slot(r: &mut Reader) -> Result<ItemStack> {
    let count = r.varint()?;
    if count <= 0 { return Ok(ItemStack::default()); }
    let item = r.varint()?;
    let (add, rem) = (r.varint()?, r.varint()?);
    // network component patches have no length prefix: only the ones we understand can be skipped
    let damage_ty = basalt_data::registry_id(basalt_data::registries::DATA_COMPONENT_TYPE, "damage").unwrap_or(3);
    let mut damage = 0;
    let mut ench = Vec::new();
    let ench_ty = basalt_data::registry_id(basalt_data::registries::DATA_COMPONENT_TYPE, "enchantments").unwrap_or(13);
    for _ in 0..add {
        let ty = r.varint()?;
        if ty == damage_ty { damage = r.varint()?; }
        else if ty == ench_ty { let n = r.varint()?; for _ in 0..n { let e = r.varint()?; let l = r.varint()?; ench.push((e, l)); } }
        else { return Err(anyhow!("component {ty} on item {item}")); }
    }
    for _ in 0..rem { r.varint()?; }
    Ok(ItemStack { count, item, damage, ench })
}

/// Decode a compact ("low precision") vec3 as used for entity velocity (see NOTES.md).
pub fn read_lpvec3(r: &mut Reader) -> Result<[f64; 3]> {
    let b0 = r.u8()?;
    if b0 == 0 { return Ok([0.0; 3]); }
    let b1 = r.u8()?;
    let hi = u32::from_be_bytes(r.take(4)?.try_into()?);
    let bits = b0 as u64 | (b1 as u64) << 8 | (hi as u64) << 16;
    let mut scale = bits & 3;
    if bits & 4 != 0 { scale |= (r.varint()? as u32 as u64) << 2; }
    let un = |c: u64| ((c & 0x7FFF) as f64 / 32766.0 * 2.0 - 1.0) * scale as f64;
    Ok([un(bits >> 3), un(bits >> 18), un(bits >> 33)])
}

fn read_meta(r: &mut Reader) -> Result<Vec<(u8, Meta)>> {
    let mut out = Vec::new();
    loop {
        let idx = r.u8()?;
        if idx == 0xff { break; }
        let ser = r.varint()?;
        let v = match ser {
            0 => Meta::Byte(r.u8()?),
            1 => Meta::Int(r.varint()?),
            2 => { r.varlong()?; Meta::Other }
            3 => Meta::Float(r.f32()?),
            4 => { r.str()?; Meta::Other }
            5 => { r.nbt()?; Meta::Other }
            6 => { if r.bool()? { r.nbt()?; } Meta::Other }
            7 => Meta::Item(read_slot(r)?),
            8 => Meta::Bool(r.bool()?),
            9 => { r.f32()?; r.f32()?; r.f32()?; Meta::Other }
            10 => { r.i64()?; Meta::Other }
            11 => { if r.bool()? { r.i64()?; } Meta::Other }
            12 => Meta::Int(r.varint()?),
            13 => { if r.bool()? { r.uuid()?; } Meta::Other }
            14 | 15 | 20 => Meta::Int(r.varint()?),
            // unknown serializer: the rest of the list can't be skipped safely
            _ => break,
        };
        out.push((idx, v));
    }
    Ok(out)
}

fn read_sound_event(r: &mut Reader) -> Result<String> {
    let id = r.varint()?;
    if id == 0 {
        let name = r.str()?;
        if r.bool()? { r.f32()?; }
        Ok(name.trim_start_matches("minecraft:").to_string())
    } else {
        Ok(basalt_data::registries::SOUND_EVENT.get(id as usize - 1).map(|s| s.to_string()).unwrap_or_default())
    }
}


/// Yaw byte: 256 steps per turn (wrapping, so interpolation goes through `entity::wrap`).
fn angle(b: u8) -> f32 { b as f32 * 360.0 / 256.0 }
/// Pitch byte: the same scale but *signed* (-128..127 → -180..177.2°), then limited to what a head
/// can do. Read unsigned, "slightly up" (0xF0) is 337.5° and the entity lerps through a backflip.
pub fn pitch_angle(b: u8) -> f32 { ((b as i8) as f32 * 360.0 / 256.0).clamp(-90.0, 90.0) }

fn handle_play(id: i32, data: &[u8], tx: &Sender<Event>, players: &mut HashMap<Uuid, String>) -> Result<()> {
    let mut r = Reader::new(data);
    let ev = match id {
        cb::LOGIN => {
            let entity_id = r.i32()?; r.bool()?;
            let n = r.varint()?; for _ in 0..n { r.str()?; }
            r.varint()?; r.varint()?; r.varint()?; r.bool()?; r.bool()?; r.bool()?;
            let dimension_type = r.varint()?; let dimension = r.str()?; r.i64()?; let gamemode = r.u8()?;
            Event::Login { entity_id, dimension_type, dimension, gamemode }
        }
        cb::RESPAWN => {
            let dimension_type = r.varint()?; let dimension = r.str()?; r.i64()?; let gamemode = r.u8()?;
            Event::Respawn { dimension_type, dimension, gamemode }
        }
        cb::LEVEL_CHUNK_WITH_LIGHT => { let cx = r.i32()?; let cz = r.i32()?; Event::Chunk { cx, cz, raw: r.rest().to_vec() } }
        cb::LIGHT_UPDATE => { let cx = r.varint()?; let cz = r.varint()?; Event::Light { cx, cz, raw: r.rest().to_vec() } }
        cb::FORGET_LEVEL_CHUNK => { let cz = r.i32()?; let cx = r.i32()?; Event::Unload { cx, cz } }
        cb::BLOCK_UPDATE => { let (x, y, z) = r.position()?; Event::Block { x, y, z, state: r.varint()? as u16 } }
        cb::SECTION_BLOCKS_UPDATE => {
            let p = r.i64()?;
            let (sx, sz, sy) = ((p >> 42) as i32, (p << 22 >> 42) as i32, (p << 44 >> 44) as i32);
            let n = r.varint()?;
            for _ in 0..n {
                let v = r.varlong()?; let local = (v & 0xFFF) as i32;
                tx.send(Event::Block { x: sx * 16 + (local >> 8 & 15), y: sy * 16 + (local & 15), z: sz * 16 + (local >> 4 & 15), state: (v >> 12) as u16 }).ok();
            }
            return Ok(());
        }
        cb::PLAYER_POSITION => {
            let id = r.varint()?;
            let pos = [r.f64()?, r.f64()?, r.f64()?]; let vel = [r.f64()?, r.f64()?, r.f64()?];
            let yaw = r.f32()?; let pitch = r.f32()?; let flags = r.i32()?;
            Event::Teleport { id, pos, vel, yaw, pitch, flags }
        }
        cb::PLAYER_ROTATION => Event::Rotate { yaw: r.f32()?, ry: r.bool()?, pitch: r.f32()?, rp: r.bool()? },
        cb::SYSTEM_CHAT => { let t = r.nbt()?; let overlay = r.bool().unwrap_or(false); if overlay { Event::Title { kind: 2, text: crate::lang::styled(&t) } } else { Event::Chat(crate::lang::styled(&t)) } }
        cb::SET_TITLE_TEXT => Event::Title { kind: 0, text: crate::lang::styled(&r.nbt()?) },
        cb::SET_SUBTITLE_TEXT => Event::Title { kind: 1, text: crate::lang::styled(&r.nbt()?) },
        cb::SET_ACTION_BAR_TEXT => Event::Title { kind: 2, text: crate::lang::styled(&r.nbt()?) },
        cb::SET_TITLES_ANIMATION => Event::TitleTimes([r.i32()?, r.i32()?, r.i32()?]),
        cb::CLEAR_TITLES => Event::ClearTitles { reset: r.bool()? },
        cb::PLAYER_CHAT => {
            let _global = r.varint()?; let sender = r.uuid()?; let _idx = r.varint()?;
            if r.bool()? { r.take(256)?; }
            let msg = r.str()?;
            let name = players.get(&sender).cloned().unwrap_or_else(|| "?".into());
            Event::Chat(format!("<{name}> {msg}"))
        }
        cb::PLAYER_COMBAT_KILL => { let player = r.varint()?; Event::Died { player, msg: crate::lang::text(&r.nbt()?) } }
        cb::COMMANDS => Event::Commands(r.rest().to_vec()),
        cb::COMMAND_SUGGESTIONS => {
            let id = r.varint()?; let start = r.varint()?; let len = r.varint()?; let n = r.varint()?;
            let mut list = Vec::new();
            for _ in 0..n.clamp(0, 1000) { list.push(r.str()?); if r.bool()? { r.nbt()?; } }
            Event::Suggestions { id, start, len, list }
        }
        cb::DISGUISED_CHAT => Event::Chat(crate::lang::styled(&r.nbt()?)),
        cb::SET_OBJECTIVE => {
            use crate::board::Update;
            let name = r.str()?; let mode = r.u8()?;
            if mode == 1 { Event::Board(Update::Objective { name, mode, title: String::new(), render: 0 }) }
            else { let title = crate::lang::styled(&r.nbt()?); let render = r.varint()?; Event::Board(Update::Objective { name, mode, title, render }) }
        }
        cb::SET_DISPLAY_OBJECTIVE => { let slot = r.varint()?; let name = r.str()?; Event::Board(crate::board::Update::Display { slot, name }) }
        cb::SET_SCORE => {
            let holder = r.str()?; let objective = r.str()?; let value = r.varint()?;
            let display = if r.bool()? { Some(crate::lang::styled(&r.nbt()?)) } else { None };
            let fmt = read_number_format(&mut r)?;
            Event::Board(crate::board::Update::Score { holder, objective, value, display, fmt })
        }
        cb::RESET_SCORE => { let holder = r.str()?; let objective = if r.bool()? { Some(r.str()?) } else { None }; Event::Board(crate::board::Update::Reset { holder, objective }) }
        cb::BOSS_EVENT => {
            use crate::board::{BossOp, Update};
            let uuid = r.uuid()?;
            let op = match r.varint()? {
                0 => { let name = crate::lang::styled(&r.nbt()?); let progress = r.f32()?; let color = r.varint()?; let overlay = r.varint()?; let flags = r.u8()?; BossOp::Add { name, progress, color, overlay, flags } }
                1 => BossOp::Remove,
                2 => BossOp::Progress(r.f32()?),
                3 => BossOp::Name(crate::lang::styled(&r.nbt()?)),
                4 => { let color = r.varint()?; let overlay = r.varint()?; BossOp::Style { color, overlay } }
                _ => BossOp::Flags(r.u8()?),
            };
            Event::Board(Update::Boss { uuid, op })
        }
        cb::SET_HEALTH => { let hp = r.f32()?; let food = r.varint()?; Event::Health { hp, food } }
        cb::SET_EXPERIENCE => { let progress = r.f32()?; let level = r.varint()?; let total = r.varint()?; Event::Experience { progress, level, total } }
        cb::GAME_EVENT => { let e = r.u8()?; let v = r.f32()?; if e == 3 { Event::GameMode(v as u8) } else { return Ok(()) } }
        cb::SET_TIME => {
            let game = r.i64()?; let n = r.varint()?; let mut day = game;
            for _ in 0..n { let clock = r.varint()?; let t = r.varlong()?; r.f32()?; r.f32()?; if clock == 0 { day = t; } }
            Event::Time { game, day }
        }
        cb::SET_HELD_SLOT => Event::HeldSlot(r.varint()? as u8),
        cb::CONTAINER_SET_SLOT => {
            let window = r.varint()?; let state = r.varint()?; let slot = r.i16()? as i32;
            Event::Slot { window, state, slot, item: read_slot(&mut r).unwrap_or_default() }
        }
        cb::CONTAINER_SET_CONTENT => {
            let window = r.varint()?; let state = r.varint()?; let n = r.varint()?;
            let mut items = Vec::new();
            let mut ok = true;
            for _ in 0..n { match read_slot(&mut r) { Ok(s) => items.push(s), Err(_) => { ok = false; break } } }
            let carried = if ok { read_slot(&mut r).unwrap_or_default() } else { ItemStack::default() };
            items.resize(n.max(0) as usize, ItemStack::default());
            Event::Content { window, state, items, carried }
        }
        cb::SET_CURSOR_ITEM => Event::Cursor(read_slot(&mut r).unwrap_or_default()),
        cb::OPEN_SCREEN => { let window = r.varint()?; let menu = r.varint()?; let title = crate::lang::text(&r.nbt()?); Event::OpenScreen { window, menu, title } }
        cb::CONTAINER_CLOSE => Event::CloseScreen(r.varint()?),
        cb::CONTAINER_SET_DATA => { let window = r.varint()?; let id = r.i16()?; let value = r.i16()?; Event::ContainerData { window, id, value } }
        cb::SET_PLAYER_INVENTORY => { let slot = r.varint()?; Event::Slot { window: -2, state: 0, slot, item: read_slot(&mut r)? } }
        cb::PLAYER_INFO_UPDATE => {
            let actions = r.u8()?; let n = r.varint()?;
            if actions & 1 == 0 { return Ok(()); }
            for _ in 0..n {
                let uuid = r.uuid()?; let name = r.str()?;
                let np = r.varint()?; for _ in 0..np { r.str()?; r.str()?; if r.bool()? { r.str()?; } }
                players.insert(uuid, name.clone());
                tx.send(Event::PlayerName { uuid, name }).ok();
                // remaining actions for this entry
                if actions & 2 != 0 && r.bool()? { r.uuid()?; r.i64()?; r.byte_array()?; r.byte_array()?; }
                if actions & 4 != 0 { r.varint()?; }
                if actions & 8 != 0 { r.bool()?; }
                if actions & 16 != 0 { r.varint()?; }
                if actions & 32 != 0 && r.bool()? { r.nbt()?; }
                if actions & 64 != 0 { r.varint()?; }
                if actions & 128 != 0 { r.bool()?; }
            }
            return Ok(());
        }
        cb::ADD_ENTITY => {
            let id = r.varint()?; let uuid = r.uuid()?; let kind = r.varint()?;
            let pos = [r.f64()?, r.f64()?, r.f64()?];
            let vel = read_lpvec3(&mut r)?; let pitch = pitch_angle(r.u8()?); let yaw = angle(r.u8()?); let head_yaw = angle(r.u8().unwrap_or(0));
            let data = r.varint().unwrap_or(0);
            Event::AddEntity { id, uuid, kind, pos, vel, yaw, pitch, head_yaw, data }
        }
        cb::MOVE_ENTITY_POS => {
            let id = r.varint()?; let d = [r.i16()? as f64 / 4096.0, r.i16()? as f64 / 4096.0, r.i16()? as f64 / 4096.0];
            let on_ground = r.bool().unwrap_or(false);
            Event::MoveEntity { id, delta: d, rot: None, on_ground }
        }
        cb::MOVE_ENTITY_POS_ROT => {
            let id = r.varint()?; let d = [r.i16()? as f64 / 4096.0, r.i16()? as f64 / 4096.0, r.i16()? as f64 / 4096.0];
            let yaw = angle(r.u8()?); let pitch = pitch_angle(r.u8()?); let on_ground = r.bool().unwrap_or(false);
            Event::MoveEntity { id, delta: d, rot: Some((yaw, pitch)), on_ground }
        }
        cb::MOVE_ENTITY_ROT => {
            let id = r.varint()?; let yaw = angle(r.u8()?); let pitch = pitch_angle(r.u8()?); let on_ground = r.bool().unwrap_or(false);
            Event::MoveEntity { id, delta: [0.0; 3], rot: Some((yaw, pitch)), on_ground }
        }
        cb::ENTITY_POSITION_SYNC => {
            let id = r.varint()?; let pos = [r.f64()?, r.f64()?, r.f64()?]; r.f64()?; r.f64()?; r.f64()?;
            let yaw = r.f32()?; let pitch = r.f32()?; let on_ground = r.bool().unwrap_or(false);
            Event::TeleportEntity { id, pos, yaw, pitch, on_ground }
        }
        cb::TELEPORT_ENTITY => {
            let id = r.varint()?; let pos = [r.f64()?, r.f64()?, r.f64()?]; r.f64()?; r.f64()?; r.f64()?;
            let yaw = r.f32()?; let pitch = r.f32()?; let _flags = r.i32()?; let on_ground = r.bool().unwrap_or(false);
            Event::TeleportEntity { id, pos, yaw, pitch, on_ground }
        }
        cb::SET_ENTITY_MOTION => { let id = r.varint()?; Event::EntityMotion { id, vel: read_lpvec3(&mut r)? } }
        cb::SET_ENTITY_DATA => { let id = r.varint()?; Event::EntityData { id, entries: read_meta(&mut r)? } }
        cb::ROTATE_HEAD => { let id = r.varint()?; Event::HeadYaw { id, yaw: angle(r.u8()?) } }
        cb::ANIMATE => { let id = r.varint()?; Event::Animate { id, action: r.u8()? } }
        cb::HURT_ANIMATION | cb::DAMAGE_EVENT => Event::Hurt { id: r.varint()? },
        cb::ENTITY_EVENT => { let id = r.i32()?; Event::EntityEvent { id, status: r.u8()? } }
        cb::TAKE_ITEM_ENTITY => { let item = r.varint()?; let collector = r.varint()?; Event::TakeItem { item, collector } }
        cb::REMOVE_ENTITIES => { let n = r.varint()?; let mut v = Vec::new(); for _ in 0..n { v.push(r.varint()?); } Event::RemoveEntities(v) }
        cb::BLOCK_DESTRUCTION => { let id = r.varint()?; let (x, y, z) = r.position()?; Event::BlockDestruction { id, pos: [x, y, z], stage: r.i8()? } }
        cb::SOUND => {
            let event = read_sound_event(&mut r)?; let category = r.varint()?;
            let pos = [r.i32()? as f64 / 8.0, r.i32()? as f64 / 8.0, r.i32()? as f64 / 8.0];
            Event::Sound { event, pos: Some(pos), entity: None, volume: r.f32()?, pitch: r.f32()?, category }
        }
        cb::SOUND_ENTITY => {
            let event = read_sound_event(&mut r)?; let category = r.varint()?; let eid = r.varint()?;
            Event::Sound { event, pos: None, entity: Some(eid), volume: r.f32()?, pitch: r.f32()?, category }
        }
        cb::LEVEL_EVENT => { let event = r.i32()?; let (x, y, z) = r.position()?; Event::LevelEvent { event, pos: [x, y, z], data: r.i32()? } }
        _ => return Ok(()),
    };
    tx.send(ev).ok();
    Ok(())
}

/// Optional score number format: absent = the number, 0 blank, 1 styled (number kept), 2 fixed text.
fn read_number_format(r: &mut Reader) -> Result<crate::board::Fmt> {
    use crate::board::Fmt;
    if !r.bool()? { return Ok(Fmt::Default); }
    Ok(match r.varint()? { 0 => Fmt::Blank, 1 => { r.nbt()?; Fmt::Default } _ => Fmt::Fixed(crate::lang::styled(&r.nbt()?)) })
}

/// Decode a chunk on the caller's thread.
pub fn decode_chunk(raw: &[u8], nsections: usize) -> Result<Chunk> { Chunk::decode(&mut Reader::new(raw), nsections) }

#[cfg(test)]
mod tests {
    #[test]
    fn lpvec3_roundtrip() {
        for v in [[0.1, 0.2, -0.1], [3.5, -0.25, 0.0], [-12.0, 7.5, 1.0]] {
            let mut w = basalt_protocol::Writer::new(); w.lpvec3(v);
            let d = super::read_lpvec3(&mut basalt_protocol::Reader::new(&w.buf)).unwrap();
            for i in 0..3 { assert!((d[i] - v[i]).abs() < 1e-3 * v[i].abs().max(1.0), "{v:?} -> {d:?}"); }
        }
    }
}

#[cfg(test)]
mod angle_tests {
    use super::*;
    #[test]
    fn pitch_is_signed_and_limited() {
        assert_eq!(pitch_angle(0), 0.0);
        assert_eq!(pitch_angle(0xF0), -22.5);          // looking slightly up (was 337.5°)
        assert_eq!(pitch_angle(0x10), 22.5);           // looking slightly down
        assert_eq!(pitch_angle(0xC0), -90.0);          // straight up
        assert_eq!(pitch_angle(0x40), 90.0);           // straight down
        assert_eq!(pitch_angle(0x80), -90.0);          // out of range → clamped, not a flip
        assert_eq!(pitch_angle(0x7F), 90.0);
    }
    #[test]
    fn move_entity_pos_rot_decodes_signed_pitch() {
        use basalt_protocol::Writer;
        let (tx, rx) = std::sync::mpsc::channel();
        let mut w = Writer::new();
        w.varint(7).i16(0).i16(0).i16(0).u8(64).u8(0xE0).bool(true);   // yaw 90°, pitch byte 0xE0 = -45°
        handle_play(cb::MOVE_ENTITY_POS_ROT, &w.buf, &tx, &mut HashMap::new()).unwrap();
        match rx.try_recv().unwrap() { Event::MoveEntity { id: 7, rot: Some((yaw, pitch)), .. } => { assert_eq!(yaw, 90.0); assert_eq!(pitch, -45.0); } _ => panic!("wrong event") }
    }
}

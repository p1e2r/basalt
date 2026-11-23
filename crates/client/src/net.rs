//! Network thread: login → configuration → play. Decodes packets into events
//! for the main thread and writes outbound packets queued by it.
use crate::world::Chunk;
use anyhow::{anyhow, bail, Result};
use basalt_protocol::{ids, text, Conn, Reader, Writer, PROTOCOL_VERSION};
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use tokio::net::TcpStream;
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use ids::play::clientbound as cb;
use ids::play::serverbound as sb;

#[derive(Debug, Clone, Default)]
pub struct ItemStack { pub count: i32, pub item: i32 }

pub enum Event {
    Registries { biomes: Vec<String>, dimension_types: Vec<String> },
    Login { entity_id: i32, dimension_type: i32, dimension: String, gamemode: u8 },
    Respawn { dimension_type: i32, dimension: String, gamemode: u8 },
    Chunk { cx: i32, cz: i32, raw: Vec<u8> },
    Light { cx: i32, cz: i32, raw: Vec<u8> },
    Unload { cx: i32, cz: i32 },
    Block { x: i32, y: i32, z: i32, state: u16 },
    Teleport { id: i32, pos: [f64; 3], vel: [f64; 3], yaw: f32, pitch: f32, flags: i32 },
    Chat(String),
    Health { hp: f32, food: i32 },
    GameMode(u8),
    Time { game: i64, day: i64 },
    HeldSlot(u8),
    Slot { window: i32, slot: i32, item: ItemStack },
    Inventory { items: Vec<ItemStack> },
    AddEntity { id: i32, uuid: Uuid, kind: i32, pos: [f64; 3], yaw: f32 },
    MoveEntity { id: i32, delta: [f64; 3], rot: Option<(f32, f32)> },
    TeleportEntity { id: i32, pos: [f64; 3], yaw: f32 },
    RemoveEntities(Vec<i32>),
    PlayerName { uuid: Uuid, name: String },
    Velocity([f64; 3]),
    Died(String),
    Disconnected(String),
}

pub struct Options { pub addr: String, pub name: String }

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
    let mut c = Conn::new(TcpStream::connect((host.as_str(), port)).await?);
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
        let mut w = Writer::with_id(ids::configuration::serverbound::CLIENT_INFORMATION);
        w.str("en_us").i8(12).varint(0).bool(true).u8(0x7f).varint(1).bool(false).bool(true).varint(0); c.send(&w).await?;
        let mut w = Writer::with_id(ids::configuration::serverbound::CUSTOM_PAYLOAD); w.str("minecraft:brand").str("basalt"); c.send(&w).await?;
        let (mut biomes, mut dims) = (Vec::new(), Vec::new());
        loop {
            let f = c.read().await?; let mut r = f.reader();
            use ids::configuration::clientbound as cc; use ids::configuration::serverbound as cs;
            match f.id {
                cc::SELECT_KNOWN_PACKS => { let mut w = Writer::with_id(cs::SELECT_KNOWN_PACKS); w.bytes(&f.data); c.send(&w).await?; }
                cc::REGISTRY_DATA => {
                    let reg = r.str()?; let n = r.varint()?; let mut names = Vec::new();
                    for _ in 0..n { names.push(r.str()?); if r.bool()? { r.nbt()?; } }
                    match reg.as_str() { "minecraft:worldgen/biome" => biomes = names, "minecraft:dimension_type" => dims = names, _ => {} }
                }
                cc::KEEP_ALIVE => { let v = r.i64()?; let mut w = Writer::with_id(cs::KEEP_ALIVE); w.i64(v); c.send(&w).await?; }
                cc::PING => { let v = r.i32()?; let mut w = Writer::with_id(cs::PONG); w.i32(v); c.send(&w).await?; }
                cc::DISCONNECT => bail!("kicked: {}", text::to_plain(&r.nbt()?)),
                cc::FINISH_CONFIGURATION => { c.send(&Writer::with_id(cs::FINISH_CONFIGURATION)).await?; break; }
                _ => {}
            }
        }
        tx.send(Event::Registries { biomes, dimension_types: dims }).ok();

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
                        cb::DISCONNECT => { let m = text::to_plain(&f.reader().nbt()?); bail!("kicked: {m}") }
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

fn read_slot(r: &mut Reader) -> Result<ItemStack> {
    let count = r.varint()?;
    if count <= 0 { return Ok(ItemStack::default()); }
    let item = r.varint()?;
    let (add, rem) = (r.varint()?, r.varint()?);
    if add != 0 || rem != 0 { return Err(anyhow!("component patch (item {item})")); }
    Ok(ItemStack { count, item })
}

/// Skip a compact ("low precision") vec3 as used for entity velocity.
fn skip_lpvec3(r: &mut Reader) -> Result<()> {
    let b0 = r.u8()?;
    if b0 == 0 { return Ok(()); }
    r.take(5)?;
    if b0 & 4 != 0 { r.varint()?; }
    Ok(())
}

fn angle(b: u8) -> f32 { b as f32 * 360.0 / 256.0 }

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
        cb::SYSTEM_CHAT => { let t = r.nbt()?; let overlay = r.bool().unwrap_or(false); if overlay { return Ok(()); } Event::Chat(text::to_plain(&t)) }
        cb::PLAYER_CHAT => {
            let _global = r.varint()?; let sender = r.uuid()?; let _idx = r.varint()?;
            if r.bool()? { r.take(256)?; }
            let msg = r.str()?;
            let name = players.get(&sender).cloned().unwrap_or_else(|| "?".into());
            Event::Chat(format!("<{name}> {msg}"))
        }
        cb::PLAYER_COMBAT_KILL => { let _id = r.varint()?; Event::Died(text::to_plain(&r.nbt()?)) }
        cb::DISGUISED_CHAT => Event::Chat(text::to_plain(&r.nbt()?)),
        cb::SET_HEALTH => { let hp = r.f32()?; let food = r.varint()?; Event::Health { hp, food } }
        cb::GAME_EVENT => { let e = r.u8()?; let v = r.f32()?; if e == 3 { Event::GameMode(v as u8) } else { return Ok(()) } }
        cb::SET_TIME => {
            let game = r.i64()?; let n = r.varint()?; let mut day = game;
            for _ in 0..n { let clock = r.varint()?; let t = r.varlong()?; r.f32()?; r.f32()?; if clock == 0 { day = t; } }
            Event::Time { game, day }
        }
        cb::SET_HELD_SLOT => Event::HeldSlot(r.varint()? as u8),
        cb::CONTAINER_SET_SLOT => {
            let window = r.varint()?; let _state = r.varint()?; let slot = r.i16()? as i32;
            Event::Slot { window, slot, item: read_slot(&mut r)? }
        }
        cb::CONTAINER_SET_CONTENT => {
            let window = r.varint()?; let _state = r.varint()?; let n = r.varint()?;
            if window != 0 { return Ok(()); }
            let mut items = Vec::new();
            for _ in 0..n { match read_slot(&mut r) { Ok(s) => items.push(s), Err(_) => break } }
            Event::Inventory { items }
        }
        cb::SET_PLAYER_INVENTORY => { let slot = r.varint()?; Event::Slot { window: -2, slot, item: read_slot(&mut r)? } }
        cb::PLAYER_INFO_UPDATE => {
            let actions = r.u8()?; let n = r.varint()?;
            if actions & 1 == 0 { return Ok(()); }
            for _ in 0..n {
                let uuid = r.uuid()?; let name = r.str()?;
                let np = r.varint()?; for _ in 0..np { r.str()?; r.str()?; if r.bool()? { r.str()?; } }
                players.insert(uuid, name.clone());
                tx.send(Event::PlayerName { uuid, name }).ok();
                // remaining actions for this entry
                if actions & 2 != 0 { if r.bool()? { r.uuid()?; r.i64()?; r.byte_array()?; r.byte_array()?; } }
                if actions & 4 != 0 { r.varint()?; }
                if actions & 8 != 0 { r.bool()?; }
                if actions & 16 != 0 { r.varint()?; }
                if actions & 32 != 0 { if r.bool()? { r.nbt()?; } }
                if actions & 64 != 0 { r.varint()?; }
                if actions & 128 != 0 { r.bool()?; }
            }
            return Ok(());
        }
        cb::ADD_ENTITY => {
            let id = r.varint()?; let uuid = r.uuid()?; let kind = r.varint()?;
            let pos = [r.f64()?, r.f64()?, r.f64()?];
            skip_lpvec3(&mut r)?; let _pitch = r.u8()?; let yaw = angle(r.u8()?);
            Event::AddEntity { id, uuid, kind, pos, yaw }
        }
        cb::MOVE_ENTITY_POS => { let id = r.varint()?; let d = [r.i16()? as f64 / 4096.0, r.i16()? as f64 / 4096.0, r.i16()? as f64 / 4096.0]; Event::MoveEntity { id, delta: d, rot: None } }
        cb::MOVE_ENTITY_POS_ROT => {
            let id = r.varint()?; let d = [r.i16()? as f64 / 4096.0, r.i16()? as f64 / 4096.0, r.i16()? as f64 / 4096.0];
            let yaw = angle(r.u8()?); let pitch = angle(r.u8()?);
            Event::MoveEntity { id, delta: d, rot: Some((yaw, pitch)) }
        }
        cb::MOVE_ENTITY_ROT => { let id = r.varint()?; let yaw = angle(r.u8()?); let pitch = angle(r.u8()?); Event::MoveEntity { id, delta: [0.0; 3], rot: Some((yaw, pitch)) } }
        cb::ENTITY_POSITION_SYNC => {
            let id = r.varint()?; let pos = [r.f64()?, r.f64()?, r.f64()?]; r.f64()?; r.f64()?; r.f64()?;
            let yaw = r.f32()?;
            Event::TeleportEntity { id, pos, yaw }
        }
        cb::REMOVE_ENTITIES => { let n = r.varint()?; let mut v = Vec::new(); for _ in 0..n { v.push(r.varint()?); } Event::RemoveEntities(v) }
        _ => return Ok(()),
    };
    tx.send(ev).ok();
    Ok(())
}

/// Decode a chunk on the caller's thread.
pub fn decode_chunk(raw: &[u8], nsections: usize) -> Result<Chunk> { Chunk::decode(&mut Reader::new(raw), nsections) }

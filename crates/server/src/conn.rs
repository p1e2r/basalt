//! Per-connection network task: handshake, status, login, configuration, then hands the
//! player to the game thread and pumps packets both ways.
use crate::game::ToGame;
use crate::registry::Registries;
use crate::Config;
use anyhow::{bail, Result};
use basalt_protocol::{ids, text, Conn, Writer, PROTOCOL_VERSION, VERSION_NAME};
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use tokio::net::TcpStream;
use uuid::Uuid;

pub struct Shared {
    pub cfg: Config,
    pub registries: Registries,
    pub data: Arc<basalt_game::GameData>,
    pub online: AtomicUsize,
    pub next_eid: AtomicI32,
    pub game: std::sync::Mutex<mpsc::Sender<ToGame>>,
    /// ops / whitelist / bans (admin.rs)
    pub admin: std::sync::Mutex<crate::admin::Lists>,
}

pub fn offline_uuid(name: &str) -> Uuid {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(format!("OfflinePlayer:{name}").as_bytes());
    let mut b: [u8; 16] = h.finalize().into();
    b[6] = b[6] & 0x0f | 0x30;
    b[8] = b[8] & 0x3f | 0x80;
    Uuid::from_bytes(b)
}

pub async fn handle(stream: TcpStream, sh: Arc<Shared>) -> Result<()> {
    let peer = stream.peer_addr()?;
    let mut c = Conn::new(stream);
    let f = c.read().await?;
    if f.id != 0 {
        bail!("expected handshake")
    }
    let mut r = f.reader();
    let proto = r.varint()?;
    let _host = r.str()?;
    let _port = r.u16()?;
    let intent = r.varint()?;
    match intent {
        1 => status(c, &sh, proto).await,
        2 | 3 => login(c, sh, proto, peer).await,
        _ => bail!("bad intent {intent}"),
    }
}

async fn status(mut c: Conn, sh: &Shared, _proto: i32) -> Result<()> {
    use ids::status::{clientbound as cb, serverbound as sb};
    loop {
        let f = c.read().await?;
        match f.id {
            sb::STATUS_REQUEST => {
                let j = serde_json::json!({
                    "version": { "name": VERSION_NAME, "protocol": PROTOCOL_VERSION },
                    "players": { "max": sh.cfg.max_players, "online": sh.online.load(Ordering::Relaxed), "sample": [] },
                    "description": { "text": sh.cfg.motd },
                    "enforcesSecureChat": false,
                });
                let mut w = Writer::with_id(cb::STATUS_RESPONSE);
                w.str(&j.to_string());
                c.send(&w).await?;
            }
            sb::PING_REQUEST => {
                let v = f.reader().i64()?;
                let mut w = Writer::with_id(cb::PONG_RESPONSE);
                w.i64(v);
                c.send(&w).await?;
                return Ok(());
            }
            _ => bail!("unexpected status packet {}", f.id),
        }
    }
}

async fn disconnect_login(c: &mut Conn, msg: &str) -> Result<()> {
    let mut w = Writer::with_id(ids::login::clientbound::LOGIN_DISCONNECT);
    w.str(&serde_json::json!({ "text": msg }).to_string());
    c.send(&w).await
}

async fn login(mut c: Conn, sh: Arc<Shared>, proto: i32, peer: std::net::SocketAddr) -> Result<()> {
    use ids::login::{clientbound as lcb, serverbound as lsb};
    let f = c.read().await?;
    if f.id != lsb::HELLO {
        bail!("expected login hello")
    }
    let name = f.reader().str()?;
    if proto != PROTOCOL_VERSION {
        disconnect_login(&mut c, &format!("Basalt speaks {VERSION_NAME} (protocol {PROTOCOL_VERSION}); you are on protocol {proto}")).await?;
        return Ok(());
    }
    if name.is_empty()
        || name.len() > 16
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        disconnect_login(&mut c, "Invalid username").await?;
        return Ok(());
    }
    let uuid = offline_uuid(&name);
    // bans, whitelist, a full server (vanilla's messages)
    let refused = sh
        .admin
        .lock()
        .unwrap()
        .refuse(&uuid.to_string(), &name, &peer.ip().to_string());
    if let Some((key, reason)) = refused {
        let mut w = Writer::with_id(lcb::LOGIN_DISCONNECT);
        let with = if key.ends_with(".reason") {
            serde_json::json!([reason])
        } else {
            serde_json::json!([])
        };
        w.str(&serde_json::json!({ "translate": key, "with": with }).to_string());
        c.send(&w).await?;
        return Ok(());
    }
    if sh.online.load(Ordering::Relaxed) >= sh.cfg.max_players
        && sh.admin.lock().unwrap().level(&uuid.to_string(), &name) < 1
    {
        let mut w = Writer::with_id(lcb::LOGIN_DISCONNECT);
        w.str(
            &serde_json::json!({ "translate": "multiplayer.disconnect.server_full" }).to_string(),
        );
        c.send(&w).await?;
        return Ok(());
    }
    if sh.cfg.compression >= 0 {
        let mut w = Writer::with_id(lcb::LOGIN_COMPRESSION);
        w.varint(sh.cfg.compression);
        c.send(&w).await?;
        c.set_threshold(sh.cfg.compression);
    }
    // 26.2 appends a 16-byte per-connection id after the profile (observed: differs every login; random works)
    let mut w = Writer::with_id(lcb::LOGIN_FINISHED);
    w.uuid(uuid).str(&name).varint(0).uuid(Uuid::new_v4());
    c.send(&w).await?;
    loop {
        let f = c.read().await?;
        match f.id {
            lsb::LOGIN_ACKNOWLEDGED => break,
            lsb::CUSTOM_QUERY_ANSWER => {}
            x => bail!("unexpected login packet {x}"),
        }
    }
    log::info!("{name} ({uuid}) logging in from {peer}");
    let view = configure(&mut c, &sh).await?;

    // ---- play: hand off to the game thread ----
    let eid = sh.next_eid.fetch_add(1, Ordering::Relaxed);
    let (otx, mut orx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let game = sh.game.lock().unwrap().clone();
    game.send(ToGame::Join {
        eid,
        uuid,
        name: name.clone(),
        out: otx,
        view,
    })?;
    sh.online.fetch_add(1, Ordering::Relaxed);
    let Conn {
        r: mut rd,
        w: mut wr,
    } = c;
    let writer = tokio::spawn(async move {
        while let Some(buf) = orx.recv().await {
            if buf.is_empty() {
                break;
            } // game asked us to close
            if wr.send(&Writer { buf }).await.is_err() {
                break;
            }
        }
    });
    let res: Result<()> = async {
        loop {
            tokio::select! {
                f = rd.read() => { let f = f?; game.send(ToGame::Packet { eid, id: f.id, data: f.data })?; }
                _ = tokio::time::sleep(std::time::Duration::from_secs(1)), if writer.is_finished() => return Ok(()),
            }
        }
    }.await;
    sh.online.fetch_sub(1, Ordering::Relaxed);
    let _ = game.send(ToGame::Leave { eid });
    writer.abort();
    if let Err(e) = &res {
        log::info!("{name} disconnected: {e}");
    }
    Ok(())
}

/// Configuration phase. Returns the client's requested view distance.
async fn configure(c: &mut Conn, sh: &Shared) -> Result<u8> {
    use ids::configuration::{clientbound as cb, serverbound as sb};
    let mut w = Writer::with_id(cb::CUSTOM_PAYLOAD);
    w.str("minecraft:brand").str("basalt");
    c.send(&w).await?;
    let mut w = Writer::with_id(cb::UPDATE_ENABLED_FEATURES);
    w.varint(1).str("minecraft:vanilla");
    c.send(&w).await?;
    let mut w = Writer::with_id(cb::SELECT_KNOWN_PACKS);
    w.varint(1).str("minecraft").str("core").str(VERSION_NAME);
    c.send(&w).await?;
    let mut view = 10u8;
    let mut sent_registries = false;
    loop {
        let f = c.read().await?;
        let mut r = f.reader();
        match f.id {
            sb::CLIENT_INFORMATION => {
                let _locale = r.str()?;
                view = r.i8()?.clamp(2, 32) as u8;
            }
            sb::SELECT_KNOWN_PACKS if !sent_registries => {
                let n = r.varint()?;
                let mut has_core = false;
                for _ in 0..n {
                    let ns = r.str()?;
                    let id = r.str()?;
                    let ver = r.str()?;
                    if ns == "minecraft" && id == "core" && ver == VERSION_NAME {
                        has_core = true;
                    }
                }
                if !has_core {
                    let mut w = Writer::with_id(cb::DISCONNECT);
                    w.nbt(&text::plain(
                        "Basalt needs a client with the vanilla 26.2 core data pack",
                    ));
                    c.send(&w).await?;
                    bail!("client lacks core pack");
                }
                for (reg, entries) in &sh.registries.synced {
                    let mut w = Writer::with_id(cb::REGISTRY_DATA);
                    w.str(reg).varint(entries.len() as i32);
                    for e in entries {
                        w.str(e).bool(false);
                    }
                    c.send(&w).await?;
                }
                let mut w = Writer::with_id(cb::UPDATE_TAGS);
                w.varint(sh.registries.tags.len() as i32);
                for (reg, tags) in &sh.registries.tags {
                    w.str(reg).varint(tags.len() as i32);
                    for (t, ids) in tags {
                        w.str(t).varint(ids.len() as i32);
                        for i in ids {
                            w.varint(*i);
                        }
                    }
                }
                c.send(&w).await?;
                c.send(&Writer::with_id(cb::FINISH_CONFIGURATION)).await?;
                sent_registries = true;
            }
            sb::FINISH_CONFIGURATION if sent_registries => return Ok(view),
            sb::CUSTOM_PAYLOAD | sb::KEEP_ALIVE | sb::PONG | sb::RESOURCE_PACK => {}
            _ => {}
        }
    }
}

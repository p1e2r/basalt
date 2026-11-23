//! Transparent logging proxy (uncompressed servers only) for protocol interop testing.
use anyhow::Result;
use basalt_protocol::{ids, Reader};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Copy, PartialEq)]
enum St { Hs, Login, Config, Play }

pub async fn run(listen: &str, target: &str, dump: &str) -> Result<()> {
    std::fs::create_dir_all(dump)?;
    let l = TcpListener::bind(listen).await?;
    loop {
        let (c, _) = l.accept().await?;
        let s = TcpStream::connect(target).await?;
        let state = Arc::new(Mutex::new(St::Hs));
        let counter = Arc::new(Mutex::new(0u32));
        let (cr, cw) = c.into_split(); let (sr, sw) = s.into_split();
        let d = dump.to_string();
        tokio::spawn(pipe(cr, sw, true, state.clone(), counter.clone(), d.clone()));
        tokio::spawn(pipe(sr, cw, false, state, counter, d));
    }
}

async fn pipe(mut r: tokio::net::tcp::OwnedReadHalf, mut w: tokio::net::tcp::OwnedWriteHalf, c2s: bool, st: Arc<Mutex<St>>, n: Arc<Mutex<u32>>, dump: String) -> Result<()> {
    let mut buf = Vec::new();
    let mut tmp = vec![0u8; 1 << 16];
    loop {
        let k = r.read(&mut tmp).await?; if k == 0 { return Ok(()); }
        w.write_all(&tmp[..k]).await?;
        buf.extend_from_slice(&tmp[..k]);
        loop {
            let mut rd = Reader::new(&buf);
            let Ok(len) = rd.varint() else { break };
            if len < 0 { buf.clear(); break; }
            let hdr = rd.pos;
            if buf.len() < hdr + len as usize { break; }
            let pkt = buf[hdr..hdr + len as usize].to_vec();
            buf.drain(..hdr + len as usize);
            // never let a packet we can't parse stop the forwarding (bytes were already written above)
            let mut pr = Reader::new(&pkt); let Ok(id) = pr.varint() else { continue }; let body = &pkt[pr.pos..];
            let mut s = st.lock().unwrap();
            let name = match (*s, c2s) {
                (St::Hs, true) => "handshake",
                (St::Login, true) => ids::login::serverbound::name(id), (St::Login, false) => ids::login::clientbound::name(id),
                (St::Config, true) => ids::configuration::serverbound::name(id), (St::Config, false) => ids::configuration::clientbound::name(id),
                (St::Play, true) => ids::play::serverbound::name(id), (St::Play, false) => ids::play::clientbound::name(id),
                _ => "?",
            };
            let skip_log = !c2s && matches!(name, "minecraft:level_chunk_with_light" | "minecraft:move_entity_pos" | "minecraft:move_entity_pos_rot" | "minecraft:rotate_head" | "minecraft:set_entity_motion" | "minecraft:entity_position_sync" | "minecraft:move_entity_rot");
            if c2s && !matches!(name, "minecraft:move_player_pos" | "minecraft:move_player_pos_rot" | "minecraft:move_player_rot" | "minecraft:client_tick_end" | "minecraft:move_player_status_only") || (!c2s && !skip_log) {
                let hex: String = body.iter().take(48).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
                println!("{} {name} [{}] {hex}", if c2s { "C>S" } else { "S>C" }, body.len());
            }
            let mut cn = n.lock().unwrap(); *cn += 1;
            std::fs::write(format!("{dump}/{:05}_{}_{}.bin", *cn, if c2s { "c2s" } else { "s2c" }, name.trim_start_matches("minecraft:")), body).ok();
            match (*s, c2s, name) {
                (St::Hs, true, _) => { *s = St::Login; }
                (St::Login, true, "minecraft:login_acknowledged") => *s = St::Config,
                (St::Config, true, "minecraft:finish_configuration") => *s = St::Play,
                _ => {}
            }
        }
    }
}

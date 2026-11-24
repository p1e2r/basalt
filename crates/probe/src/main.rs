use anyhow::Result;
use basalt_protocol::{ids, text, Conn, Writer, PROTOCOL_VERSION};
use tokio::net::TcpStream;
mod proxy;

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("proxy") {
        let a: Vec<String> = std::env::args().collect();
        return proxy::run(&a[2], &a[3], &a[4]).await;
    }
    let addr = std::env::args().nth(1).unwrap_or("127.0.0.1:25566".into());
    let secs: u64 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let dump = std::env::args().nth(3);
    if let Some(d) = &dump {
        std::fs::create_dir_all(d)?;
    }
    let (host, port) = addr.split_once(':').unwrap();
    let mut c = Conn::new(TcpStream::connect(&addr).await?);
    let mut w = Writer::with_id(0);
    w.varint(PROTOCOL_VERSION)
        .str(host)
        .u16(port.parse()?)
        .varint(2);
    c.send(&w).await?;
    let mut w = Writer::with_id(ids::login::serverbound::HELLO);
    w.str("Probe").uuid(uuid::Uuid::nil());
    c.send(&w).await?;
    let mut state = "login";
    let mut n = 0;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        let f = match tokio::time::timeout_at(deadline, c.read()).await {
            Ok(f) => f?,
            Err(_) => break,
        };
        let name = match state {
            "login" => ids::login::clientbound::name(f.id),
            "configuration" => ids::configuration::clientbound::name(f.id),
            _ => ids::play::clientbound::name(f.id),
        };
        println!("[{state}] {name} ({} bytes)", f.data.len());
        if let Some(d) = &dump {
            std::fs::write(
                format!(
                    "{d}/{n:04}_{state}_{}.bin",
                    name.trim_start_matches("minecraft:")
                ),
                &f.data,
            )?;
        }
        n += 1;
        let mut r = f.reader();
        match (state, f.id) {
            ("login", ids::login::clientbound::LOGIN_COMPRESSION) => {
                let t = r.varint()?;
                c.set_threshold(t);
            }
            ("login", ids::login::clientbound::LOGIN_FINISHED) => {
                c.send(&Writer::with_id(
                    ids::login::serverbound::LOGIN_ACKNOWLEDGED,
                ))
                .await?;
                state = "configuration";
                let mut w = Writer::with_id(ids::configuration::serverbound::CLIENT_INFORMATION);
                w.str("en_us")
                    .i8(4)
                    .varint(0)
                    .bool(true)
                    .u8(0x7f)
                    .varint(1)
                    .bool(false)
                    .bool(true)
                    .varint(0);
                c.send(&w).await?;
            }
            ("login", ids::login::clientbound::LOGIN_DISCONNECT) => {
                println!("disconnect: {}", r.str()?);
                break;
            }
            ("configuration", ids::configuration::clientbound::SELECT_KNOWN_PACKS) => {
                let mut w = Writer::with_id(ids::configuration::serverbound::SELECT_KNOWN_PACKS);
                w.bytes(&f.data);
                c.send(&w).await?;
            }
            ("configuration", ids::configuration::clientbound::REGISTRY_DATA) => {
                let reg = r.str()?;
                let n = r.varint()?;
                let mut with_data = 0;
                for _ in 0..n {
                    let _k = r.str()?;
                    if r.bool()? {
                        r.nbt()?;
                        with_data += 1;
                    }
                }
                println!("    registry {reg}: {n} entries, {with_data} with data");
            }
            ("configuration", ids::configuration::clientbound::FINISH_CONFIGURATION) => {
                c.send(&Writer::with_id(
                    ids::configuration::serverbound::FINISH_CONFIGURATION,
                ))
                .await?;
                state = "play";
            }
            ("configuration", ids::configuration::clientbound::KEEP_ALIVE) => {
                let v = r.i64()?;
                let mut w = Writer::with_id(ids::configuration::serverbound::KEEP_ALIVE);
                w.i64(v);
                c.send(&w).await?;
            }
            ("play", ids::play::clientbound::KEEP_ALIVE) => {
                let v = r.i64()?;
                let mut w = Writer::with_id(ids::play::serverbound::KEEP_ALIVE);
                w.i64(v);
                c.send(&w).await?;
            }
            ("play", ids::play::clientbound::PLAYER_POSITION) => {
                let tid = r.varint()?;
                let mut w = Writer::with_id(ids::play::serverbound::ACCEPT_TELEPORTATION);
                w.varint(tid);
                c.send(&w).await?;
            }
            ("play", ids::play::clientbound::SYSTEM_CHAT) => {
                println!("    chat: {}", text::to_plain(&r.nbt()?));
            }
            ("play", ids::play::clientbound::DISCONNECT)
            | ("configuration", ids::configuration::clientbound::DISCONNECT) => {
                println!("    kicked: {}", text::to_plain(&r.nbt()?));
                break;
            }
            _ => {}
        }
    }
    Ok(())
}

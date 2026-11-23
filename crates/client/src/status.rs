//! Server list pings: the status handshake (MOTD, players, version, favicon) and round-trip time,
//! one background thread per server, results sent back over a channel.
use crate::assets::{decode_png, Image};
use anyhow::{anyhow, bail, Result};
use basalt_protocol::{ids, Conn, Writer, PROTOCOL_VERSION};
use serde_json::Value;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

pub struct Status {
    /// MOTD as coloured runs per line
    pub motd: Vec<Vec<(String, [u8; 4])>>,
    pub online: i64,
    pub max: i64,
    pub version: String,
    pub protocol: i64,
    pub icon: Option<Image>,
    pub ping_ms: u64,
}

pub enum PingState { Pinging(Instant), Done(Status), Failed(String) }

/// Ping `addr` on a background thread; the result arrives on `tx` tagged with the address.
pub fn spawn(addr: String, tx: Sender<(String, Result<Status>)>) {
    std::thread::Builder::new().name("ping".into()).spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let r = rt.block_on(async { tokio::time::timeout(Duration::from_secs(5), ping(&addr)).await.map_err(|_| anyhow!("timed out"))? });
        let _ = tx.send((addr, r));
    }).unwrap();
}

async fn ping(addr: &str) -> Result<Status> {
    let (host, port) = addr.rsplit_once(':').map(|(h, p)| (h.to_string(), p.parse().unwrap_or(25565))).unwrap_or((addr.to_string(), 25565));
    let mut c = Conn::new(tokio::net::TcpStream::connect((host.as_str(), port)).await?);
    let mut w = Writer::with_id(0); w.varint(PROTOCOL_VERSION).str(&host).u16(port).varint(1); c.send(&w).await?;
    c.send(&Writer::with_id(ids::status::serverbound::STATUS_REQUEST)).await?;
    let f = c.read().await?;
    if f.id != ids::status::clientbound::STATUS_RESPONSE { bail!("unexpected status packet {}", f.id); }
    let j: Value = serde_json::from_str(&f.reader().str()?)?;
    let t0 = Instant::now();
    let mut w = Writer::with_id(ids::status::serverbound::PING_REQUEST); w.i64(0x5eed); c.send(&w).await?;
    let _ = c.read().await?;
    let ping_ms = t0.elapsed().as_millis() as u64;
    let icon = j["favicon"].as_str().and_then(|s| s.strip_prefix("data:image/png;base64,")).and_then(|b| base64(b)).and_then(|png| decode_png(&png).ok());
    Ok(Status {
        motd: motd_lines(&j["description"]),
        online: j["players"]["online"].as_i64().unwrap_or(0), max: j["players"]["max"].as_i64().unwrap_or(0),
        version: j["version"]["name"].as_str().unwrap_or("").to_string(), protocol: j["version"]["protocol"].as_i64().unwrap_or(-1),
        icon, ping_ms,
    })
}

fn base64(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> { Some(match c { b'A'..=b'Z' => c - b'A', b'a'..=b'z' => c - b'a' + 26, b'0'..=b'9' => c - b'0' + 52, b'+' => 62, b'/' => 63, _ => return None } as u32) };
    let bytes: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=').collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for ch in bytes.chunks(4) {
        let mut n = 0u32;
        for (i, c) in ch.iter().enumerate() { n |= val(*c)? << (18 - 6 * i); }
        out.push((n >> 16) as u8);
        if ch.len() > 2 { out.push((n >> 8) as u8); }
        if ch.len() > 3 { out.push(n as u8); }
    }
    Some(out)
}

/// The 16 chat colours (vanilla's legacy formatting codes 0-f; standard values).
const COLORS: [(&str, [u8; 3]); 16] = [("black", [0, 0, 0]), ("dark_blue", [0, 0, 170]), ("dark_green", [0, 170, 0]), ("dark_aqua", [0, 170, 170]), ("dark_red", [170, 0, 0]),
    ("dark_purple", [170, 0, 170]), ("gold", [255, 170, 0]), ("gray", [170, 170, 170]), ("dark_gray", [85, 85, 85]), ("blue", [85, 85, 255]), ("green", [85, 255, 85]),
    ("aqua", [85, 255, 255]), ("red", [255, 85, 85]), ("light_purple", [255, 85, 255]), ("yellow", [255, 255, 85]), ("white", [255, 255, 255])];

fn color(name: &str) -> Option<[u8; 4]> {
    if let Some(h) = name.strip_prefix('#') { let v = u32::from_str_radix(h, 16).ok()?; return Some([(v >> 16) as u8, (v >> 8) as u8, v as u8, 255]); }
    COLORS.iter().find(|(n, _)| *n == name).map(|(_, c)| [c[0], c[1], c[2], 255])
}

/// Flatten a text component (string, object with extra, or array; `§` codes inside strings) into
/// coloured runs split at newlines. Default colour is the server list's gray.
pub fn motd_lines(v: &Value) -> Vec<Vec<(String, [u8; 4])>> {
    let mut runs: Vec<(String, [u8; 4])> = Vec::new();
    fn walk(v: &Value, col: [u8; 4], runs: &mut Vec<(String, [u8; 4])>) {
        match v {
            Value::String(s) => legacy(s, col, runs),
            Value::Array(a) => for x in a { walk(x, col, runs) },
            Value::Object(o) => {
                let c = o.get("color").and_then(|c| c.as_str()).and_then(color).unwrap_or(col);
                if let Some(t) = o.get("text").and_then(|t| t.as_str()) { legacy(t, c, runs); }
                else if let Some(k) = o.get("translate").and_then(|t| t.as_str()) { legacy(&crate::lang::tr(k, &[]), c, runs); }
                if let Some(e) = o.get("extra") { walk(e, c, runs); }
            }
            _ => {}
        }
    }
    fn legacy(s: &str, mut col: [u8; 4], runs: &mut Vec<(String, [u8; 4])>) {
        let mut cur = String::new();
        let mut it = s.chars();
        while let Some(ch) = it.next() {
            if ch == '§' {
                if let Some(code) = it.next() {
                    if !cur.is_empty() { runs.push((std::mem::take(&mut cur), col)); }
                    if let Some(d) = code.to_digit(16) { let c = COLORS[d as usize].1; col = [c[0], c[1], c[2], 255]; } else if code == 'r' { col = [170, 170, 170, 255]; }
                }
                continue;
            }
            cur.push(ch);
        }
        if !cur.is_empty() { runs.push((cur, col)); }
    }
    walk(v, [170, 170, 170, 255], &mut runs);
    let mut lines = vec![Vec::new()];
    for (t, c) in runs {
        for (i, part) in t.split('\n').enumerate() {
            if i > 0 { lines.push(Vec::new()); }
            if !part.is_empty() { lines.last_mut().unwrap().push((part.to_string(), c)); }
        }
    }
    lines.truncate(2);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn motd_and_base64() {
        let l = motd_lines(&serde_json::json!({"text": "A §aB\nC", "extra": [{"text": "D", "color": "red"}]}));
        assert_eq!(l.len(), 2);
        assert_eq!(l[0][0].0, "A "); assert_eq!(l[0][1], ("B".to_string(), [85, 255, 85, 255]));
        assert_eq!(l[1][1], ("D".to_string(), [255, 85, 85, 255]));
        assert_eq!(base64("aGVsbG8=").unwrap(), b"hello");
    }
}

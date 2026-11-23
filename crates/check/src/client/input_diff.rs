//! `basalt-check client-input`: input parity of basalt-client against the vanilla client.
//!
//! One scripted input sequence (walk, sprint, jump, sneak, look, hotbar, dig a block, place a block,
//! inventory + a few clicks incl. a drag) is run (a) by the real vanilla 26.2 client, driven through
//! the oracle mod's control channel (tools/oracle-mod, SCHEMA.md), and (b) by `basalt-client
//! --script`, each against its own fresh Basalt server (ports 25700+). Both log every serverbound
//! packet (vanilla: wire bytes from the mod's netty hook; basalt-client: its send path). The logs
//! are canonicalised (packet name + the fields that are input, not timing/sequence numbers), runs of
//! identical entries are collapsed, and the two sequences are diffed. Movement packets are compared
//! by summary (net displacement within a tolerance), never packet by packet: basalt-client ticks on
//! wall-clock time, vanilla on its own tick counter.
//!
//! usage: basalt-check client-input [--oracle-dir vanilla/oracle] [--ctl-port 25789] [--no-launch]
//!          [--client target/release/basalt-client] [--jar vanilla/client.jar] [--out target/check]
//!          [--keep-going] [--show-log]
use super::oracle::{Ctl, Oracle, DEFAULT_CTL_PORT};
use crate::server::BasaltServer;
use anyhow::{bail, Context, Result};
use basalt_protocol::{ids, Reader};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub struct Opts {
    pub oracle_dir: PathBuf,
    pub ctl_port: u16,
    pub launch: bool,
    pub client: PathBuf,
    pub jar: PathBuf,
    pub out: PathBuf,
    pub show_log: bool,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts {
            oracle_dir: "vanilla/oracle".into(),
            ctl_port: DEFAULT_CTL_PORT,
            launch: true,
            client: "target/release/basalt-client".into(),
            jar: "vanilla/client.jar".into(),
            out: "target/check".into(),
            show_log: false,
        }
    }
}

pub fn parse_args(args: &[String]) -> Result<Opts> {
    let mut o = Opts::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut v = || {
            it.next()
                .cloned()
                .with_context(|| format!("{a} needs a value"))
        };
        match a.as_str() {
            "--oracle-dir" => o.oracle_dir = v()?.into(),
            "--ctl-port" => o.ctl_port = v()?.parse()?,
            "--no-launch" => o.launch = false,
            "--client" => o.client = v()?.into(),
            "--jar" => o.jar = v()?.into(),
            "--out" => o.out = v()?.into(),
            "--show-log" => o.show_log = true,
            other => bail!("client-input: unknown argument {other}"),
        }
    }
    Ok(o)
}

/// Server arguments shared by every run of these harnesses: flat, survival (so digging takes time), no mobs.
pub const SERVER_ARGS: &[&str] = &[
    "--memory",
    "--flat",
    "--view",
    "4",
    "--no-spawning",
    "--gamemode",
    "survival",
];

/// Ports 25700..25759 only (the task's range); an explicit CHECK_PORT_BASE wins.
/// The range is shared with other agents/harnesses, so start at a pid-dependent offset (the 60-port window stays below 25800).
pub fn use_port_range() {
    if std::env::var("CHECK_PORT_BASE").is_err() {
        std::env::set_var(
            "CHECK_PORT_BASE",
            (25700 + std::process::id() % 40).to_string(),
        );
    }
}

fn op(v: Value) -> Value {
    v
}

/// The scripted input. Everything before the "begin" mark is setup and not compared.
pub fn script() -> Vec<Value> {
    let mut s = vec![
        op(json!({"op": "wait_join"})),
        op(json!({"op": "wait", "ticks": 20})),
        op(json!({"op": "command", "text": "give @s minecraft:dirt 64"})),
        op(json!({"op": "wait", "ticks": 15})),
        op(json!({"op": "look", "yaw": 0, "pitch": 0})),
        op(json!({"op": "wait", "ticks": 5})),
        op(json!({"op": "mark", "label": "begin"})),
    ];
    let w = |t: u32| json!({"op": "wait", "ticks": t});
    s.extend([
        // walk, sprint, stop
        json!({"op": "move", "forward": true}),
        w(10),
        json!({"op": "sprint", "action": "hold"}),
        w(10),
        json!({"op": "move", "forward": false}),
        json!({"op": "sprint", "action": "release"}),
        w(6),
        // jump, sneak
        json!({"op": "jump", "action": "tap"}),
        w(15),
        json!({"op": "sneak", "action": "press"}),
        w(8),
        json!({"op": "sneak", "action": "release"}),
        w(4),
        // look: absolute then relative
        json!({"op": "look", "yaw": 90, "pitch": 30}),
        w(4),
        json!({"op": "look", "yaw": -45, "pitch": -20, "relative": true}),
        w(4),
        // hotbar
        json!({"op": "hotbar", "slot": 3}),
        w(3),
        json!({"op": "hotbar", "slot": 0}),
        w(3),
        // dig the grass block under the feet (survival: takes ~18 ticks), then place dirt on the block below
        json!({"op": "look", "yaw": 0, "pitch": 90}),
        w(4),
        json!({"op": "attack", "action": "press"}),
        w(40),
        json!({"op": "attack", "action": "release"}),
        w(4),
        json!({"op": "use", "action": "tap"}),
        w(6),
        // inventory: open, a few clicks (pickup, put down, right-click, shift-click, number key, drop), a drag, close
        json!({"op": "inventory", "action": "open"}),
        w(6),
        json!({"op": "click", "slot": 36, "button": 0, "mode": 0}),
        w(2),
        json!({"op": "click", "slot": 20, "button": 0, "mode": 0}),
        w(2),
        json!({"op": "click", "slot": 20, "button": 1, "mode": 0}),
        w(2),
        json!({"op": "click", "slot": 20, "button": 0, "mode": 1}),
        w(2),
        json!({"op": "click", "slot": 37, "button": 0, "mode": 2}),
        w(2),
        json!({"op": "click", "slot": 36, "button": 0, "mode": 4}),
        w(2),
        json!({"op": "click", "slot": 20, "button": 0, "mode": 0}),
        w(2),
        json!({"op": "drag", "slots": [10, 11, 12], "button": 1}),
        w(3),
        json!({"op": "inventory", "action": "close"}),
        w(6),
        json!({"op": "mark", "label": "end"}),
        json!({"op": "flush"}),
    ]);
    s
}

// ------------------------------------------------------------------------------ log parsing

/// One serverbound play packet from a log: tick, name, body bytes (after the id).
#[derive(Clone, Debug)]
pub struct Pkt {
    pub tick: i64,
    pub name: String,
    pub body: Vec<u8>,
}

fn unhex(h: &str) -> Vec<u8> {
    (0..h.len() / 2)
        .filter_map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).ok())
        .collect()
}

/// Packets between marks `from` and `to` of a JSONL packet log (vanilla mod or basalt-client format).
pub fn read_log(path: &Path, from: &str, to: &str) -> Result<Vec<Pkt>> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let (mut on, mut seen) = (false, false);
    let mut v = Vec::new();
    for l in text.lines() {
        let Ok(j) = serde_json::from_str::<Value>(l) else {
            continue;
        };
        if let Some(m) = j["mark"].as_str() {
            if m == from {
                on = true;
                seen = true;
            } else if m == to {
                on = false;
            }
            continue;
        }
        if !on {
            continue;
        }
        let data = unhex(j["hex"].as_str().unwrap_or(""));
        let mut r = Reader::new(&data);
        let Ok(id) = r.varint() else { continue };
        v.push(Pkt {
            tick: j["tick"].as_i64().unwrap_or(-1),
            name: ids::play::serverbound::name(id)
                .trim_start_matches("minecraft:")
                .to_string(),
            body: data[r.pos..].to_vec(),
        });
    }
    if !seen {
        bail!("{}: no '{from}' mark (did the script run?)", path.display());
    }
    Ok(v)
}

/// Packets that are timing or bookkeeping, not input.
const IGNORED: &[&str] = &[
    "client_tick_end",
    "keep_alive",
    "pong",
    "chunk_batch_received",
    "accept_teleportation",
    "ping_request",
    "custom_payload",
    "client_information",
    "player_loaded",
    "move_player_status_only",
    "chat_command",
    "chat_command_signed",
    "chat",
    "resource_pack",
    "chat_ack",
    "teleport_to_entity",
];

/// Canonical text for one packet: its name plus the fields that are input (not sequence numbers, timing or hashes).
pub fn canon(p: &Pkt) -> Option<String> {
    if IGNORED.contains(&p.name.as_str()) || p.name.starts_with("move_player") {
        return None;
    }
    let mut r = Reader::new(&p.body);
    let f = (|| -> Result<String> {
        Ok(match p.name.as_str() {
            "set_carried_item" => format!("slot={}", r.i16()?),
            "player_action" => {
                let st = r.varint()?;
                let (x, y, z) = r.position()?;
                let face = r.u8()?;
                format!("status={st} pos={x},{y},{z} face={face}")
            }
            "use_item_on" => {
                let hand = r.varint()?;
                let (x, y, z) = r.position()?;
                let face = r.varint()?;
                format!("hand={hand} pos={x},{y},{z} face={face}")
            }
            "use_item" | "swing" => format!("hand={}", r.varint()?),
            "player_command" => {
                let _eid = r.varint()?;
                format!("action={}", r.varint()?)
            }
            "player_input" => format!("flags={:#04x}", r.u8()?),
            "container_click" => {
                let win = r.varint()?;
                let _state = r.varint()?;
                let slot = r.i16()?;
                let btn = r.i8()?;
                let mode = r.varint()?;
                let n = r.varint()?;
                format!("window={win} slot={slot} button={btn} mode={mode} changed={n}")
            }
            "container_close" => format!("window={}", r.varint()?),
            _ => String::new(),
        })
    })();
    Some(match f {
        Ok(s) if s.is_empty() => p.name.clone(),
        Ok(s) => format!("{} {s}", p.name),
        Err(_) => format!("{} <undecodable>", p.name),
    })
}

/// Canonical sequence with runs of identical entries collapsed to one.
pub fn sequence(pk: &[Pkt]) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for p in pk {
        if let Some(c) = canon(p) {
            if v.last() != Some(&c) {
                v.push(c);
            }
        }
    }
    v
}

/// Net displacement over the movement packets (x, z), as a movement summary.
pub fn displacement(pk: &[Pkt]) -> (f64, f64, usize) {
    let mut first: Option<(f64, f64)> = None;
    let mut last = (0.0, 0.0);
    let mut n = 0;
    for p in pk
        .iter()
        .filter(|p| p.name == "move_player_pos" || p.name == "move_player_pos_rot")
    {
        let mut r = Reader::new(&p.body);
        if let (Ok(x), Ok(_), Ok(z)) = (r.f64(), r.f64(), r.f64()) {
            if first.is_none() {
                first = Some((x, z));
            }
            last = (x, z);
            n += 1;
        }
    }
    let f = first.unwrap_or(last);
    (last.0 - f.0, last.1 - f.1, n)
}

/// Longest-common-subsequence diff: (only in a, only in b) with their positions.
pub type Indexed = Vec<(usize, String)>;

pub fn diff_seq(a: &[String], b: &[String]) -> (Indexed, Indexed) {
    let (n, m) = (a.len(), b.len());
    let mut t = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i][j] = if a[i] == b[j] {
                t[i + 1][j + 1] + 1
            } else {
                t[i + 1][j].max(t[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut oa, mut ob) = (0, 0, Vec::new(), Vec::new());
    while i < n && j < m {
        if a[i] == b[j] {
            i += 1;
            j += 1;
        } else if t[i + 1][j] >= t[i][j + 1] {
            oa.push((i, a[i].clone()));
            i += 1;
        } else {
            ob.push((j, b[j].clone()));
            j += 1;
        }
    }
    while i < n {
        oa.push((i, a[i].clone()));
        i += 1;
    }
    while j < m {
        ob.push((j, b[j].clone()));
        j += 1;
    }
    (oa, ob)
}

// ------------------------------------------------------------------------------ the two runs

pub fn start_server(out: &Path, tag: &str) -> Result<BasaltServer> {
    let log = out.join(format!("client-input/{tag}.server.log"));
    let mut last = None;
    for _ in 0..6 {
        // another harness may grab the port between the free check and the bind: retry on a new port
        match BasaltServer::start(SERVER_ARGS, &log) {
            Ok(s) => return Ok(s),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap())
}

/// Run the script on the vanilla client against a fresh Basalt server; returns the packet log path.
pub fn run_vanilla(o: &Opts, ops: &[Value], work: &Path, tag: &str) -> Result<PathBuf> {
    let srv = start_server(&o.out, &format!("{tag}-vanilla"))?;
    let _oracle = Oracle::connect_port(&o.oracle_dir, o.launch, o.ctl_port)?;
    let mut ctl = Ctl::connect(o.ctl_port, Duration::from_secs(20))?;
    let log = std::fs::canonicalize(work)
        .unwrap_or(work.to_path_buf())
        .join(format!("{tag}-vanilla.jsonl"));
    ctl.call(
        json!({"op": "log", "file": log.to_string_lossy()}),
        Duration::from_secs(10),
    )?;
    ctl.call(
        json!({"op": "connect", "addr": format!("127.0.0.1:{}", srv.port)}),
        Duration::from_secs(10),
    )?;
    let res = ctl.run(ops, Duration::from_secs(60))?;
    for (i, r) in res.iter().enumerate() {
        if r["ok"] != json!(true) {
            bail!(
                "vanilla client: command {} {} failed: {}",
                i,
                ops[i],
                r["error"]
                    .as_str()
                    .unwrap_or("?")
                    .lines()
                    .next()
                    .unwrap_or("")
            );
        }
    }
    let _ = ctl.call(json!({"op": "disconnect"}), Duration::from_secs(10));
    let _ = ctl.run(
        &[
            json!({"op": "wait", "ticks": 10}),
            json!({"op": "log", "close": true}),
        ],
        Duration::from_secs(10),
    );
    drop(srv);
    Ok(log)
}

/// Run the script on basalt-client (`--script`) against a fresh Basalt server.
pub fn run_basalt(o: &Opts, ops: &[Value], work: &Path, tag: &str) -> Result<PathBuf> {
    let srv = start_server(&o.out, &format!("{tag}-basalt"))?;
    let script = work.join(format!("{tag}.script.jsonl"));
    std::fs::write(
        &script,
        ops.iter().map(|v| v.to_string() + "\n").collect::<String>(),
    )?;
    let log = work.join(format!("{tag}-basalt.jsonl"));
    if !o.client.exists() {
        bail!(
            "{} missing: cargo build --release -p basalt-client",
            o.client.display()
        );
    }
    let outp = Command::new(&o.client)
        .arg("--jar")
        .arg(&o.jar)
        .arg("--name")
        .arg("BasaltBot")
        .arg("--addr")
        .arg(format!("127.0.0.1:{}", srv.port))
        .arg("--script")
        .arg(&script)
        .arg("--script-log")
        .arg(&log)
        .env("RUST_LOG", "warn")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("starting basalt-client --script")?;
    if !outp.status.success() {
        bail!(
            "basalt-client --script failed: {}",
            String::from_utf8_lossy(&outp.stderr)
                .lines()
                .rev()
                .take(3)
                .collect::<Vec<_>>()
                .join(" | ")
        );
    }
    drop(srv);
    Ok(log)
}

pub fn run(o: &Opts) -> Result<usize> {
    use_port_range();
    let work = o.out.join("client-input");
    std::fs::create_dir_all(&work)?;
    let ops = script();
    eprintln!(
        "client-input: {} ops; vanilla client via {}, basalt-client via {}",
        ops.len(),
        o.oracle_dir.display(),
        o.client.display()
    );
    let vlog = run_vanilla(o, &ops, &work, "input")?;
    let blog = run_basalt(o, &ops, &work, "input")?;
    let (v, b) = (
        read_log(&vlog, "begin", "end")?,
        read_log(&blog, "begin", "end")?,
    );
    let (sv, sb) = (sequence(&v), sequence(&b));
    if o.show_log {
        println!("vanilla sequence:");
        for (i, s) in sv.iter().enumerate() {
            println!("  {i:3} {s}");
        }
        println!("basalt sequence:");
        for (i, s) in sb.iter().enumerate() {
            println!("  {i:3} {s}");
        }
    }
    // swings are timing (vanilla swings every tick while digging, basalt-client once per action): counted, not diffed
    let swings = |pk: &[Pkt]| pk.iter().filter(|p| p.name == "swing").count();
    println!(
        "swing packets (info): vanilla {} basalt {}",
        swings(&v),
        swings(&b)
    );
    let no_swing = |s: &[String]| {
        s.iter()
            .filter(|e| !e.starts_with("swing "))
            .cloned()
            .collect::<Vec<_>>()
    };
    let (sv, sb) = (no_swing(&sv), no_swing(&sb));
    let (only_v, only_b) = diff_seq(&sv, &sb);
    let known = crate::fuzz::known_divergences();
    let class = |side: &str, e: &str| {
        let name = e.split(' ').next().unwrap_or("?");
        let mode = if name == "container_click" {
            e.split("mode=")
                .nth(1)
                .and_then(|m| m.split(' ').next())
                .map(|m| format!("/mode{m}"))
                .unwrap_or_default()
        } else {
            String::new()
        };
        format!("{side}/{name}{mode}")
    };
    let mut new = 0;
    for (side, list, label) in [
        ("missing", &only_v, "MISSING in basalt-client (vanilla #"),
        ("extra", &only_b, "EXTRA in basalt-client (basalt #"),
    ] {
        for (i, s) in list.iter() {
            let c = class(side, s);
            let is_known = known.contains(&format!("client_input {c}"));
            if !is_known {
                new += 1;
            }
            println!(
                "{} {label}{i}): {s}   [{c}]",
                if is_known { "known" } else { "NEW  " }
            );
        }
    }
    let (vd, bd) = (displacement(&v), displacement(&b));
    // basalt-client steps on wall-clock time, so allow 35 % + 0.5 block on the net displacement
    let tol = |a: f64, b: f64| (a - b).abs() <= 0.35 * a.abs().max(b.abs()) + 0.5;
    let move_ok = tol(vd.0, bd.0) && tol(vd.1, bd.1);
    println!("movement: vanilla dx={:.2} dz={:.2} ({} pos packets), basalt dx={:.2} dz={:.2} ({} pos packets): {}", vd.0, vd.1, vd.2, bd.0, bd.1, bd.2, if move_ok { "ok" } else { "MISMATCH" });
    let mism = new + (!move_ok) as usize;
    println!(
        "RESULT client_input vanilla_events={} basalt_events={} divergences={} new={mism}",
        sv.len(),
        sb.len(),
        only_v.len() + only_b.len()
    );
    Ok(mism)
}

pub fn main(args: &[String]) -> i32 {
    match parse_args(args).and_then(|o| run(&o)) {
        Ok(0) => 0,
        Ok(_) => 1,
        Err(e) => {
            eprintln!("client-input: {e:#}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pk(name: &str, body: &[u8]) -> Pkt {
        Pkt {
            tick: 0,
            name: name.into(),
            body: body.to_vec(),
        }
    }
    #[test]
    fn canonical_ignores_sequence_numbers_and_collapses_runs() {
        // player_action: status 0, pos long, face 1, sequence 1 vs 7
        let mut a = vec![0u8];
        a.extend(0x6fc3u64.to_be_bytes());
        a.push(1);
        a.push(1);
        let mut b = a.clone();
        *b.last_mut().unwrap() = 7;
        assert_eq!(
            canon(&pk("player_action", &a)),
            canon(&pk("player_action", &b))
        );
        let seq = sequence(&[
            pk("swing", &[0]),
            pk("swing", &[0]),
            pk("client_tick_end", &[]),
            pk("swing", &[0]),
        ]);
        assert_eq!(seq, vec!["swing hand=0".to_string()]);
    }
    #[test]
    fn lcs_diff_finds_missing_and_extra() {
        let a: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        let b: Vec<String> = ["a", "c", "d"].map(String::from).to_vec();
        let (oa, ob) = diff_seq(&a, &b);
        assert_eq!(oa, vec![(1, "b".to_string())]);
        assert_eq!(ob, vec![(2, "d".to_string())]);
    }
    #[test]
    fn script_has_begin_and_end_marks() {
        let s = script();
        assert!(s.iter().any(|o| o["label"] == "begin") && s.iter().any(|o| o["label"] == "end"));
    }
}

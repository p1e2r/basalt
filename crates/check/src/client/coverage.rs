//! Client harnesses that need basalt-client (and a GPU) but not the real vanilla client:
//!
//! * `basalt-check client-assets`: asset coverage, every block state and every item rendered through
//!   `basalt-client --asset-grid` (no missing model / texture outside tools/asset-allowlist.txt).
//! * `basalt-check client-ui [--names a,b] [--no-vanilla]`: the headless UI scenarios of
//!   `basalt-client --scenario` (inventory, crafting table, chest, furnace, menus, ...) against a
//!   fresh Basalt server, one result per scenario (clicks asserted against server state).
use super::{client_bin, no_gpu, skip};
use crate::server::BasaltServer;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The scenarios of crates/client/src/scenario.rs (`ALL`); keep in sync.
pub const UI_SCENARIOS: &[&str] = &[
    "edge_chunks",
    "inventory",
    "crafting_table",
    "death",
    "dig",
    "dig_others",
    "models",
    "interp",
    "head_pitch",
    "first_person_arm",
    "command_suggestions",
    "sounds",
    "menus",
    "held_item",
    "break_particles",
    "options_pages",
    "chest",
    "furnace",
    "server_list",
    "singleplayer",
    "falling_block",
    "sprint",
];

/// Source paths the UI scenarios / asset grid depend on.
pub fn affected() -> Vec<&'static str> {
    let mut p: Vec<&'static str> = super::RENDER_AFFECTED.to_vec();
    p.extend([
        "crates/server/**",
        "crates/game/**",
        "tools/asset-allowlist.txt",
        "crates/check/src/client/coverage.rs",
        "crates/check/src/server.rs",
        "crates/check/Cargo.toml",
    ]);
    p
}

struct Cli {
    client: PathBuf,
    jar: PathBuf,
    out: PathBuf,
    names: Vec<String>,
    vanilla: Option<String>,
}

fn parse(args: &[String]) -> Result<Cli> {
    let mut c = Cli {
        client: client_bin(),
        jar: basalt_paths::client_jar(),
        out: "target/check".into(),
        names: vec![],
        vanilla: None,
    };
    let mut vanilla =
        Some(std::env::var("VANILLA_ADDR").unwrap_or_else(|_| "127.0.0.1:25566".into()));
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut v = || {
            it.next()
                .cloned()
                .with_context(|| format!("{a} needs a value"))
        };
        match a.as_str() {
            "--client" => c.client = v()?.into(),
            "--jar" => c.jar = v()?.into(),
            "--out" => c.out = v()?.into(),
            "--names" => {
                c.names = v()?
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect()
            }
            "--vanilla" => vanilla = Some(v()?),
            "--no-vanilla" => vanilla = None,
            other => bail!("unknown argument {other}"),
        }
    }
    // the scenarios that compare against the vanilla server need it reachable; otherwise they skip themselves
    c.vanilla = vanilla.filter(|a| {
        use std::net::ToSocketAddrs;
        a.to_socket_addrs()
            .ok()
            .and_then(|mut x| x.next())
            .map(|s| {
                std::net::TcpStream::connect_timeout(&s, std::time::Duration::from_millis(800))
                    .is_ok()
            })
            .unwrap_or(false)
    });
    Ok(c)
}

fn need_client(c: &Cli) -> Result<()> {
    if !c.client.exists() {
        bail!(
            "{} missing: cargo build --release -p basalt-client",
            c.client.display()
        );
    }
    Ok(())
}

// ------------------------------------------------------------------------------ assets

fn grid(c: &Cli, kind: &str, dir: &Path) -> Result<(u64, u64, u64, bool)> {
    let png = dir.join(format!("{kind}.png"));
    let _ = std::fs::remove_file(png.with_extension("json"));
    let out = Command::new(&c.client)
        .arg("--jar")
        .arg(&c.jar)
        .arg("--asset-grid")
        .arg(&png)
        .arg(if kind == "items" {
            "--items"
        } else {
            "--blocks"
        })
        .args(["--allowlist", "tools/asset-allowlist.txt"])
        .env("RUST_LOG", "warn")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("starting basalt-client --asset-grid")?;
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    if !png.with_extension("json").exists() {
        if no_gpu(&text) {
            return skip("no graphics adapter for basalt-client");
        }
        bail!(
            "asset-grid {kind} produced no report: {}",
            text.lines().rev().take(3).collect::<Vec<_>>().join(" | ")
        );
    }
    let j: serde_json::Value = serde_json::from_slice(&std::fs::read(png.with_extension("json"))?)?;
    for p in j["problems"].as_array().into_iter().flatten() {
        println!("  missing {kind}: {p}");
    }
    Ok((
        j["cells"].as_u64().unwrap_or(0),
        j["total"].as_u64().unwrap_or(0),
        j["problems"]
            .as_array()
            .map(|a| a.len() as u64)
            .unwrap_or(0),
        out.status.success(),
    ))
}

pub fn assets_run(args: &[String]) -> Result<usize> {
    let c = parse(args)?;
    need_client(&c)?;
    let dir = c.out.join("client-assets");
    std::fs::create_dir_all(&dir)?;
    let (bc, bt, bp, bok) = grid(&c, "blocks", &dir)?;
    let (ic, it, ip, iok) = grid(&c, "items", &dir)?;
    let failed = (bp + ip) as usize + (!bok || !iok) as usize;
    println!("assets: {bt} block states rendered through {bc} distinct models, {it} items through {ic} icons; unexplained problems {} (grids in {})", bp + ip, dir.display());
    println!(
        "RESULT client_assets block_states={bt} item_icons={it} problems={} failed={failed}",
        bp + ip
    );
    Ok(failed)
}

pub fn assets_main(args: &[String]) -> i32 {
    super::finish("client_assets", assets_run(args))
}

// ------------------------------------------------------------------------------ UI scenarios

pub fn ui_run(args: &[String]) -> Result<usize> {
    let c = parse(args)?;
    need_client(&c)?;
    let names: Vec<String> = if c.names.is_empty() {
        UI_SCENARIOS.iter().map(|s| s.to_string()).collect()
    } else {
        c.names.clone()
    };
    super::input_diff::use_port_range();
    let dir = c.out.join("client-ui");
    std::fs::create_dir_all(&dir)?;
    let srv = BasaltServer::start(
        &["--memory", "--flat", "--no-spawning"],
        &dir.join("server.log"),
    )?;
    let mut cmd = Command::new(&c.client);
    cmd.arg("--jar")
        .arg(&c.jar)
        .arg("--addr")
        .arg(srv.addr())
        .arg("--scenario")
        .arg(names.join(","));
    if let Some(v) = &c.vanilla {
        cmd.arg("--vanilla").arg(v);
    }
    let out = cmd
        .env("BASALT_SCENARIO_SHOTS", dir.join("shots"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("starting basalt-client --scenario")?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    let _ = std::fs::write(dir.join("client.out"), &text);
    let _ = std::fs::write(dir.join("client.err"), &err);
    let lines: Vec<&str> = text.lines().filter(|l| l.starts_with("SCENARIO")).collect();
    if lines.is_empty() {
        if no_gpu(&(text.clone() + &err)) {
            return skip("no graphics adapter for basalt-client");
        }
        bail!(
            "basalt-client --scenario printed no SCENARIO line: {}",
            err.lines().rev().take(3).collect::<Vec<_>>().join(" | ")
        );
    }
    let (mut failed, mut skipped, mut passed) = (0, 0, 0);
    for l in &lines {
        println!("{l}");
        match l.split_whitespace().nth(2) {
            Some("PASS") => passed += 1,
            Some("SKIP") => skipped += 1,
            _ => failed += 1,
        }
    }
    let missing = names
        .iter()
        .filter(|n| !lines.iter().any(|l| l.contains(&format!("M7/{n} "))))
        .count();
    failed += missing;
    println!(
        "RESULT client_ui scenarios={} pass={passed} skipped={skipped} failed={failed}",
        names.len()
    );
    Ok(failed)
}

pub fn ui_main(args: &[String]) -> i32 {
    super::finish("client_ui", ui_run(args))
}

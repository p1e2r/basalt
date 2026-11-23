//! Ports of tools/server-smoke.sh and tools/client-scenarios.sh.
use crate::util::{self, Guard};
use anyhow::Result;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// End-to-end smoke test: basalt-server (in-memory world) driven by two headless basalt-clients.
pub fn smoke(args: &[String]) -> Result<i32> {
    let out = PathBuf::from(args.first().map(String::as_str).unwrap_or("target/smoke"));
    let port = env_or("PORT", "25580");
    std::fs::create_dir_all(&out)?;
    if util::cargo_ok(&["build", "-q", "-p", "basalt-server", "-p", "basalt-client"]).is_err() {
        return Ok(1);
    }
    let server_log = out.join("server.log");
    let server = Guard(util::spawn_logged(
        Command::new(util::exe("debug", "basalt-server")).args([
            "--memory",
            "--flat",
            "--port",
            &port,
            "--gamemode",
            "creative",
        ]),
        &server_log,
    )?);
    util::wait_for_log(&server_log, "listening", Duration::from_secs(10));

    let client = util::exe("debug", "basalt-client");
    let addr = format!("127.0.0.1:{port}");
    let alice_png = out.join("alice.png");
    let bob_png = out.join("bob.png");
    let (alice_log, bob_log) = (out.join("alice.log"), out.join("bob.log"));
    // 1. join, chat, command, place, walk; screenshot
    let mut alice = util::spawn_logged(
        Command::new(&client)
            .args([&addr, "--name", "Alice", "--screenshot"])
            .arg(&alice_png)
            .args([
                "--wait",
                "10",
                "--chat",
                "hello from alice",
                "--cmd",
                "give @s stone 10",
                "--look",
                "0,60",
                "--click",
                "r",
            ]),
        &alice_log,
    )?;
    std::thread::sleep(Duration::from_secs(2));
    // 2. a second player joining at the same time (multiplayer visibility)
    util::run_logged(
        Command::new(&client)
            .args([&addr, "--name", "Bob", "--screenshot"])
            .arg(&bob_png)
            .args(["--wait", "14", "--walk", "4", "--look", "90,10"]),
        &bob_log,
    )?;
    let _ = alice.wait();

    let mut fail = false;
    let mut check = |log: &Path, needle: &str, what: &str| {
        let ok = if needle.starts_with("entities [1-9]") {
            has_entities(&util::read_lossy(log))
        } else {
            util::read_lossy(log).contains(needle)
        };
        println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        fail |= !ok;
    };
    check(&alice_log, "screenshot", "alice joined and rendered");
    check(&bob_log, "screenshot", "bob joined and rendered");
    check(&alice_log, "<Alice> hello from alice", "chat echoed");
    check(&server_log, "Bob joined", "server saw second player");
    check(&alice_log, "entities [1-9]", "alice saw another entity");
    check(&server_log, "issued /give", "command received");
    let warns: Vec<String> = util::read_lossy(&server_log)
        .lines()
        .filter(|l| {
            ["WARN", "ERROR", "bad packet", "panicked"]
                .iter()
                .any(|k| l.contains(k))
        })
        .map(String::from)
        .collect();
    if !warns.is_empty() {
        for w in &warns {
            println!("{w}");
        }
        println!("FAIL server warnings");
        fail = true;
    }
    drop(server);
    Ok(fail as i32)
}

/// `entities N` with N >= 1 anywhere in the log (the grep `entities [1-9]` of the old script).
fn has_entities(log: &str) -> bool {
    log.match_indices("entities ").any(|(i, m)| {
        log[i + m.len()..]
            .chars()
            .next()
            .is_some_and(|c| ('1'..='9').contains(&c))
    })
}

/// M7 client scenarios against a fresh in-memory Basalt server (offscreen rendering: needs a GPU).
pub fn client_scenarios(args: &[String]) -> Result<i32> {
    let which = args.first().cloned().unwrap_or_else(|| "all".into());
    let port = env_or("PORT", "25790");
    let vanilla = env_or("VANILLA", "127.0.0.1:25566");
    let out = PathBuf::from(env_or("OUT", "target/client-scenarios"));
    std::fs::create_dir_all(&out)?;
    if util::cargo_ok(&[
        "build",
        "-q",
        "--release",
        "-p",
        "basalt-server",
        "-p",
        "basalt-client",
    ])
    .is_err()
    {
        return Ok(1);
    }
    let server_log = out.join("server.log");
    let server = Guard(util::spawn_logged(
        Command::new(util::exe("release", "basalt-server")).args([
            "--memory",
            "--flat",
            "--port",
            &port,
            "--no-spawning",
        ]),
        &server_log,
    )?);
    util::wait_for_log(&server_log, "listening", Duration::from_secs(20));

    let client_err = std::fs::File::create(out.join("client.log"))?;
    let mut client = Command::new(util::exe("release", "basalt-client"))
        .args([
            "--addr",
            &format!("127.0.0.1:{port}"),
            "--vanilla",
            &vanilla,
            "--scenario",
            &which,
        ])
        .env("BASALT_SCENARIO_SHOTS", out.join("shots"))
        .stdout(Stdio::piped())
        .stderr(client_err)
        .spawn()?;
    let mut all = std::fs::File::create(out.join("client.out"))?;
    let mut results = std::fs::File::create(out.join("results.txt"))?;
    for line in BufReader::new(client.stdout.take().expect("piped stdout")).lines() {
        let line = line?;
        writeln!(all, "{line}")?;
        if line.starts_with("SCENARIO") {
            println!("{line}");
            writeln!(results, "{line}")?;
        }
    }
    let mut rc = client.wait()?.code().unwrap_or(1);
    if util::read_lossy(&server_log)
        .lines()
        .any(|l| l.contains("panicked"))
    {
        println!("FAIL server panicked");
        rc = 1;
    }
    drop(server);
    Ok(rc)
}

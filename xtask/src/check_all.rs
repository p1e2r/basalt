//! Port of tools/check-all.sh: unit tests, worldgen parity vs vanilla, bot scenarios vs Basalt,
//! differential runs vs the vanilla test server, optional fuzz harnesses and real-client smoke.
//! Output: a pass/fail table grouped by milestone; target/check/results.json; non-zero exit on failure.
use crate::util::{self, clip, read_lossy, secs, Guard};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone)]
struct R {
    group: String,
    id: String,
    server: String,
    pass: bool,
    skip: bool,
    secs: f64,
    failures: Vec<String>,
    notes: Vec<String>,
}

impl R {
    fn new(group: &str, id: &str, server: &str, pass: bool, secs: f64, detail: &str) -> R {
        R {
            group: group.into(),
            id: id.into(),
            server: server.into(),
            pass,
            skip: false,
            secs,
            failures: if pass { vec![] } else { vec![detail.into()] },
            notes: vec![detail.into()],
        }
    }
    fn skip(group: &str, id: &str, server: &str, detail: &str) -> R {
        R {
            skip: true,
            notes: vec![detail.into()],
            ..R::new(group, id, server, true, 0.0, detail)
        }
    }
    fn from_json(v: &Value) -> R {
        let strs = |k: &str| {
            v[k].as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        };
        R {
            group: v["group"].as_str().unwrap_or("").into(),
            id: v["id"].as_str().unwrap_or("").into(),
            server: v["server"].as_str().unwrap_or("").into(),
            pass: v["pass"].as_bool().unwrap_or(false),
            skip: v["skip"].as_bool().unwrap_or(false),
            secs: v["secs"].as_f64().unwrap_or(0.0),
            failures: strs("failures"),
            notes: strs("notes"),
        }
    }
    fn to_json(&self) -> Value {
        let mut v = json!({"group": self.group, "id": self.id, "server": self.server, "pass": self.pass, "secs": self.secs, "failures": self.failures, "notes": self.notes});
        if self.skip {
            v["skip"] = json!(true);
        }
        v
    }
}

const ORDER: [&str; 12] = [
    "Build",
    "Tests",
    "M1",
    "M2",
    "M3",
    "M4",
    "M5",
    "Env",
    "Items",
    "Diff",
    "Fuzz",
    "Real client",
];

fn order_of(g: &str) -> usize {
    ORDER.iter().position(|o| *o == g).unwrap_or(99)
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// Vanilla reference worlds: generated headless by the official server jar, cached per seed+version
/// (vanilla/parity/<seed>, marker done-26.2; shared with tools/parity.sh).
fn gen_vanilla_world(seed: &str, java: &str) -> Result<()> {
    let dir = PathBuf::from("vanilla/parity").join(seed);
    if dir.join("done-26.2").exists() {
        return Ok(());
    }
    let jar = Path::new("vanilla/server.jar");
    if !jar.exists() {
        crate::extract_data::download_server_jar(jar)?;
    }
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let port = util::free_port()?;
    std::fs::write(dir.join("eula.txt"), "eula=true\n")?;
    std::fs::write(
        dir.join("server.properties"),
        format!(
            "level-seed={seed}\nserver-port={port}\nonline-mode=false\nlevel-type=minecraft\\:normal\ngenerate-structures=true\nspawn-protection=0\nview-distance=2\nsimulation-distance=2\nmax-tick-time=-1\npause-when-empty-seconds=-1\nnetwork-compression-threshold=-1\nsync-chunk-writes=false\n"
        ),
    )?;
    let log = dir.join("server.log");
    let mut child = Command::new(java)
        .current_dir(&dir)
        .args(["-Xmx2G", "-jar", "../../server.jar", "nogui"])
        .stdin(Stdio::piped())
        .stdout(Stdio::from(std::fs::File::create(&log)?))
        .stderr(Stdio::from(std::fs::File::create(dir.join("server.err"))?))
        .spawn()?;
    // the console is the child's stdin pipe (portable: no fifo needed)
    let mut console = child.stdin.take().expect("piped stdin");
    if !util::wait_for_log(&log, "Done (", Duration::from_secs(300)) {
        let _ = child.kill();
        bail!("vanilla server for seed {seed} didn't start");
    }
    // force-load chunks -5..4 (one ring more than the compared -3..3), wait until saved full
    writeln!(console, "forceload add -80 -80 79 79")?;
    let region = dir.join("world/dimensions/minecraft/overworld/region");
    let compare = util::exe("release/examples", "compare");
    let mut ok = false;
    for _ in 0..40 {
        std::thread::sleep(Duration::from_secs(5));
        writeln!(console, "save-all flush")?;
        std::thread::sleep(Duration::from_secs(2));
        if region.is_dir() {
            if let Ok(o) = Command::new(&compare)
                .arg(&region)
                .args(["0", "0", "3", seed])
                .output()
            {
                if String::from_utf8_lossy(&o.stdout).contains("chunks=49 ") {
                    ok = true;
                    break;
                }
            }
        }
    }
    let _ = writeln!(console, "stop");
    drop(console);
    let _ = child.wait();
    if ok {
        std::fs::write(dir.join("done-26.2"), "")?;
        Ok(())
    } else {
        bail!("vanilla world for seed {seed} never reached 49 saved chunks")
    }
}

fn parity_verdict(result_line: &str) -> (bool, String) {
    let kv: HashMap<&str, &str> = result_line
        .split_whitespace()
        .skip(1)
        .filter_map(|p| p.split_once('='))
        .collect();
    let th = [
        ("biomes", 99.9),
        ("shape", 99.0),
        ("exact", 98.5),
        ("features", 98.5),
    ];
    let bad: Vec<String> = th
        .iter()
        .filter(|(k, v)| kv.get(k).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0) < *v)
        .map(|(k, v)| format!("{k} {} < {v}", kv.get(k).unwrap_or(&"None")))
        .collect();
    let mut d = th
        .iter()
        .map(|(k, _)| format!("{k}={}", kv.get(k).unwrap_or(&"None")))
        .collect::<Vec<_>>()
        .join(" ");
    if !bad.is_empty() {
        d += &format!(" | {}", bad.join("; "));
    }
    (bad.is_empty(), d)
}

pub fn run(args: &[String]) -> Result<i32> {
    let (mut quick, mut vanilla, mut real, mut fuzz, mut fuzz_long) =
        (false, true, false, false, false);
    for a in args {
        match a.as_str() {
            "--fuzz" => fuzz = true,
            "--fuzz-long" => {
                fuzz = true;
                fuzz_long = true
            }
            "--quick" => quick = true,
            "--no-vanilla" => vanilla = false,
            "--real-client" => real = true,
            "--ci" => {
                quick = true;
                vanilla = false;
                real = false
            }
            "-h" | "--help" => {
                println!("usage: cargo xtask check-all [--quick] [--no-vanilla] [--real-client] [--ci] [--fuzz] [--fuzz-long]\nenv: VANILLA_ADDR (127.0.0.1:25566), VANILLA_DIR (vanilla/srv), PARITY_SEEDS (\"12345 42 1\"), CHECK_JOBS (4), JAVA (java)");
                return Ok(0);
            }
            other => bail!("unknown option {other}"),
        }
    }
    let out = PathBuf::from("target/check");
    std::fs::create_dir_all(&out)?;
    let _ = std::fs::remove_file(out.join("scenarios.json"));
    let java = env_or("JAVA", "java");
    let vanilla_addr = env_or("VANILLA_ADDR", "127.0.0.1:25566");
    let vanilla_dir = env_or("VANILLA_DIR", "vanilla/srv");
    let seeds = env_or("PARITY_SEEDS", if quick { "12345" } else { "12345 42 1" });
    let radius = if quick { "2" } else { "3" };
    let jobs = env_or("CHECK_JOBS", "4");
    let mut rows: Vec<R> = Vec::new();
    let check_bin = util::exe("release", "basalt-check");
    let server_bin = util::exe("release", "basalt-server");

    println!("== build");
    let t = Instant::now();
    let st = util::status(util::cargo().args([
        "build",
        "--release",
        "-p",
        "basalt-server",
        "-p",
        "basalt-check",
        "-q",
    ]))?;
    if !st.success() || !server_bin.exists() || !check_bin.exists() {
        println!("build failed");
        return Ok(1);
    }
    if !util::status(util::cargo().args([
        "build",
        "--release",
        "-p",
        "basalt-worldgen",
        "--example",
        "compare",
        "-q",
    ]))?
    .success()
    {
        println!("compare example build failed");
        return Ok(1);
    }
    rows.push(R::new(
        "Build",
        "cargo build --release",
        "basalt",
        true,
        secs(t),
        "server, check, worldgen compare",
    ));

    // ---------------------------------------------------------------- 1. unit tests
    println!("== cargo test --workspace");
    let t = Instant::now();
    let tlog = out.join("cargo-test.log");
    let st = util::run_logged(util::cargo().args(["test", "--workspace", "-q"]), &tlog)?;
    let tl = read_lossy(&tlog);
    if st.success() {
        let n: u64 = tl
            .lines()
            .filter(|l| l.starts_with("test result"))
            .filter_map(|l| l.split_whitespace().nth(3)?.parse::<u64>().ok())
            .sum();
        rows.push(R::new(
            "Tests",
            "cargo test --workspace",
            "basalt",
            true,
            secs(t),
            &format!("{n} tests passed"),
        ));
    } else {
        let d: Vec<&str> = tl
            .lines()
            .filter(|l| l.contains("FAILED") || l.contains("panicked") || l.contains("error"))
            .take(3)
            .collect();
        rows.push(R::new(
            "Tests",
            "cargo test --workspace",
            "basalt",
            false,
            secs(t),
            &d.join(" "),
        ));
    }

    // ---------------------------------------------------------------- 2. worldgen parity
    println!("== worldgen parity (seeds: {seeds})");
    let compare = util::exe("release/examples", "compare");
    for seed in seeds.split_whitespace() {
        let t = Instant::now();
        if let Err(e) = gen_vanilla_world(seed, &java) {
            rows.push(R::new(
                "M3",
                &format!("parity seed {seed}"),
                "basalt",
                false,
                secs(t),
                &format!("vanilla world generation failed: {e}"),
            ));
            continue;
        }
        let region = format!("vanilla/parity/{seed}/world/dimensions/minecraft/overworld/region");
        let o = Command::new(&compare)
            .arg(&region)
            .args(["0", "0", radius, seed])
            .output()?;
        let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
        let _ = std::fs::write(out.join(format!("parity-{seed}.log")), &stdout);
        let _ = std::fs::write(out.join(format!("parity-{seed}.err")), &o.stderr);
        let res = stdout
            .lines()
            .find(|l| l.starts_with("RESULT"))
            .unwrap_or("RESULT");
        // thresholds: biomes >= 99.9, shape >= 99.0, exact >= 98.5, features >= 98.5
        let (ok, d) = parity_verdict(res);
        rows.push(R::new(
            "M3",
            &format!("parity seed {seed} r{radius}"),
            "basalt",
            ok,
            secs(t),
            &d,
        ));
    }
    if Path::new("vanilla/parity/12345/done-26.2").exists() {
        std::env::set_var("VANILLA_WORLD", "vanilla/parity/12345/world");
    }

    // ---------------------------------------------------------------- 3+4. scenarios (and differential)
    let t = Instant::now();
    let slog = out.join("scenarios.log");
    let sjson = out.join("scenarios.json");
    let mut c = Command::new(&check_bin);
    if vanilla {
        println!("== scenarios vs Basalt + differential vs vanilla {vanilla_addr}");
        c.args([
            "--diff",
            "--vanilla",
            &vanilla_addr,
            "--vanilla-dir",
            &vanilla_dir,
        ]);
    } else {
        println!("== scenarios vs Basalt");
    }
    util::run_logged(c.args(["--jobs", &jobs, "--json"]).arg(&sjson), &slog)?;
    let scen_secs = secs(t);

    // ---------------------------------------------------------------- 4b. fuzz harnesses (opt-in)
    let fuzz_ids = ["clicks", "commands", "dig", "place", "worldgen"];
    if fuzz {
        println!("== fuzz harnesses (clicks commands dig place worldgen)");
        let mut c = Command::new(&check_bin);
        c.args([
            "--fuzz",
            "all",
            "--jobs",
            &jobs,
            "--vanilla",
            &vanilla_addr,
            "--vanilla-dir",
            &vanilla_dir,
            "--out",
        ])
        .arg(&out);
        if !vanilla {
            c.arg("--no-vanilla");
        }
        if fuzz_long {
            c.arg("--long");
        }
        util::run_logged(&mut c, &out.join("fuzz.log"))?;
        let mut res: HashMap<String, HashMap<String, String>> = HashMap::new();
        for l in read_lossy(&out.join("fuzz.log"))
            .lines()
            .filter(|l| l.starts_with("RESULT fuzz="))
        {
            let kv: HashMap<String, String> = l
                .split_whitespace()
                .skip(1)
                .filter_map(|p| p.split_once('='))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            if let Some(f) = kv.get("fuzz") {
                res.insert(f.clone(), kv);
            }
        }
        for i in fuzz_ids {
            let id = format!("fuzz_{i}");
            rows.push(match res.get(i) {
                None => R::new(
                    "Fuzz",
                    &id,
                    "both",
                    false,
                    0.0,
                    "no RESULT line (see target/check/fuzz.log)",
                ),
                Some(kv) if kv.get("skipped").map(String::as_str) == Some("1") => {
                    let mut r = R::skip(
                        "Fuzz",
                        &id,
                        "both",
                        "skipped (no vanilla server or no data)",
                    );
                    r.secs = kv.get("secs").and_then(|s| s.parse().ok()).unwrap_or(0.0);
                    r
                }
                Some(kv) => {
                    let g = |k: &str| kv.get(k).cloned().unwrap_or_else(|| "None".into());
                    let num = |k: &str| kv.get(k).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
                    let ok = g("new") == "0" && num("errors") * 5 <= num("cases");
                    let d = format!(
                        "cases={} mismatches={} new={} info={} errors={}",
                        g("cases"),
                        g("mismatches"),
                        g("new"),
                        g("info"),
                        g("errors")
                    );
                    R::new(
                        "Fuzz",
                        &id,
                        "both",
                        ok,
                        kv.get("secs").and_then(|s| s.parse().ok()).unwrap_or(0.0),
                        &d,
                    )
                }
            });
        }
    } else {
        for i in fuzz_ids {
            rows.push(R::skip(
                "Fuzz",
                &format!("fuzz_{i}"),
                "both",
                "not run (use --fuzz or --fuzz-long)",
            ));
        }
    }

    // ---------------------------------------------------------------- 5. real client smoke
    if real {
        rows.push(real_client(&out, &server_bin)?);
    }

    // ---------------------------------------------------------------- report
    match std::fs::read(&sjson)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
    {
        Some(Value::Array(a)) => rows.extend(a.iter().map(R::from_json)),
        _ => rows.push(R::new(
            "M1",
            "scenario runner",
            "basalt",
            false,
            scen_secs,
            "runner produced no results (see target/check/scenarios.log)",
        )),
    }
    rows.sort_by(|a, b| {
        (order_of(&a.group), &a.id, &a.server).cmp(&(order_of(&b.group), &b.id, &b.server))
    });
    let fails = rows.iter().filter(|r| !r.pass).count();
    let skips = rows.iter().filter(|r| r.skip).count();
    let w = rows
        .iter()
        .map(|r| r.id.chars().count())
        .max()
        .unwrap_or(10);
    println!(
        "\n{:11} {:w$} {:8} {:6} {:>7}  details",
        "group", "check", "server", "result", "time"
    );
    let mut group: Option<&str> = None;
    for r in &rows {
        if group.is_some() && group != Some(r.group.as_str()) {
            println!();
        }
        group = Some(&r.group);
        let det = if !r.pass {
            r.failures.join("; ")
        } else {
            r.notes.first().cloned().unwrap_or_default()
        };
        let res = if r.skip {
            "skip"
        } else if r.pass {
            "pass"
        } else {
            "FAIL"
        };
        println!(
            "{:11} {:w$} {:8} {:6} {:>6.1}s  {}",
            r.group,
            r.id,
            r.server,
            res,
            r.secs,
            clip(&det, 150)
        );
    }
    let mut groups: Vec<(String, u32, u32)> = Vec::new();
    for r in rows.iter().filter(|r| !r.skip) {
        match groups.iter_mut().find(|g| g.0 == r.group) {
            Some(g) => {
                g.1 += r.pass as u32;
                g.2 += 1;
            }
            None => groups.push((r.group.clone(), r.pass as u32, 1)),
        }
    }
    groups.sort_by_key(|g| order_of(&g.0));
    println!(
        "\nsummary: {}",
        groups
            .iter()
            .map(|(g, p, n)| format!("{g} {p}/{n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("scenario phase wall time {scen_secs:.0}s; {fails} failure(s), {skips} skipped");
    let results = json!({"pass": fails == 0, "failures": fails, "scenario_wall_secs": scen_secs, "results": rows.iter().map(R::to_json).collect::<Vec<_>>()});
    std::fs::write(
        out.join("results.json"),
        serde_json::to_string_pretty(&results)?,
    )?;
    Ok((fails > 0) as i32)
}

/// Launch the official client (via tools/run-vanilla-client.py, needs Python + an installed launcher
/// 26.2) against a fresh in-memory Basalt server and check that it joins.
fn real_client(out: &Path, server_bin: &Path) -> Result<R> {
    println!("== real client smoke");
    let t = Instant::now();
    let name = "vanilla 26.2 client joins Basalt";
    let port = util::free_port()?;
    let slog = out.join("realclient-server.log");
    let mut server = util::spawn_logged(
        Command::new(server_bin).args([
            "--memory",
            "--flat",
            "--port",
            &port.to_string(),
            "--no-spawning",
        ]),
        &slog,
    )?;
    util::wait_for_log(&slog, "listening", Duration::from_secs(20));
    let log = PathBuf::from("vanilla/mcclient/logs/latest.log");
    // the client's game dir is shared with other tools: wait until no other client is running
    #[cfg(unix)]
    for _ in 0..60 {
        let busy = Command::new("pgrep")
            .args(["-f", "net.minecraft.client.main.Main"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !busy {
            break;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    let _ = std::fs::remove_file(&log);
    let client = Guard(util::spawn_logged(
        Command::new(util::python()).args([
            "tools/run-vanilla-client.py",
            &format!("127.0.0.1:{port}"),
            "CheckReal",
        ]),
        &out.join("realclient.out"),
    )?);
    let joined = util::wait_for_log(
        &log,
        "[CHAT] CheckReal joined the game",
        Duration::from_secs(120),
    );
    std::thread::sleep(Duration::from_secs(8));
    let png = out.join("realclient.png");
    let _ = crate::window::screenshot(&[png.to_string_lossy().into_owned()]);
    let pats = [
        "Failed to decode",
        "Network Protocol Error",
        "Disconnected",
        "Error decoding",
        "Exception in thread",
        "lost connection",
    ];
    let errs: Vec<String> = read_lossy(&log)
        .lines()
        .filter(|l| {
            let low = l.to_lowercase();
            (pats.iter().any(|p| low.contains(&p.to_lowercase()))
                || (low.contains("packet") && low.contains("error")))
                && !l.contains("profile key pair")
        })
        .take(3)
        .map(String::from)
        .collect();
    util::kill_tree(client.id()); // only our own client (by pid)
    drop(client);
    util::stop_gracefully(&mut server, 15);
    let srv_errs: Vec<String> = read_lossy(&slog)
        .lines()
        .filter(|l| l.contains("bad packet") || l.contains("panicked"))
        .take(2)
        .map(String::from)
        .collect();
    let all = format!("{} {}", errs.join(" "), srv_errs.join(" "));
    Ok(if joined && all.trim().is_empty() {
        R::new(
            "Real client",
            name,
            "basalt",
            true,
            secs(t),
            &format!("joined; screenshot {}", png.display()),
        )
    } else {
        R::new(
            "Real client",
            name,
            "basalt",
            false,
            secs(t),
            &format!("joined={joined} {all}"),
        )
    })
}

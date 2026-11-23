//! `basalt-check client-anim`: entity-model and first-person animation parity of basalt-client vs
//! the real vanilla client.
//!
//! The oracle mod (tools/oracle-mod, SCHEMA.md control ops `bones` and `fp`) records, per client
//! tick, the vanilla client's own model part poses for an entity (here the local player: idle, walk,
//! sneak, attack swings) and, per rendered frame, the first-person hand/held-item transforms. Those
//! dumps are cached under `<oracle-dir>/anim/` (gitignored); `basalt-client --anim-eval` feeds each
//! frame's inputs (walk animation, head rotation, age, attack time, crouching, death time; attack
//! progress and equip height) into Basalt's own animation code, and this harness diffs every part
//! angle/pivot and every hand matrix. Any other `*.jsonl` bone dump placed in `<oracle-dir>/anim/`
//! (e.g. mob recordings made by hand on a vanilla server) is checked as well.
//!
//! usage: basalt-check client-anim [--record] [--oracle-dir vanilla/oracle] [--ctl-port 25789] [--no-launch]
//! Prints `RESULT client_anim files=.. frames=.. mismatches=.. known=.. new=..`. Swimming poses are a
//! known gap (counted under `known`, not failing).
use super::input_diff::{parse_args, start_server, use_port_range, Opts};
use super::oracle::{Ctl, Oracle};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Tolerance for part angles (rad), pivots (px) and hand-matrix entries.
const TOL: f64 = 2e-3;

fn wait(n: u32) -> Value {
    json!({"op": "wait", "ticks": n})
}

/// The recorded actions: (file stem, recorder op, script after recording started).
fn actions(dir: &Path) -> Vec<(String, Value, Vec<Value>)> {
    let f = |n: &str| {
        dir.join(format!("{n}.jsonl"))
            .to_string_lossy()
            .into_owned()
    };
    let bones =
        |n: &str| json!({"op": "bones", "type": "self", "file": f(n), "partials": [0.0, 0.5]});
    vec![
        ("self-idle".into(), bones("self-idle"), vec![wait(30)]),
        (
            "self-walk".into(),
            bones("self-walk"),
            vec![
                json!({"op": "move", "forward": true}),
                wait(30),
                json!({"op": "move", "forward": false}),
                wait(15),
            ],
        ),
        (
            "self-sneak".into(),
            bones("self-sneak"),
            vec![
                json!({"op": "sneak", "action": "press"}),
                wait(10),
                json!({"op": "move", "forward": true}),
                wait(20),
                json!({"op": "move", "forward": false}),
                json!({"op": "sneak", "action": "release"}),
                wait(10),
            ],
        ),
        (
            "self-swing".into(),
            bones("self-swing"),
            vec![
                wait(5),
                json!({"op": "attack"}),
                wait(10),
                json!({"op": "attack"}),
                wait(3),
                json!({"op": "attack"}),
                wait(12),
            ],
        ),
        (
            "fp-swing".into(),
            json!({"op": "fp", "file": f("fp-swing")}),
            vec![
                wait(5),
                json!({"op": "attack"}),
                wait(12),
                json!({"op": "attack"}),
                wait(3),
                json!({"op": "attack"}),
                wait(12),
            ],
        ),
    ]
}

fn stop(stem: &str) -> Value {
    if stem.starts_with("fp-") {
        json!({"op": "fp"})
    } else {
        json!({"op": "bones"})
    }
}

/// Record the missing (or, with `force`, all) actions on the real client against a fresh Basalt server.
fn record(o: &Opts, dir: &Path, force: bool) -> Result<usize> {
    let todo: Vec<_> = actions(dir)
        .into_iter()
        .filter(|(n, _, _)| force || !dir.join(format!("{n}.jsonl")).exists())
        .collect();
    if todo.is_empty() {
        return Ok(0);
    }
    let srv = start_server(&o.out, "client-anim")?;
    let _oracle = Oracle::connect_port(&o.oracle_dir, o.launch, o.ctl_port)?;
    let mut ctl = Ctl::connect(o.ctl_port, Duration::from_secs(20))?;
    ctl.call(
        json!({"op": "connect", "addr": format!("127.0.0.1:{}", srv.port)}),
        Duration::from_secs(10),
    )?;
    let mut ops = vec![
        json!({"op": "wait_join"}),
        wait(40),
        json!({"op": "command", "text": "gamemode survival"}),
        json!({"op": "look", "yaw": 0, "pitch": 0}),
        json!({"op": "hotbar", "slot": 0}),
        wait(20),
    ];
    for (n, start, script) in &todo {
        ops.push(start.clone());
        ops.extend(script.iter().cloned());
        ops.push(stop(n));
        ops.push(wait(5));
    }
    let res = ctl.run(&ops, Duration::from_secs(120))?;
    if let Some((i, r)) = res.iter().enumerate().find(|(_, r)| r["ok"] != json!(true)) {
        bail!(
            "vanilla client: command {i} {} failed: {}",
            ops[i],
            r["error"]
                .as_str()
                .unwrap_or("?")
                .lines()
                .next()
                .unwrap_or("")
        );
    }
    let _ = ctl.call(json!({"op": "disconnect"}), Duration::from_secs(10));
    drop(srv);
    Ok(todo.len())
}

fn eval(o: &Opts, dump: &Path, out: &Path) -> Result<Vec<Value>> {
    let st = Command::new(&o.client)
        .arg("--jar")
        .arg(&o.jar)
        .arg("--anim-eval")
        .arg(dump)
        .arg("--out")
        .arg(out)
        .env("RUST_LOG", "warn")
        .output()
        .context("running basalt-client --anim-eval")?;
    if !st.status.success() {
        bail!(
            "basalt-client --anim-eval {} failed: {}",
            dump.display(),
            String::from_utf8_lossy(&st.stderr)
                .lines()
                .last()
                .unwrap_or("")
        );
    }
    Ok(std::fs::read_to_string(out)?
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

fn ang(d: f64) -> f64 {
    let t = std::f64::consts::TAU;
    (d + t / 2.0).rem_euclid(t) - t / 2.0
}

#[derive(Default)]
struct Tally {
    frames: usize,
    bad: usize,
    known: usize,
    worst: Vec<String>,
}

impl Tally {
    fn miss(&mut self, known: bool, what: String) {
        if known {
            self.known += 1;
            return;
        }
        self.bad += 1;
        if self.worst.len() < 12 {
            self.worst.push(what);
        }
    }
}

fn compare(file: &str, v: &[Value], b: &[Value], t: &mut Tally) {
    let v: Vec<&Value> = v
        .iter()
        .filter(|l| l.get("frames").is_some() || matches!(l["kind"].as_str(), Some("arm" | "item")))
        .collect();
    for (lv, lb) in v.iter().zip(b) {
        if let Some(k) = lv["kind"].as_str() {
            // first-person: only frames without view bobbing / hand sway (Basalt's hand matrix excludes them)
            let f = |k: &str| lv[k].as_f64().unwrap_or(0.0);
            if f("bob").abs() > 1e-6
                || (f("xRot") - f("xBob")).abs() > 1e-3
                || (f("yRot") - f("yBob")).abs() > 1e-3
            {
                continue;
            }
            t.frames += 1;
            let (mv, mb) = (lv["m"].as_array(), lb["m"].as_array());
            let d = match (mv, mb) {
                (Some(a), Some(c)) => a
                    .iter()
                    .zip(c)
                    .map(|(x, y)| (x.as_f64().unwrap_or(0.0) - y.as_f64().unwrap_or(0.0)).abs())
                    .fold(0.0, f64::max),
                _ => 1.0,
            };
            if d > TOL {
                t.miss(
                    false,
                    format!(
                        "{file} tick {} {k} matrix off by {d:.4} (attack {:.3})",
                        lv["tick"],
                        f("attack")
                    ),
                );
            }
            continue;
        }
        let (Some(fv), Some(fb)) = (lv["frames"].as_array(), lb["frames"].as_array()) else {
            continue;
        };
        for (fv, fb) in fv.iter().zip(fb) {
            t.frames += 1;
            let swim = fv["state"]["swimAmount"].as_f64().unwrap_or(0.0) > 0.0;
            let Some(parts) = fv["parts"].as_object() else {
                continue;
            };
            for (name, pv) in parts {
                let pb = &fb["parts"][name];
                if pb.is_null() {
                    // vanilla child layers (hat, sleeves, beak, ...) are part of their parent's cubes in Basalt
                    if name.contains('/') {
                        continue;
                    }
                    t.miss(
                        swim,
                        format!("{file} tick {} part {name} missing in Basalt", lv["tick"]),
                    );
                    continue;
                }
                for (i, c) in ["x", "y", "z", "xRot", "yRot", "zRot"].iter().enumerate() {
                    let (a, b) = (pv[i].as_f64().unwrap_or(0.0), pb[i].as_f64().unwrap_or(0.0));
                    let d = if i >= 3 { ang(b - a) } else { b - a };
                    if d.abs() > TOL {
                        t.miss(
                            swim,
                            format!(
                                "{file} tick {} pt {} {name}.{c}: vanilla {a:.4} basalt {b:.4}",
                                lv["tick"], fv["pt"]
                            ),
                        );
                    }
                }
            }
            if let (Some(mv), Some(mb)) = (fv["rot_matrix"].as_array(), fb["rot_matrix"].as_array())
            {
                let d = [0, 1, 2, 4, 5, 6, 8, 9, 10]
                    .iter()
                    .map(|&i| (mv[i].as_f64().unwrap_or(0.0) - mb[i].as_f64().unwrap_or(0.0)).abs())
                    .fold(0.0, f64::max);
                if d > TOL {
                    t.miss(
                        swim,
                        format!(
                            "{file} tick {} body rotation off by {d:.4} (deathTime {})",
                            lv["tick"], fv["state"]["deathTime"]
                        ),
                    );
                }
            }
        }
    }
}

pub fn run(o: &Opts, force: bool) -> Result<bool> {
    use_port_range();
    let dir = o.oracle_dir.join("anim");
    std::fs::create_dir_all(&dir)?;
    let dir = std::fs::canonicalize(&dir)?;
    match record(o, &dir, force) {
        Ok(0) => eprintln!("client-anim: using cached dumps in {}", dir.display()),
        Ok(n) => eprintln!("client-anim: recorded {n} actions on the vanilla client"),
        Err(e) => bail!("recording on the vanilla client failed: {e:#}"),
    }
    let work = o.out.join("client-anim");
    std::fs::create_dir_all(&work)?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    files.sort();
    let mut t = Tally::default();
    for f in &files {
        let name = f
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let v: Vec<Value> = std::fs::read_to_string(f)?
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        let b = eval(o, f, &work.join(format!("{name}.basalt.jsonl")))?;
        let before = t.bad;
        compare(&name, &v, &b, &mut t);
        eprintln!("  {name:28} {} mismatches", t.bad - before);
    }
    for w in &t.worst {
        eprintln!("    {w}");
    }
    println!(
        "RESULT client_anim files={} frames={} mismatches={} known={} new={}",
        files.len(),
        t.frames,
        t.bad + t.known,
        t.known,
        t.bad
    );
    Ok(t.bad == 0 && !files.is_empty())
}

pub fn main(args: &[String]) -> i32 {
    let force = args.iter().any(|a| a == "--record");
    let rest: Vec<String> = args.iter().filter(|a| *a != "--record").cloned().collect();
    let o = match parse_args(&rest) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("client-anim: {e:#}");
            return 2;
        }
    };
    match run(&o, force) {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(e) => {
            eprintln!("client-anim: {e:#}");
            println!("RESULT client_anim error");
            1
        }
    }
}

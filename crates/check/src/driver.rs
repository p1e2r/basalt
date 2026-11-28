//! Extended command line: fuzz harnesses, harness groups, `--affected` selection, result cache.
//!
//! basalt-check --fuzz [ID[,ID..]|all] [--cases N] [--seed S] [--long] [--replay FILE] [--jobs N]
//! basalt-check --all | --affected [--base REF] [--group G] [--no-vanilla] [--no-cache] [--long]
//! basalt-check --list-affected [--base REF]     list harnesses whose declared paths changed
//! basalt-check --list-harnesses                 every registered harness with its declared paths
//! common: --vanilla HOST:PORT (default $VANILLA_ADDR or 127.0.0.1:25566) --vanilla-dir DIR --out DIR
//!         --update-known (record new fuzz mismatch classes in crates/check/known-divergences.txt)
//!
//! Without any of those flags the legacy scenario runner in main.rs handles the command line.
use crate::fuzz::{self, Opts, Rng};
use crate::harness::{self, Cache, Entry, HKind};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

const EXT_FLAGS: &[&str] = &[
    "--fuzz",
    "--all",
    "--affected",
    "--list-affected",
    "--list-harnesses",
    "--group",
    "--replay",
];

#[derive(Default)]
struct Args {
    fuzz: Option<Vec<String>>,
    affected: bool,
    list_affected: bool,
    list_harnesses: bool,
    group: Option<String>,
    base: Option<String>,
    no_cache: bool,
    no_vanilla: bool,
    update_known: bool,
    update_baseline: bool,
    filter: Vec<String>,
    opts: Opts,
    vanilla: Option<String>,
    vdir: String,
}

fn parse(argv: &[String]) -> Result<Args, String> {
    let mut a = Args {
        vdir: "vanilla/srv".into(),
        opts: Opts {
            seed: 1,
            jobs: 4,
            out: "target/check".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut i = 0;
    let val = |i: &mut usize| -> Result<String, String> {
        *i += 1;
        argv.get(*i)
            .cloned()
            .ok_or_else(|| format!("{} needs a value", argv[*i - 1]))
    };
    while i < argv.len() {
        match argv[i].as_str() {
            "--fuzz" => {
                let mut ids = Vec::new();
                while let Some(n) = argv.get(i + 1) {
                    if n.starts_with("--") {
                        break;
                    }
                    ids.extend(
                        n.split(',')
                            .filter(|s| !s.is_empty() && *s != "all")
                            .map(String::from),
                    );
                    i += 1;
                }
                a.fuzz = Some(ids);
            }
            "--affected" => a.affected = true,
            "--all" => {}
            "--list-affected" => a.list_affected = true,
            "--list-harnesses" => a.list_harnesses = true,
            "--group" => a.group = Some(val(&mut i)?),
            "--base" => a.base = Some(val(&mut i)?),
            "--no-cache" => a.no_cache = true,
            "--no-vanilla" => a.no_vanilla = true,
            "--update-known" => a.update_known = true,
            "--update-baseline" => a.update_baseline = true,
            "--long" => a.opts.long = true,
            "--cases" => a.opts.cases = val(&mut i)?.parse().map_err(|_| "bad --cases")?,
            "--seed" => a.opts.seed = val(&mut i)?.parse().map_err(|_| "bad --seed")?,
            "--jobs" => a.opts.jobs = val(&mut i)?.parse().map_err(|_| "bad --jobs")?,
            "--replay" => a.opts.replay = Some(val(&mut i)?.into()),
            "--out" => a.opts.out = val(&mut i)?.into(),
            "--vanilla" => a.vanilla = Some(val(&mut i)?),
            "--vanilla-dir" => a.vdir = val(&mut i)?,
            "--filter" => {
                a.filter = val(&mut i)?
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect()
            }
            "--diff" | "--basalt" => {}
            other => return Err(format!("unknown argument {other}")),
        }
        i += 1;
    }
    Ok(a)
}

fn reachable(addr: &str) -> bool {
    use std::net::ToSocketAddrs;
    addr.to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .map(|s| {
            std::net::TcpStream::connect_timeout(&s, std::time::Duration::from_millis(800)).is_ok()
        })
        .unwrap_or(false)
}

/// Seeds for the worldgen harness: the three long-cached ones first, then derived ones.
pub fn worldgen_seeds(n: usize, seed: u64) -> Vec<i64> {
    let mut v: Vec<i64> = vec![
        12345, 42, 1, 7, 2026, 99999, 31337, 5, 777, 123456789, 8675309, 4242, 1000003, 55, 600613,
        20260, 314159, 271828, 161803, 9001, 424242, 808, 65535, 2468,
    ];
    let mut r = Rng::new(seed ^ 0x5eed);
    while v.len() < n {
        v.push((r.next_u64() % 2_000_000_000) as i64);
    }
    v.truncate(n);
    v
}

/// Compare sites: (centre chunk x, z, radius), all inside the force-loaded -5..4 chunk square.
pub const WORLDGEN_SITES: &[(i32, i32, i32)] =
    &[(0, 0, 3), (-3, -3, 2), (2, 2, 2), (-3, 2, 2), (2, -3, 2)];

struct RunRow {
    id: String,
    kind: &'static str,
    result: &'static str,
    secs: f64,
    detail: String,
}

pub fn try_main() -> Option<i32> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if !argv.iter().any(|a| EXT_FLAGS.contains(&a.as_str())) {
        return None;
    }
    let mut a = match parse(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return Some(2);
        }
    };
    let entries = harness::all();
    if a.list_harnesses {
        for e in &entries {
            println!(
                "{:22} {:8} {:6} {}\n    paths: {}",
                e.id,
                format!("{:?}", e.kind),
                e.group,
                e.desc,
                e.paths.join(" ")
            );
        }
        return Some(0);
    }
    // --replay FILE implies the harness named by its directory
    if let Some(p) = &a.opts.replay {
        let h = p
            .parent()
            .and_then(|d| d.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if fuzz::find(&h).is_none() && h != "worldgen" {
            eprintln!("--replay: cannot tell the harness from {} (expected target/check/cases/<harness>/...)", p.display());
            return Some(2);
        }
        a.fuzz = Some(vec![h]);
        a.no_cache = true;
    }
    let base = a.base.clone().unwrap_or_else(harness::default_base);
    let changed = harness::changed_files(&base);
    if a.list_affected {
        println!("base {base}, {} changed file(s)", changed.len());
        for (e, hits) in harness::affected(&entries, &changed) {
            println!(
                "{:22} {:8} {:6} {}",
                e.id,
                format!("{:?}", e.kind),
                e.group,
                hits.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
                    + if hits.len() > 3 { ", ..." } else { "" }
            );
        }
        return Some(0);
    }
    // ---- selection
    let mut sel: Vec<&Entry> = entries.iter().collect();
    if let Some(ids) = &a.fuzz {
        sel.retain(|e| {
            e.id.starts_with("fuzz_")
                && (ids.is_empty()
                    || ids
                        .iter()
                        .any(|i| e.id.trim_start_matches("fuzz_") == i.trim_start_matches("fuzz_")))
        });
    }
    match &a.group {
        Some(g) => sel.retain(|e| e.group.eq_ignore_ascii_case(g)),
        None => sel.retain(|e| !e.group.eq_ignore_ascii_case("client")),
    } // client group only on request
    if !a.filter.is_empty() {
        sel.retain(|e| a.filter.iter().any(|f| e.id.contains(f.as_str())));
    }
    if a.affected {
        let hit: std::collections::HashSet<String> = harness::affected(&entries, &changed)
            .into_iter()
            .map(|(e, _)| e.id.clone())
            .collect();
        sel.retain(|e| hit.contains(&e.id));
    }
    if sel.is_empty() {
        match &a.group {
            Some(g) if !entries.iter().any(|e| e.group.eq_ignore_ascii_case(g)) => {
                eprintln!("note: no harnesses are registered for group `{g}` yet; nothing to run")
            }
            _ => eprintln!("no harness selected (nothing affected since {base}, or no match)"),
        }
        println!("RESULT run harnesses=0 failed=0");
        return Some(0);
    }
    let vaddr = a
        .vanilla
        .clone()
        .or_else(|| std::env::var("VANILLA_ADDR").ok())
        .unwrap_or("127.0.0.1:25566".into());
    let vanilla = if a.no_vanilla {
        None
    } else if reachable(&vaddr) {
        Some(vaddr.clone())
    } else {
        eprintln!(
            "note: vanilla server {vaddr} is not reachable: differential harnesses will be skipped"
        );
        None
    };
    let out = a.opts.out.clone();
    let _ = std::fs::create_dir_all(&out);
    let files = harness::tracked_files();
    let mut cache = Cache::load(&out);
    let mut rows: Vec<RunRow> = Vec::new();
    let t_all = Instant::now();

    // ---- cache lookup
    let seed = a.opts.seed;
    let key_of = |e: &Entry, a: &Args| -> String {
        let (s, n) = match e.kind {
            HKind::Fuzz => (
                seed,
                fuzz::find(&e.id)
                    .map(|h| fuzz::case_count(h, &a.opts))
                    .unwrap_or(0),
            ),
            HKind::Script if e.id == "fuzz_worldgen" => (seed, worldgen_count(a)),
            _ => (0, 0),
        };
        Cache::key(
            e,
            s,
            n,
            &harness::content_hash(e, &files),
            vanilla.is_some(),
        )
    };
    let mut todo: Vec<&Entry> = Vec::new();
    for e in &sel {
        let k = key_of(e, &a);
        if !a.no_cache && a.opts.replay.is_none() {
            if let Some(h) = cache.hit(&k) {
                rows.push(RunRow {
                    id: e.id.clone(),
                    kind: kind_name(e),
                    result: "cached",
                    secs: 0.0,
                    detail: format!(
                        "passed before for identical inputs/code ({})",
                        h["summary"].as_str().unwrap_or("")
                    ),
                });
                continue;
            }
        }
        todo.push(e);
    }

    // ---- scenarios (one legacy-runner invocation)
    let scen: Vec<&&Entry> = todo.iter().filter(|e| e.kind == HKind::Scenario).collect();
    if !scen.is_empty() {
        let json_path = out.join("affected-scenarios.json");
        let _ = std::fs::remove_file(&json_path);
        let ids: Vec<String> = scen.iter().map(|e| e.id.clone()).collect();
        let exe = std::env::current_exe().unwrap_or_else(|_| {
            format!(
                "target/release/basalt-check{}",
                std::env::consts::EXE_SUFFIX
            )
            .into()
        });
        let mut c = Command::new(exe);
        c.arg("--filter")
            .arg(ids.join(","))
            .arg("--jobs")
            .arg(a.opts.jobs.to_string())
            .arg("--json")
            .arg(&json_path)
            .arg("--out")
            .arg(&out);
        if let Some(v) = &vanilla {
            c.arg("--diff")
                .arg("--vanilla")
                .arg(v)
                .arg("--vanilla-dir")
                .arg(&a.vdir);
        }
        eprintln!(
            "== {} scenario(s){}",
            ids.len(),
            if vanilla.is_some() {
                " vs Basalt + differential vs vanilla"
            } else {
                " vs Basalt"
            }
        );
        let t = Instant::now();
        let _ = c.status();
        let secs = t.elapsed().as_secs_f64();
        let res: Vec<Value> = std::fs::read_to_string(&json_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        for e in &scen {
            let mine: Vec<&Value> = res
                .iter()
                .filter(|r| r["id"] == json!(e.id) || r["id"] == json!(format!("diff:{}", e.id)))
                .collect();
            // a vanilla-only scenario has nothing to run without a vanilla server
            let vanilla_only = crate::scenarios::ALL
                .iter()
                .any(|sp| sp.id == e.id && sp.servers == crate::runner::Servers::VanillaOnly);
            if mine.is_empty() && vanilla.is_none() && vanilla_only {
                rows.push(RunRow {
                    id: e.id.clone(),
                    kind: "scenario",
                    result: "SKIP",
                    secs: 0.0,
                    detail: "vanilla-only scenario, no vanilla server".into(),
                });
                continue;
            }
            let pass = !mine.is_empty() && mine.iter().all(|r| r["pass"] == json!(true));
            let detail = if mine.is_empty() {
                "no result (runner failed?)".into()
            } else if pass {
                format!("{} result row(s)", mine.len())
            } else {
                mine.iter()
                    .filter(|r| r["pass"] != json!(true))
                    .filter_map(|r| {
                        r["failures"]
                            .get(0)
                            .and_then(|f| f.as_str())
                            .map(String::from)
                    })
                    .next()
                    .unwrap_or_default()
            };
            if pass {
                cache.store_pass(&key_of(e, &a), &detail);
            }
            rows.push(RunRow {
                id: e.id.clone(),
                kind: "scenario",
                result: if pass { "pass" } else { "FAIL" },
                secs: secs / scen.len() as f64,
                detail,
            });
        }
    }

    // ---- fuzz harnesses (in process)
    let fz: Vec<&&Entry> = todo.iter().filter(|e| e.kind == HKind::Fuzz).collect();
    if !fz.is_empty() {
        let hs: Vec<&'static fuzz::Harness> = fz.iter().filter_map(|e| fuzz::find(&e.id)).collect();
        eprintln!(
            "== fuzz: {} (seed {}, {})",
            hs.iter().map(|h| h.id).collect::<Vec<_>>().join(", "),
            a.opts.seed,
            if a.opts.long { "long" } else { "default size" }
        );
        let t = Instant::now();
        let reports = fuzz::run_harnesses(
            &hs,
            a.opts.clone(),
            vanilla.clone().map(|v| (a.vdir.clone().into(), v)),
            &out,
        );
        let _ = t;
        let mut new_known: Vec<String> = Vec::new();
        for r in &reports {
            println!("\n{}", r.text.trim_end());
            println!("{}", r.result_line());
            let e = sel
                .iter()
                .find(|e| e.id == format!("fuzz_{}", r.id))
                .unwrap();
            let result = if r.skipped.is_some() {
                "SKIP"
            } else if r.pass() {
                "pass"
            } else {
                "FAIL"
            };
            let detail = match &r.skipped {
                Some(s) => s.clone(),
                None => format!(
                    "cases={} mismatches={} (new {}) info={} errors={}",
                    r.cases, r.mismatches, r.new, r.info, r.errors
                ),
            };
            if r.pass() && a.opts.replay.is_none() {
                cache.store_pass(&key_of(e, &a), &detail);
            }
            for c in &r.new_classes {
                new_known.push(format!("{} {}", r.id, c));
            }
            rows.push(RunRow {
                id: e.id.clone(),
                kind: "fuzz",
                result,
                secs: r.secs,
                detail,
            });
        }
        if a.update_known && !new_known.is_empty() {
            let p = std::env::var("CHECK_KNOWN")
                .unwrap_or_else(|_| "crates/check/known-divergences.txt".into());
            let mut s = std::fs::read_to_string(&p).unwrap_or_default();
            if !s.is_empty() && !s.ends_with('\n') {
                s.push('\n');
            }
            for k in &new_known {
                s += &format!("{k}\n");
            }
            let _ = std::fs::write(&p, s);
            eprintln!("recorded {} new class(es) in {p}", new_known.len());
        }
    }

    // ---- client group: pick a free oracle control port when the default is taken by another oracle client
    if todo.iter().any(|e| e.id.starts_with("client_"))
        && std::env::var_os("ORACLE_CTL_PORT").is_none()
        && std::net::TcpListener::bind(("127.0.0.1", crate::client::oracle::DEFAULT_CTL_PORT))
            .is_err()
    {
        if let Some(p) =
            (25760..25780).find(|p| std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok())
        {
            std::env::set_var("ORACLE_CTL_PORT", p.to_string());
        }
    }
    // ---- client UI scenarios: one basalt-client process for every selected scenario, one row each
    let ui: Vec<&&Entry> = todo
        .iter()
        .filter(|e| e.id.starts_with("client_ui_"))
        .collect();
    if !ui.is_empty() {
        let t = Instant::now();
        let names: Vec<String> = ui
            .iter()
            .map(|e| e.id.trim_start_matches("client_ui_").to_string())
            .collect();
        let exe = std::env::current_exe().unwrap_or_else(|_| {
            format!(
                "target/release/basalt-check{}",
                std::env::consts::EXE_SUFFIX
            )
            .into()
        });
        eprintln!(
            "== client UI scenarios ({}) against a fresh Basalt server",
            names.len()
        );
        let mut c = Command::new(exe);
        c.arg("client-ui")
            .arg("--names")
            .arg(names.join(","))
            .arg("--out")
            .arg(&out);
        if a.no_vanilla {
            c.arg("--no-vanilla");
        }
        let o = c.output();
        let txt = o
            .map(|o| {
                String::from_utf8_lossy(&o.stdout).into_owned()
                    + &String::from_utf8_lossy(&o.stderr)
            })
            .unwrap_or_default();
        let secs = t.elapsed().as_secs_f64();
        let skipped_all = txt.lines().find(|l| {
            l.starts_with("RESULT client_ui") && l.contains("skipped=1") && l.contains("reason=")
        });
        for e in &ui {
            let n = e.id.trim_start_matches("client_ui_");
            let line = txt
                .lines()
                .find(|l| l.starts_with("SCENARIO") && l.contains(&format!("M7/{n} ")));
            let (result, detail): (&'static str, String) = match (line, skipped_all) {
                (_, Some(l)) => (
                    "SKIP",
                    l.split("reason=").nth(1).unwrap_or("").replace('_', " "),
                ),
                (Some(l), _) => {
                    let mut it = l.splitn(4, ' ');
                    let (_, _, verdict, rest) = (
                        it.next(),
                        it.next(),
                        it.next().unwrap_or(""),
                        it.next().unwrap_or(""),
                    );
                    (
                        match verdict {
                            "PASS" => "pass",
                            "SKIP" => "SKIP",
                            _ => "FAIL",
                        },
                        rest.to_string(),
                    )
                }
                (None, None) => (
                    "FAIL",
                    format!(
                        "no result line ({})",
                        txt.lines()
                            .rev()
                            .find(|l| !l.trim().is_empty())
                            .unwrap_or("")
                    ),
                ),
            };
            if result == "pass" {
                cache.store_pass(&key_of(e, &a), &detail);
            }
            rows.push(RunRow {
                id: e.id.clone(),
                kind: "script",
                result,
                secs: secs / ui.len() as f64,
                detail,
            });
        }
    }

    // ---- scripts
    for e in todo
        .iter()
        .filter(|e| e.kind == HKind::Script && !e.id.starts_with("client_ui_"))
    {
        let t = Instant::now();
        if e.id == "fuzz_worldgen" || e.id == "worldgen_nether" || e.id == "worldgen_end" {
            let nether = e.id == "worldgen_nether";
            let end = e.id == "worldgen_end";
            let (seeds, path) = if nether {
                (
                    vec![12345i64, 42],
                    std::path::PathBuf::from("vanilla/parity-nether"),
                )
            } else if end {
                (
                    vec![12345i64, 42],
                    std::path::PathBuf::from("vanilla/parity-end"),
                )
            } else {
                worldgen_cases(&a)
            };
            let mut c = Command::new("bash");
            c.arg("tools/parity.sh").arg("--out").arg(&out);
            if nether {
                c.arg("--nether");
            }
            if end {
                c.arg("--end");
            }
            for s in &seeds {
                c.arg(s.to_string());
            }
            eprintln!(
                "== worldgen parity: {} seed(s) x {} site(s), cases {}",
                seeds.len(),
                WORLDGEN_SITES.len(),
                path.display()
            );
            let o = c.output();
            let (txt, ok) = match o {
                Ok(o) => (
                    String::from_utf8_lossy(&o.stdout).into_owned()
                        + &String::from_utf8_lossy(&o.stderr),
                    true,
                ),
                Err(e) => (e.to_string(), false),
            };
            for l in txt.lines().filter(|l| {
                l.starts_with("PARITY") || l.starts_with("RESULT") || l.starts_with("note")
            }) {
                println!("{l}");
            }
            let res = txt
                .lines()
                .rev()
                .find(|l| l.starts_with("RESULT fuzz=worldgen"))
                .unwrap_or("")
                .to_string();
            let kv: std::collections::HashMap<&str, &str> = res
                .split_whitespace()
                .skip(1)
                .filter_map(|p| p.split_once('='))
                .collect();
            let skipped = kv.get("skipped") == Some(&"1");
            let pass = ok && !res.is_empty() && kv.get("new") == Some(&"0") && !skipped;
            let result = if skipped {
                "SKIP"
            } else if pass {
                "pass"
            } else {
                "FAIL"
            };
            let detail = if res.is_empty() {
                "parity script produced no RESULT line".into()
            } else {
                res.trim_start_matches("RESULT ").to_string()
            };
            if pass && a.opts.replay.is_none() {
                cache.store_pass(&key_of(e, &a), &detail);
            }
            rows.push(RunRow {
                id: e.id.clone(),
                kind: "script",
                result,
                secs: t.elapsed().as_secs_f64(),
                detail,
            });
        } else if e.id == "worldgen_structures" {
            eprintln!("== structure parity (tools/structure-parity.sh)");
            let o = Command::new("bash")
                .arg("tools/structure-parity.sh")
                .arg("--out")
                .arg(&out)
                .output();
            let (txt, ok) = match o {
                Ok(o) => (
                    String::from_utf8_lossy(&o.stdout).into_owned()
                        + &String::from_utf8_lossy(&o.stderr),
                    true,
                ),
                Err(e) => (e.to_string(), false),
            };
            for l in txt.lines().filter(|l| {
                l.starts_with("PARITY") || l.starts_with("RESULT") || l.starts_with("note")
            }) {
                println!("{l}");
            }
            let res = txt
                .lines()
                .rev()
                .find(|l| l.starts_with("RESULT fuzz=structures"))
                .unwrap_or("")
                .to_string();
            let kv: std::collections::HashMap<&str, &str> = res
                .split_whitespace()
                .skip(1)
                .filter_map(|p| p.split_once('='))
                .collect();
            let pass = ok
                && !res.is_empty()
                && kv.get("new") == Some(&"0")
                && kv.get("errors") == Some(&"0");
            let detail = if res.is_empty() {
                "structure parity script produced no RESULT line".into()
            } else {
                res.trim_start_matches("RESULT ").to_string()
            };
            if pass && a.opts.replay.is_none() {
                cache.store_pass(&key_of(e, &a), &detail);
            }
            rows.push(RunRow {
                id: e.id.clone(),
                kind: "script",
                result: if pass { "pass" } else { "FAIL" },
                secs: t.elapsed().as_secs_f64(),
                detail,
            });
        } else if e.id == "client_mesh" {
            if let Some(why) =
                crate::client::oracle::unavailable(std::path::Path::new("vanilla/oracle"))
            {
                rows.push(RunRow {
                    id: e.id.clone(),
                    kind: "script",
                    result: "SKIP",
                    secs: 0.0,
                    detail: why,
                });
                continue;
            }
            let exe = std::env::current_exe().unwrap_or_else(|_| {
                format!(
                    "target/release/basalt-check{}",
                    std::env::consts::EXE_SUFFIX
                )
                .into()
            });
            eprintln!("== client-mesh (oracle block-meshing parity)");
            let o = Command::new(exe)
                .arg("client-mesh")
                .arg("--out")
                .arg(&out)
                .output();
            let txt = o
                .map(|o| {
                    String::from_utf8_lossy(&o.stdout).into_owned()
                        + &String::from_utf8_lossy(&o.stderr)
                })
                .unwrap_or_default();
            let res = txt
                .lines()
                .rev()
                .find(|l| l.starts_with("RESULT client_mesh"))
                .unwrap_or("")
                .to_string();
            let kv: std::collections::HashMap<&str, &str> = res
                .split_whitespace()
                .skip(2)
                .filter_map(|p| p.split_once('='))
                .collect();
            // a regression gate: no more mismatching quads than the stored baseline (known gaps in CLAUDE.md)
            let base: u64 = std::fs::read_to_string("tools/baselines/client-mesh.txt")
                .ok()
                .and_then(|t| t.trim().parse().ok())
                .unwrap_or(0);
            let pass = !res.is_empty()
                && kv
                    .get("mismatches")
                    .and_then(|m| m.parse::<u64>().ok())
                    .map(|m| m <= base)
                    .unwrap_or(false);
            let detail = if res.is_empty() {
                format!(
                    "client-mesh produced no RESULT line: {}",
                    txt.lines()
                        .rev()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("")
                )
            } else {
                res.trim_start_matches("RESULT ").to_string()
            };
            if pass && a.opts.replay.is_none() {
                cache.store_pass(&key_of(e, &a), &detail);
            }
            rows.push(RunRow {
                id: e.id.clone(),
                kind: "script",
                result: if pass { "pass" } else { "FAIL" },
                secs: t.elapsed().as_secs_f64(),
                detail,
            });
        } else if matches!(
            e.id.as_str(),
            "client_input"
                | "client_dig"
                | "client_water"
                | "client_anim"
                | "client_clicks"
                | "client_visual"
                | "client_assets"
                | "client_perf"
        ) {
            // real-client parity (oracle control channel + a display) or basalt-client-only checks (GPU); each prints a RESULT line
            let exe = std::env::current_exe().unwrap_or_else(|_| {
                format!(
                    "target/release/basalt-check{}",
                    std::env::consts::EXE_SUFFIX
                )
                .into()
            });
            let sub = e.id.replace('_', "-");
            let needs_oracle = matches!(
                e.id.as_str(),
                "client_input" | "client_dig" | "client_water" | "client_clicks" | "client_visual"
            );
            if needs_oracle {
                if let Some(why) =
                    crate::client::oracle::unavailable(std::path::Path::new("vanilla/oracle"))
                {
                    rows.push(RunRow {
                        id: e.id.clone(),
                        kind: "script",
                        result: "SKIP",
                        secs: 0.0,
                        detail: why,
                    });
                    continue;
                }
            }
            eprintln!(
                "== {sub}{}",
                if needs_oracle {
                    " (real vanilla client via the oracle)"
                } else {
                    ""
                }
            );
            let mut c = Command::new(exe);
            c.arg(&sub).arg("--out").arg(&out);
            if a.update_baseline && matches!(e.id.as_str(), "client_visual" | "client_perf") {
                c.arg("--update-baseline");
            }
            if e.id == "client_assets" && a.no_vanilla {
                c.arg("--no-vanilla");
            }
            let o = c.output();
            let txt = o
                .map(|o| {
                    String::from_utf8_lossy(&o.stdout).into_owned()
                        + &String::from_utf8_lossy(&o.stderr)
                })
                .unwrap_or_default();
            for l in txt.lines().filter(|l| {
                l.starts_with("SCENE ")
                    || l.starts_with("PERF ")
                    || l.starts_with("SKIP ")
                    || l.contains("missing ")
            }) {
                println!("{l}");
            }
            let res = txt
                .lines()
                .rev()
                .find(|l| l.starts_with(&format!("RESULT {}", e.id)))
                .unwrap_or("")
                .to_string();
            let has = |kv: &str| res.split_whitespace().any(|p| p == kv);
            let skipped = has("skipped=1");
            let pass = !res.is_empty()
                && !skipped
                && (has("new=0") || has("failed=0") || has("prediction_overridden_by_server=0"));
            let detail = if res.is_empty() {
                format!(
                    "{sub} produced no RESULT line: {}",
                    txt.lines()
                        .rev()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("")
                )
            } else if skipped {
                res.split("reason=").nth(1).unwrap_or("").replace('_', " ")
            } else {
                res.trim_start_matches("RESULT ").to_string()
            };
            if pass && a.opts.replay.is_none() {
                cache.store_pass(&key_of(e, &a), &detail);
            }
            rows.push(RunRow {
                id: e.id.clone(),
                kind: "script",
                result: if skipped {
                    "SKIP"
                } else if pass {
                    "pass"
                } else {
                    "FAIL"
                },
                secs: t.elapsed().as_secs_f64(),
                detail,
            });
        } else {
            rows.push(RunRow {
                id: e.id.clone(),
                kind: "script",
                result: "SKIP",
                secs: 0.0,
                detail: "no runner registered for this script harness here".into(),
            });
        }
    }
    cache.save();

    // ---- table
    rows.sort_by(|x, y| x.id.cmp(&y.id));
    println!(
        "\n{:24} {:9} {:7} {:>7}  details",
        "harness", "kind", "result", "time"
    );
    let mut failed = 0;
    for r in &rows {
        if r.result == "FAIL" {
            failed += 1;
        }
        println!(
            "{:24} {:9} {:7} {:>6.1}s  {}",
            r.id,
            r.kind,
            r.result,
            r.secs,
            r.detail.chars().take(150).collect::<String>()
        );
    }
    let count = |k: &str| rows.iter().filter(|r| r.result == k).count();
    println!(
        "\n{} harness(es): {} pass, {} cached, {} skipped, {} failed in {:.0}s (base {base})",
        rows.len(),
        count("pass"),
        count("cached"),
        count("SKIP"),
        failed,
        t_all.elapsed().as_secs_f64()
    );
    println!(
        "RESULT run harnesses={} failed={} cached={} skipped={}",
        rows.len(),
        failed,
        count("cached"),
        count("SKIP")
    );
    let j: Vec<Value> = rows.iter().map(|r| json!({"id": r.id, "kind": r.kind, "result": r.result, "secs": r.secs, "detail": r.detail})).collect();
    let _ = std::fs::write(
        out.join("affected-results.json"),
        serde_json::to_string_pretty(&j).unwrap_or_default(),
    );
    Some(if failed > 0 { 1 } else { 0 })
}

fn kind_name(e: &Entry) -> &'static str {
    match e.kind {
        HKind::Scenario => "scenario",
        HKind::Fuzz => "fuzz",
        HKind::Script => "script",
    }
}

fn worldgen_count(a: &Args) -> usize {
    if a.opts.cases > 0 {
        a.opts.cases
    } else if a.opts.long {
        24
    } else {
        6
    }
}

/// Seeds to compare (replayed or generated) and the recorded cases file.
fn worldgen_cases(a: &Args) -> (Vec<i64>, PathBuf) {
    if let Some(p) = &a.opts.replay {
        let seeds = fuzz::read_cases(p)
            .unwrap_or_default()
            .iter()
            .filter_map(|c| c["seed"].as_i64())
            .collect();
        return (seeds, p.clone());
    }
    let n = worldgen_count(a);
    let seeds = worldgen_seeds(n, a.opts.seed);
    let cases: Vec<Value> = seeds.iter().map(|s| json!({"seed": s, "sites": WORLDGEN_SITES.iter().map(|x| json!([x.0, x.1, x.2])).collect::<Vec<_>>()})).collect();
    let path = a
        .opts
        .out
        .join("cases")
        .join("worldgen")
        .join(format!("{}-{}.jsonl", a.opts.seed, n));
    fuzz::write_cases(&path, &cases);
    (seeds, path)
}

//! Port of tools/pr_ready.sh: fmt gate (new files only), clippy (new warnings vs baseline), unit tests,
//! affected harnesses. Same flags, same baselines in tools/baselines.
//!
//! usage: cargo xtask pr-ready [--client] [--all] [--no-vanilla] [--update-baselines] [--base REF]
//!   --client           also run the client harness group (`basalt-check --group client --affected`)
//!   --all              run every harness instead of only those affected by the diff against --base
//!   --no-vanilla       don't use the vanilla test server (differential/fuzz harnesses are skipped)
//!   --update-baselines rewrite tools/baselines/* from the current tree (only to ACCEPT current warnings /
//!                      unformatted files; reviewers should see the diff)
//!   --base REF         base for "affected" (default: merge-base with main)
//! Gates:
//!   fmt      `cargo fmt --check`: the tree predates rustfmt, so only files NOT listed in
//!            tools/baselines/fmt-dirty.txt must be clean (new files are not exempt).
//!   clippy   `cargo clippy --workspace --all-targets`: warnings AND errors are compared with
//!            tools/baselines/clippy.txt (counted per file + message); only NEW ones fail.
//!   tests    `cargo test --workspace`
//!   harness  `basalt-check --affected` (results cached in target/check/cache.json, see
//!            crates/check/src/harness.rs)
use crate::util::{self, clip, read_lossy, run_logged, secs};
use anyhow::{bail, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;
use std::time::Instant;

struct Row {
    name: String,
    result: &'static str,
    secs: f64,
    detail: String,
}

/// `Diff in /abs/path/file.rs:12:` -> `path/file.rs` relative to the repo root, with `/` separators.
fn fmt_dirty_files(log: &str) -> BTreeSet<String> {
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    let mut out = BTreeSet::new();
    for l in log.lines() {
        let Some(rest) = l.strip_prefix("Diff in ") else {
            continue;
        };
        let mut p = rest.trim_end_matches(':').to_string();
        if let Some((head, tail)) = p.rsplit_once(':') {
            if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
                p = head.to_string();
            }
        }
        let p = p.replace('\\', "/");
        let p = p.strip_prefix(&format!("{cwd}/")).unwrap_or(&p).to_string();
        out.insert(p);
    }
    out
}

/// (path, kind, message) -> count, from `cargo clippy --message-format=short` output.
fn clippy_counts(log: &str) -> BTreeMap<(String, String, String), u32> {
    let mut seen = BTreeSet::new();
    let mut cnt = BTreeMap::new();
    for l in log.lines() {
        let l = l.trim_end();
        let mut it = l.splitn(4, ':');
        let (Some(path), Some(line), Some(col), Some(rest)) =
            (it.next(), it.next(), it.next(), it.next())
        else {
            continue;
        };
        if line.is_empty()
            || !line.chars().all(|c| c.is_ascii_digit())
            || col.is_empty()
            || !col.chars().all(|c| c.is_ascii_digit())
        {
            continue;
        }
        let rest = rest.trim_start();
        let kind = if rest.starts_with("warning") {
            "warning"
        } else if rest.starts_with("error") {
            "error"
        } else {
            continue;
        };
        let mut after = &rest[kind.len()..];
        if after.starts_with('[') {
            let Some(end) = after.find(']') else { continue };
            after = &after[end + 1..];
        }
        let Some(msg) = after.strip_prefix(": ") else {
            continue;
        };
        let path = path.replace('\\', "/");
        if !seen.insert((
            path.clone(),
            line.to_string(),
            col.to_string(),
            msg.to_string(),
        )) {
            continue; // lib + lib test report the same warning twice
        }
        let msg = clip(&msg.split_whitespace().collect::<Vec<_>>().join(" "), 160);
        *cnt.entry((path, kind.to_string(), msg)).or_insert(0) += 1;
    }
    cnt
}

fn write_counts(p: &Path, c: &BTreeMap<(String, String, String), u32>) -> Result<()> {
    let mut s = String::new();
    for ((path, kind, msg), n) in c {
        s += &format!("{n}\t{path}\t{kind}\t{msg}\n");
    }
    std::fs::write(p, s)?;
    Ok(())
}

fn load_counts(p: &Path) -> BTreeMap<(String, String, String), u32> {
    let mut d = BTreeMap::new();
    for l in read_lossy(p).lines() {
        let f: Vec<&str> = l.splitn(4, '\t').collect();
        if f.len() == 4 {
            d.insert(
                (f[1].to_string(), f[2].to_string(), f[3].to_string()),
                f[0].parse().unwrap_or(0),
            );
        }
    }
    d
}

fn is_harness_summary(l: &str) -> bool {
    let digits = l.chars().take_while(|c| c.is_ascii_digit()).count();
    digits > 0 && l[digits..].starts_with(" harness")
}

pub fn run(args: &[String]) -> Result<i32> {
    let (mut client, mut all, mut vanilla, mut update) = (false, false, true, false);
    let mut base: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--client" => client = true,
            "--all" => all = true,
            "--no-vanilla" => vanilla = false,
            "--update-baselines" => update = true,
            "--base" => base = Some(it.next().cloned().unwrap_or_default()),
            "-h" | "--help" => {
                println!("usage: cargo xtask pr-ready [--client] [--all] [--no-vanilla] [--update-baselines] [--base REF]\nSee the header of xtask/src/pr_ready.rs and CLAUDE.md for the gates.");
                return Ok(0);
            }
            other => bail!("unknown option {other}"),
        }
    }
    let out = Path::new("target/check");
    let baselines = Path::new("tools/baselines");
    std::fs::create_dir_all(out)?;
    std::fs::create_dir_all(baselines)?;
    let mut rows: Vec<Row> = Vec::new();
    let mut fail = false;
    let row =
        |rows: &mut Vec<Row>, name: &str, result: &'static str, t: Instant, detail: String| {
            rows.push(Row {
                name: name.into(),
                result,
                secs: secs(t),
                detail,
            })
        };

    // ---------------------------------------------------------------- fmt
    let t = Instant::now();
    let fmt_log = out.join("fmt.log");
    run_logged(util::cargo().args(["fmt", "--check"]), &fmt_log)?;
    let dirty = fmt_dirty_files(&read_lossy(&fmt_log));
    let fmt_base = baselines.join("fmt-dirty.txt");
    if update {
        std::fs::write(
            &fmt_base,
            dirty.iter().map(|s| format!("{s}\n")).collect::<String>(),
        )?;
    }
    if !fmt_base.exists() {
        std::fs::write(&fmt_base, "")?;
    }
    let base_set: BTreeSet<String> = read_lossy(&fmt_base)
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();
    let new_dirty: Vec<&String> = dirty.iter().filter(|f| !base_set.contains(*f)).collect();
    if new_dirty.is_empty() {
        row(
            &mut rows,
            "cargo fmt --check",
            "pass",
            t,
            format!(
                "no new unformatted files ({} baseline files predate rustfmt)",
                base_set.len()
            ),
        );
    } else {
        let list = new_dirty
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        row(
            &mut rows,
            "cargo fmt --check",
            "FAIL",
            t,
            format!("unformatted (run cargo fmt on them): {}", clip(&list, 200)),
        );
        fail = true;
    }

    // ---------------------------------------------------------------- clippy
    let t = Instant::now();
    let clippy_log = out.join("clippy.log");
    run_logged(
        util::cargo().args([
            "clippy",
            "--workspace",
            "--all-targets",
            "--message-format=short",
        ]),
        &clippy_log,
    )?;
    let now = clippy_counts(&read_lossy(&clippy_log));
    let clippy_now = out.join("clippy.now");
    write_counts(&clippy_now, &now)?;
    let clippy_base = baselines.join("clippy.txt");
    if update {
        std::fs::copy(&clippy_now, &clippy_base)?;
    }
    if !clippy_base.exists() {
        std::fs::write(&clippy_base, "")?;
    }
    let base_c = load_counts(&clippy_base);
    let new: Vec<(&(String, String, String), u32)> = now
        .iter()
        .filter(|(k, n)| **n > *base_c.get(*k).unwrap_or(&0))
        .map(|(k, n)| (k, n - base_c.get(k).unwrap_or(&0)))
        .collect();
    let (tb, tn): (u32, u32) = (base_c.values().sum(), now.values().sum());
    if new.is_empty() {
        row(
            &mut rows,
            "cargo clippy (new warnings)",
            "pass",
            t,
            format!("baseline {tb} warnings ({tn} now)"),
        );
    } else {
        let sample = new
            .iter()
            .take(3)
            .map(|(k, _)| format!("{}: {}", k.0, clip(&k.2, 60)))
            .collect::<Vec<_>>()
            .join("; ");
        row(
            &mut rows,
            "cargo clippy (new warnings)",
            "FAIL",
            t,
            format!(
                "{} new (baseline {tb}, now {tn}): {sample}",
                new.iter().map(|(_, n)| n).sum::<u32>()
            ),
        );
        fail = true;
    }

    // ---------------------------------------------------------------- unit tests
    let t = Instant::now();
    let test_log = out.join("cargo-test.log");
    let st = run_logged(util::cargo().args(["test", "--workspace", "-q"]), &test_log)?;
    let tl = read_lossy(&test_log);
    if st.success() {
        let n: u64 = tl
            .lines()
            .filter(|l| l.starts_with("test result"))
            .filter_map(|l| l.split_whitespace().nth(3)?.parse::<u64>().ok())
            .sum();
        row(
            &mut rows,
            "cargo test --workspace",
            "pass",
            t,
            format!("{n} tests passed"),
        );
    } else {
        let d: Vec<&str> = tl
            .lines()
            .filter(|l| l.contains("FAILED") || l.contains("panicked") || l.contains("error"))
            .take(3)
            .collect();
        row(
            &mut rows,
            "cargo test --workspace",
            "FAIL",
            t,
            clip(&d.join(" "), 200),
        );
        fail = true;
    }

    // ---------------------------------------------------------------- harnesses
    let t = Instant::now();
    let build_log = out.join("build.log");
    let ok1 = run_logged(
        util::cargo().args([
            "build",
            "--release",
            "-p",
            "basalt-server",
            "-p",
            "basalt-check",
            "-q",
        ]),
        &build_log,
    )?
    .success();
    let ok2 = ok1 && {
        let st = util::cargo()
            .args([
                "build",
                "--release",
                "-p",
                "basalt-worldgen",
                "--example",
                "compare",
                "-q",
            ])
            .output()?;
        let mut f = std::fs::OpenOptions::new().append(true).open(&build_log)?;
        std::io::Write::write_all(&mut f, &st.stdout)?;
        std::io::Write::write_all(&mut f, &st.stderr)?;
        st.status.success()
    };
    if ok2 {
        let mut hargs: Vec<String> = Vec::new();
        if !vanilla {
            hargs.push("--no-vanilla".into());
        }
        if let Some(b) = &base {
            hargs.push("--base".into());
            hargs.push(b.clone());
        }
        let sel = if all { "--all" } else { "--affected" };
        let t = Instant::now();
        let hlog = out.join("pr_ready-harness.log");
        let check = util::exe("release", "basalt-check");
        let st = run_logged(Command::new(&check).arg(sel).args(&hargs), &hlog)?;
        let hl = read_lossy(&hlog);
        let mut summary = hl
            .lines()
            .rfind(|l| is_harness_summary(l))
            .unwrap_or("")
            .to_string();
        if summary.is_empty() {
            summary = hl
                .lines()
                .rfind(|l| {
                    l.starts_with("RESULT run")
                        || l.contains("nothing affected")
                        || l.contains("no harness")
                })
                .unwrap_or("")
                .to_string();
        }
        let name = format!("harnesses ({})", sel.trim_start_matches("--"));
        if st.success() {
            row(&mut rows, &name, "pass", t, summary);
        } else {
            row(
                &mut rows,
                &name,
                "FAIL",
                t,
                format!("{summary}; see {}", hlog.display()),
            );
            fail = true;
        }
        if client {
            let t = Instant::now();
            let clog = out.join("pr_ready-client.log");
            let mut c = Command::new(&check);
            c.args(["--group", "client"]);
            if !all {
                c.arg("--affected");
            }
            let st = run_logged(c.args(&hargs), &clog)?;
            let cl = read_lossy(&clog);
            if cl.contains("no harnesses are registered for group") {
                row(
                    &mut rows,
                    "harnesses (client group)",
                    "skip",
                    t,
                    "no client harness group registered yet (note: nothing run)".into(),
                );
            } else if st.success() {
                let d = cl
                    .lines()
                    .rfind(|l| is_harness_summary(l) || l.contains("nothing affected"))
                    .unwrap_or("");
                row(
                    &mut rows,
                    "harnesses (client group)",
                    "pass",
                    t,
                    clip(d, 120),
                );
            } else {
                row(
                    &mut rows,
                    "harnesses (client group)",
                    "FAIL",
                    t,
                    format!("see {}", clog.display()),
                );
                fail = true;
            }
        }
    } else {
        let bl = read_lossy(&build_log);
        let tail: Vec<&str> = bl
            .lines()
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        row(
            &mut rows,
            "build (release server + check)",
            "FAIL",
            t,
            clip(&tail.join(" "), 200),
        );
        fail = true;
    }

    // ---------------------------------------------------------------- table
    println!();
    println!("{:<32} {:<6} {:>8}  details", "check", "result", "time");
    for r in &rows {
        println!(
            "{:<32} {:<6} {:>7}s  {}",
            r.name, r.result, r.secs, r.detail
        );
    }
    println!();
    println!("pr_ready: {}", if fail { "FAIL" } else { "PASS" });
    // keep target/check from growing: the newest 3 runs per harness stay
    crate::clean_disk::prune_check_outputs();
    Ok(fail as i32)
}

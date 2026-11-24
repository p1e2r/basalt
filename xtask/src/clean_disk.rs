//! `cargo xtask clean-disk [--aggressive] [--dry-run]`: safe disk cleanup for the checkout.
//!
//! 1. Agent worktrees (.claude/worktrees/*): a worktree is *active* (left alone) when a process has
//!    its cwd inside it, a file in it changed in the last 30 minutes, or git has a lock in it.
//!    Inactive worktrees without unmerged commits and without uncommitted changes are removed
//!    (`git worktree remove`; the branch stays); inactive ones with unmerged work only lose their
//!    `target/`. `git worktree prune` afterwards.
//! 2. Build folders (`target/`, `target-play/`, and the shared target of the worktrees): build
//!    artifacts not used for 2 days are swept with cargo-sweep (installed on first use); with
//!    `--aggressive` the cutoff is 0 days and `*/incremental` is dropped. A folder a running process
//!    builds in or runs from is skipped.
//! 3. Harness outputs (`target/check/<harness>/...`): the newest 3 runs per harness are kept (a run
//!    = entries written within 30 minutes of each other), older ones go; the motion frame dumps
//!    (`client-visual/*/motion-*`) always go. Never touched: `vanilla/` (jars, data, the oracle's
//!    cached dumps and parity worlds — expensive to regenerate), `data/`, the user's recordings.
//!
//! Prints the size before and after (`du -sk` of the repository's parent folder).
use crate::util;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

struct Opts {
    aggressive: bool,
    dry: bool,
}

fn size_gb(p: &Path) -> f64 {
    let out = Command::new("du").arg("-sk").arg(p).output();
    out.ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .next()
                .and_then(|k| k.parse::<f64>().ok())
        })
        .unwrap_or(0.0)
        / 1024.0
        / 1024.0
}

/// Directories that are some running process's cwd (lsof), canonicalised.
fn process_cwds() -> Vec<PathBuf> {
    let out = Command::new("lsof")
        .args(["-a", "-d", "cwd", "-Fn"])
        .output();
    let Ok(o) = out else { return Vec::new() };
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter_map(|l| l.strip_prefix('n'))
        .map(PathBuf::from)
        .collect()
}

/// Executables of running processes (to keep a build folder a server/client runs from).
fn process_exes() -> Vec<String> {
    let out = Command::new("ps").args(["-axo", "command="]).output();
    let Ok(o) = out else { return Vec::new() };
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .map(|l| l.split_whitespace().next().unwrap_or("").to_string())
        .collect()
}

fn recently_modified(dir: &Path, within: Duration) -> bool {
    let now = SystemTime::now();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name();
            // build output and git internals change on their own; look at sources and outputs
            if name == "target" || name == ".git" {
                continue;
            }
            let Ok(md) = std::fs::symlink_metadata(&p) else {
                continue;
            };
            if md.file_type().is_symlink() {
                continue;
            }
            if md.is_dir() {
                stack.push(p);
                continue;
            }
            if md
                .modified()
                .ok()
                .and_then(|m| now.duration_since(m).ok())
                .map(|a| a < within)
                .unwrap_or(false)
            {
                return true;
            }
        }
    }
    false
}

fn git(args: &[&str], dir: &Path) -> String {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn remove(p: &Path, o: &Opts, why: &str) {
    println!(
        "  {} {} ({why}, {:.2} GB)",
        if o.dry { "would remove" } else { "remove" },
        p.display(),
        size_gb(p)
    );
    if !o.dry {
        let _ = std::fs::remove_dir_all(p);
    }
}

fn worktrees(root: &Path, o: &Opts) {
    let wt = root.join(".claude/worktrees");
    let Ok(rd) = std::fs::read_dir(&wt) else {
        return;
    };
    let cwds = process_cwds();
    for e in rd.flatten() {
        let p = e.path();
        if !p.join(".git").exists() {
            continue;
        }
        let canon = std::fs::canonicalize(&p).unwrap_or(p.clone());
        let used = cwds.iter().any(|c| c.starts_with(&canon));
        let locked_git = p.join(".git").is_dir() && p.join(".git/index.lock").exists();
        let active = used || locked_git || recently_modified(&p, Duration::from_secs(30 * 60));
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if active {
            println!("  keep {name} (active)");
            continue;
        }
        let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"], &p)
            .trim()
            .to_string();
        let unmerged = git(&["log", "--oneline", &format!("main..{branch}")], root)
            .lines()
            .count();
        let dirty = git(&["status", "--porcelain", "--untracked-files=no"], &p)
            .lines()
            .count();
        if unmerged == 0 && dirty == 0 {
            println!(
                "  {} worktree {name} (branch {branch} is merged)",
                if o.dry { "would remove" } else { "remove" }
            );
            if !o.dry {
                let _ = Command::new("git")
                    .arg("-C")
                    .arg(root)
                    .args(["worktree", "remove", "--force", "--force"])
                    .arg(&p)
                    .status();
                // untracked leftovers (.DS_Store, symlinks to data/vanilla — removed as links, not followed)
                let _ = std::fs::remove_dir_all(&p);
            }
        } else {
            println!("  keep {name} ({unmerged} unmerged commit(s), {dirty} changed file(s))");
            let t = p.join("target");
            if t.is_dir()
                && !std::fs::symlink_metadata(&t)
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false)
            {
                remove(&t, o, "build output of an inactive worktree");
            }
        }
    }
    if !o.dry {
        let _ = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["worktree", "prune"])
            .status();
    }
}

fn sweep(dir: &Path, o: &Opts) {
    if !dir.is_dir() {
        return;
    }
    let canon = std::fs::canonicalize(dir).unwrap_or(dir.to_path_buf());
    let busy_cwd = process_cwds().iter().any(|c| c.starts_with(&canon));
    let short = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    // absolute paths, or relative ones launched from the checkout (`target-play/release/basalt-server`)
    let busy_exe = process_exes().iter().any(|e| {
        e.starts_with(canon.to_string_lossy().as_ref())
            || e.starts_with(&format!("{short}/"))
            || e.starts_with(&format!("./{short}/"))
            || e.contains(&format!("/basalt-git/{short}/"))
    });
    let building = Command::new("pgrep")
        .args(["-f", "cargo (build|test|clippy|run)"])
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);
    if busy_cwd || busy_exe {
        println!("  skip {} (a running process uses it)", dir.display());
        return;
    }
    if Command::new("cargo")
        .args(["sweep", "--version"])
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        println!("  installing cargo-sweep");
        let _ = Command::new("cargo")
            .args(["install", "--quiet", "cargo-sweep"])
            .status();
    }
    let days = if o.aggressive { "0" } else { "2" };
    println!("  cargo sweep --time {days} {}", dir.display());
    // cargo-sweep takes the project; the build folder comes from CARGO_TARGET_DIR
    let mut c = Command::new("cargo");
    c.args(["sweep", "--time", days]);
    if o.dry {
        c.arg("--dry-run");
    }
    c.arg(util::root()).env("CARGO_TARGET_DIR", dir);
    let _ = c.status();
    if o.aggressive && !building {
        for prof in ["debug", "release"] {
            let inc = dir.join(prof).join("incremental");
            if inc.is_dir() {
                remove(&inc, o, "incremental cache (rebuilt on demand)");
            }
        }
    }
}

/// target/check/<harness>/ (and cases/<harness>/): keep the newest 3 runs.
fn prune_outputs(check: &Path, o: &Opts) {
    let Ok(rd) = std::fs::read_dir(check) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    if let Ok(cases) = std::fs::read_dir(check.join("cases")) {
        dirs.extend(cases.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
    }
    for d in dirs {
        if d.ends_with("cases") {
            continue;
        }
        // motion frame dumps are recordings already summarised: always drop
        if d.ends_with("client-visual") {
            if let Ok(rd) = std::fs::read_dir(&d) {
                for s in rd.flatten() {
                    if let Ok(r2) = std::fs::read_dir(s.path()) {
                        for m in r2.flatten() {
                            if m.file_name().to_string_lossy().starts_with("motion-") {
                                remove(&m.path(), o, "motion frame dump");
                            }
                        }
                    }
                }
            }
        }
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        let mut entries: Vec<(SystemTime, PathBuf)> = rd
            .flatten()
            .filter_map(|e| {
                e.metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .map(|t| (t, e.path()))
            })
            .collect();
        entries.sort_by_key(|e| std::cmp::Reverse(e.0));
        // group into runs: a gap of more than 30 minutes starts an older run
        let mut runs = 0;
        let mut last: Option<SystemTime> = None;
        for (t, p) in entries {
            let new_run = last
                .map(|l| {
                    l.duration_since(t)
                        .map(|g| g > Duration::from_secs(30 * 60))
                        .unwrap_or(false)
                })
                .unwrap_or(true);
            if new_run {
                runs += 1;
            }
            last = Some(t);
            if runs > 3 {
                println!(
                    "  {} {} (run {runs} of {})",
                    if o.dry { "would remove" } else { "remove" },
                    p.display(),
                    d.display()
                );
                if !o.dry {
                    if p.is_dir() {
                        let _ = std::fs::remove_dir_all(&p);
                    } else {
                        let _ = std::fs::remove_file(&p);
                    }
                }
            }
        }
    }
}

/// Output pruning only (called at the end of pr-ready / check-all).
pub fn prune_check_outputs() {
    let root = util::root();
    prune_outputs(
        &root.join("target/check"),
        &Opts {
            aggressive: false,
            dry: false,
        },
    );
}

pub fn run(args: &[String]) -> Result<i32> {
    let o = Opts {
        aggressive: args.iter().any(|a| a == "--aggressive"),
        dry: args.iter().any(|a| a == "--dry-run"),
    };
    let root = util::root();
    let parent = root.parent().map(Path::to_path_buf).unwrap_or(root.clone());
    let before = size_gb(&parent);
    println!(
        "clean-disk{}{}: {} is {before:.1} GB",
        if o.aggressive { " --aggressive" } else { "" },
        if o.dry { " (dry run)" } else { "" },
        parent.display()
    );
    println!("worktrees:");
    worktrees(&root, &o);
    println!("build folders:");
    sweep(&root.join("target"), &o);
    sweep(&root.join("target-play"), &o);
    println!("harness outputs:");
    prune_outputs(&root.join("target/check"), &o);
    let after = size_gb(&parent);
    println!("clean-disk: {before:.1} GB -> {after:.1} GB");
    Ok(0)
}

//! Shared helpers: repo root, exe paths, child-process guards, log waiting.
use anyhow::{bail, Context, Result};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// The workspace root (xtask lives in `<root>/xtask`).
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a parent dir")
        .to_path_buf()
}

/// Enter the repo root so relative paths (`data`, `vanilla`, `target`) behave as in the old scripts.
pub fn enter_root() -> Result<()> {
    std::env::set_current_dir(root()).context("cd to repo root")
}

/// `target/<profile>/<name>` (with `.exe` on Windows).
pub fn exe(profile: &str, name: &str) -> PathBuf {
    PathBuf::from("target")
        .join(profile)
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

pub fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

/// Run a command to completion with inherited stdio.
pub fn status(cmd: &mut Command) -> Result<ExitStatus> {
    cmd.status()
        .with_context(|| format!("failed to run {:?}", cmd.get_program()))
}

/// Run `cargo <args>` and bail with the exit status if it fails.
pub fn cargo_ok(args: &[&str]) -> Result<()> {
    let st = status(cargo().args(args))?;
    if !st.success() {
        bail!("cargo {} failed ({st})", args.join(" "));
    }
    Ok(())
}

/// Run a command with stdout+stderr appended to `log`. Returns the exit status.
pub fn run_logged(cmd: &mut Command, log: &Path) -> Result<ExitStatus> {
    let f = File::create(log).with_context(|| format!("creating {}", log.display()))?;
    cmd.stdout(Stdio::from(f.try_clone()?))
        .stderr(Stdio::from(f));
    status(cmd)
}

/// Spawn a command with stdout+stderr redirected to `log`.
pub fn spawn_logged(cmd: &mut Command, log: &Path) -> Result<Child> {
    let f = File::create(log).with_context(|| format!("creating {}", log.display()))?;
    cmd.stdout(Stdio::from(f.try_clone()?))
        .stderr(Stdio::from(f));
    cmd.spawn()
        .with_context(|| format!("failed to spawn {:?}", cmd.get_program()))
}

/// Kills (and reaps) the child when dropped, so a failing task never leaks a server.
pub struct Guard(pub Child);

impl Guard {
    pub fn id(&self) -> u32 {
        self.0.id()
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Ask a child to shut down gracefully (SIGINT on unix, so servers save), then wait up to `secs`
/// before killing it. On Windows there is no portable equivalent, so it is killed straight away.
pub fn stop_gracefully(child: &mut Child, secs: u64) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .arg("-INT")
            .arg(child.id().to_string())
            .status();
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(secs) {
            if let Ok(Some(_)) = child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    #[cfg(not(unix))]
    let _ = secs;
    let _ = child.kill();
    let _ = child.wait();
}

/// Poll `log` until it contains `needle`; false on timeout.
pub fn wait_for_log(log: &Path, needle: &str, timeout: Duration) -> bool {
    let t0 = Instant::now();
    while t0.elapsed() < timeout {
        if std::fs::read(log)
            .map(|b| String::from_utf8_lossy(&b).contains(needle))
            .unwrap_or(false)
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

pub fn read_lossy(p: &Path) -> String {
    std::fs::read(p)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}

/// A free TCP port chosen by the OS (used for throw-away vanilla servers).
pub fn free_port() -> Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port())
}

/// `python3` on unix, `python` on Windows (only for the optional Python helpers).
pub fn python() -> String {
    std::env::var("PYTHON").unwrap_or_else(|_| {
        if cfg!(windows) {
            "python".into()
        } else {
            "python3".into()
        }
    })
}

/// First `n` chars of `s` (char-safe).
pub fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Kill a process and its children (the python launcher spawns java). Only ever called with pids we started.
pub fn kill_tree(pid: u32) {
    #[cfg(unix)]
    {
        let _ = Command::new("pkill")
            .args(["-P", &pid.to_string()])
            .status();
        let _ = Command::new("kill").arg(pid.to_string()).status();
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .output();
    }
}

pub fn secs(t: Instant) -> f64 {
    (t.elapsed().as_secs_f64() * 10.0).round() / 10.0
}

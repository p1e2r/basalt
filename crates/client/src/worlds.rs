//! Singleplayer: worlds under <config dir>/saves, each played through an integrated
//! `basalt-server --local` child process (loopback only, stopped through its stdin so it saves).
use crate::menu::Config;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Where `basalt-server` finds ./data (set from the client's --data).
pub static DATA_DIR: std::sync::OnceLock<String> = std::sync::OnceLock::new();

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct WorldMeta {
    pub name: String,
    /// 0 survival, 1 creative, 2 adventure, 3 spectator
    pub gamemode: u8,
    pub difficulty: u8,
    pub commands: bool,
    pub flat: bool,
    pub seed: i64,
    /// unix seconds
    pub last_played: u64,
}

impl Default for WorldMeta {
    fn default() -> Self {
        WorldMeta {
            name: "New World".into(),
            gamemode: 0,
            difficulty: 2,
            commands: true,
            flat: false,
            seed: 0,
            last_played: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct WorldEntry {
    pub dir: PathBuf,
    pub folder: String,
    pub meta: WorldMeta,
}

const META: &str = "basalt-client.json";

pub fn saves_dir() -> PathBuf {
    Config::dir().join("saves")
}
pub fn backups_dir() -> PathBuf {
    Config::dir().join("backups")
}
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Saved worlds, most recently played first.
pub fn list() -> Vec<WorldEntry> {
    let mut v: Vec<WorldEntry> = std::fs::read_dir(saves_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| {
            let dir = e.path();
            let folder = e.file_name().to_string_lossy().to_string();
            let meta = std::fs::read(dir.join(META))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_else(|| WorldMeta {
                    name: folder.clone(),
                    ..Default::default()
                });
            WorldEntry { dir, folder, meta }
        })
        .collect();
    v.sort_by(|a, b| {
        b.meta
            .last_played
            .cmp(&a.meta.last_played)
            .then(a.folder.cmp(&b.folder))
    });
    v
}

/// "23/11/2026, 19.28" in local time, like the vanilla world list.
pub fn format_time(secs: u64) -> String {
    match jiff::Timestamp::from_second(secs as i64) {
        Ok(t) => t
            .to_zoned_datetime(jiff::tz::TimeZone::system())
            .strftime("%d/%m/%Y, %H.%M")
            .to_string(),
        Err(_) => String::new(),
    }
}

pub fn save_meta(dir: &Path, m: &WorldMeta) -> Result<()> {
    std::fs::write(dir.join(META), serde_json::to_vec_pretty(m)?)?;
    Ok(())
}

/// A folder name for a new world like vanilla picks it: the name with unsafe characters
/// replaced, " (n)" appended while taken.
pub fn new_folder(name: &str) -> String {
    let base: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " _-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim()
        .to_string();
    let base = if base.is_empty() {
        "World".to_string()
    } else {
        base
    };
    let mut f = base.clone();
    let mut n = 1;
    while saves_dir().join(&f).exists() {
        f = format!("{base} ({n})");
        n += 1;
    }
    f
}

/// Seed from the text field: empty = random, a number = itself, text = its string hash (like vanilla).
pub fn parse_seed(s: &str) -> i64 {
    let s = s.trim();
    if s.is_empty() {
        return (now() as i64)
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
    }
    s.parse::<i64>().unwrap_or_else(|_| {
        s.chars()
            .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(c as i32)) as i64
    })
}

pub fn create(meta: &WorldMeta) -> Result<PathBuf> {
    let dir = saves_dir().join(new_folder(&meta.name));
    std::fs::create_dir_all(&dir)?;
    save_meta(&dir, meta)?;
    Ok(dir)
}

pub fn delete(dir: &Path) -> Result<()> {
    std::fs::remove_dir_all(dir).context("deleting world")
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)?.flatten() {
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()))?;
        } else {
            std::fs::copy(&p, to.join(e.file_name()))?;
        }
    }
    Ok(())
}

/// Copy the world folder into the backups folder (timestamped).
pub fn backup(w: &WorldEntry) -> Result<PathBuf> {
    let to = backups_dir().join(format!("{}_{}", w.folder, now()));
    copy_dir(&w.dir, &to)?;
    Ok(to)
}

pub fn open_folder(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = Command::new(opener).arg(dir).spawn();
}

/// A running integrated server.
pub struct Integrated {
    child: Child,
    stdin: Option<ChildStdin>,
    pub port: Option<u16>,
    port_rx: mpsc::Receiver<Result<u16, String>>,
    pub started: Instant,
    pub dir: PathBuf,
}

fn server_exe() -> PathBuf {
    let name = if cfg!(windows) {
        "basalt-server.exe"
    } else {
        "basalt-server"
    };
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join(name)))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(name))
}

impl Drop for Integrated {
    fn drop(&mut self) {
        // still running (e.g. the window closed): ask it to save and stop, then make sure it exits
        if let Some(mut s) = self.stdin.take() {
            let _ = writeln!(s, "stop");
        }
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(10) {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Integrated {
    pub fn start(w: &WorldEntry) -> Result<Integrated> {
        let m = &w.meta;
        let mut cmd = Command::new(server_exe());
        cmd.args(["--local", "--port", "0", "--world"])
            .arg(&w.dir)
            .args([
                "--data",
                DATA_DIR.get().map(|s| s.as_str()).unwrap_or("data"),
                "--seed",
                &m.seed.to_string(),
            ]);
        if m.flat {
            cmd.arg("--flat");
        }
        if m.gamemode == 1 {
            cmd.args(["--gamemode", "creative"]);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .with_context(|| format!("starting {}", server_exe().display()))?;
        let stdin = child.stdin.take();
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("no server stderr"))?;
        let (tx, port_rx) = mpsc::channel();
        let log_path = w.dir.join("server.log");
        std::thread::Builder::new()
            .name("integrated-log".into())
            .spawn(move || {
                let mut log = std::fs::File::create(log_path).ok();
                let mut sent = false;
                for line in std::io::BufReader::new(stderr)
                    .lines()
                    .map_while(|l| l.ok())
                {
                    if let Some(f) = log.as_mut() {
                        let _ = writeln!(f, "{line}");
                    }
                    if !sent {
                        if let Some(p) = line
                            .split("listening on port ")
                            .nth(1)
                            .and_then(|p| p.trim().parse::<u16>().ok())
                        {
                            let _ = tx.send(Ok(p));
                            sent = true;
                        } else if line.contains("Error") || line.contains("panicked") {
                            let _ = tx.send(Err(line.clone()));
                            sent = true;
                        }
                    }
                }
                if !sent {
                    let _ = tx.send(Err("integrated server exited".into()));
                }
            })?;
        Ok(Integrated {
            child,
            stdin,
            port: None,
            port_rx,
            started: Instant::now(),
            dir: w.dir.clone(),
        })
    }

    /// Ok(Some(port)) once listening, Ok(None) while starting.
    pub fn poll(&mut self) -> Result<Option<u16>> {
        if self.port.is_some() {
            return Ok(self.port);
        }
        match self.port_rx.try_recv() {
            Ok(Ok(p)) => {
                self.port = Some(p);
                Ok(Some(p))
            }
            Ok(Err(e)) => Err(anyhow!(e)),
            Err(mpsc::TryRecvError::Empty) if self.started.elapsed() > Duration::from_secs(60) => {
                Err(anyhow!("integrated server did not start"))
            }
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(anyhow!("integrated server exited")),
        }
    }

    /// Ask the server to save and stop, waiting for it (see Drop).
    pub fn stop(self) {
        drop(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn seeds() {
        assert_eq!(parse_seed("12345"), 12345);
        assert_eq!(parse_seed("a"), 97);
        assert_ne!(parse_seed(""), parse_seed("0"));
    }
}

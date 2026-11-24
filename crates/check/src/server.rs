//! Server processes: launching/restarting basalt-server for a scenario, and driving the long-running
//! vanilla test server through its console fifo.
use anyhow::{bail, Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant};

static NEXT_PORT: AtomicU16 = AtomicU16::new(0);

/// A free TCP port in [base, base+60) (base from $CHECK_PORT_BASE, default 25640).
pub fn free_port() -> u16 {
    let base: u16 = std::env::var("CHECK_PORT_BASE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(25640);
    for _ in 0..240 {
        let p = base + NEXT_PORT.fetch_add(1, Ordering::Relaxed) % 60;
        if std::net::TcpListener::bind(("0.0.0.0", p)).is_ok() {
            return p;
        }
    }
    base
}

pub struct BasaltServer {
    pub port: u16,
    pub args: Vec<String>,
    pub log: PathBuf,
    child: Option<Child>,
    starts: u32,
    logs: Vec<PathBuf>,
}

pub fn server_bin() -> PathBuf {
    std::env::var("BASALT_SERVER")
        .map(Into::into)
        .unwrap_or_else(|_| {
            format!(
                "target/release/basalt-server{}",
                std::env::consts::EXE_SUFFIX
            )
            .into()
        })
}

impl BasaltServer {
    /// Start `basalt-server --port P --debug <args>` and wait until it listens.
    pub fn start(args: &[&str], log: &Path) -> Result<BasaltServer> {
        let mut s = BasaltServer {
            port: free_port(),
            args: args.iter().map(|a| a.to_string()).collect(),
            log: log.to_path_buf(),
            child: None,
            starts: 0,
            logs: Vec::new(),
        };
        s.launch()?;
        Ok(s)
    }

    fn launch(&mut self) -> Result<()> {
        if let Some(d) = self.log.parent() {
            std::fs::create_dir_all(d)?;
        }
        self.starts += 1;
        let log_path = if self.starts == 1 {
            self.log.clone()
        } else {
            self.log.with_extension(format!("{}.log", self.starts))
        };
        let f = std::fs::File::create(&log_path)?;
        let child = Command::new(server_bin())
            .arg("--port")
            .arg(self.port.to_string())
            .arg("--debug")
            .arg("--data")
            .arg(crate::data_dir())
            .args(&self.args)
            .env("RUST_LOG", "info")
            .stdout(Stdio::from(f.try_clone()?))
            .stderr(Stdio::from(f))
            .spawn()
            .with_context(|| {
                format!(
                    "starting {} (build it with cargo build --release -p basalt-server)",
                    server_bin().display()
                )
            })?;
        self.child = Some(child);
        let t0 = Instant::now();
        loop {
            let s = std::fs::read_to_string(&log_path).unwrap_or_default();
            let s = s.as_str();
            if s.contains("listening on port") {
                break;
            }
            if let Some(c) = self.child.as_mut() {
                if let Ok(Some(st)) = c.try_wait() {
                    bail!(
                        "basalt-server exited ({st}): {}",
                        s.lines().rev().take(5).collect::<Vec<_>>().join(" | ")
                    );
                }
            }
            if t0.elapsed() > Duration::from_secs(120) {
                bail!("basalt-server did not start listening");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.logs.push(log_path);
        Ok(())
    }

    pub fn addr(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    /// Graceful stop (SIGINT → the server saves the world), then kill if it hangs.
    pub fn stop(&mut self) {
        let Some(mut c) = self.child.take() else {
            return;
        };
        // SIGINT lets the server save the world; Windows has no equivalent for a child process, so it is killed below
        #[cfg(unix)]
        let _ = Command::new("kill")
            .arg("-INT")
            .arg(c.id().to_string())
            .status();
        #[cfg(not(unix))]
        let _ = c.kill();
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(15) {
            if let Ok(Some(_)) = c.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = c.kill();
        let _ = c.wait();
    }

    pub fn restart(&mut self) -> Result<()> {
        self.stop();
        self.launch()
    }

    /// All log text written so far (every run of this server).
    pub fn log_text(&self) -> String {
        self.logs
            .iter()
            .map(|l| std::fs::read_to_string(l).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Drop for BasaltServer {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// The long-running vanilla server: console fifo + server.log.
pub struct Vanilla {
    pub dir: PathBuf,
    pub addr: String,
}

impl Vanilla {
    pub fn new(dir: &Path, addr: &str) -> Vanilla {
        Vanilla {
            dir: dir.to_path_buf(),
            addr: addr.to_string(),
        }
    }

    pub fn log_len(&self) -> usize {
        std::fs::metadata(self.dir.join("server.log"))
            .map(|m| m.len() as usize)
            .unwrap_or(0)
    }
    pub fn log_since(&self, off: usize) -> String {
        let s = std::fs::read(self.dir.join("server.log")).unwrap_or_default();
        String::from_utf8_lossy(&s[off.min(s.len())..]).into_owned()
    }

    /// Run a console command; wait for a log line containing one of `expect` (if given).
    pub fn console(&self, cmd: &str, expect: &[&str]) -> Result<String> {
        let off = self.log_len();
        let fifo = self.dir.join("console.fifo");
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .open(&fifo)
            .with_context(|| format!("opening {}", fifo.display()))?;
        writeln!(f, "{cmd}")?;
        drop(f);
        if expect.is_empty() {
            return Ok(String::new());
        }
        let t0 = Instant::now();
        loop {
            let s = self.log_since(off);
            if expect.iter().any(|e| s.contains(e)) {
                return Ok(s);
            }
            if t0.elapsed() > Duration::from_secs(4) {
                bail!("vanilla console: no reply to `{cmd}`");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Fresh player: delete its saved data (it must be offline).
    pub fn reset_player(&self, name: &str) {
        let uuid = crate::bot::offline_uuid(name);
        for ext in ["dat", "dat_old"] {
            let _ = std::fs::remove_file(
                self.dir
                    .join("world/players/data")
                    .join(format!("{uuid}.{ext}")),
            );
        }
    }

    /// Op a player who is online (resolved from the player list — no profile lookup, which in
    /// offline mode may fail or lower-case the name). Retries if the console line got lost.
    pub fn op_online(&self, name: &str) -> Result<()> {
        let made = format!("Made {name} a server operator");
        for _ in 0..3 {
            if self
                .console(&format!("op {name}"), &[&made, "already is an operator"])
                .is_ok()
            {
                return Ok(());
            }
        }
        bail!("could not op {name} via the vanilla console")
    }

    /// Old name kept for the probe: reset data + op (works only for names the server can resolve).
    pub fn prepare_player(&self, name: &str) -> Result<()> {
        self.reset_player(name);
        self.op_online(name)
    }
}

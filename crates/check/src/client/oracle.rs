//! File-based request/response channel to the test-only vanilla-client oracle mod
//! (tools/oracle-mod): requests go to DIR/in/<id>.json, responses appear as DIR/out/<id>.json.
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Default TCP port of the oracle mod's scripted-input control channel (SCHEMA.md "Control channel").
pub const DEFAULT_CTL_PORT: u16 = 25790;

pub struct Oracle { pub dir: PathBuf, pub ctl_port: u16, child: Option<Child> }

impl Oracle {
    /// Connect to a running oracle client, or (with `launch`) start one via
    /// tools/oracle-mod/launch.py and wait until it answers a ping.
    pub fn connect(dir: &Path, launch: bool) -> Result<Oracle> { Oracle::connect_port(dir, launch, DEFAULT_CTL_PORT) }

    /// Like [`Oracle::connect`] with an explicit control-channel port (passed to the launched client).
    pub fn connect_port(dir: &Path, launch: bool, ctl_port: u16) -> Result<Oracle> {
        let mut o = Oracle { dir: dir.to_path_buf(), ctl_port, child: None };
        std::fs::create_dir_all(dir.join("in"))?; std::fs::create_dir_all(dir.join("out"))?;
        if o.ping(Duration::from_secs(if launch { 4 } else { 20 })).is_ok() { return Ok(o); }
        if !launch { bail!("vanilla oracle not answering in {} (start it with tools/oracle-mod/launch.py, build with tools/oracle-mod/build.sh)", dir.display()); }
        if !dir.join("basalt-oracle.jar").exists() { bail!("{} missing: run tools/oracle-mod/build.sh", dir.join("basalt-oracle.jar").display()); }
        let _ = std::fs::remove_file(dir.join("out/_ready.json"));
        let log = std::fs::File::create(dir.join("client.log"))?;
        eprintln!("oracle: launching the vanilla oracle client (log {})", dir.join("client.log").display());
        let child = Command::new("python3").arg("tools/oracle-mod/launch.py").env("ORACLE_DIR", dir).env("ORACLE_CTL_PORT", ctl_port.to_string())
            .stdout(Stdio::from(log.try_clone()?)).stderr(Stdio::from(log)).spawn().context("starting tools/oracle-mod/launch.py")?;
        o.child = Some(child);
        let t0 = Instant::now();
        while !dir.join("out/_ready.json").exists() {
            if t0.elapsed() > Duration::from_secs(240) { bail!("oracle client did not become ready in 240 s (see {})", dir.join("client.log").display()); }
            if let Some(c) = o.child.as_mut() { if let Ok(Some(st)) = c.try_wait() { bail!("oracle client exited early ({st}); see {}", dir.join("client.log").display()); } }
            std::thread::sleep(Duration::from_millis(500));
        }
        o.ping(Duration::from_secs(30))?;
        Ok(o)
    }

    pub fn ping(&self, timeout: Duration) -> Result<()> {
        let id = format!("ping-{}-{}", std::process::id(), Instant::now().elapsed().as_nanos());
        let p = self.request_bytes(&id, format!("{{\"id\":\"{id}\",\"kind\":\"ping\"}}").as_bytes(), timeout)?;
        let _ = std::fs::remove_file(p);
        Ok(())
    }

    /// Submit a request (its "id" must equal `id`) and wait for the response file; returns its path.
    pub fn request_bytes(&self, id: &str, body: &[u8], timeout: Duration) -> Result<PathBuf> {
        let out = self.dir.join("out").join(format!("{id}.json"));
        let _ = std::fs::remove_file(&out);
        let part = self.dir.join("in").join(format!("{id}.json.part"));
        std::fs::write(&part, body)?;
        let inp = self.dir.join("in").join(format!("{id}.json"));
        std::fs::rename(&part, &inp)?;
        let t0 = Instant::now();
        while !out.exists() {
            if t0.elapsed() > timeout {
                let _ = std::fs::remove_file(&inp);
                return Err(anyhow!("oracle: no response for {id} within {:?}", timeout));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(out)
    }
}

impl Drop for Oracle {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() { let _ = c.kill(); let _ = c.wait(); }
    }
}

/// Client for the oracle mod's control channel: line-based JSON over TCP on 127.0.0.1. Commands are
/// executed by the vanilla client at the start of its ticks, in order; a blocking command (`wait`,
/// `wait_join`, `wait_screen`) holds back the ones behind it, so a script sent in one go is tick-exact.
pub struct Ctl { r: BufReader<TcpStream>, w: TcpStream, next: i64 }

impl Ctl {
    pub fn connect(port: u16, timeout: Duration) -> Result<Ctl> {
        let t0 = Instant::now();
        loop {
            match TcpStream::connect(("127.0.0.1", port)) {
                Ok(s) => { s.set_nodelay(true).ok(); return Ok(Ctl { r: BufReader::new(s.try_clone()?), w: s, next: 1 }); }
                Err(e) => if t0.elapsed() > timeout { bail!("oracle control channel 127.0.0.1:{port} not reachable ({e}); is the oracle client running with this mod build?"); },
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Send one command and wait for its response (fails if the response has `"ok": false`).
    pub fn call(&mut self, op: Value, timeout: Duration) -> Result<Value> {
        let mut v = self.run(&[op], timeout)?;
        let r = v.remove(0);
        if r["ok"] != json!(true) { bail!("oracle: {}", r["error"].as_str().unwrap_or("command failed")); }
        Ok(r)
    }

    /// Pipeline `ops` (all written before reading) and collect one response per op. The overall
    /// timeout is `timeout` plus 50 ms per tick the script waits. Failed commands are returned, not raised.
    pub fn run(&mut self, ops: &[Value], timeout: Duration) -> Result<Vec<Value>> {
        let mut buf = String::new();
        let mut ticks = 0u64;
        for op in ops {
            let mut o = op.clone();
            o["id"] = json!(self.next); self.next += 1;
            ticks += o["ticks"].as_u64().unwrap_or(0) + if o["op"] == "wait_join" { o["timeout"].as_u64().unwrap_or(1200) } else { 0 };
            buf.push_str(&o.to_string()); buf.push('\n');
        }
        self.w.write_all(buf.as_bytes())?;
        let deadline = Instant::now() + timeout + Duration::from_millis(50 * ticks);
        self.r.get_ref().set_read_timeout(Some(Duration::from_secs(1)))?;
        let mut out = Vec::with_capacity(ops.len());
        let mut line = String::new();
        while out.len() < ops.len() {
            match self.r.read_line(&mut line) {
                Ok(0) => bail!("oracle control channel closed"),
                Ok(_) => { out.push(serde_json::from_str(line.trim()).with_context(|| format!("bad response {line:?}"))?); line.clear(); }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => { if Instant::now() > deadline { bail!("oracle: no response to command {} ({}) within the timeout", out.len(), ops[out.len()]); } }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(out)
    }
}

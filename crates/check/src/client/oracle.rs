//! File-based request/response channel to the test-only vanilla-client oracle mod
//! (tools/oracle-mod): requests go to DIR/in/<id>.json, responses appear as DIR/out/<id>.json.
use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub struct Oracle { pub dir: PathBuf, child: Option<Child> }

impl Oracle {
    /// Connect to a running oracle client, or (with `launch`) start one via
    /// tools/oracle-mod/launch.py and wait until it answers a ping.
    pub fn connect(dir: &Path, launch: bool) -> Result<Oracle> {
        let mut o = Oracle { dir: dir.to_path_buf(), child: None };
        std::fs::create_dir_all(dir.join("in"))?; std::fs::create_dir_all(dir.join("out"))?;
        if o.ping(Duration::from_secs(if launch { 4 } else { 20 })).is_ok() { return Ok(o); }
        if !launch { bail!("vanilla oracle not answering in {} (start it with tools/oracle-mod/launch.py, build with tools/oracle-mod/build.sh)", dir.display()); }
        if !dir.join("basalt-oracle.jar").exists() { bail!("{} missing: run tools/oracle-mod/build.sh", dir.join("basalt-oracle.jar").display()); }
        let _ = std::fs::remove_file(dir.join("out/_ready.json"));
        let log = std::fs::File::create(dir.join("client.log"))?;
        eprintln!("client-mesh: launching the vanilla oracle client (log {})", dir.join("client.log").display());
        let child = Command::new("python3").arg("tools/oracle-mod/launch.py").env("ORACLE_DIR", dir)
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

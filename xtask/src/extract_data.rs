//! Port of tools/extract-data.sh: download the official server jar via Mojang's version manifest and
//! run its built-in data generator into ./data (Mojang data stays on the user's machine, gitignored).
use crate::util;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

const VER: &str = "26.2";
const MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

/// Fetch a URL with curl (ships with macOS, Linux and Windows 10+) and parse it as JSON.
fn fetch_json(url: &str) -> Result<Value> {
    let out = Command::new("curl")
        .args(["-fsSL", url])
        .output()
        .context("running curl (is it installed?)")?;
    if !out.status.success() {
        bail!(
            "curl {url} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(serde_json::from_slice(&out.stdout)?)
}

/// Download Mojang's server jar for [`VER`] to `dest`.
pub fn download_server_jar(dest: &Path) -> Result<()> {
    let manifest = fetch_json(MANIFEST)?;
    let url = manifest["versions"]
        .as_array()
        .and_then(|v| v.iter().find(|v| v["id"] == VER))
        .and_then(|v| v["url"].as_str())
        .ok_or_else(|| anyhow!("version {VER} not in Mojang's manifest"))?;
    let ver = fetch_json(url)?;
    let srv = ver["downloads"]["server"]["url"]
        .as_str()
        .ok_or_else(|| anyhow!("no server download for {VER}"))?;
    let tmp = dest.with_extension("jar.part");
    let st = util::status(Command::new("curl").args(["-fL", "-o"]).arg(&tmp).arg(srv))?;
    if !st.success() {
        bail!("downloading the server jar failed ({st})");
    }
    std::fs::rename(&tmp, dest)?;
    Ok(())
}

pub fn run() -> Result<i32> {
    std::fs::create_dir_all("vanilla")?;
    let jar = Path::new("vanilla/server.jar");
    if !jar.exists() {
        download_server_jar(jar)?;
    }
    let java = std::env::var("JAVA").unwrap_or_else(|_| "java".into());
    let _ = std::fs::remove_dir_all("vanilla/gen");
    let st = util::status(Command::new(&java).current_dir("vanilla").args([
        "-DbundlerMainClass=net.minecraft.data.Main",
        "-jar",
        "server.jar",
        "--reports",
        "--server",
        "--output",
        "gen",
    ]))
    .with_context(|| format!("is Java 25+ installed? (JAVA={java})"))?;
    if !st.success() {
        bail!("the data generator failed ({st})");
    }
    let _ = std::fs::remove_dir_all("data");
    std::fs::create_dir_all("data")?;
    for d in ["reports", "data"] {
        copy_dir(
            &Path::new("vanilla/gen").join(d),
            &Path::new("data").join(d),
        )?;
    }
    println!("data extracted to ./data");
    Ok(0)
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let e = e?;
        let dst = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &dst)?;
        } else {
            std::fs::copy(e.path(), dst)?;
        }
    }
    Ok(())
}

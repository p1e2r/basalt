//! `basalt-check client-perf [--update-baseline] [--rd 8,16,32] [--seconds N]`: performance benchmarks of
//! basalt-client (`--bench`, crates/client/src/bench.rs) at render distance 8 / 16 / 32 against a fresh
//! Basalt server, compared with tools/baselines/client-perf.json (numbers only, recorded on the
//! machine that ran `--update-baseline`). Fails when, against the baseline, FPS dropped by more than
//! 20 %, the mean section meshing time grew by more than 20 % or resident memory grew by more than
//! 20 %. Without a baseline entry the numbers are printed and the row fails (record one first).
use super::{client_bin, no_gpu, skip};
use crate::server::BasaltServer;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub const BASELINE: &str = "tools/baselines/client-perf.json";
/// Allowed regression against the baseline.
pub const TOLERANCE: f64 = 0.20;

/// Source paths the benchmark depends on.
pub fn affected() -> Vec<&'static str> {
    let mut p: Vec<&'static str> = super::RENDER_AFFECTED.to_vec();
    p.extend([
        "crates/worldgen/**",
        "crates/server/**",
        "crates/check/src/client/perf.rs",
        "crates/check/src/server.rs",
        BASELINE,
    ]);
    p
}

struct Cli {
    client: PathBuf,
    jar: PathBuf,
    out: PathBuf,
    rds: Vec<u32>,
    seconds: f32,
    update: bool,
}

fn parse(args: &[String]) -> Result<Cli> {
    let mut c = Cli {
        client: client_bin(),
        jar: basalt_paths::client_jar(),
        out: "target/check".into(),
        rds: vec![8, 16, 32],
        seconds: 30.0,
        update: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut v = || {
            it.next()
                .cloned()
                .with_context(|| format!("{a} needs a value"))
        };
        match a.as_str() {
            "--client" => c.client = v()?.into(),
            "--jar" => c.jar = v()?.into(),
            "--out" => c.out = v()?.into(),
            "--rd" => c.rds = v()?.split(',').filter_map(|x| x.parse().ok()).collect(),
            "--seconds" => c.seconds = v()?.parse()?,
            "--update-baseline" => c.update = true,
            other => bail!("client-perf: unknown argument {other}"),
        }
    }
    Ok(c)
}

#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub fps: f64,
    pub p99_ms: f64,
    pub mesh_ms: f64,
    pub rss_mb: f64,
    /// GPU section buffers: deterministic, unlike the process RSS (macOS compresses/swaps pages: 50..280 MB between runs)
    pub gpu_mb: f64,
}

fn sample_of(j: &Value) -> Sample {
    Sample {
        fps: j["fps_avg"].as_f64().unwrap_or(0.0),
        p99_ms: j["frame_ms"]["p99"].as_f64().unwrap_or(0.0),
        mesh_ms: j["mesh_ms_per_section"]["avg"].as_f64().unwrap_or(0.0),
        rss_mb: j["memory"]["rss_kb"].as_f64().unwrap_or(0.0) / 1024.0,
        gpu_mb: j["memory"]["gpu_section_buffers_kb"]
            .as_f64()
            .unwrap_or(0.0)
            / 1024.0,
    }
}

/// Which of the three gated metrics regressed by more than `tol` (fps down, mesh time and memory up).
pub fn regressions(base: &Sample, now: &Sample, tol: f64) -> Vec<String> {
    let mut v = vec![];
    if base.fps > 0.0 && now.fps < base.fps * (1.0 - tol) {
        v.push(format!(
            "fps {:.1} < {:.1} (baseline {:.1} -{:.0}%)",
            now.fps,
            base.fps * (1.0 - tol),
            base.fps,
            tol * 100.0
        ));
    }
    if base.mesh_ms > 0.0 && now.mesh_ms > base.mesh_ms * (1.0 + tol) {
        v.push(format!(
            "mesh {:.2} ms > {:.2} ms (baseline {:.2} +{:.0}%)",
            now.mesh_ms,
            base.mesh_ms * (1.0 + tol),
            base.mesh_ms,
            tol * 100.0
        ));
    }
    if base.gpu_mb > 0.0 && now.gpu_mb > base.gpu_mb * (1.0 + tol) {
        v.push(format!(
            "GPU section memory {:.1} MB > {:.1} MB (baseline {:.1} +{:.0}%)",
            now.gpu_mb,
            base.gpu_mb * (1.0 + tol),
            base.gpu_mb,
            tol * 100.0
        ));
    }
    v
}

fn base_sample(b: &Value, key: &str) -> Option<Sample> {
    let e = b.get(key)?;
    Some(Sample {
        fps: e["fps_avg"].as_f64()?,
        p99_ms: e["frame_p99_ms"].as_f64().unwrap_or(0.0),
        mesh_ms: e["mesh_ms_avg"].as_f64()?,
        rss_mb: e["rss_mb"].as_f64().unwrap_or(0.0),
        gpu_mb: e["gpu_mb"].as_f64()?,
    })
}

/// The benchmark world is superflat: chunk generation speed and terrain shape would otherwise make the
/// load phase of each run (which the numbers include) vary more than the renderer does.
fn server_args(view: &str) -> Vec<String> {
    [
        "--memory",
        "--no-spawning",
        "--gamemode",
        "creative",
        "--flat",
        "--view",
        view,
    ]
    .map(String::from)
    .to_vec()
}

pub fn run(args: &[String]) -> Result<usize> {
    let c = parse(args)?;
    if !c.client.exists() {
        bail!(
            "{} missing: cargo build --release -p basalt-client",
            c.client.display()
        );
    }
    super::input_diff::use_port_range();
    let dir = c.out.join("client-perf");
    std::fs::create_dir_all(&dir)?;
    let base: Value = std::fs::read_to_string(BASELINE)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(json!({}));
    let mut newbase = base.as_object().cloned().unwrap_or_default();
    let mut failed = 0;
    for &rd in &c.rds {
        let view = rd.to_string();
        let args: Vec<String> = server_args(&view);
        let argr: Vec<&str> = args.iter().map(String::as_str).collect();
        let srv = BasaltServer::start(&argr, &dir.join(format!("rd{rd}.server.log")))?;
        let json_path = dir.join(format!("rd{rd}.json"));
        let _ = std::fs::remove_file(&json_path);
        let out = Command::new(&c.client)
            .arg("--jar")
            .arg(&c.jar)
            .args(["--name", "BenchBot", "--addr"])
            .arg(srv.addr())
            .arg("--bench")
            .arg(&json_path)
            .args([
                "--render-distance",
                &view,
                "--seconds",
                &c.seconds.to_string(),
                "--size",
                "1280x720",
                "--bench-mode",
                "rotate",
                "--pos",
                "0.5,-50,0.5",
            ])
            .env("RUST_LOG", "warn")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .context("starting basalt-client --bench")?;
        drop(srv);
        let text = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        if !json_path.exists() {
            if no_gpu(&text) {
                return skip("no graphics adapter for basalt-client");
            }
            bail!(
                "bench rd {rd} produced no report: {}",
                text.lines().rev().take(3).collect::<Vec<_>>().join(" | ")
            );
        }
        let j: Value = serde_json::from_slice(&std::fs::read(&json_path)?)?;
        let s = sample_of(&j);
        let key = format!("rd{rd}");
        let line = format!("rd {rd:2}: {:7.1} fps (p99 frame {:.1} ms), mesh {:.2} ms/section, {} sections, {} chunks, rss {:.0} MB (info), GPU sections {:.1} MB", s.fps, s.p99_ms, s.mesh_ms, j["sections_meshed"], j["chunks_loaded"], s.rss_mb, s.gpu_mb);
        if c.update {
            println!("PERF {line} (recorded)");
            newbase.insert(key, json!({"fps_avg": round(s.fps, 1), "frame_p99_ms": round(s.p99_ms, 2), "mesh_ms_avg": round(s.mesh_ms, 3), "rss_mb": round(s.rss_mb, 0), "gpu_mb": round(s.gpu_mb, 1)}));
            continue;
        }
        match base_sample(&base, &key) {
            Some(b) => {
                let r = regressions(&b, &s, TOLERANCE);
                if r.is_empty() {
                    println!(
                        "PERF {line}: ok vs baseline ({:.1} fps, {:.2} ms, {:.1} MB GPU)",
                        b.fps, b.mesh_ms, b.gpu_mb
                    );
                } else {
                    failed += 1;
                    println!("PERF {line}: REGRESSION {}", r.join("; "));
                }
            }
            None => {
                failed += 1;
                println!("PERF {line}: no baseline for {key} (run with --update-baseline)");
            }
        }
    }
    if c.update {
        std::fs::create_dir_all("tools/baselines")?;
        newbase.insert("_note".into(), json!("numbers recorded by `basalt-check client-perf --update-baseline` on one machine; fail at >20% regression"));
        std::fs::write(
            BASELINE,
            serde_json::to_string_pretty(&Value::Object(newbase))? + "\n",
        )?;
        eprintln!("client-perf: wrote {BASELINE}");
    }
    println!("RESULT client_perf rds={} failed={failed}", c.rds.len());
    Ok(failed)
}

fn round(x: f64, d: i32) -> f64 {
    let m = 10f64.powi(d);
    (x * m).round() / m
}

pub fn main(args: &[String]) -> i32 {
    super::finish("client_perf", run(args))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn regression_thresholds() {
        let b = Sample {
            fps: 100.0,
            p99_ms: 20.0,
            mesh_ms: 2.0,
            rss_mb: 1000.0,
            gpu_mb: 100.0,
        };
        assert!(regressions(
            &b,
            &Sample {
                fps: 85.0,
                p99_ms: 30.0,
                mesh_ms: 2.3,
                rss_mb: 1150.0,
                gpu_mb: 115.0
            },
            0.2
        )
        .is_empty());
        assert_eq!(
            regressions(
                &b,
                &Sample {
                    fps: 70.0,
                    p99_ms: 20.0,
                    mesh_ms: 2.0,
                    rss_mb: 1000.0,
                    gpu_mb: 100.0
                },
                0.2
            )
            .len(),
            1
        );
        assert_eq!(
            regressions(
                &b,
                &Sample {
                    fps: 70.0,
                    p99_ms: 20.0,
                    mesh_ms: 3.0,
                    rss_mb: 100.0,
                    gpu_mb: 130.0
                },
                0.2
            )
            .len(),
            3
        );
    }
}

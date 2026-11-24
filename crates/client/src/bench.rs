//! Benchmark: `--bench out.json --addr HOST:PORT --render-distance N --seconds S [--bench-mode rotate|fly]
//! [--pos x,y,z] [--size WxH]`. Joins, then for S seconds either rotates in place (yaw 0→360°) or flies
//! along +X at 10 blocks/s, rendering offscreen every frame (GPU work waited for), and writes FPS,
//! frame-time percentiles, per-section mesh times, sections meshed, memory and chunk
//! arrival→visible latency to JSON.
use crate::assets::Assets;
use crate::{Args, Game, GameOpts};
use anyhow::{bail, Result};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn pct(v: &[f32], p: f32) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s[((s.len() - 1) as f32 * p).round() as usize]
}
fn avg(v: &[f32]) -> f32 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f32>() / v.len() as f32
    }
}
fn rss_kb() -> u64 {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(0)
}

pub fn run(args: &Args, assets: Arc<Assets>, out: &str) -> Result<()> {
    let cfg = crate::config_from_args(args);
    let rd = cfg.render_distance;
    let mut g = Game::new(
        None,
        args.size,
        assets,
        crate::load_rules(args),
        GameOpts {
            name: args.name.clone(),
            addr: Some(args.addr.clone()),
            auto_respawn: true,
            sounds: crate::sound::Sounds::null(),
            cfg,
        },
    )?;
    g.hide_gui = args.no_gui;
    let t = Instant::now();
    while !(g.spawned && g.loaded_sent) {
        g.update();
        std::thread::sleep(Duration::from_millis(5));
        if let Some(d) = &g.disconnected {
            bail!("disconnected: {d}");
        }
        if t.elapsed() > Duration::from_secs(30) {
            bail!("did not spawn");
        }
    }
    let start_pos = args.pos.unwrap_or(g.player.pos);
    g.player.flying = true;
    g.player.pos = start_pos;
    g.player.prev_pos = start_pos;
    g.player.pitch = 0.0;
    let mut frames: Vec<f32> = Vec::new();
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f32() < args.seconds {
        let el = t0.elapsed().as_secs_f32();
        if args.bench_mode == "fly" {
            g.player.pos = [start_pos[0] + el as f64 * 10.0, start_pos[1], start_pos[2]];
            g.player.yaw = -90.0;
        } else {
            g.player.yaw = 360.0 * el / args.seconds;
        }
        let f0 = Instant::now();
        let alpha = g.update();
        let fd = g.frame(alpha);
        let f = fd.as_frame(g.world.min_y as f32);
        g.gpu.render_offscreen(&f);
        drop(f);
        frames.push(f0.elapsed().as_secs_f32() * 1000.0);
        if let Some(d) = &g.disconnected {
            bail!("disconnected: {d}");
        }
    }
    let secs = t0.elapsed().as_secs_f32();
    let n = frames.len();
    let mut sorted = frames.clone();
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let worst1 = &sorted[..(n / 100).max(1)];
    let s = &g.stats;
    let report = json!({
        "render_distance": rd, "seconds": secs, "mode": args.bench_mode, "size": [args.size.0, args.size.1],
        "frames": n, "fps_avg": n as f32 / secs, "fps_1pct_low": 1000.0 / avg(worst1),
        "frame_ms": { "avg": avg(&frames), "p50": pct(&frames, 0.5), "p90": pct(&frames, 0.9), "p99": pct(&frames, 0.99), "max": sorted.first().copied().unwrap_or(0.0) },
        "mesh_ms_per_section": { "avg": avg(&s.mesh_ms), "p95": pct(&s.mesh_ms, 0.95) },
        "sections_meshed": s.sections_meshed, "sections_resident": g.gpu.sections.len(), "chunks_loaded": g.world.chunks.len(),
        "chunk_visible_latency_ms": { "count": s.chunk_latency_ms.len(), "avg": avg(&s.chunk_latency_ms), "p95": pct(&s.chunk_latency_ms, 0.95), "max": s.chunk_latency_ms.iter().cloned().fold(0.0, f32::max) },
        "memory": { "rss_kb": rss_kb(), "gpu_section_buffers_kb": g.gpu.section_bytes() / 1024 },
    });
    std::fs::write(out, serde_json::to_vec_pretty(&report)?)?;
    println!(
        "bench rd {rd}: {:.1} fps avg, {:.1} 1% low, {} sections meshed, {} chunks → {out}",
        n as f32 / secs,
        1000.0 / avg(worst1),
        s.sections_meshed,
        g.world.chunks.len()
    );
    Ok(())
}

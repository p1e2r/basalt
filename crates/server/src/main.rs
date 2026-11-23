//! Basalt server: a clean-room Minecraft Java 26.2 server.
//!
//! Usage: basalt-server [--port 25565] [--world DIR | --memory] [--seed N] [--view N]
//!                      [--gamemode survival|creative] [--compression N] [--data ./data] [--flat] [--no-spawning] [--debug]
mod commands;
mod conn;
mod game;
mod gen;
mod registry;

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize};
use std::sync::{mpsc, Arc};

pub struct Config {
    pub port: u16,
    pub world_dir: Option<PathBuf>,
    pub seed: i64,
    pub view_distance: u8,
    pub max_players: usize,
    pub motd: String,
    pub gamemode: u8,
    pub compression: i32,
    pub data_dir: PathBuf,
    pub flat: bool,
    /// Enables /basalt:state, /basalt:block, /basalt:items (test harness queries).
    pub debug: bool,
    /// Disable natural mob spawning (summoned mobs still work).
    pub no_spawning: bool,
    /// Integrated server for the client's singleplayer: bind 127.0.0.1 only, and stop (saving)
    /// on a "stop" line or end of stdin, so the parent can shut it down on any OS.
    pub local: bool,
}

fn parse_args() -> Config {
    let mut c = Config { port: 25565, world_dir: Some("world".into()), seed: 12345, view_distance: 10, max_players: 20,
        motd: "A Basalt server".into(), gamemode: 0, compression: 256, data_dir: "data".into(), flat: false, no_spawning: false, debug: false, local: false };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut v = || it.next().unwrap_or_default();
        match k.as_str() {
            "--port" => c.port = v().parse().unwrap_or(25565),
            "--world" => c.world_dir = Some(v().into()),
            "--memory" => c.world_dir = None,
            "--seed" => c.seed = v().parse().unwrap_or(12345),
            "--view" => c.view_distance = v().parse().unwrap_or(10),
            "--motd" => c.motd = v(),
            "--gamemode" => c.gamemode = match v().as_str() { "creative" | "1" => 1, "adventure" | "2" => 2, "spectator" | "3" => 3, _ => 0 },
            "--compression" => c.compression = v().parse().unwrap_or(256),
            "--data" => c.data_dir = v().into(),
            "--flat" => c.flat = true,
            "--debug" => c.debug = true,
            "--no-spawning" => c.no_spawning = true,
            "--local" => c.local = true,
            other => eprintln!("unknown argument {other}"),
        }
    }
    c
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cfg = parse_args();
    let registries = registry::Registries::load(&cfg.data_dir).context("loading registries")?;
    log::info!("loaded {} synced registries, {} tag registries", registries.synced.len(), registries.tags.len());
    let data = std::sync::Arc::new(basalt_game::GameData::load(&cfg.data_dir).context("loading game data")?);
    let plains = registries.id("minecraft:worldgen/biome", "minecraft:plains").unwrap_or(0) as u8;
    let (tx, rx) = mpsc::channel();
    let port = cfg.port;
    let local = cfg.local;
    let sh = Arc::new(conn::Shared { cfg, registries, data, online: AtomicUsize::new(0), next_eid: AtomicI32::new(1), game: std::sync::Mutex::new(tx) });
    let stop = Arc::new(AtomicBool::new(false));
    let generator: Arc<dyn gen::Generator> = if sh.cfg.flat { Arc::new(gen::Flat::new(plains)) } else {
        let t = std::time::Instant::now();
        let g = basalt_worldgen::Generator::try_new(sh.cfg.seed, &sh.cfg.data_dir).context("loading worldgen settings")?;
        log::info!("worldgen ready in {:.2?} (seed {})", t.elapsed(), sh.cfg.seed);
        Arc::new(gen::Vanilla(g))
    };
    let game = game::Game::new(sh.clone(), generator);
    let stop2 = stop.clone();
    let game_thread = std::thread::Builder::new().name("game".into()).spawn(move || game.run(rx, stop2))?;

    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind((if local { "127.0.0.1" } else { "0.0.0.0" }, port)).await?;
        let port = listener.local_addr()?.port();
        log::info!("Basalt listening on port {port}");
        // --local: the parent process stops us through stdin
        let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
        // without --local the sender must stay alive, or `changed()` would end the loop at once
        let _keep = if local { None } else { Some(stop_tx.clone()) };
        if local {
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
                while let Ok(Some(l)) = lines.next_line().await { if l.trim() == "stop" { break; } }
                let _ = stop_tx.send(true);
            });
        }
        loop {
            tokio::select! {
                _ = stop_rx.changed() => { log::info!("stopping (stdin)"); break; }
                a = listener.accept() => {
                    let (s, _) = a?;
                    let sh = sh.clone();
                    tokio::spawn(async move { if let Err(e) = conn::handle(s, sh).await { log::debug!("connection: {e:#}"); } });
                }
                _ = tokio::signal::ctrl_c() => { log::info!("stopping"); break; }
            }
        }
        anyhow::Ok(())
    })?;
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = game_thread.join();
    Ok(())
}

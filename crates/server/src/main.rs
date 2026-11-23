//! Basalt server: a clean-room Minecraft Java 26.2 server.
//!
//! Usage: basalt-server [--port 25565] [--world DIR | --memory] [--seed N] [--view N]
//!                      [--gamemode survival|creative] [--compression N] [--data ./data] [--flat] [--no-spawning] [--debug]
mod admin;
mod cmdparse;
mod commands;
mod conn;
mod game;
mod gen;
mod level;
mod registry;
mod snbt;

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
    /// server root holding server.properties / ops.json / ... (None for --memory and --local)
    pub admin_root: Option<PathBuf>,
    /// difficulty a new world starts with (server.properties `difficulty`)
    pub difficulty: u8,
    pub whitelist: bool,
    pub op_level: u8,
}

fn parse_args() -> Config {
    let mut c = Config { port: 25565, world_dir: Some("world".into()), seed: 12345, view_distance: 10, max_players: 20,
        motd: "A Basalt server".into(), gamemode: 0, compression: 256, data_dir: "data".into(), flat: false, no_spawning: false, debug: false, local: false, admin_root: None, difficulty: 1, whitelist: false, op_level: 4 };
    let mut given: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut v = || it.next().unwrap_or_default();
        given.insert(k.clone());
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
    // server.properties in the server root (the folder holding the world); flags win over it
    if !c.local {
        if let Some(w) = &c.world_dir {
            // an existing server.properties in the working folder or next to the world (vanilla's
            // layout); otherwise the admin files live inside the world folder (never in an
            // unrelated parent such as the home directory)
            let parent = w.parent().filter(|p| !p.as_os_str().is_empty()).map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
            let root = [PathBuf::from("."), parent].into_iter().find(|r| r.join("server.properties").exists()).unwrap_or_else(|| w.clone());
            let props = admin::Properties::load_or_create(&root);
            let num = |k: &str| props.get(k).and_then(|v| v.parse::<i64>().ok());
            if !given.contains("--port") { if let Some(v) = num("server-port") { c.port = v as u16; } }
            if !given.contains("--motd") { if let Some(v) = props.get("motd") { c.motd = v.to_string(); } }
            if let Some(v) = num("max-players") { c.max_players = v.max(0) as usize; }
            if !given.contains("--view") { if let Some(v) = num("view-distance") { c.view_distance = v.clamp(2, 32) as u8; } }
            if !given.contains("--compression") { if let Some(v) = num("network-compression-threshold") { c.compression = v as i32; } }
            if !given.contains("--gamemode") { if let Some(v) = props.get("gamemode") { c.gamemode = match v { "creative" | "1" => 1, "adventure" | "2" => 2, "spectator" | "3" => 3, _ => 0 }; } }
            if !given.contains("--seed") { if let Some(v) = props.get("level-seed") { c.seed = v.parse().unwrap_or_else(|_| v.chars().fold(0i32, |h, ch| h.wrapping_mul(31).wrapping_add(ch as i32)) as i64); } }
            c.difficulty = match props.get("difficulty") { Some("peaceful" | "0") => 0, Some("normal" | "2") => 2, Some("hard" | "3") => 3, _ => 1 };
            c.whitelist = props.bool("white-list");
            c.op_level = num("op-permission-level").unwrap_or(4).clamp(1, 4) as u8;
            if props.bool("online-mode") { log::warn!("online-mode=true in server.properties is ignored: Basalt is offline-mode only"); }
            c.admin_root = Some(root);
            // an existing world's seed wins over server.properties (vanilla behaviour)
            if !given.contains("--seed") { if let Some(seed) = level::load(w).seed { c.seed = seed; } }
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
    let lists = admin::Lists::load(cfg.admin_root.as_deref(), cfg.whitelist, cfg.op_level);
    let sh = Arc::new(conn::Shared { cfg, registries, data, online: AtomicUsize::new(0), next_eid: AtomicI32::new(1), game: std::sync::Mutex::new(tx), admin: std::sync::Mutex::new(lists) });
    let stop = Arc::new(AtomicBool::new(false));
    let generator: Arc<dyn gen::Generator> = if sh.cfg.flat { Arc::new(gen::Flat::new(plains)) } else {
        let t = std::time::Instant::now();
        let g = basalt_worldgen::Generator::try_new(sh.cfg.seed, &sh.cfg.data_dir).context("loading worldgen settings")?;
        log::info!("worldgen ready in {:.2?} (seed {})", t.elapsed(), sh.cfg.seed);
        Arc::new(gen::Vanilla(g))
    };
    // the Nether always generates normally (vanilla superflat worlds keep a normal Nether too)
    let nether = basalt_worldgen::Generator::try_new_dim(sh.cfg.seed, &sh.cfg.data_dir, basalt_worldgen::Dimension::Nether).context("loading nether worldgen settings")?;
    let game = game::Game::new(sh.clone(), vec![generator, Arc::new(gen::Vanilla(nether))]);
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

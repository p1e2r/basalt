//! basalt-check: a GPU-free protocol bot, server-state queries for Basalt (`--debug`) and vanilla
//! (`data get` / `execute if block`), and scenario tests that run against either server — or
//! both, comparing outcomes (differential mode).
pub mod bot;
pub mod driver;
pub mod fuzz;
pub mod harness;
pub mod client;
pub mod physics;
pub mod proto;
pub mod query;
pub mod runner;
pub mod scenarios;
pub mod server;
pub mod world;

use std::sync::{Arc, OnceLock};

/// Directory with the data generator output (`$BASALT_DATA` or ./data).
pub fn data_dir() -> std::path::PathBuf { std::env::var("BASALT_DATA").map(Into::into).unwrap_or_else(|_| "data".into()) }

/// Gameplay rules (dig times etc.) from ./data, loaded once.
pub fn game_data() -> Arc<basalt_game::GameData> {
    static G: OnceLock<Arc<basalt_game::GameData>> = OnceLock::new();
    G.get_or_init(|| Arc::new(basalt_game::GameData::load(&data_dir()).expect("loading game data (run `cargo xtask extract-data`)"))).clone()
}

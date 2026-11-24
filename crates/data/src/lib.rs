//! Game data tables generated at build time from the official data generator reports.
#![allow(clippy::all)]

pub const PROTOCOL_VERSION: i32 = 776;
pub const VERSION_NAME: &str = "26.2";
pub const WORLD_VERSION: i32 = 4903;

#[derive(Debug)]
pub struct BlockInfo {
    pub name: &'static str,
    pub kind: &'static str,
    pub default_state: u32,
    pub first_state: u32,
    pub num_states: u32,
    pub props: &'static [(&'static str, &'static [&'static str])],
}

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

pub mod block;
pub mod sound_group;
pub use block::BlockState;

/// Look up a registry entry's numeric id.
pub fn registry_id(registry: &[&str], name: &str) -> Option<i32> {
    let n = name.trim_start_matches("minecraft:");
    registry.iter().position(|e| *e == n).map(|i| i as i32)
}

//! "client" harness group: differential checks of basalt-client against the vanilla client's
//! own outputs, dumped by the test-only oracle mod (tools/oracle-mod; schema in its SCHEMA.md).
//! Affected source paths (for the harness registry): crates/client/src/{mesh,assets}.rs,
//! crates/client/src/shader.wgsl.
pub mod anim_diff;
pub mod click_oracle;
pub mod input_diff;
pub mod mesh_diff;
pub mod oracle;
pub mod rng;

/// Source paths whose changes make the client-mesh harness worth re-running.
pub const MESH_AFFECTED: &[&str] = &["crates/client/src/mesh.rs", "crates/client/src/assets.rs", "crates/client/src/shader.wgsl"];

/// Source paths whose changes make the animation-parity harness (`client-anim`) worth re-running.
pub const ANIM_AFFECTED: &[&str] = &["crates/client/src/{entity,anim,main,script}.rs", "crates/check/src/client/{anim_diff,input_diff,oracle}.rs", "tools/oracle-mod/**"];

/// Source paths whose changes make the input-parity / click-oracle stages worth re-running.
/// (`client-input` / `client-clicks` drive the vanilla client through the oracle mod's control
/// channel, see tools/oracle-mod/SCHEMA.md.)
pub const INPUT_AFFECTED: &[&str] = &["crates/client/src/{main,script,container,player,net}.rs", "crates/server/src/{conn,commands}.rs", "crates/server/src/game/**", "crates/game/**"];

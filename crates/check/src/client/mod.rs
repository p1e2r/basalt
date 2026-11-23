//! "client" harness group: differential checks of basalt-client against the vanilla client's
//! own outputs, dumped by the test-only oracle mod (tools/oracle-mod; schema in its SCHEMA.md).
//! Affected source paths (for the harness registry): crates/client/src/{mesh,assets}.rs,
//! crates/client/src/shader.wgsl.
pub mod mesh_diff;
pub mod oracle;
pub mod rng;

/// Source paths whose changes make the client-mesh harness worth re-running.
pub const MESH_AFFECTED: &[&str] = &["crates/client/src/mesh.rs", "crates/client/src/assets.rs", "crates/client/src/shader.wgsl"];

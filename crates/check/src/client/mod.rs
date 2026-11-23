//! "client" harness group: differential checks of basalt-client against the vanilla client's
//! own outputs, dumped by the test-only oracle mod (tools/oracle-mod; schema in its SCHEMA.md),
//! plus the client-only harnesses: visual parity (`visual`), asset coverage and UI scenarios
//! (`coverage`) and performance benchmarks (`perf`).
//! Affected source paths (for the harness registry): crates/client/src/{mesh,assets}.rs,
//! crates/client/src/shader.wgsl.
pub mod anim_diff;
pub mod click_oracle;
pub mod coverage;
pub mod imgcmp;
pub mod input_diff;
pub mod mesh_diff;
pub mod oracle;
pub mod perf;
pub mod rng;
pub mod visual;

/// Source paths whose changes make the client-mesh harness worth re-running.
pub const MESH_AFFECTED: &[&str] = &["crates/client/src/mesh.rs", "crates/client/src/assets.rs", "crates/client/src/shader.wgsl"];

/// Source paths whose changes make the animation-parity harness (`client-anim`) worth re-running.
pub const ANIM_AFFECTED: &[&str] = &["crates/client/src/{entity,anim,main,script}.rs", "crates/check/src/client/{anim_diff,input_diff,oracle}.rs", "tools/oracle-mod/**"];

/// Source paths whose changes make the input-parity / click-oracle stages worth re-running.
/// (`client-input` / `client-clicks` drive the vanilla client through the oracle mod's control
/// channel, see tools/oracle-mod/SCHEMA.md.)
pub const INPUT_AFFECTED: &[&str] = &["crates/client/src/{main,script,container,player,net}.rs", "crates/server/src/{conn,commands}.rs", "crates/server/src/game/**", "crates/game/**"];

/// Anything that changes what basalt-client draws or how fast it does so.
pub const RENDER_AFFECTED: &[&str] = &["crates/client/**", "crates/data/**", "crates/world/**", "crates/protocol/**", "Cargo.lock"];

/// A harness that cannot run here (no GPU, no real client, no display) fails with this error; the
/// binaries print `SKIP` and the driver reports the row as skipped instead of failed.
#[derive(Debug)]
pub struct Skip(pub String);

impl std::fmt::Display for Skip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for Skip {}

pub fn skip<T>(why: impl Into<String>) -> anyhow::Result<T> {
    Err(Skip(why.into()).into())
}

/// Turn a harness result into an exit code, printing `RESULT <id> skipped=1 ...` for [`Skip`]s.
pub fn finish(id: &str, r: anyhow::Result<usize>) -> i32 {
    match r {
        Ok(0) => 0,
        Ok(_) => 1,
        Err(e) => match e.downcast_ref::<Skip>() {
            Some(s) => {
                println!("SKIP {id}: {s}");
                println!("RESULT {id} skipped=1 reason={}", s.0.replace(' ', "_"));
                0
            }
            None => {
                eprintln!("{id}: {e:#}");
                2
            }
        },
    }
}

/// True when basalt-client output says there is no usable graphics adapter (headless CI).
pub fn no_gpu(output: &str) -> bool {
    output.contains("no graphics adapter found") || output.contains("No suitable") || output.contains("request_adapter")
}

/// `target/release/basalt-client` (env BASALT_CLIENT overrides).
pub fn client_bin() -> std::path::PathBuf {
    std::env::var("BASALT_CLIENT").map(Into::into).unwrap_or_else(|_| format!("target/release/basalt-client{}", std::env::consts::EXE_SUFFIX).into())
}

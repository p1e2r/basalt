//! `cargo xtask <task>`: the cross-platform replacement for the bash/swift scripts under tools/.
//! The old tools/*.sh are thin wrappers around these subcommands.
mod check_all;
mod extract_data;
mod pr_ready;
mod smoke;
mod util;
mod window;

use anyhow::{bail, Result};

const USAGE: &str = "usage: cargo xtask <task> [args]
tasks:
  extract-data                 download the official 26.2 server jar and run its data generator into ./data (Java 25+)
  smoke [OUTDIR]               basalt-server + two basalt-clients end to end (env PORT, default 25580)
  client-scenarios [NAMES|all] basalt-client --scenario against a fresh in-memory server (env PORT=25790, VANILLA, OUT)
  pr-ready [--client] [--all] [--no-vanilla] [--update-baselines] [--base REF]
  check-all [--quick] [--no-vanilla] [--real-client] [--ci] [--fuzz] [--fuzz-long]
  window-screenshot [OUT.png]  capture only the Minecraft window (macOS/Windows/Linux)
  window-bounds                print the Minecraft window bounds: x y w h";

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(task) = args.next() else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let rest: Vec<String> = args.collect();
    util::enter_root()?;
    let code = match task.as_str() {
        "extract-data" => extract_data::run()?,
        "smoke" => smoke::smoke(&rest)?,
        "client-scenarios" => smoke::client_scenarios(&rest)?,
        "pr-ready" => pr_ready::run(&rest)?,
        "check-all" => check_all::run(&rest)?,
        "window-screenshot" => window::screenshot(&rest)?,
        "window-bounds" => window::bounds()?,
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            0
        }
        other => bail!("unknown task `{other}`\n{USAGE}"),
    };
    std::process::exit(code);
}

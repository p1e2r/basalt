//! Harness registry (scenarios, fuzz harnesses, scripts) with the source paths each can be affected by,
//! `--affected` selection from a git diff, and the result cache (`target/check/cache.json`).
//!
//! Every harness declares globs of repo paths whose change can alter its result. `basalt-check
//! --affected` runs only harnesses whose declared paths changed relative to a base ref; the cache
//! additionally skips harnesses that already passed for identical (id, seed, n, content hash of the
//! declared paths, data version, vanilla/no-vanilla).
//!
//! The client agent registers its group by adding entries to [`EXTRA`] (group "client").
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HKind { Scenario, Fuzz, Script }

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: String,
    pub kind: HKind,
    pub group: String,
    pub paths: Vec<String>,
    pub desc: String,
}

/// Paths every Basalt-server scenario / fuzz harness depends on (bot, protocol, data, world, server hub).
pub const SERVER_COMMON: &[&str] = &[
    "crates/check/src/{bot,runner,query,proto,world,physics,server,lib}.rs",
    "crates/check/Cargo.toml",
    "crates/protocol/**", "crates/data/**", "crates/nbt/**", "crates/world/**",
    "crates/server/src/{main,conn,registry,gen}.rs", "crates/server/src/game.rs", "crates/server/Cargo.toml",
    "Cargo.toml", "Cargo.lock",
];

/// Entries registered by other agents (e.g. the client oracle's group "client"): `(id, group, paths, desc)`.
pub const EXTRA: &[(&str, &str, &[&str], &str)] = &[];

fn scenario_paths(id: &str) -> Vec<&'static str> {
    let mut p: Vec<&'static str> = SERVER_COMMON.to_vec();
    p.extend(["crates/check/src/scenarios/mod.rs", "crates/check/src/scenarios/util.rs"]);
    let by_prefix: &[(&str, &[&str])] = &[
        ("m1_", &["crates/check/src/scenarios/m1.rs", "crates/server/src/game/track.rs"]),
        ("m2_", &["crates/check/src/scenarios/m1.rs", "crates/server/src/game/track.rs", "crates/server/src/commands.rs", "crates/server/src/cmdparse.rs", "crates/server/src/game/cmd.rs"]),
        ("m3_", &["crates/check/src/scenarios/m3.rs", "crates/worldgen/**"]),
        ("m4_", &["crates/check/src/scenarios/m4.rs", "crates/game/**", "crates/server/src/game.rs", "crates/server/src/game/{inv,items,health,combat,env,explosion}.rs"]),
        ("m5_", &["crates/check/src/scenarios/m5.rs", "crates/game/**", "crates/server/src/game/{mob,ai,path,arrow,combat,explosion,spawn,health,track,items}.rs"]),
        ("m6_", &["crates/check/src/scenarios/m6.rs"]),
        ("env_", &["crates/check/src/scenarios/env.rs", "crates/game/**", "crates/server/src/game/{env,health}.rs"]),
        ("items_", &["crates/check/src/scenarios/m4.rs", "crates/game/**", "crates/server/src/game/{items,track}.rs"]),
    ];
    for (pre, ps) in by_prefix { if id.starts_with(pre) { p.extend(ps.iter()); } }
    p
}

fn fuzz_paths(id: &str) -> Vec<&'static str> {
    let mut p: Vec<&'static str> = SERVER_COMMON.to_vec();
    p.extend(["crates/check/src/fuzz/mod.rs", "crates/check/src/fuzz/arena.rs", "crates/check/known-divergences.txt"]);
    match id {
        "clicks" => p.extend(["crates/check/src/fuzz/clicks.rs", "crates/game/**", "crates/server/src/game/inv.rs", "crates/server/src/game/items.rs"]),
        "commands" => p.extend(["crates/check/src/fuzz/commands.rs", "crates/server/src/commands.rs", "crates/server/src/cmdparse.rs", "crates/server/src/game/cmd.rs", "crates/game/**", "crates/server/src/game/{mob,env,health}.rs"]),
        "dig" => p.extend(["crates/check/src/fuzz/dig.rs", "crates/game/**", "crates/server/src/game.rs", "crates/server/src/game/{items,health}.rs"]),
        "place" => p.extend(["crates/check/src/fuzz/place.rs", "crates/game/**", "crates/server/src/game/items.rs"]),
        _ => {}
    }
    p
}

pub fn all() -> Vec<Entry> {
    let mut v = Vec::new();
    for s in crate::scenarios::ALL {
        v.push(Entry { id: s.id.into(), kind: HKind::Scenario, group: s.group.into(), paths: scenario_paths(s.id).into_iter().map(String::from).collect(), desc: s.desc.into() });
    }
    for h in crate::fuzz::HARNESSES {
        v.push(Entry { id: format!("fuzz_{}", h.id), kind: HKind::Fuzz, group: "Fuzz".into(), paths: fuzz_paths(h.id).into_iter().map(String::from).collect(), desc: h.desc.into() });
    }
    v.push(Entry { id: "fuzz_worldgen".into(), kind: HKind::Script, group: "Fuzz".into(), paths: ["crates/worldgen/**", "crates/data/**", "crates/nbt/**", "tools/parity.sh", "crates/check/src/driver.rs"].map(String::from).to_vec(), desc: "worldgen parity vs vanilla over N seeds x several sites (tools/parity.sh)".into() });
    // the client oracle's block-meshing parity (crates/check/src/client): `basalt-check client-mesh`
    let mut client_paths: Vec<String> = crate::client::MESH_AFFECTED.iter().map(|s| s.to_string()).collect();
    client_paths.extend(["crates/check/src/client/**", "tools/oracle-mod/**", "crates/check/Cargo.toml", "crates/data/**", "crates/world/**", "Cargo.lock"].map(String::from));
    v.push(Entry { id: "client_mesh".into(), kind: HKind::Script, group: "client".into(), paths: client_paths, desc: "block-meshing parity of basalt-client vs the vanilla client oracle".into() });
    // scripted vanilla-client input via the oracle control channel (crates/check/src/client/{input_diff,click_oracle}.rs)
    for (id, desc) in [("client_input", "serverbound packet parity of basalt-client vs the real vanilla client for a scripted input sequence"),
        ("client_dig", "empty-hand digging (survival stone/dirt, creative) on the real client vs basalt-client, on Basalt and vanilla servers"), ("client_clicks", "container clicks of the real vanilla client (prediction + packets) vs the Basalt server's answers")] {
        let mut p: Vec<String> = crate::client::INPUT_AFFECTED.iter().map(|s| s.to_string()).collect();
        p.extend(["crates/check/src/client/{input_diff,click_oracle,oracle,mod}.rs", "crates/check/src/fuzz/clicks.rs", "tools/oracle-mod/**", "crates/check/Cargo.toml", "crates/protocol/**", "crates/data/**", "crates/world/**", "crates/server/Cargo.toml", "Cargo.lock"].map(String::from));
        v.push(Entry { id: id.into(), kind: HKind::Script, group: "client".into(), paths: p, desc: desc.into() });
    }
    // entity-model + first-person animation parity vs the real client's own bone/hand dumps (client/anim_diff.rs)
    v.push(Entry { id: "client_anim".into(), kind: HKind::Script, group: "client".into(), paths: crate::client::ANIM_AFFECTED.iter().map(|s| s.to_string()).collect(), desc: "model part poses (player/mob bone dumps) and first-person hand transforms of basalt-client vs the vanilla client oracle".into() });
    // client group (visual parity vs the real client, asset coverage, UI scenarios, benchmarks)
    let mut vis: Vec<String> = crate::client::RENDER_AFFECTED.iter().map(|s| s.to_string()).collect();
    vis.extend(["crates/check/src/client/{visual,imgcmp,oracle,mod}.rs", "crates/server/**", "crates/worldgen/**", "tools/oracle-mod/**", "tools/mc-window-*.swift", "xtask/src/window.rs", "tools/baselines/client-visual.json", "crates/check/Cargo.toml"].map(String::from));
    v.push(Entry { id: "client_visual".into(), kind: HKind::Script, group: "client".into(), paths: vis, desc: "visual parity: real vanilla client window vs basalt-client frame (per-tile SSIM + histogram) for 8 scenes".into() });
    v.push(Entry { id: "client_assets".into(), kind: HKind::Script, group: "client".into(), paths: crate::client::coverage::affected().into_iter().map(String::from).collect(), desc: "asset coverage: every block state and item renders (basalt-client --asset-grid)".into() });
    v.push(Entry { id: "client_perf".into(), kind: HKind::Script, group: "client".into(), paths: crate::client::perf::affected().into_iter().map(String::from).collect(), desc: "benchmarks at render distance 8/16/32 vs tools/baselines/client-perf.json (fail on >20% regression)".into() });
    for name in crate::client::coverage::UI_SCENARIOS {
        v.push(Entry { id: format!("client_ui_{name}"), kind: HKind::Script, group: "client".into(), paths: crate::client::coverage::affected().into_iter().map(String::from).collect(), desc: format!("basalt-client --scenario {name} against a fresh Basalt server") });
    }
    for (id, group, paths, desc) in EXTRA { v.push(Entry { id: (*id).into(), kind: HKind::Script, group: (*group).into(), paths: paths.iter().map(|s| s.to_string()).collect(), desc: (*desc).into() }); }
    v
}

// ------------------------------------------------------------------ globs

/// Expand `{a,b}` alternatives (one level, may repeat).
fn expand(p: &str) -> Vec<String> {
    if let (Some(a), Some(b)) = (p.find('{'), p.find('}')) {
        if a < b { return p[a + 1..b].split(',').flat_map(|alt| expand(&format!("{}{}{}", &p[..a], alt, &p[b + 1..]))).collect(); }
    }
    vec![p.to_string()]
}

fn seg_match(p: &[u8], s: &[u8]) -> bool {
    match (p.first(), s.first()) {
        (None, None) => true,
        (Some(b'*'), _) => seg_match(&p[1..], s) || (!s.is_empty() && seg_match(p, &s[1..])),
        (Some(a), Some(b)) if a == b => seg_match(&p[1..], &s[1..]),
        _ => false,
    }
}

fn path_match(p: &[&str], s: &[&str]) -> bool {
    match p.first() {
        None => s.is_empty(),
        Some(&"**") => (0..=s.len()).any(|k| path_match(&p[1..], &s[k..])),
        Some(seg) => !s.is_empty() && seg_match(seg.as_bytes(), s[0].as_bytes()) && path_match(&p[1..], &s[1..]),
    }
}

/// `crates/game/**`, `crates/server/src/game/{inv,items}.rs`, `*.toml` style globs.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let sp: Vec<&str> = path.split('/').collect();
    expand(pattern).iter().any(|pat| { let pp: Vec<&str> = pat.split('/').collect(); path_match(&pp, &sp) })
}

pub fn matches_any(patterns: &[String], path: &str) -> bool { patterns.iter().any(|p| glob_match(p, path)) }

// ------------------------------------------------------------------ git helpers

fn git(args: &[&str]) -> Option<String> {
    let o = Command::new("git").args(args).output().ok()?;
    if !o.status.success() { return None; }
    Some(String::from_utf8_lossy(&o.stdout).into_owned())
}

/// Default base: merge-base of HEAD with main (falls back to origin/main, then HEAD).
pub fn default_base() -> String {
    for r in ["main", "origin/main", "master"] {
        if let Some(s) = git(&["merge-base", "HEAD", r]) { return s.trim().to_string(); }
    }
    "HEAD".into()
}

/// Files changed since `base` (committed, staged, unstaged) plus untracked non-ignored files.
pub fn changed_files(base: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    if let Some(s) = git(&["diff", "--name-only", base]) { set.extend(s.lines().map(String::from)); }
    if let Some(s) = git(&["ls-files", "--others", "--exclude-standard"]) { set.extend(s.lines().map(String::from)); }
    set
}

pub fn tracked_files() -> Vec<String> {
    let mut v: BTreeSet<String> = BTreeSet::new();
    if let Some(s) = git(&["ls-files", "-co", "--exclude-standard"]) { v.extend(s.lines().filter(|l| Path::new(l).is_file()).map(String::from)); }
    v.into_iter().collect()
}

pub fn affected<'a>(entries: &'a [Entry], changed: &BTreeSet<String>) -> Vec<(&'a Entry, Vec<String>)> {
    entries.iter().filter_map(|e| {
        let hits: Vec<String> = changed.iter().filter(|f| matches_any(&e.paths, f)).cloned().collect();
        if hits.is_empty() { None } else { Some((e, hits)) }
    }).collect()
}

// ------------------------------------------------------------------ cache

/// Content hash of every file matching the entry's declared paths.
pub fn content_hash(e: &Entry, files: &[String]) -> String {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    for f in files.iter().filter(|f| matches_any(&e.paths, f)) {
        h.update(f.as_bytes()); h.update([0]);
        h.update(std::fs::read(f).unwrap_or_default());
        h.update([0]);
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>()[..16].to_string()
}

pub fn data_version() -> String {
    // the protocol/data version the tables were generated for, plus the generated report's size (changes with a new data extraction)
    let size = std::fs::metadata(crate::data_dir().join("reports/blocks.json")).map(|m| m.len()).unwrap_or(0);
    format!("{}/{}/{}", basalt_data::VERSION_NAME, basalt_data::WORLD_VERSION, size)
}

pub struct Cache { path: PathBuf, map: serde_json::Map<String, Value> }

impl Cache {
    pub fn load(out_dir: &Path) -> Cache {
        let path = out_dir.join("cache.json");
        let map = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default();
        Cache { path, map }
    }
    pub fn key(e: &Entry, seed: u64, n: usize, hash: &str, vanilla: bool) -> String {
        format!("{}|seed={seed}|n={n}|code={hash}|data={}|vanilla={}", e.id, data_version(), vanilla as u8)
    }
    pub fn hit(&self, key: &str) -> Option<&Value> { self.map.get(key).filter(|v| v["pass"] == json!(true)) }
    pub fn store_pass(&mut self, key: &str, summary: &str) {
        // drop older passes of the same harness (other code hashes) to keep the file small
        let id = key.split('|').next().unwrap_or("").to_string();
        self.map.retain(|k, _| !(k.split('|').next() == Some(id.as_str()) && k.split('|').nth(1) == key.split('|').nth(1) && k.split('|').nth(2) == key.split('|').nth(2)));
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        self.map.insert(key.to_string(), json!({ "pass": true, "ts": ts, "summary": summary }));
    }
    pub fn save(&self) {
        if let Some(d) = self.path.parent() { let _ = std::fs::create_dir_all(d); }
        let _ = std::fs::write(&self.path, serde_json::to_string_pretty(&Value::Object(self.map.clone())).unwrap_or_default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn globs() {
        assert!(glob_match("crates/game/**", "crates/game/src/lib.rs"));
        assert!(!glob_match("crates/game/**", "crates/gamex/src/lib.rs"));
        assert!(glob_match("crates/server/src/game/{inv,items}.rs", "crates/server/src/game/inv.rs"));
        assert!(!glob_match("crates/server/src/game/{inv,items}.rs", "crates/server/src/game/mob.rs"));
        assert!(glob_match("crates/check/src/{bot,runner}.rs", "crates/check/src/bot.rs"));
        assert!(glob_match("**/*.toml", "crates/check/Cargo.toml"));
        assert!(glob_match("Cargo.lock", "Cargo.lock"));
    }
}

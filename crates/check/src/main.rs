//! basalt-check: run scenario tests against Basalt and/or the vanilla test server.
//!
//! basalt-check [--basalt] [--vanilla HOST:PORT] [--vanilla-dir DIR] [--diff] [--filter S[,S..]]
//!              [--jobs N] [--out DIR] [--json FILE] [--list]
//! basalt-check client-mesh [--seed S] [--n N] ...   (see client/mesh_diff.rs)
//!
//! Default: all scenarios against fresh Basalt servers (target/release/basalt-server, one per
//! scenario). `--vanilla` also runs them against the vanilla server (sequentially; bots are op'd
//! via its console fifo in --vanilla-dir, default vanilla/srv). `--diff` = both + comparison.
use basalt_check::query::Kind;
use basalt_check::runner::{self, Outcome, RunOpts, Servers};
use basalt_check::scenarios::ALL;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    if let Some(code) = basalt_check::driver::try_main() { std::process::exit(code); }
    // subcommand: client-mesh (block-meshing parity vs the vanilla client oracle, see client/mesh_diff.rs)
    if std::env::args().nth(1).as_deref() == Some("client-mesh") {
        let a: Vec<String> = std::env::args().skip(2).collect();
        std::process::exit(basalt_check::client::mesh_diff::main(&a));
    }
    let mut basalt = false; let mut vanilla: Option<String> = None; let mut vdir = "vanilla/srv".to_string(); let mut diff = false;
    let mut filter: Vec<String> = Vec::new(); let mut jobs = 4usize; let mut out = "target/check".to_string(); let mut json: Option<String> = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--basalt" => basalt = true,
            "--vanilla" => vanilla = it.next(),
            "--vanilla-dir" => vdir = it.next().unwrap_or(vdir),
            "--diff" => diff = true,
            "--filter" => filter = it.next().unwrap_or_default().split(',').filter(|s| !s.is_empty()).map(String::from).collect(),
            "--jobs" => jobs = it.next().and_then(|j| j.parse().ok()).unwrap_or(4).max(1),
            "--out" => out = it.next().unwrap_or(out),
            "--json" => json = it.next(),
            "--probe" => {
                // debug: join ADDR as NAME and print what the bot knows each second
                let addr = it.next().unwrap_or("127.0.0.1:25566".into()); let name = it.next().unwrap_or("Probe".into());
                let bot = basalt_check::bot::Bot::connect(&addr, &name).unwrap_or_else(|e| { eprintln!("{e:#}"); std::process::exit(1) });
                if addr.ends_with(":25566") { if let Err(e) = basalt_check::server::Vanilla::new(std::path::Path::new(&vdir), &addr).op_online(&name) { eprintln!("op: {e:#}"); } bot.wait_until(std::time::Duration::from_secs(5), |s| s.op_level >= 2); }
                for c in it.by_ref() {
                    if let Some(t) = c.strip_prefix("wait:") { bot.wait_ticks(t.parse().unwrap_or(20)); continue; }
                    if let Some(a) = c.strip_prefix("action:") { let v: Vec<i32> = a.split(',').filter_map(|x| x.parse().ok()).collect(); bot.player_action(v[0], [v.get(1).copied().unwrap_or(0), v.get(2).copied().unwrap_or(0), v.get(3).copied().unwrap_or(0)], 0); continue; }
                    if let Some(a) = c.strip_prefix("held:") { bot.set_held(a.parse().unwrap_or(0)); continue; }
                    if let Some(a) = c.strip_prefix("tp:") { let v: Vec<f64> = a.split(',').filter_map(|x| x.parse().ok()).collect(); if let Err(e) = bot.tp([v[0], v[1], v[2]]) { println!("tp: {e}"); } continue; }
                    for l in bot.cmd_reply(&c).unwrap_or_default() { println!("reply: {:?} {:?} {:?}", l.plain, l.key, l.args); }
                }
                { let s = bot.st(); let t0 = s.damage.first().map(|d| d.0).unwrap_or_else(std::time::Instant::now); println!("teleports {:?} items {:?}", s.teleports.iter().map(|t| t.1).collect::<Vec<_>>(), s.entities.values().filter(|e| e.kind == "item").map(|e| (e.pos, e.item.as_ref().map(|i| (i.name(), i.count)))).collect::<Vec<_>>()); println!("damage {:?} flags {} inv {:?}", s.damage.iter().map(|d| (d.1.clone(), (d.0 - t0).as_millis() / 50)).collect::<Vec<_>>(), s.flags, s.inv.iter().enumerate().filter(|(_, i)| i.count > 0).map(|(k, i)| (k, i.name(), i.count)).collect::<Vec<_>>()); }
                for _ in 0..5 { { let s = bot.st(); println!("pos {:?} vel {:?} ground {} hp {} food {} air {} chunks {} ticks {} gm {}", s.body.pos, s.body.vel, s.body.on_ground, s.health, s.food, s.air, s.world.chunks.len(), s.ticks, s.gamemode); } std::thread::sleep(std::time::Duration::from_secs(1)); }
                return;
            }
            "--list" => { for s in ALL { println!("{:6} {:28} {:?} {:?}{}  {}", s.group, s.id, s.world, s.servers, if s.diff { " diff" } else { "" }, s.desc); } return; }
            other => { eprintln!("unknown argument {other}"); std::process::exit(2); }
        }
    }
    if diff && vanilla.is_none() { vanilla = Some("127.0.0.1:25566".into()); }
    if vanilla.is_none() || diff { basalt = true; }
    let run_basalt = basalt;
    let run_vanilla = vanilla.is_some();
    let opts = Arc::new(RunOpts { out_dir: out.clone().into(), vanilla: vanilla.clone().map(|a| (vdir.clone().into(), a)) });
    let selected: Vec<(usize, &'static runner::Spec)> = ALL.iter().enumerate()
        .filter(|(_, s)| filter.is_empty() || filter.iter().any(|f| s.id.contains(f.as_str()) || s.group.eq_ignore_ascii_case(f)))
        .collect();
    let t0 = Instant::now();
    let results: Arc<Mutex<Vec<Outcome>>> = Arc::new(Mutex::new(Vec::new()));
    let print = |o: &Outcome| {
        let first = o.failures.first().cloned().unwrap_or_default();
        eprintln!("{} {:8} {:28} {:5.1}s {}", if o.pass { "ok  " } else { "FAIL" }, o.server.name(), o.id, o.secs, first);
    };
    std::thread::scope(|sc| {
        // vanilla: one at a time against the shared server
        if run_vanilla {
            let (sel, opts, results) = (selected.clone(), opts.clone(), results.clone());
            sc.spawn(move || {
                for (i, s) in sel { if s.servers == Servers::BasaltOnly { continue; } let o = runner::run_spec(s, i, Kind::Vanilla, &opts); print(&o); results.lock().unwrap().push(o); }
            });
        }
        if run_basalt {
            let queue = Arc::new(Mutex::new(selected.iter().filter(|(_, s)| s.servers != Servers::VanillaOnly).cloned().collect::<Vec<_>>()));
            for _ in 0..jobs {
                let (q, opts, results) = (queue.clone(), opts.clone(), results.clone());
                sc.spawn(move || loop {
                    let Some((i, s)) = ({ let mut q = q.lock().unwrap(); if q.is_empty() { None } else { Some(q.remove(0)) } }) else { break };
                    let o = runner::run_spec(s, i, Kind::Basalt, &opts); print(&o); results.lock().unwrap().push(o);
                });
            }
        }
    });
    let mut results = Arc::try_unwrap(results).unwrap().into_inner().unwrap();
    if diff {
        for (_, s) in &selected {
            if !s.diff || s.servers != Servers::Both { continue; }
            let v = results.iter().find(|o| o.id == s.id && o.server == Kind::Vanilla).cloned();
            let b = results.iter().find(|o| o.id == s.id && o.server == Kind::Basalt).cloned();
            if let (Some(v), Some(b)) = (v, b) { let d = runner::diff(s, &v, &b); print(&d); results.push(d); }
        }
    }
    // table grouped by milestone
    let order = ["M1", "M2", "M3", "M4", "M5", "Env", "Items", "Diff"];
    let mut groups: BTreeMap<usize, Vec<&Outcome>> = BTreeMap::new();
    for o in &results { groups.entry(order.iter().position(|g| *g == o.group).unwrap_or(99)).or_default().push(o); }
    println!("\n{:6} {:30} {:8} {:7} {:>7}  details", "group", "scenario", "server", "result", "time");
    let mut fails = 0;
    for (_, mut v) in groups {
        v.sort_by(|a, b| a.id.cmp(&b.id).then(a.server.name().cmp(b.server.name())));
        for o in v {
            if !o.pass { fails += 1; }
            let det = if o.pass { o.notes.first().cloned().unwrap_or_default() } else { o.failures.join("; ") };
            let det: String = det.chars().take(140).collect();
            println!("{:6} {:30} {:8} {:7} {:>6.1}s  {}", o.group, o.id, o.server.name(), if o.pass { "pass" } else { "FAIL" }, o.secs, det);
        }
    }
    println!("\n{} results, {} failed, {:.1}s", results.len(), fails, t0.elapsed().as_secs_f64());
    if let Some(p) = json {
        let j = serde_json::Value::Array(results.iter().map(|o| o.to_json()).collect());
        if let Some(d) = std::path::Path::new(&p).parent() { let _ = std::fs::create_dir_all(d); }
        let _ = std::fs::write(&p, serde_json::to_string_pretty(&j).unwrap());
    }
    std::process::exit(if fails > 0 { 1 } else { 0 });
}

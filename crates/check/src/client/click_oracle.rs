//! `basalt-check client-clicks`: container-click oracle stage. Random click sequences (the clicks
//! fuzzer's own generator: all modes 0-6, drag phases, shift/number-key/double-click/drop) are
//! executed by the real vanilla client in its player inventory screen (through the screen's own
//! `slotClicked`, so the client predicts and serialises exactly what vanilla does) against a fresh
//! Basalt server. For every click it records
//!   * what the client sent: window, slot, button, mode and the changed-slot / carried hashes,
//!   * the client's own prediction of the slots right after the click (the next tick),
//!   * the slots after the server answered (4 ticks later; vanilla clients overwrite the prediction
//!     with the server's set_slot / set_content).
//!
//! A prediction that the Basalt server then overrides is a divergence between vanilla's click logic
//! and Basalt's (the same class the clicks fuzzer finds server-to-server, found here against the real
//! client). The record (target/check/client-clicks/<seed>-<n>.json) is plain JSON so other checks
//! can compare their own predictions to vanilla's.
//!
//! Scaling up: --n 2000 --seed S (each case ~5 s of client ticks; cases are independent, so run
//! several seeds in sequence). Chest screens: Basalt has no chests yet; to record chest clicks run the
//! oracle client against the vanilla server on 25566 by hand (control channel: connect, then use the
//! block, then the same `click` ops) -- the channel is screen-agnostic.
//!
//! usage: basalt-check client-clicks [--seed S] [--n N] [--oracle-dir vanilla/oracle] [--ctl-port 25789]
//!          [--no-launch] [--out target/check] [--show K]
use super::input_diff::{read_log, use_port_range, Pkt};
use super::oracle::{Ctl, Oracle, DEFAULT_CTL_PORT};
use crate::fuzz::{clicks, Rng};
use anyhow::{bail, Context, Result};
use basalt_protocol::Reader;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

pub struct Opts {
    pub seed: u64,
    pub n: usize,
    pub oracle_dir: PathBuf,
    pub ctl_port: u16,
    pub launch: bool,
    pub out: PathBuf,
    pub show: usize,
}

pub fn parse_args(args: &[String]) -> Result<Opts> {
    let mut o = Opts {
        seed: 1,
        n: 30,
        oracle_dir: "vanilla/oracle".into(),
        ctl_port: DEFAULT_CTL_PORT,
        launch: true,
        out: "target/check".into(),
        show: 4,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut v = || {
            it.next()
                .cloned()
                .with_context(|| format!("{a} needs a value"))
        };
        match a.as_str() {
            "--seed" => o.seed = v()?.parse()?,
            "--n" => o.n = v()?.parse()?,
            "--oracle-dir" => o.oracle_dir = v()?.into(),
            "--ctl-port" => o.ctl_port = v()?.parse()?,
            "--no-launch" => o.launch = false,
            "--out" => o.out = v()?.into(),
            "--show" => o.show = v()?.parse()?,
            other => bail!("client-clicks: unknown argument {other}"),
        }
    }
    Ok(o)
}

/// Cases without a crafting table (the table screen needs a placed block; the generator's table cases are skipped).
pub fn cases(seed: u64, n: usize) -> Vec<Value> {
    let mut v = Vec::new();
    let mut i = 0usize;
    while v.len() < n && i < n * 8 {
        let mut r = Rng::for_case(seed, i);
        let c = (clicks::HARNESS.gen)(&mut r, i);
        i += 1;
        if c["table"].as_bool() != Some(true) {
            v.push(c);
        }
    }
    v
}

fn w(t: u32) -> Value {
    json!({"op": "wait", "ticks": t})
}

/// Ops for one case: setup, then per click: click, query after 1 tick (prediction), query after 5 (settled).
fn case_ops(i: usize, case: &Value) -> Vec<Value> {
    let mut s = vec![
        json!({"op": "mark", "label": format!("case{i}")}),
        json!({"op": "inventory", "action": "close"}),
        w(2),
        json!({"op": "command", "text": "clear"}),
        json!({"op": "command", "text": "kill @e[type=item]"}),
        w(6),
    ];
    for e in case["inv"].as_array().into_iter().flatten() {
        s.push(json!({"op": "command", "text": format!("give @s minecraft:{} {}", e[0].as_str().unwrap_or("dirt"), e[1])}));
    }
    s.extend([
        w(10),
        json!({"op": "inventory", "action": "open"}),
        w(4),
        json!({"op": "click", "slot": -999, "button": 0, "mode": 0}),
        w(4),
        json!({"op": "query"}),
        json!({"op": "mark", "label": format!("case{i}-clicks")}),
    ]);
    for c in case["clicks"].as_array().into_iter().flatten() {
        s.push(json!({"op": "click", "slot": c[0], "button": c[1], "mode": c[2]}));
        s.push(w(1));
        s.push(json!({"op": "query"}));
        s.push(w(4));
        s.push(json!({"op": "query"}));
    }
    s.extend([
        json!({"op": "inventory", "action": "close"}),
        w(2),
        json!({"op": "mark", "label": format!("case{i}-end")}),
    ]);
    s
}

fn slots_of(q: &Value) -> Vec<String> {
    q["menu"]["slots"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| {
            if s["count"].as_i64().unwrap_or(0) == 0 {
                "air".to_string()
            } else {
                format!(
                    "{}x{}",
                    s["id"]
                        .as_str()
                        .unwrap_or("?")
                        .trim_start_matches("minecraft:"),
                    s["count"]
                )
            }
        })
        .collect()
}
fn carried_of(q: &Value) -> String {
    let c = &q["menu"]["carried"];
    if c["count"].as_i64().unwrap_or(0) == 0 {
        "air".into()
    } else {
        format!(
            "{}x{}",
            c["id"]
                .as_str()
                .unwrap_or("?")
                .trim_start_matches("minecraft:"),
            c["count"]
        )
    }
}

/// HashedStack: present bool, item varint, count varint, added components (type + int hash), removed component types.
fn hashed(r: &mut Reader) -> Result<String> {
    if !r.bool()? {
        return Ok("empty".into());
    }
    let item = r.varint()?;
    let count = r.varint()?;
    let added = r.varint()?;
    let mut comps = Vec::new();
    for _ in 0..added {
        let t = r.varint()?;
        let h = r.i32()?;
        comps.push(format!("{t}:{h:08x}"));
    }
    let removed = r.varint()?;
    for _ in 0..removed {
        comps.push(format!("-{}", r.varint()?));
    }
    Ok(format!(
        "{item}x{count}{}",
        if comps.is_empty() {
            String::new()
        } else {
            format!("[{}]", comps.join(","))
        }
    ))
}

/// The container_click packets of a case, decoded.
fn sent_clicks(pk: &[Pkt]) -> Vec<Value> {
    pk.iter().filter(|p| p.name == "container_click").filter_map(|p| {
        let mut r = Reader::new(&p.body);
        let (win, state, slot, btn, mode) = (r.varint().ok()?, r.varint().ok()?, r.i16().ok()?, r.i8().ok()?, r.varint().ok()?);
        let n = r.varint().ok()?;
        let mut changed = Vec::new();
        for _ in 0..n { let s = r.i16().ok()?; changed.push(json!([s, hashed(&mut r).ok()?])); }
        let carried = hashed(&mut r).ok()?;
        Some(json!({"window": win, "state": state, "slot": slot, "button": btn, "mode": mode, "changed": changed, "carried": carried, "tick": p.tick}))
    }).collect()
}

pub fn run(o: &Opts) -> Result<usize> {
    use_port_range();
    let cs = cases(o.seed, o.n);
    let work = o.out.join("client-clicks");
    std::fs::create_dir_all(&work)?;
    let tag = format!("{}-{}", o.seed, o.n);
    let srv = super::input_diff::start_server(&o.out, &format!("clicks-{tag}"))?;
    let _oracle = Oracle::connect_port(&o.oracle_dir, o.launch, o.ctl_port)?;
    let mut ctl = Ctl::connect(o.ctl_port, Duration::from_secs(20))?;
    let log = std::fs::canonicalize(&work)
        .unwrap_or(work.clone())
        .join(format!("{tag}-vanilla.jsonl"));
    ctl.call(
        json!({"op": "log", "file": log.to_string_lossy()}),
        Duration::from_secs(10),
    )?;
    ctl.call(
        json!({"op": "connect", "addr": format!("127.0.0.1:{}", srv.port)}),
        Duration::from_secs(10),
    )?;
    ctl.call(json!({"op": "wait_join"}), Duration::from_secs(60))?;
    ctl.call(json!({"op": "wait", "ticks": 20}), Duration::from_secs(10))?;
    eprintln!(
        "client-clicks: {} cases (seed {}), {} clicks",
        cs.len(),
        o.seed,
        cs.iter()
            .map(|c| c["clicks"].as_array().map(|a| a.len()).unwrap_or(0))
            .sum::<usize>()
    );
    let mut records = Vec::new();
    let (mut total, mut bad) = (0usize, 0usize);
    let mut classes: BTreeMap<String, (usize, String)> = BTreeMap::new();
    for (i, case) in cs.iter().enumerate() {
        let ops = case_ops(i, case);
        let res = ctl.run(&ops, Duration::from_secs(60))?;
        for (k, r) in res.iter().enumerate() {
            if r["ok"] != json!(true) {
                bail!(
                    "case {i}: command {} failed: {}",
                    ops[k],
                    r["error"]
                        .as_str()
                        .unwrap_or("?")
                        .lines()
                        .next()
                        .unwrap_or("")
                );
            }
        }
        let queries: Vec<&Value> = res
            .iter()
            .zip(&ops)
            .filter(|(_, o)| o["op"] == "query")
            .map(|(r, _)| r)
            .collect();
        // queries: [init, (pred, settled) per click]
        let init = queries[0];
        let clicks_v = case["clicks"].as_array().cloned().unwrap_or_default();
        let mut steps = Vec::new();
        for (k, c) in clicks_v.iter().enumerate() {
            let (pred, settled) = (queries[1 + 2 * k], queries[2 + 2 * k]);
            let (sp, ss) = (slots_of(pred), slots_of(settled));
            let (cp, cs_) = (carried_of(pred), carried_of(settled));
            let mode = c[2].as_i64().unwrap_or(-1);
            let drag_mid = mode == 5 && (c[1].as_i64().unwrap_or(0) & 3) < 2; // the server only answers a drag at its end
            total += 1;
            let diff: Vec<String> = (0..sp.len().min(ss.len()))
                .filter(|&j| sp[j] != ss[j])
                .take(4)
                .map(|j| format!("slot {j}: predicted {} server {}", sp[j], ss[j]))
                .collect();
            let mismatch = !drag_mid && (!diff.is_empty() || cp != cs_);
            if mismatch {
                bad += 1;
                let e = classes
                    .entry(format!("mode{mode}"))
                    .or_insert((0, String::new()));
                e.0 += 1;
                if e.1.is_empty() {
                    e.1 = format!(
                        "case {i} click {k} {c}: {} cursor predicted {cp} server {cs_}",
                        diff.join("; ")
                    );
                }
            }
            steps.push(json!({"click": c, "predicted": sp, "predicted_cursor": cp, "settled": ss, "settled_cursor": cs_, "mismatch": mismatch}));
        }
        ctl.run(&[json!({"op": "flush"})], Duration::from_secs(10))?;
        let pk =
            read_log(&log, &format!("case{i}-clicks"), &format!("case{i}-end")).unwrap_or_default();
        let sent = sent_clicks(&pk);
        records.push(json!({"case": case, "init": slots_of(init), "sent": sent, "steps": steps}));
    }
    let _ = ctl.call(json!({"op": "disconnect"}), Duration::from_secs(10));
    let file = work.join(format!("{tag}.json"));
    std::fs::write(
        &file,
        serde_json::to_vec_pretty(
            &json!({"seed": o.seed, "n": cs.len(), "oracle": "vanilla-26.2", "cases": records}),
        )?,
    )?;
    println!("record: {}", file.display());
    for (class, (n, ex)) in classes.iter().take(o.show.max(12)) {
        println!("MISMATCH {class}: {n} click(s); e.g. {ex}");
    }
    println!(
        "RESULT client_clicks cases={} clicks={total} prediction_overridden_by_server={bad}",
        cs.len()
    );
    drop(srv);
    Ok(bad)
}

pub fn main(args: &[String]) -> i32 {
    match parse_args(args).and_then(|o| run(&o)) {
        Ok(0) => 0,
        Ok(_) => 1,
        Err(e) => {
            eprintln!("client-clicks: {e:#}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cases_are_deterministic_and_skip_tables() {
        let (a, b) = (cases(3, 10), cases(3, 10));
        assert_eq!(a, b);
        assert!(a.iter().all(|c| c["table"] != json!(true)) && a.len() == 10);
    }
    #[test]
    fn ops_have_two_queries_per_click() {
        let case =
            json!({"table": false, "inv": [["dirt", 5]], "clicks": [[36, 0, 0], [-999, 0, 0]]});
        let ops = case_ops(0, &case);
        assert_eq!(ops.iter().filter(|o| o["op"] == "query").count(), 1 + 2 * 2);
    }
    #[test]
    fn hashed_stack_decodes() {
        let mut r = Reader::new(&[1, 0x37, 0x40, 0, 0]);
        assert_eq!(hashed(&mut r).unwrap(), "55x64");
        let mut r = Reader::new(&[0]);
        assert_eq!(hashed(&mut r).unwrap(), "empty");
    }
}

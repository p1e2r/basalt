//! `basalt-check client-water`: player movement in water, the real 26.2 client (oracle input
//! channel) vs basalt-client, both on the same fresh Basalt server (the client simulates its own
//! movement; the server only relays). Cases, each between marks: falling into a pool from 10 blocks
//! above the surface, holding Space from the bottom, sprint-swimming, climbing out at a ledge. The
//! serverbound move packets give each client's position per tick; the traces (relative to the mark
//! tick) are compared and the largest difference is reported per case and axis.
//!
//! Prints `CASE <name> max_dy=.. max_dh=.. first_bad_tick=..` and `RESULT client_water cases=.. failed=..`.
use super::input_diff::{self, read_log, Opts, Pkt};
use anyhow::Result;
use basalt_protocol::Reader;
use serde_json::{json, Value};

/// Pool: stone basin x 2000..2010, z 2000..2010, water 2001..2009 x 191..199 x 2001..2009
/// (surface at y 200), dry ground beyond the east wall at y 200.
fn setup() -> Vec<Value> {
    let c = |t: &str| json!({"op": "command", "text": t});
    vec![
        json!({"op": "wait_join"}),
        json!({"op": "wait", "ticks": 80}),
        c("gamemode survival"),
        c("effect clear @s"),
        c("fill 1998 189 1998 2014 215 2012 air"),
        c("fill 2000 190 2000 2010 199 2010 stone"),
        c("fill 2010 190 2000 2014 199 2010 stone"),
        c("fill 2001 191 2001 2009 199 2009 water"),
        json!({"op": "wait", "ticks": 20}),
    ]
}

/// (name, ops before the mark, ops measured, ticks measured)
fn cases() -> Vec<(&'static str, Vec<Value>, Vec<Value>)> {
    let c = |t: &str| json!({"op": "command", "text": t});
    let w = |t: u32| json!({"op": "wait", "ticks": t});
    vec![
        (
            "fall_10",
            vec![c("tp @s 2005.5 210 2005.5 0 0"), w(2)],
            vec![w(70)],
        ),
        (
            "swim_up",
            vec![c("tp @s 2005.5 191 2005.5 0 0"), w(30)],
            vec![
                json!({"op": "jump", "action": "press"}),
                w(60),
                json!({"op": "jump", "action": "release"}),
                w(20),
            ],
        ),
        (
            "sprint_swim",
            vec![c("tp @s 2002.5 195 2005.5 -90 0"), w(30)],
            vec![
                json!({"op": "sprint", "action": "press"}),
                json!({"op": "move", "forward": true}),
                w(25),
                json!({"op": "move", "forward": false}),
                json!({"op": "sprint", "action": "release"}),
                w(15),
            ],
        ),
        // from standing on the pool floor (a defined start: the two clients' scripts may differ by a
        // tick in when the keys go down): swim up against the east wall and climb out onto the ground
        (
            "ledge_exit",
            vec![c("tp @s 2008.5 191 2005.5 -90 0"), w(40)],
            vec![
                json!({"op": "move", "forward": true}),
                json!({"op": "jump", "action": "press"}),
                w(75),
                json!({"op": "jump", "action": "release"}),
                json!({"op": "move", "forward": false}),
                w(10),
            ],
        ),
    ]
}

fn script() -> Vec<Value> {
    let mut ops = setup();
    for (name, pre, measured) in cases() {
        ops.extend(pre);
        ops.push(json!({"op": "mark", "label": format!("{name}_begin")}));
        ops.extend(measured);
        ops.push(json!({"op": "mark", "label": format!("{name}_end")}));
    }
    ops.push(json!({"op": "flush"}));
    ops
}

/// Per-tick position from the move packets, held between packets, relative to the first tick.
fn trace(pk: &[Pkt]) -> Vec<[f64; 3]> {
    let t0 = pk.first().map(|p| p.tick).unwrap_or(0);
    let mut out: Vec<[f64; 3]> = Vec::new();
    let mut last: Option<[f64; 3]> = None;
    for p in pk {
        if !(p.name == "move_player_pos" || p.name == "move_player_pos_rot") {
            continue;
        }
        let mut r = Reader::new(&p.body);
        let (Ok(x), Ok(y), Ok(z)) = (r.f64(), r.f64(), r.f64()) else {
            continue;
        };
        let t = (p.tick - t0).max(0) as usize;
        while out.len() < t {
            if let Some(l) = last {
                out.push(l);
            } else {
                break;
            }
        }
        if out.len() == t {
            out.push([x, y, z]);
        } else if let Some(o) = out.get_mut(t) {
            *o = [x, y, z];
        }
        last = Some([x, y, z]);
    }
    out
}

pub fn run(o: &Opts) -> Result<usize> {
    input_diff::use_port_range();
    let work = o.out.join("client-water");
    std::fs::create_dir_all(&work)?;
    let ops = script();
    let srv = input_diff::start_server(&o.out, "water")?;
    let addr = format!("127.0.0.1:{}", srv.port);
    let vlog = input_diff::run_vanilla_at(o, &ops, &work, "water", Some(&addr))?;
    let blog = input_diff::run_basalt_at(o, &ops, &work, "water", Some(&addr))?;
    drop(srv);
    let mut failed = 0;
    let mut n = 0;
    let mut report = Vec::new();
    for (name, _, _) in cases() {
        let (b, e) = (format!("{name}_begin"), format!("{name}_end"));
        let (v, bb) = (
            trace(&read_log(&vlog, &b, &e)?),
            trace(&read_log(&blog, &b, &e)?),
        );
        n += 1;
        // each trace starts at that client's first move packet, which can be a tick apart: align
        // with the shift (-2..2) that fits best
        let fit = |sh: i64| -> f64 {
            let mut m = 0.0f64;
            for t in 2..v.len() as i64 {
                let u = t + sh;
                if u < 0 || u >= bb.len() as i64 {
                    continue;
                }
                let (a, b) = (v[t as usize], bb[u as usize]);
                m = m.max(
                    (a[1] - b[1]).abs()
                        + ((a[0] - v[0][0]) - (b[0] - bb[0][0])).abs()
                        + ((a[2] - v[0][2]) - (b[2] - bb[0][2])).abs(),
                );
            }
            m
        };
        let shift = (-2..=2)
            .min_by(|a, b| fit(*a).partial_cmp(&fit(*b)).unwrap())
            .unwrap_or(0);
        let bb: Vec<[f64; 3]> = if shift >= 0 {
            bb[shift as usize..].to_vec()
        } else {
            std::iter::repeat_n(bb[0], (-shift) as usize)
                .chain(bb.iter().copied())
                .collect()
        };
        let len = v.len().min(bb.len());
        let (mut dy, mut dh, mut first) = (0.0f64, 0.0f64, None);
        // the first two ticks are the teleport settling
        for t in 2..len {
            // each vanilla sample against Basalt's at t-1..t+1: the two scripts may press or release a
            // key a tick apart, which shifts everything after it by a tick without being a physics bug
            let at = |u: usize| {
                let y = (v[t][1] - bb[u][1]).abs();
                // horizontal: relative to each client's start (the teleports land at the same spot anyway)
                let h = ((v[t][0] - v[0][0]) - (bb[u][0] - bb[0][0]))
                    .hypot((v[t][2] - v[0][2]) - (bb[u][2] - bb[0][2]));
                (y, h)
            };
            let (y, h) = [t.saturating_sub(1), t, (t + 1).min(len - 1)]
                .into_iter()
                .map(at)
                .min_by(|a, b| (a.0 + a.1).partial_cmp(&(b.0 + b.1)).unwrap())
                .unwrap();
            dy = dy.max(y);
            dh = dh.max(h);
            if first.is_none() && (y > 0.05 || h > 0.1) {
                first = Some(t);
            }
        }
        let bad = first.is_some();
        if bad {
            failed += 1;
        }
        println!(
            "CASE {name:12} {} max_dy={dy:.3} max_dh={dh:.3} first_bad_tick={} ticks={len}",
            if bad { "FAIL" } else { "pass" },
            first.map(|t| t.to_string()).unwrap_or("-".into())
        );
        let sample = |tr: &[[f64; 3]]| {
            tr.iter()
                .step_by(5)
                .map(|p| format!("{:.2}", p[1]))
                .collect::<Vec<_>>()
                .join(" ")
        };
        report.push(json!({"case": name, "vanilla_y": sample(&v), "basalt_y": sample(&bb), "max_dy": dy, "max_dh": dh}));
    }
    std::fs::write(
        work.join("water.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    println!("traces in {}", work.join("water.json").display());
    println!("RESULT client_water cases={n} failed={failed}");
    Ok(failed)
}

pub fn main(args: &[String]) -> i32 {
    let o = match input_diff::parse_args(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("client-water: {e:#}");
            return 2;
        }
    };
    match run(&o) {
        Ok(f) => (f > 0) as i32,
        Err(e) => {
            eprintln!("client_water: {e:#}");
            2
        }
    }
}

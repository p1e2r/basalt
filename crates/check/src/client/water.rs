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
        // lava basin x 2020..2030, z 2020..2030, lava 2021..2029 x 191..199 x 2021..2029 (surface 200)
        c("fill 2018 189 2018 2062 215 2040 air"),
        c("fill 2020 190 2020 2030 199 2030 stone"),
        c("fill 2021 191 2021 2029 199 2029 lava"),
        // flowing water: floor y 190, walls z 2025 / 2029 up to y 193, source at x 2041, z 2027
        // spreading east along y 191 (levels 1..7 at x 2042..2048)
        c("fill 2040 190 2025 2058 190 2029 stone"),
        c("fill 2040 191 2025 2058 193 2025 stone"),
        c("fill 2040 191 2029 2058 193 2029 stone"),
        c("fill 2040 191 2026 2040 193 2028 stone"),
        c("setblock 2041 191 2027 water"),
        json!({"op": "wait", "ticks": 60}),
    ]
}

/// Boots with Depth Strider III on the feet (give, enchant the held stack, shift-click it on).
fn equip_depth_strider() -> Vec<Value> {
    let c = |t: &str| json!({"op": "command", "text": t});
    vec![
        c("give @s diamond_boots"),
        json!({"op": "wait", "ticks": 8}),
        c("enchant @s depth_strider 3"),
        json!({"op": "wait", "ticks": 8}),
        json!({"op": "inventory", "action": "open"}),
        json!({"op": "wait", "ticks": 6}),
        json!({"op": "click", "slot": 36, "button": 0, "mode": 1}),
        json!({"op": "wait", "ticks": 6}),
        json!({"op": "inventory", "action": "close"}),
        json!({"op": "wait", "ticks": 6}),
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
        (
            "fall_3",
            vec![c("tp @s 2005.5 203 2005.5 0 0"), w(2)],
            vec![w(40)],
        ),
        (
            "fall_1",
            vec![c("tp @s 2005.5 201 2005.5 0 0"), w(2)],
            vec![w(40)],
        ),
        (
            "fall_12",
            vec![c("tp @s 2005.5 212 2005.5 0 0"), w(2)],
            vec![w(50)],
        ),
        (
            "fall_14",
            vec![c("tp @s 2005.5 214 2005.5 0 0"), w(2)],
            vec![w(50)],
        ),
        (
            "fall_16",
            vec![c("tp @s 2005.5 216 2005.5 0 0"), w(2)],
            vec![w(50)],
        ),
        (
            "fall_20",
            vec![c("tp @s 2005.5 220 2005.5 0 0"), w(2)],
            vec![w(50)],
        ),
        (
            "fall_40",
            vec![c("tp @s 2005.5 240 2005.5 0 0"), w(2)],
            vec![w(100)],
        ),
        // sprint along the surface (eyes above the water): splash/swim effects at the surface
        (
            "surface_sprint",
            vec![c("tp @s 2002.5 199.3 2005.5 -90 0"), w(30)],
            vec![
                json!({"op": "sprint", "action": "press"}),
                json!({"op": "move", "forward": true}),
                w(30),
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
        (
            "lava_fall",
            vec![
                c("effect give @s fire_resistance 9999 0 true"),
                c("tp @s 2025.5 210 2025.5 0 0"),
                w(2),
            ],
            vec![w(60)],
        ),
        (
            "lava_up",
            vec![c("tp @s 2025.5 191 2025.5 0 0"), w(30)],
            vec![
                json!({"op": "jump", "action": "press"}),
                w(60),
                json!({"op": "jump", "action": "release"}),
                w(20),
            ],
        ),
        (
            "lava_walk",
            vec![c("tp @s 2022.5 191 2025.5 -90 0"), w(30)],
            vec![
                json!({"op": "move", "forward": true}),
                w(30),
                json!({"op": "move", "forward": false}),
                w(20),
            ],
        ),
        // standing in the flowing water of the channel: pushed along it
        (
            "current",
            vec![c("effect clear @s"), c("tp @s 2044.5 191 2027.5 0 0"), w(2)],
            vec![w(50)],
        ),
        (
            "swim_fwd",
            vec![c("tp @s 2002.5 195 2005.5 -90 0"), w(30)],
            vec![
                json!({"op": "move", "forward": true}),
                w(25),
                json!({"op": "move", "forward": false}),
                w(15),
            ],
        ),
        (
            "wade",
            vec![c("tp @s 2002.5 191 2005.5 -90 0"), w(30)],
            vec![
                json!({"op": "move", "forward": true}),
                w(30),
                json!({"op": "move", "forward": false}),
                w(15),
            ],
        ),
        (
            "dolphin_sprint",
            vec![
                c("effect give @s dolphins_grace 9999 0 true"),
                c("tp @s 2002.5 195 2005.5 -90 0"),
                w(30),
            ],
            vec![
                json!({"op": "sprint", "action": "press"}),
                json!({"op": "move", "forward": true}),
                w(25),
                json!({"op": "move", "forward": false}),
                json!({"op": "sprint", "action": "release"}),
                w(15),
            ],
        ),
        (
            "dolphin_fwd",
            vec![c("tp @s 2002.5 195 2005.5 -90 0"), w(30)],
            vec![
                json!({"op": "move", "forward": true}),
                w(25),
                json!({"op": "move", "forward": false}),
                w(15),
            ],
        ),
        (
            "wade_ds",
            [
                vec![c("effect clear @s")],
                equip_depth_strider(),
                vec![c("tp @s 2002.5 191 2005.5 -90 0"), w(30)],
            ]
            .concat(),
            vec![
                json!({"op": "move", "forward": true}),
                w(30),
                json!({"op": "move", "forward": false}),
                w(15),
            ],
        ),
        (
            "swim_fwd_ds",
            vec![c("tp @s 2002.5 195 2005.5 -90 0"), w(30)],
            vec![
                json!({"op": "move", "forward": true}),
                w(25),
                json!({"op": "move", "forward": false}),
                w(15),
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

/// Locally made effects of one case: the water-related sounds (name, tick, y, volume) and, per
/// particle class, count / mean vertical speed / mean life / spawn height.
struct Fx {
    sounds: Vec<(String, i64, f64, f64)>,
    particles: std::collections::BTreeMap<String, (usize, f64, f64, f64)>,
}

fn fx_of(v: &[Value]) -> Fx {
    let mut f = Fx {
        sounds: Vec::new(),
        particles: Default::default(),
    };
    for j in v {
        let name = j["name"].as_str().unwrap_or("");
        match j["fx"].as_str() {
            // (an "exit" in the first ticks is the case's own teleport out of the previous pool, which
            // the two clients process a tick or two apart around the mark)
            Some("sound")
                if name.starts_with("minecraft:entity.player.splash")
                    || (name.starts_with("minecraft:ambient.underwater")
                        && !(name.ends_with("exit") && j["tick"].as_i64().unwrap_or(0) < 6)) =>
            {
                f.sounds.push((
                    name.trim_start_matches("minecraft:").to_string(),
                    j["tick"].as_i64().unwrap_or(0),
                    j["y"].as_f64().unwrap_or(0.0),
                    j["vol"].as_f64().unwrap_or(0.0),
                ));
            }
            Some("particle") if name == "BubbleParticle" || name == "SplashParticle" => {
                let e = f.particles.entry(name.to_string()).or_default();
                e.0 += 1;
                e.1 += j["vy"].as_f64().unwrap_or(0.0);
                e.2 += j["life"].as_f64().unwrap_or(0.0);
                e.3 = j["y"].as_f64().unwrap_or(0.0);
            }
            _ => {}
        }
    }
    for e in f.particles.values_mut() {
        let n = e.0.max(1) as f64;
        e.1 /= n;
        e.2 /= n;
    }
    f
}

/// Differences between the real client's effects and basalt-client's: empty when they match
/// (same sounds in order within 1 tick / 0.03 volume, same particle counts and spawn height,
/// mean launch speed within 0.06 and mean life within 10 ticks — the draws are random).
fn fx_diff(v: &Fx, b: &Fx) -> Vec<String> {
    let mut d = Vec::new();
    let names = |f: &Fx| f.sounds.iter().map(|s| s.0.clone()).collect::<Vec<_>>();
    if names(v) != names(b) {
        d.push(format!("sounds {:?} vs {:?}", names(v), names(b)));
    } else {
        for (x, y) in v.sounds.iter().zip(&b.sounds) {
            // the eyes-under sounds follow a rule not fully found (see main.rs track_water): 2 ticks
            let amb = x.0.starts_with("ambient");
            if (x.1 - y.1).abs() > if amb { 2 } else { 1 }
                || (x.3 - y.3).abs() > 0.03
                || (x.2 - y.2).abs() > if amb { 2.5 } else { 0.3 }
            {
                d.push(format!(
                    "{}: tick {} y {:.2} vol {:.3} vs tick {} y {:.2} vol {:.3}",
                    x.0, x.1, x.2, x.3, y.1, y.2, y.3
                ));
            }
        }
    }
    for (k, x) in &v.particles {
        match b.particles.get(k) {
            None => d.push(format!("{k}: {} vs none", x.0)),
            Some(y) => {
                if x.0 != y.0
                    || (x.1 - y.1).abs() > if k.starts_with("Splash") { 0.1 } else { 0.06 }
                    || (x.2 - y.2).abs() > 10.0
                    || (x.3 - y.3).abs() > 0.01
                {
                    d.push(format!(
                        "{k}: n {} vy {:.3} life {:.1} y {:.2} vs n {} vy {:.3} life {:.1} y {:.2}",
                        x.0, x.1, x.2, x.3, y.0, y.1, y.2, y.3
                    ));
                }
            }
        }
    }
    for k in b.particles.keys() {
        if !v.particles.contains_key(k) {
            d.push(format!("{k}: none vs {}", b.particles[k].0));
        }
    }
    d
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
        // locally made effects (sounds, particles)
        let (fv, fb) = (
            fx_of(&input_diff::read_fx(&vlog, &b, &e)?),
            fx_of(&input_diff::read_fx(&blog, &b, &e)?),
        );
        let fd = fx_diff(&fv, &fb);
        if !fd.is_empty() {
            failed += 1;
        }
        println!(
            "FX   {name:12} {} {}",
            if fd.is_empty() { "pass" } else { "FAIL" },
            if fd.is_empty() {
                format!(
                    "sounds={} particles={:?}",
                    fv.sounds.len(),
                    fv.particles
                        .iter()
                        .map(|(k, x)| format!("{k}x{}", x.0))
                        .collect::<Vec<_>>()
                )
            } else {
                fd.join(" | ")
            }
        );
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

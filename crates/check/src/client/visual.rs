//! `basalt-check client-visual`: visual parity of basalt-client against the real vanilla client.
//!
//! For each scene the same server (a fresh in-memory Basalt server: flat world or seed 12345
//! terrain), the same position / rotation / time of day / render distance / GUI scale are used by
//! (a) the real vanilla 26.2 client, driven through the oracle mod's control channel, whose window
//! is captured with `cargo xtask window-screenshot` (only the Minecraft window; the title bar is
//! cropped), and (b) basalt-client's offscreen frame (`--script` mode, ops `config`, `gui`,
//! `screenshot`) rendered at the size the vanilla capture came out at. The two images are
//! resized to a common size and compared on a grid of tiles: SSIM of the luma plus the distance
//! between per-channel colour histograms. Per-scene limits live in tools/baselines/client-visual.json
//! (numbers only; `--update-baseline` rewrites them from the measured values with a margin).
//! Captures, diff images and side-by-side PNGs go to target/check/client-visual/<scene>/.
//!
//! usage: basalt-check client-visual [--scene a,b] [--update-baseline] [--oracle-dir vanilla/oracle-visual]
//!          [--ctl-port N] [--client PATH] [--jar PATH] [--out target/check] [--win WxH]
use super::imgcmp::{self, Img};
use super::input_diff::use_port_range;
use super::oracle::{self, Ctl, Oracle};
use crate::server::BasaltServer;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub const BASELINE: &str = "tools/baselines/client-visual.json";
const COLS: usize = 8;
const ROWS: usize = 6;
const CMP: (usize, usize) = (384, 216);

pub struct Opts {
    pub oracle_dir: PathBuf,
    pub ctl_port: u16,
    pub client: PathBuf,
    pub jar: PathBuf,
    pub out: PathBuf,
    pub scenes: Vec<String>,
    pub update: bool,
    pub win: (u32, u32),
}

pub fn parse_args(args: &[String]) -> Result<Opts> {
    let mut o = Opts {
        oracle_dir: "vanilla/oracle-visual".into(),
        ctl_port: oracle::ctl_port(),
        client: super::client_bin(),
        jar: basalt_paths::client_jar(),
        out: "target/check".into(),
        scenes: vec![],
        update: false,
        win: (640, 360),
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut v = || {
            it.next()
                .cloned()
                .with_context(|| format!("{a} needs a value"))
        };
        match a.as_str() {
            "--oracle-dir" => o.oracle_dir = v()?.into(),
            "--ctl-port" => o.ctl_port = v()?.parse()?,
            "--client" => o.client = v()?.into(),
            "--jar" => o.jar = v()?.into(),
            "--out" => o.out = v()?.into(),
            "--scene" | "--scenes" => o.scenes = v()?.split(',').map(String::from).collect(),
            "--update-baseline" => o.update = true,
            "--win" => {
                let s = v()?;
                let (w, h) = s.split_once('x').context("--win WxH")?;
                o.win = (w.parse()?, h.parse()?);
            }
            other => bail!("client-visual: unknown argument {other}"),
        }
    }
    Ok(o)
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum World {
    Flat,
    Terrain,
    None,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    /// the world with the HUD hidden (F1), flying at `pos`
    View,
    /// the survival inventory screen
    Inventory,
    /// the title screen with the panorama
    Title,
}

pub struct Scene {
    pub name: &'static str,
    pub world: World,
    pub kind: Kind,
    pub time: i64,
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub desc: &'static str,
    /// ticks to wait after the teleport (falling time)
    pub settle: u32,
}

/// Landmarks and rooms built in the flat world by the first client that joins (idempotent: both
/// clients may run them). The flat surface is y = -61 (grass), so blocks start at y = -60.
pub const FLAT_SETUP: &[&str] = &[
    // a wall, a log, wool and glass 10..14 blocks south of the spawn
    "fill 4 -60 10 8 -57 12 stone_bricks",
    "fill -2 -60 14 2 -58 14 red_wool",
    "setblock -7 -60 11 oak_log",
    "setblock -7 -59 11 oak_log",
    "setblock -7 -58 11 oak_log",
    "fill -4 -60 9 -2 -58 9 blue_stained_glass",
    "setblock 0 -60 8 glowstone",
    // an 8x8x10 pool sunk into the ground (water sources) at x 14..21, z 14..21
    "fill 14 -61 14 21 -52 21 water",
    // a closed stone room (9x7x9 inside) with two light sources
    "fill 30 -60 30 40 -52 40 stone",
    "fill 31 -59 31 39 -53 39 air",
    "setblock 33 -53 33 glowstone",
    "setblock 37 -59 36 torch",
    // a raised grass mound with plants on top, for the close-up on grass sides (z-fighting check)
    "fill -12 -60 -6 -10 -59 -5 grass_block",
    "setblock -12 -58 -6 short_grass",
    "setblock -11 -58 -6 fern",
];

pub fn scenes() -> Vec<Scene> {
    let s = |name, world, kind, time, pos, yaw, pitch, desc| Scene {
        name,
        world,
        kind,
        time,
        pos,
        yaw,
        pitch,
        desc,
        settle: if pos[1] > 100.0 {
            260
        } else if pos[1] > -55.0 {
            160
        } else {
            40
        },
    };
    vec![
        s(
            "flat_noon",
            World::Flat,
            Kind::View,
            6000,
            [0.5, -60.0, 0.5],
            0.0,
            0.0,
            "flat world at noon looking at the horizon, landmarks 10 blocks ahead",
        ),
        s(
            "flat_sunset",
            World::Flat,
            Kind::View,
            12800,
            [0.5, -60.0, 0.5],
            70.0,
            2.0,
            "sunset towards the west",
        ),
        s(
            "flat_night",
            World::Flat,
            Kind::View,
            18000,
            [0.5, -60.0, 0.5],
            20.0,
            -35.0,
            "midnight, looking up (moon, stars)",
        ),
        s(
            "grass_sides",
            World::Flat,
            Kind::View,
            6000,
            [-8.5, -60.0, -3.5],
            120.0,
            12.0,
            "close up on grass block sides at a grazing angle (side overlay must not z-fight), plants on top",
        ),
        s(
            "underwater",
            World::Flat,
            Kind::View,
            6000,
            [17.5, -45.0, 17.5],
            30.0,
            -10.0,
            "dropped into the pool and sunk to the bottom (water fog, surface seen from below)",
        ),
        s(
            "cave_room",
            World::Flat,
            Kind::View,
            6000,
            [34.5, -59.0, 34.5],
            15.0,
            8.0,
            "closed stone room lit by glowstone and a torch (block light, AO)",
        ),
        s(
            "terrain_spawn",
            World::Terrain,
            Kind::View,
            6000,
            [0.5, 200.0, 0.5],
            25.0,
            20.0,
            "seed 12345 terrain from above spawn at noon",
        ),
        s(
            "inventory",
            World::Flat,
            Kind::Inventory,
            6000,
            [0.5, -60.0, 0.5],
            0.0,
            0.0,
            "survival inventory screen over the world",
        ),
        s(
            "title",
            World::None,
            Kind::Title,
            0,
            [0.0; 3],
            0.0,
            0.0,
            "title screen with panorama",
        ),
    ]
}

fn w(t: u32) -> Value {
    json!({"op": "wait", "ticks": t})
}

fn cmd(t: &str) -> Value {
    json!({"op": "command", "text": t})
}

/// Ops (identical for both clients except the GUI toggle) up to the moment of capture.
fn scene_ops(sc: &Scene, setup: &[&str], vanilla: bool) -> Vec<Value> {
    let mut v = vec![];
    match sc.kind {
        Kind::Title => {
            if vanilla {
                v.push(json!({"op": "wait_screen", "screen": "TitleScreen", "timeout": 400}));
            }
            v.push(w(100));
        }
        Kind::View | Kind::Inventory => {
            let survival = sc.kind == Kind::Inventory;
            v.push(cmd(if survival {
                "gamemode survival"
            } else {
                "gamemode creative"
            }));
            v.push(w(10));
            for c in setup {
                v.push(cmd(c));
            }
            v.push(w(20));
            v.push(cmd(&format!("time set {}", sc.time)));
            v.push(cmd(&format!(
                "tp @s {} {} {}",
                sc.pos[0], sc.pos[1], sc.pos[2]
            )));
            v.push(w(10));
            v.push(json!({"op": "look", "yaw": sc.yaw, "pitch": sc.pitch}));
            // the player falls to rest (ground, pool bottom) from the teleport height: same end state in both clients
            v.push(w(sc.settle));
            if survival {
                v.push(json!({"op": "inventory", "action": "open"}));
                v.push(w(20));
            } else if vanilla {
                v.push(json!({"op": "key", "name": "key.toggleGui", "action": "tap"}));
                v.push(w(20));
            } else {
                v.push(json!({"op": "gui", "hidden": true}));
                v.push(w(20));
            }
        }
    }
    v
}

/// Undo what `scene_ops` left on the vanilla client so the next scene starts clean.
fn scene_cleanup(sc: &Scene) -> Vec<Value> {
    match sc.kind {
        Kind::View => vec![
            json!({"op": "key", "name": "key.toggleGui", "action": "tap"}),
            w(2),
        ],
        Kind::Inventory => vec![json!({"op": "inventory", "action": "close"}), w(5)],
        Kind::Title => vec![],
    }
}

fn server_args(world: World) -> Vec<&'static str> {
    let mut a = vec![
        "--memory",
        "--view",
        "8",
        "--no-spawning",
        "--gamemode",
        "creative",
    ];
    match world {
        World::Flat => a.push("--flat"),
        _ => a.extend(["--seed", "12345"]),
    }
    a
}

/// vanilla options.txt for deterministic captures (no clouds, particles, view bobbing, shadows).
fn write_options(dir: &Path) -> Result<()> {
    let g = dir.join("game");
    std::fs::create_dir_all(&g)?;
    let o = "pauseOnLostFocus:false\nonboardAccessibility:false\ntutorialStep:none\nskipMultiplayerWarning:true\njoinedFirstServer:true\n\
soundCategory_master:0.0\nsoundCategory_music:0.0\nmusicToast:\"never\"\nao:true\nrenderDistance:8\nsimulationDistance:8\nguiScale:2\nfov:0.0\n\
renderClouds:\"false\"\nparticles:2\nbobView:false\nentityShadows:false\nenableVsync:false\nmaxFps:60\nfovEffectScale:0.0\n\
damageTiltStrength:0.0\nscreenEffectScale:1.0\npanoramaScrollSpeed:0.0\nhideSplashTexts:true\nshowAutosaveIndicator:false\n\
notificationDisplayTime:0.0\ninGameNotification:false\nautoJump:false\nmenuBackgroundBlurriness:5\ntextureFiltering:0\nmipmapLevels:4\n\
fullscreen:false\nbiomeBlendRadius:2\ngamma:0.5\nadvancedItemTooltips:false\nrealmsNotifications:false\nattackIndicator:0\nlang:en_us\n";
    std::fs::write(g.join("options.txt"), o)?;
    Ok(())
}

/// Capture only the vanilla window with `xtask window-screenshot` (MC_WINDOW_PID picks our client).
fn capture_window(pid: Option<u32>, out: &Path) -> Result<()> {
    let _ = std::fs::remove_file(out);
    std::fs::create_dir_all(out.parent().unwrap())?;
    let abs = std::fs::canonicalize(out.parent().unwrap())?.join(out.file_name().unwrap());
    let prebuilt = ["target/release/xtask", "target/debug/xtask"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists());
    let mut c = match prebuilt {
        Some(p) => Command::new(p),
        None => {
            let mut c = Command::new("cargo");
            c.arg("xtask");
            c
        }
    };
    c.arg("window-screenshot").arg(&abs);
    if let Some(p) = pid {
        c.env("MC_WINDOW_PID", p.to_string());
    }
    let o = c.output().context("running xtask window-screenshot")?;
    if !o.status.success() || !abs.exists() {
        return super::skip(format!(
            "cannot capture the vanilla window ({})",
            String::from_utf8_lossy(&o.stderr)
                .trim()
                .lines()
                .last()
                .unwrap_or(
                    "no Minecraft window; screen-recording permission or a display is needed"
                )
        ));
    }
    Ok(())
}

fn run_ctl(ctl: &mut Ctl, ops: &[Value], what: &str) -> Result<()> {
    let res = ctl.run(ops, Duration::from_secs(90))?;
    for (i, r) in res.iter().enumerate() {
        if r["ok"] != json!(true) {
            bail!(
                "vanilla client ({what}): op {} failed: {}",
                ops[i],
                r["error"]
                    .as_str()
                    .unwrap_or("?")
                    .lines()
                    .next()
                    .unwrap_or("")
            );
        }
    }
    Ok(())
}

fn run_basalt(o: &Opts, ops: &[Value], addr: &str, work: &Path, tag: &str) -> Result<()> {
    let script = work.join(format!("{tag}.script.jsonl"));
    std::fs::write(
        &script,
        ops.iter().map(|v| v.to_string() + "\n").collect::<String>(),
    )?;
    if !o.client.exists() {
        bail!(
            "{} missing: cargo build --release -p basalt-client",
            o.client.display()
        );
    }
    let out = Command::new(&o.client)
        .args(["--jar"])
        .arg(&o.jar)
        .args(["--name", "BasaltShot", "--addr", addr, "--script"])
        .arg(&script)
        .env("RUST_LOG", "warn")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("starting basalt-client --script")?;
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        if super::no_gpu(&text) {
            return super::skip("no graphics adapter for basalt-client");
        }
        bail!(
            "basalt-client --script failed: {}",
            text.lines().rev().take(3).collect::<Vec<_>>().join(" | ")
        );
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub mean_ssim: f64,
    pub min_tile_ssim: f64,
    pub mean_hist: f64,
    pub max_tile_hist: f64,
}

fn load_baseline() -> Value {
    std::fs::read_to_string(BASELINE)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(json!({}))
}

fn limits_of(b: &Value, scene: &str) -> Option<Limits> {
    let e = b.get(scene)?;
    Some(Limits {
        mean_ssim: e["mean_ssim"].as_f64()?,
        min_tile_ssim: e["min_tile_ssim"].as_f64()?,
        mean_hist: e["mean_hist"].as_f64()?,
        max_tile_hist: e["max_tile_hist"].as_f64()?,
    })
}

/// Floors/ceilings recorded from a measurement: the measured value with a margin for run-to-run noise.
fn limits_from(r: &imgcmp::Report) -> Limits {
    let r3 = |x: f64| (x * 1000.0).round() / 1000.0;
    Limits {
        mean_ssim: r3((r.mean_ssim() - 0.06).max(0.0)),
        min_tile_ssim: r3((r.min_ssim() - 0.10).max(-1.0)),
        mean_hist: r3((r.mean_hist() + 0.04).min(1.0)),
        max_tile_hist: r3((r.max_hist() + 0.06).min(1.0)),
    }
}

/// Compare one scene, write the images, return (pass, report line).
fn judge(
    o: &Opts,
    name: &str,
    v: &Img,
    b: &Img,
    base: &Value,
    newbase: &mut serde_json::Map<String, Value>,
) -> Result<(bool, String)> {
    let dir = o.out.join("client-visual").join(name);
    v.save(&dir.join("vanilla.png"))?;
    b.save(&dir.join("basalt.png"))?;
    let rep = imgcmp::compare(v, b, COLS, ROWS, CMP.0, CMP.1);
    let m = format!(
        "mean_ssim={:.3} min_tile={:.3} mean_hist={:.3} max_hist={:.3}",
        rep.mean_ssim(),
        rep.min_ssim(),
        rep.mean_hist(),
        rep.max_hist()
    );
    let lim = limits_of(base, name);
    let (pass, note) = match lim {
        Some(l) => {
            let mut why = vec![];
            if rep.mean_ssim() < l.mean_ssim {
                why.push(format!("mean_ssim < {}", l.mean_ssim));
            }
            if rep.min_ssim() < l.min_tile_ssim {
                why.push(format!("min_tile < {}", l.min_tile_ssim));
            }
            if rep.mean_hist() > l.mean_hist {
                why.push(format!("mean_hist > {}", l.mean_hist));
            }
            if rep.max_hist() > l.max_tile_hist {
                why.push(format!("max_hist > {}", l.max_tile_hist));
            }
            if why.is_empty() {
                (
                    true,
                    format!(
                        "(limits {}/{}/{}/{})",
                        l.mean_ssim, l.min_tile_ssim, l.mean_hist, l.max_tile_hist
                    ),
                )
            } else {
                (false, format!("REGRESSION: {}", why.join(", ")))
            }
        }
        None => (
            o.update,
            "no limits recorded (run with --update-baseline)".into(),
        ),
    };
    let pass = pass || o.update;
    let l = lim.unwrap_or_else(|| limits_from(&rep));
    if !pass || o.update {
        let d = imgcmp::diff_image(v, b, &rep, l.min_tile_ssim.max(0.0), l.max_tile_hist);
        d.save(&dir.join("diff.png"))?;
        imgcmp::side_by_side(&v.resize(b.w, b.h), b, &d).save(&dir.join("side-by-side.png"))?;
    }
    if o.update {
        let n = limits_from(&rep);
        newbase.insert(name.into(), json!({"mean_ssim": n.mean_ssim, "min_tile_ssim": n.min_tile_ssim, "mean_hist": n.mean_hist, "max_tile_hist": n.max_tile_hist}));
    }
    Ok((pass, format!("{m} {note}")))
}

/// Number of title-bar rows to drop from a window capture of the wanted aspect ratio.
fn title_bar(cap: &Img, win: (u32, u32)) -> usize {
    let content_h = (cap.w as f64 * win.1 as f64 / win.0 as f64).round() as usize;
    cap.h.saturating_sub(content_h)
}

pub fn run(o: &Opts) -> Result<usize> {
    if !cfg!(any(target_os = "macos", windows, target_os = "linux")) {
        return super::skip("no window capture on this platform");
    }
    if let Some(why) = oracle::unavailable(&o.oracle_dir) {
        return super::skip(why);
    }
    if !o.client.exists() {
        bail!(
            "{} missing: cargo build --release -p basalt-client",
            o.client.display()
        );
    }
    use_port_range();
    oracle::ensure_built(&o.oracle_dir)?;
    write_options(&o.oracle_dir)?;
    std::env::set_var("ORACLE_WIN", format!("{}x{}", o.win.0, o.win.1));
    let work = o.out.join("client-visual");
    std::fs::create_dir_all(&work)?;
    let base = load_baseline();
    let mut newbase = base.as_object().cloned().unwrap_or_default();
    let all = scenes();
    let wanted: Vec<&Scene> = all
        .iter()
        .filter(|s| o.scenes.is_empty() || o.scenes.iter().any(|n| n == s.name))
        .collect();
    if wanted.is_empty() {
        bail!(
            "no scene matches {:?} (known: {})",
            o.scenes,
            all.iter().map(|s| s.name).collect::<Vec<_>>().join(", ")
        );
    }
    eprintln!(
        "client-visual: {} scene(s); vanilla client window {}x{} via {}",
        wanted.len(),
        o.win.0,
        o.win.1,
        o.oracle_dir.display()
    );
    let oracle = Oracle::connect_port(&o.oracle_dir, true, o.ctl_port)?;
    let mut ctl = Ctl::connect(o.ctl_port, Duration::from_secs(20))?;
    let (mut failed, mut done) = (0usize, 0usize);
    let mut size: Option<(usize, usize)> = None;
    for world in [World::Flat, World::Terrain, World::None] {
        let sc: Vec<&&Scene> = wanted.iter().filter(|s| s.world == world).collect();
        if sc.is_empty() {
            continue;
        }
        let srv = if world == World::None {
            None
        } else {
            let log = o.out.join(format!("client-visual/{world:?}.server.log"));
            let mut last = None;
            let mut s = None;
            for _ in 0..6 {
                match BasaltServer::start(&server_args(world), &log) {
                    Ok(x) => {
                        s = Some(x);
                        break;
                    }
                    Err(e) => last = Some(e),
                }
            }
            Some(s.ok_or_else(|| last.unwrap_or_else(|| anyhow!("server")))?)
        };
        let addr = srv.as_ref().map(|s| s.addr()).unwrap_or_default();
        // ---- vanilla pass
        let mut vcaps: Vec<(&Scene, Img)> = vec![];
        if let Some(_s) = &srv {
            ctl.call(
                json!({"op": "connect", "addr": addr}),
                Duration::from_secs(10),
            )?;
            run_ctl(
                &mut ctl,
                &[
                    json!({"op": "wait_join", "timeout": 1200}),
                    w(60),
                    json!({"op": "grab", "on": false}),
                ],
                "join",
            )?;
        }
        for (i, s) in sc.iter().enumerate() {
            let setup: &[&str] = if world == World::Flat && i == 0 {
                FLAT_SETUP
            } else {
                &[]
            };
            run_ctl(&mut ctl, &scene_ops(s, setup, true), s.name)?;
            let png = work.join(s.name).join("capture.png");
            capture_window(oracle.pid(), &png)?;
            let raw = Img::load(&png)?;
            let img = raw.crop_top(title_bar(&raw, o.win));
            size.get_or_insert((img.w, img.h));
            vcaps.push((s, img));
            run_ctl(&mut ctl, &scene_cleanup(s), s.name)?;
        }
        if srv.is_some() {
            // park the vanilla player far away (it must not stand inside basalt-client's view); it leaves when the server stops
            run_ctl(&mut ctl, &[cmd("tp @s 3000.5 300 3000.5"), w(10)], "park")?;
        }
        // ---- basalt-client pass at the vanilla frame size
        let (fw, fh) = size.unwrap();
        for (s, vimg) in &vcaps {
            let shot = work.join(s.name).join("basalt-raw.png");
            let _ = std::fs::remove_file(&shot);
            let mut ops = vec![
                json!({"op": "config", "size": [fw, fh], "render_distance": 8, "gui_scale": 2, "title": s.kind == Kind::Title}),
            ];
            if s.kind != Kind::Title {
                ops.push(json!({"op": "wait_join"}));
                ops.push(w(30));
            }
            ops.extend(scene_ops(s, &[], false));
            ops.push(json!({"op": "screenshot", "file": shot.to_string_lossy(), "settle": 10}));
            run_basalt(o, &ops, &addr, &work, s.name)?;
            let b = Img::load(&shot)
                .with_context(|| format!("basalt-client produced no {}", shot.display()))?;
            let (pass, line) = judge(o, s.name, vimg, &b, &base, &mut newbase)?;
            println!(
                "SCENE {:14} {} {}",
                s.name,
                if pass { "pass" } else { "FAIL" },
                line
            );
            done += 1;
            if !pass {
                failed += 1;
            }
        }
        if srv.is_some() {
            drop(srv);
            // the stopped server drops the vanilla client onto its disconnect / progress screen
            run_ctl(
                &mut ctl,
                &[
                    w(60),
                    json!({"op": "escape"}),
                    json!({"op": "wait_screen", "screen": "TitleScreen", "timeout": 200}),
                    w(10),
                ],
                "leave",
            )?;
        }
    }
    let _ = ctl.call(json!({"op": "quit"}), Duration::from_secs(5));
    if o.update {
        std::fs::create_dir_all("tools/baselines")?;
        std::fs::write(
            BASELINE,
            serde_json::to_string_pretty(&Value::Object(newbase))? + "\n",
        )?;
        eprintln!("client-visual: wrote {BASELINE}");
    }
    println!("images in {}", work.display());
    println!("RESULT client_visual scenes={done} failed={failed}");
    Ok(failed)
}

pub fn main(args: &[String]) -> i32 {
    super::finish("client_visual", parse_args(args).and_then(|o| run(&o)))
}

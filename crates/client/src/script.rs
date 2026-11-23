//! Scripted-input mode (`--script FILE [--script-log OUT.jsonl]`), the basalt-client side of
//! `basalt-check client-input`: replays the same JSON-lines ops the vanilla oracle mod's control
//! channel accepts (tools/oracle-mod/SCHEMA.md "control channel") through the same entry points the
//! window uses, and logs every serverbound packet as `{"tick","seq","hex"}` (hex = packet id varint +
//! body) so the harness can diff it against the vanilla client's own log.
//! Ticks are wall-clock 50 ms steps from the start of the script; supported ops: connect (ignored,
//! the address is the CLI one), wait_join, wait, move, look, jump, sneak, sprint, attack, use, hotbar,
//! inventory, escape, click, drag, command, chat, mark, query, drop, swap.
use crate::{Args, Assets, Game, GameOpts};
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::keyboard::KeyCode;

static TICK: AtomicU64 = AtomicU64::new(0);
static LOG: Mutex<Option<(std::fs::File, u64)>> = Mutex::new(None);

/// Called for every packet the client queues (main.rs `send`).
pub(crate) fn log_packet(buf: &[u8]) {
    let Ok(mut g) = LOG.lock() else { return };
    let Some((f, seq)) = g.as_mut() else { return };
    let hex: String = buf.iter().map(|b| format!("{b:02x}")).collect();
    let _ = writeln!(
        f,
        "{{\"tick\":{},\"seq\":{},\"hex\":\"{hex}\"}}",
        TICK.load(Ordering::Relaxed),
        *seq
    );
    *seq += 1;
}

fn mark(label: &str) {
    if let Ok(mut g) = LOG.lock() {
        if let Some((f, _)) = g.as_mut() {
            let _ = writeln!(
                f,
                "{{\"tick\":{},\"mark\":{}}}",
                TICK.load(Ordering::Relaxed),
                Value::String(label.into())
            );
        }
    }
}

struct Run {
    g: Game,
    t0: Instant,
    /// `--frames DIR --fps N`: rendered frames (PNG, named by script tick and frame number)
    frames: Option<(std::path::PathBuf, f32, Instant, u64)>,
}

impl Run {
    fn step(&mut self) -> Result<()> {
        TICK.store(
            (self.t0.elapsed().as_millis() / 50) as u64,
            Ordering::Relaxed,
        );
        let alpha = self.g.update();
        if let Some((dir, fps, last, n)) = &mut self.frames {
            if self.g.spawned && last.elapsed().as_secs_f32() >= 1.0 / *fps {
                *last = Instant::now();
                let rgb = self.g.capture(alpha)?;
                let (w, h) = self.g.gpu.size;
                let path = dir.join(format!("f{:05}_t{:05}.png", *n, TICK.load(Ordering::Relaxed)));
                crate::render::save_png(&path.to_string_lossy(), w, h, &rgb)?;
                *n += 1;
            }
        }
        if let Some(d) = &self.g.disconnected {
            if self.g.connected
                || !matches!(self.g.screen, Some(crate::menu::Screen::Connecting { .. }))
            {
                bail!("disconnected: {d}");
            }
        }
        std::thread::sleep(Duration::from_millis(4));
        Ok(())
    }
    fn wait_ticks(&mut self, n: u64) -> Result<()> {
        let until = Instant::now() + Duration::from_millis(50 * n);
        while Instant::now() < until {
            self.step()?;
        }
        Ok(())
    }
    fn wait_until(&mut self, secs: f32, f: impl Fn(&Game) -> bool) -> Result<bool> {
        let t = Instant::now();
        while t.elapsed().as_secs_f32() < secs {
            if f(&self.g) {
                return Ok(true);
            }
            self.step()?;
        }
        Ok(f(&self.g))
    }

    /// Through the window's own key path, so press handlers (double-tap fly, ...) run too.
    fn key_hold(&mut self, k: KeyCode, down: bool) {
        if down == self.g.keys.contains(&k) {
            return;
        }
        self.g.key(k, down, None);
    }

    /// press / hold / release / tap(ticks) for a held-input key.
    fn held(&mut self, op: &Value, k: KeyCode, mouse: Option<u8>) -> Result<()> {
        let action = op["action"].as_str().unwrap_or("tap");
        let set = |r: &mut Run, down: bool| match mouse {
            Some(b) => r.g.mouse_button(b, down),
            None => r.key_hold(k, down),
        };
        match action {
            "press" | "hold" => set(self, true),
            "release" => set(self, false),
            "tap" => {
                set(self, true);
                self.wait_ticks(op["ticks"].as_u64().unwrap_or(1))?;
                set(self, false);
            }
            a => bail!("action {a}"),
        }
        Ok(())
    }

    fn exec(&mut self, op: &Value) -> Result<()> {
        let name = op["op"].as_str().ok_or_else(|| anyhow!("op missing"))?;
        match name {
            // oracle-only recorders (bone/first-person dumps, camera/bob options) have no effect here
            "connect" | "grab" | "disconnect" | "log" | "flush" | "release_all" | "bones" | "fp" | "bob" | "camera" => {}
            "hud" => self.g.hide_gui = !op["on"].as_bool().unwrap_or(true),
            // {"op":"frames","dir":D,"fps":N} starts dumping rendered frames, {"op":"frames"} stops
            "frames" => {
                self.frames = match op["dir"].as_str() {
                    Some(d) => {
                        std::fs::create_dir_all(d)?;
                        Some((d.into(), op["fps"].as_f64().unwrap_or(20.0) as f32, Instant::now(), 0))
                    }
                    None => None,
                };
            }
            "wait_join" => {
                if !self.wait_until(30.0, |g| g.spawned && g.loaded_sent && g.screen.is_none())? {
                    bail!("did not join");
                }
                self.wait_until(15.0, |g| {
                    g.dirty.is_empty() && g.in_flight.is_empty() && !g.world.chunks.is_empty()
                })?;
            }
            "wait" => self.wait_ticks(op["ticks"].as_u64().unwrap_or(1))?,
            "move" => {
                for (k, code) in [
                    ("forward", KeyCode::KeyW),
                    ("back", KeyCode::KeyS),
                    ("left", KeyCode::KeyA),
                    ("right", KeyCode::KeyD),
                ] {
                    if let Some(b) = op[k].as_bool() {
                        self.key_hold(code, b);
                    }
                }
            }
            "look" => {
                let rel = op["relative"].as_bool().unwrap_or(false);
                if let Some(y) = op["yaw"].as_f64() {
                    self.g.player.yaw = (if rel { self.g.player.yaw } else { 0.0 }) + y as f32;
                }
                if let Some(p) = op["pitch"].as_f64() {
                    self.g.player.pitch = ((if rel { self.g.player.pitch } else { 0.0 })
                        + p as f32)
                        .clamp(-90.0, 90.0);
                }
            }
            "jump" => self.held(op, KeyCode::Space, None)?,
            "sneak" => self.held(op, KeyCode::ShiftLeft, None)?,
            "sprint" => self.held(op, KeyCode::ControlLeft, None)?,
            "attack" => self.held(op, KeyCode::F24, Some(0))?,
            "use" => self.held(op, KeyCode::F24, Some(1))?,
            "hotbar" => {
                let n = op["slot"].as_u64().unwrap_or(0) as usize;
                let digits = [
                    KeyCode::Digit1,
                    KeyCode::Digit2,
                    KeyCode::Digit3,
                    KeyCode::Digit4,
                    KeyCode::Digit5,
                    KeyCode::Digit6,
                    KeyCode::Digit7,
                    KeyCode::Digit8,
                    KeyCode::Digit9,
                ];
                self.g.key(digits[n.min(8)], true, None);
                self.step()?;
                self.g.key(digits[n.min(8)], false, None);
            }
            "inventory" => {
                let a = op["action"].as_str().unwrap_or("toggle");
                let open = matches!(self.g.screen, Some(crate::menu::Screen::Container(_)));
                if !open && (a == "open" || a == "toggle") {
                    self.g.key(KeyCode::KeyE, true, None);
                    self.g.key(KeyCode::KeyE, false, None);
                } else if open && (a == "close" || a == "toggle") {
                    self.g.close_container(true);
                }
            }
            "escape" => {
                if matches!(self.g.screen, Some(crate::menu::Screen::Container(_))) {
                    self.g.close_container(true);
                }
            }
            "key" => {
                let n = op["name"].as_str().unwrap_or("");
                let code = match n {
                    "drop" => KeyCode::KeyQ,
                    "swap" | "swap_offhand" => KeyCode::KeyF,
                    "inventory" => KeyCode::KeyE,
                    other => bail!("key {other}"),
                };
                self.g.key(code, true, None);
                self.g.key(code, false, None);
            }
            "click" => self.g.click(
                op["slot"].as_i64().unwrap_or(0) as i16,
                op["button"].as_i64().unwrap_or(0) as i8,
                op["mode"].as_i64().unwrap_or(0) as i32,
            ),
            "drag" => {
                let slots: Vec<i16> = op["slots"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_i64())
                            .map(|v| v as i16)
                            .collect()
                    })
                    .unwrap_or_default();
                self.g
                    .send_drag(op["button"].as_u64().unwrap_or(0) as u8, &slots);
            }
            "command" => self.g.send_chat(&format!(
                "/{}",
                op["text"].as_str().unwrap_or("").trim_start_matches('/')
            )),
            "chat" => self.g.send_chat(op["text"].as_str().unwrap_or("")),
            "mark" => mark(op["label"].as_str().unwrap_or("")),
            "query" => println!(
                "QUERY pos={:?} yaw={} pitch={} held={}",
                self.g.player.pos, self.g.player.yaw, self.g.player.pitch, self.g.held
            ),
            other => bail!("unknown op {other}"),
        }
        Ok(())
    }
}

pub fn run(args: &Args, assets: Arc<Assets>, file: &str) -> Result<()> {
    let dir = std::env::temp_dir().join(format!("basalt-script-{}", std::process::id()));
    std::env::set_var("BASALT_CONFIG_DIR", &dir);
    if let Some(p) = &args.script_log {
        *LOG.lock().unwrap() = Some((std::fs::File::create(p)?, 0));
    }
    let ops: Vec<Value> = std::fs::read_to_string(file)?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<std::result::Result<_, _>>()?;
    let name = args.name.clone();
    let cfg = crate::menu::Config {
        name: name.clone(),
        render_distance: 4,
        ..Default::default()
    };
    let sounds = crate::load_sounds(args, false);
    let g = Game::new(
        None,
        if args.frames.is_some() { args.size } else { (854, 480) },
        assets,
        crate::load_rules(args),
        GameOpts {
            name,
            addr: Some(args.addr.clone()),
            auto_respawn: false,
            sounds,
            cfg,
        },
    )?;
    let frames = match &args.frames {
        Some(d) => {
            std::fs::create_dir_all(d)?;
            Some((std::path::PathBuf::from(d), args.fps.max(1.0), Instant::now(), 0))
        }
        None => None,
    };
    let mut g = g;
    // --time T pins the sky/daylight (recordings compared against the vanilla client)
    g.fixed_time = args.time;
    let mut r = Run {
        g,
        t0: Instant::now(),
        frames,
    };
    let res = (|| -> Result<()> {
        for (i, op) in ops.iter().enumerate() {
            r.exec(op).map_err(|e| anyhow!("op {i} {op}: {e:#}"))?;
        }
        r.wait_ticks(10)
    })();
    if let Ok(mut g) = LOG.lock() {
        if let Some((f, _)) = g.as_mut() {
            let _ = f.flush();
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    res
}

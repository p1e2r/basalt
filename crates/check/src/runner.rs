//! Scenario context, running scenarios against Basalt and/or vanilla, differential comparison
//! and reporting.
use crate::bot::Bot;
use crate::query::{self, Kind, ServerState};
use crate::server::{BasaltServer, Vanilla};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Which world a Basalt server for this scenario starts with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WorldKind { Flat, Generated, FlatOnDisk }

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Servers { Both, BasaltOnly, VanillaOnly }

pub struct Spec {
    pub id: &'static str,
    pub group: &'static str,
    pub desc: &'static str,
    pub world: WorldKind,
    pub servers: Servers,
    /// Compare metrics between vanilla and Basalt.
    pub diff: bool,
    pub run: fn(&mut Ctx) -> Result<()>,
}

#[derive(Clone, Debug)]
pub struct Metric { pub key: String, pub value: f64, pub tol: f64 }

#[derive(Clone, Debug)]
pub struct Outcome {
    pub id: String,
    pub group: String,
    pub server: Kind,
    pub pass: bool,
    pub failures: Vec<String>,
    pub notes: Vec<String>,
    pub metrics: Vec<Metric>,
    pub secs: f64,
}

impl Outcome {
    pub fn to_json(&self) -> Value {
        json!({ "id": self.id, "group": self.group, "server": self.server.name(), "pass": self.pass, "secs": (self.secs * 10.0).round() / 10.0,
            "failures": self.failures, "notes": self.notes,
            "metrics": self.metrics.iter().map(|m| (m.key.clone(), json!({"value": m.value, "tol": m.tol}))).collect::<serde_json::Map<_, _>>() })
    }
}

pub struct Ctx {
    pub kind: Kind,
    pub spec: &'static Spec,
    pub idx: usize,
    pub basalt: Option<BasaltServer>,
    pub vanilla: Option<Vanilla>,
    pub out_dir: PathBuf,
    pub failures: Vec<String>,
    pub notes: Vec<String>,
    pub metrics: Vec<Metric>,
    pub names: Vec<String>,
    bots: u32,
}

/// Arena floor level (well above vanilla terrain near spawn).
pub const ARENA_Y: i32 = 200;

impl Ctx {
    pub fn addr(&self) -> String {
        match (&self.basalt, &self.vanilla) { (Some(b), _) => b.addr(), (_, Some(v)) => v.addr.clone(), _ => String::new() }
    }
    pub fn is_vanilla(&self) -> bool { self.kind == Kind::Vanilla }

    /// Connect a new bot with a name unique to this scenario (fresh player data on vanilla).
    pub fn bot(&mut self) -> Result<Bot> {
        self.bots += 1;
        let short: String = self.spec.id.chars().filter(|c| c.is_ascii_alphanumeric()).take(11).collect();
        let name = format!("c{:02}{}{}", self.idx, self.bots, short);
        let name: String = name.chars().take(16).collect();
        if let Some(v) = &self.vanilla { v.reset_player(&name); }
        self.names.push(name.clone());
        let bot = Bot::connect(&self.addr(), &name)?;
        if let Some(v) = &self.vanilla {
            v.op_online(&name)?;
            if !bot.wait_until(Duration::from_secs(5), |s| s.op_level >= 2) { bail!("{name}: op permission level never arrived"); }
        }
        Ok(bot)
    }

    pub fn check(&mut self, cond: bool, msg: impl Into<String>) -> bool {
        if !cond { self.failures.push(msg.into()); }
        cond
    }
    /// |got - want| <= tol, recorded as a failure otherwise.
    pub fn approx(&mut self, what: &str, got: f64, want: f64, tol: f64) -> bool {
        self.check((got - want).abs() <= tol, format!("{what}: got {got}, want {want}±{tol}"))
    }
    /// A value compared between servers in differential mode.
    pub fn metric(&mut self, key: &str, value: f64, tol: f64) { self.metrics.push(Metric { key: key.into(), value, tol }); }
    pub fn note(&mut self, s: impl Into<String>) { self.notes.push(s.into()); }

    pub fn state(&self, bot: &Bot) -> Result<ServerState> { query::state(self.kind, bot) }
    pub fn block_is(&self, bot: &Bot, p: [i32; 3], want: &str) -> Result<bool> { query::block_is(self.kind, bot, p, want) }
    pub fn items_near(&self, bot: &Bot, r: f64) -> Result<i64> { query::items_near(self.kind, bot, r) }

    pub fn gamemode(&self, bot: &Bot, gm: &str) -> Result<()> {
        let want = match gm { "survival" => 0, "creative" => 1, "adventure" => 2, _ => 3 };
        if bot.st().gamemode == want { return Ok(()); }
        bot.cmd_reply(&format!("gamemode {gm}"))?;
        if !bot.wait_until(Duration::from_secs(3), |s| s.gamemode == want) { bail!("gamemode {gm} not applied"); }
        Ok(())
    }

    /// Origin of this scenario's arena (distinct per scenario so vanilla runs don't collide).
    pub fn arena_origin(&self) -> [i32; 3] {
        let i = self.idx as i32;
        [-160 + (i % 8) * 40, ARENA_Y, -160 + (i / 8) * 56]
    }

    /// Build a clear arena: stone floor `sx`×`sz` at ARENA_Y with `h` blocks of air above (and a
    /// one-block air margin). Leaves the bot flying in creative above it. Returns the floor origin.
    pub fn arena(&mut self, bot: &Bot, sx: i32, sz: i32, h: i32) -> Result<[i32; 3]> {
        let o = self.arena_origin();
        self.gamemode(bot, "creative")?;
        bot.fly(true);
        bot.tp([o[0] as f64 + sx as f64 / 2.0, (o[1] + 3) as f64, o[2] as f64 + sz as f64 / 2.0])?;
        bot.set_frozen(true);
        // all chunks under the arena
        let ok = bot.wait_until(Duration::from_secs(30), |s| ((o[0] - 1) >> 4..=(o[0] + sx) >> 4).all(|cx| ((o[2] - 1) >> 4..=(o[2] + sz) >> 4).all(|cz| s.world.chunks.contains_key(&(cx, cz)))));
        if !ok { bail!("arena chunks did not load"); }
        bot.cmd_reply(&format!("fill {} {} {} {} {} {} air", o[0] - 1, o[1], o[2] - 1, o[0] + sx, o[1] + h, o[2] + sz))?;
        if self.is_vanilla() {
            // leftovers from earlier runs on the long-running server: persistent mobs, items, arrows
            bot.cmd_reply(&format!("kill @e[type=!minecraft:player,x={},y={},z={},dx={},dy={},dz={}]", o[0] - 3, o[1] - 8, o[2] - 3, sx + 6, h + 16, sz + 6))?;
        }
        bot.cmd_reply(&format!("fill {} {} {} {} {} {} stone", o[0], o[1], o[2], o[0] + sx - 1, o[1], o[2] + sz - 1))?;
        bot.set_frozen(false);
        Ok(o)
    }

    /// Put the bot on the arena floor at block (x, z) in `gamemode` and wait until it stands.
    pub fn stand(&mut self, bot: &Bot, x: i32, z: i32, gm: &str) -> Result<()> {
        bot.tp([x as f64 + 0.5, (ARENA_Y + 1) as f64, z as f64 + 0.5])?;
        self.gamemode(bot, gm)?;
        bot.fly(false);
        if !bot.wait_on_ground(Duration::from_secs(5)) { bail!("bot not standing at {x},{z}: {:?}", bot.st().body.pos); }
        Ok(())
    }

    pub fn restart(&mut self) -> Result<()> {
        match &mut self.basalt { Some(b) => b.restart(), None => bail!("restart is only possible for Basalt") }
    }
}

// ------------------------------------------------------------------ running

pub struct RunOpts {
    pub out_dir: PathBuf,
    pub vanilla: Option<(PathBuf, String)>,
}

pub fn run_spec(spec: &'static Spec, idx: usize, kind: Kind, opts: &RunOpts) -> Outcome {
    let t0 = Instant::now();
    let mut o = Outcome { id: spec.id.into(), group: spec.group.into(), server: kind, pass: false, failures: Vec::new(), notes: Vec::new(), metrics: Vec::new(), secs: 0.0 };
    let dir = opts.out_dir.join(kind.name());
    let _ = std::fs::create_dir_all(&dir);
    let mut ctx = Ctx { kind, spec, idx, basalt: None, vanilla: None, out_dir: dir.clone(), failures: Vec::new(), notes: Vec::new(), metrics: Vec::new(), names: Vec::new(), bots: 0 };
    let setup: Result<()> = (|| {
        match kind {
            Kind::Basalt => {
                let log = dir.join(format!("{}.server.log", spec.id));
                let world_dir = dir.join(format!("{}.world", spec.id));
                let _ = std::fs::remove_dir_all(&world_dir);
                let wd = world_dir.to_string_lossy().to_string();
                let args: Vec<&str> = match spec.world {
                    WorldKind::Flat => vec!["--memory", "--flat", "--view", "6", "--no-spawning"],
                    WorldKind::Generated => vec!["--memory", "--seed", "12345", "--view", "6", "--no-spawning"],
                    WorldKind::FlatOnDisk => vec!["--world", &wd, "--flat", "--view", "6", "--no-spawning"],
                };
                ctx.basalt = Some(BasaltServer::start(&args, &log)?);
            }
            Kind::Vanilla => {
                let Some((d, a)) = &opts.vanilla else { bail!("no vanilla server configured") };
                ctx.vanilla = Some(Vanilla::new(d, a));
            }
        }
        Ok(())
    })();
    let vlog_start = ctx.vanilla.as_ref().map(|v| v.log_len()).unwrap_or(0);
    match setup {
        Err(e) => ctx.failures.push(format!("setup: {e:#}")),
        Ok(()) => {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (spec.run)(&mut ctx)));
            match r {
                Ok(Ok(())) => {}
                Ok(Err(e)) => ctx.failures.push(format!("{e:#}")),
                Err(p) => ctx.failures.push(format!("panic: {}", p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default())),
            }
        }
    }
    // server-side red flags
    if let Some(b) = &mut ctx.basalt {
        b.stop();
        let log = b.log_text();
        for l in log.lines().filter(|l| l.contains("panicked") || l.contains("bad packet") || l.contains(" ERROR ")).take(3) { ctx.failures.push(format!("server log: {l}")); }
    }
    if let Some(v) = &ctx.vanilla {
        let log = v.log_since(vlog_start);
        for l in log.lines().filter(|l| ctx.names.iter().any(|n| l.contains(n.as_str())) && (l.contains("moved wrongly") || l.contains("moved too quickly") || l.contains("was kicked") || l.contains("Flying is not enabled"))).take(3) {
            ctx.failures.push(format!("vanilla log: {l}"));
        }
    }
    o.pass = ctx.failures.is_empty();
    o.failures = ctx.failures; o.notes = ctx.notes; o.metrics = ctx.metrics;
    o.secs = t0.elapsed().as_secs_f64();
    o
}

/// Compare a scenario's metrics between vanilla (reference) and Basalt.
pub fn diff(spec: &Spec, vanilla: &Outcome, basalt: &Outcome) -> Outcome {
    let mut o = Outcome { id: format!("diff:{}", spec.id), group: "Diff".into(), server: Kind::Basalt, pass: true, failures: Vec::new(), notes: Vec::new(), metrics: Vec::new(), secs: 0.0 };
    if !vanilla.pass { o.notes.push(format!("vanilla run failed: {}", vanilla.failures.join("; "))); }
    let mut keys: Vec<&str> = vanilla.metrics.iter().map(|m| m.key.as_str()).chain(basalt.metrics.iter().map(|m| m.key.as_str())).collect();
    keys.sort(); keys.dedup();
    for k in keys {
        let v = vanilla.metrics.iter().find(|m| m.key == k);
        let b = basalt.metrics.iter().find(|m| m.key == k);
        match (v, b) {
            (Some(v), Some(b)) => {
                let tol = v.tol.max(b.tol);
                if (v.value - b.value).abs() > tol + 1e-9 { o.failures.push(format!("{k}: vanilla {} basalt {} (tol {tol})", v.value, b.value)); }
                else { o.notes.push(format!("{k}: {} = {}", v.value, b.value)); }
            }
            (Some(v), None) => o.failures.push(format!("{k}: vanilla {} basalt missing", v.value)),
            (None, Some(b)) => o.failures.push(format!("{k}: vanilla missing basalt {}", b.value)),
            _ => {}
        }
    }
    o.pass = o.failures.is_empty() && vanilla.pass;
    if !vanilla.pass { o.failures.push("vanilla reference run failed".into()); }
    o
}

//! Randomized differential testing framework: seeded case generators, case recording and replay,
//! running every case against vanilla and Basalt, normalized diffing with explicit tolerances, and a
//! compact report (mismatch classes with their smallest reproducing inputs, `RESULT` lines).
//!
//! A harness is a [`Harness`]: `gen` makes case `i` from a seeded [`Rng`] (a JSON value that fully
//! determines the case), `run` executes cases on one server and returns one JSON result per case,
//! `compare` diffs (case, vanilla result, basalt result) into classified mismatches.
//! Cases are recorded under `target/check/cases/<harness>/<seed>-<n>.jsonl` and replayed with
//! `basalt-check --fuzz <harness> --replay <file>`.
use crate::bot::Bot;
use crate::query::Kind;
use crate::runner::{Ctx, Outcome, RunOpts, Servers, Spec, WorldKind};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

pub mod arena;
pub mod clicks;
pub mod commands;
pub mod dig;
pub mod place;

// ------------------------------------------------------------------ rng

/// splitmix64: tiny, seedable, good enough for case generation.
#[derive(Clone, Debug)]
pub struct Rng(pub u64);

impl Rng {
    pub fn new(seed: u64) -> Rng { Rng(seed ^ 0x9E37_79B9_7F4A_7C15) }
    /// Independent stream for case `i` of a run with `seed` (cases do not depend on each other).
    pub fn for_case(seed: u64, i: usize) -> Rng { let mut r = Rng::new(seed.wrapping_mul(0xD134_2543_DE82_EF95).wrapping_add(i as u64)); r.next_u64(); r }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: usize) -> usize { if n == 0 { 0 } else { (self.next_u64() % n as u64) as usize } }
    /// Inclusive range.
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 { lo + self.below((hi - lo + 1) as usize) as i64 }
    pub fn f64(&mut self) -> f64 { (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64 }
    pub fn chance(&mut self, p: f64) -> bool { self.f64() < p }
    pub fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T { &v[self.below(v.len())] }
    /// Index chosen with the given weights.
    pub fn weighted(&mut self, w: &[u32]) -> usize {
        let total: u32 = w.iter().sum();
        let mut x = self.below(total.max(1) as usize) as u32;
        for (i, &k) in w.iter().enumerate() { if x < k { return i; } x -= k; }
        w.len() - 1
    }
}

// ------------------------------------------------------------------ harness definition

pub struct Mismatch {
    pub class: String,
    pub detail: String,
    /// false = informational (reported, never failing)
    pub fail: bool,
}
impl Mismatch {
    pub fn fail(class: impl Into<String>, detail: impl Into<String>) -> Mismatch { Mismatch { class: class.into(), detail: detail.into(), fail: true } }
    pub fn info(class: impl Into<String>, detail: impl Into<String>) -> Mismatch { Mismatch { class: class.into(), detail: detail.into(), fail: false } }
}

pub struct Harness {
    /// Short id used in file names, `--fuzz` and the RESULT line (`clicks`).
    pub id: &'static str,
    pub desc: &'static str,
    pub default_cases: usize,
    pub long_cases: usize,
    pub gen: fn(&mut Rng, usize) -> Value,
    /// Run `cases` on the server in `ctx`; one result per case.
    pub run: fn(&mut Ctx, &[Value]) -> Result<Vec<Value>>,
    pub compare: fn(&Value, &Value, &Value) -> Vec<Mismatch>,
}

#[derive(Clone, Debug, Default)]
pub struct Opts {
    pub seed: u64,
    /// 0 = harness default.
    pub cases: usize,
    pub long: bool,
    pub replay: Option<PathBuf>,
    pub out: PathBuf,
    /// Parallel bots per server.
    pub jobs: usize,
}

static OPTS: OnceLock<Opts> = OnceLock::new();
pub fn opts() -> &'static Opts { OPTS.get().expect("fuzz options not set") }

pub static HARNESSES: &[&Harness] = &[&clicks::HARNESS, &commands::HARNESS, &dig::HARNESS, &place::HARNESS];

pub fn find(id: &str) -> Option<&'static Harness> {
    let id = id.trim_start_matches("fuzz_");
    HARNESSES.iter().copied().find(|h| h.id == id)
}

type Store = Mutex<HashMap<(String, &'static str), (Vec<Value>, f64, Option<String>)>>;
fn results_store() -> &'static Store { static S: OnceLock<Store> = OnceLock::new(); S.get_or_init(Default::default) }
fn cases_store() -> &'static Mutex<HashMap<String, Arc<Vec<Value>>>> { static S: OnceLock<Mutex<HashMap<String, Arc<Vec<Value>>>>> = OnceLock::new(); S.get_or_init(Default::default) }

fn drive(h: &'static Harness, ctx: &mut Ctx) -> Result<()> {
    let cases = cases_store().lock().unwrap().get(h.id).cloned().unwrap_or_default();
    let t0 = Instant::now();
    let r = (h.run)(ctx, &cases);
    let secs = t0.elapsed().as_secs_f64();
    let (res, err) = match r { Ok(v) => (v, None), Err(e) => (vec![Value::Null; cases.len()], Some(format!("{e:#}"))) };
    let n = res.iter().filter(|v| !v.is_null()).count();
    ctx.note(format!("{n}/{} cases run in {secs:.0}s", cases.len()));
    results_store().lock().unwrap().insert((h.id.to_string(), ctx.kind.name()), (res, secs, err.clone()));
    match err { Some(e) => Err(anyhow::anyhow!(e)), None => Ok(()) }
}

fn run_clicks(ctx: &mut Ctx) -> Result<()> { drive(&clicks::HARNESS, ctx) }
fn run_commands(ctx: &mut Ctx) -> Result<()> { drive(&commands::HARNESS, ctx) }
fn run_dig(ctx: &mut Ctx) -> Result<()> { drive(&dig::HARNESS, ctx) }
fn run_place(ctx: &mut Ctx) -> Result<()> { drive(&place::HARNESS, ctx) }

pub static SPECS: &[Spec] = &[
    Spec { id: "fuzz_clicks", group: "Fuzz", desc: "random container click sequences: inventory + crafting grids", world: WorldKind::Flat, servers: Servers::Both, diff: false, run: run_clicks },
    Spec { id: "fuzz_commands", group: "Fuzz", desc: "random valid/invalid command strings: outcome class + translation key", world: WorldKind::Flat, servers: Servers::Both, diff: false, run: run_commands },
    Spec { id: "fuzz_dig", group: "Fuzz", desc: "dig acceptance time: block x tool x on_ground/in_water", world: WorldKind::Flat, servers: Servers::Both, diff: false, run: run_dig },
    Spec { id: "fuzz_place", group: "Fuzz", desc: "block placement: placed block + 6 neighbours (orientation, halves, waterlogging)", world: WorldKind::Flat, servers: Servers::Both, diff: false, run: run_place },
];

// ------------------------------------------------------------------ worker pool helper

/// Run `f(bot, worker, case_index, case)` for every case over `jobs` bots in parallel.
/// `setup(ctx, worker_index) -> Bot` prepares each worker's bot (arena etc.). Errors per case become
/// `{"error": "..."}` results; a worker whose bot died stops.
pub fn parallel<S, F>(ctx: &mut Ctx, cases: &[Value], jobs: usize, mut setup: S, f: F) -> Result<Vec<Value>>
where S: FnMut(&mut Ctx, usize) -> Result<Bot>, F: Fn(&Bot, usize, usize, &Value) -> Result<Value> + Sync {
    let jobs = jobs.max(1).min(cases.len().max(1));
    let mut bots = Vec::new();
    for w in 0..jobs { bots.push(setup(ctx, w)?); }
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<Value>> = Mutex::new(vec![Value::Null; cases.len()]);
    std::thread::scope(|sc| {
        for (w, bot) in bots.iter().enumerate() {
            let (next, out, f) = (&next, &out, &f);
            sc.spawn(move || loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= cases.len() { break; }
                if bot.alive().is_err() { out.lock().unwrap()[i] = json!({"error": "bot disconnected"}); break; }
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(bot, w, i, &cases[i])));
                let v = match r { Ok(Ok(v)) => v, Ok(Err(e)) => json!({"error": format!("{e:#}")}), Err(_) => json!({"error": "panic"}) };
                out.lock().unwrap()[i] = v;
            });
        }
    });
    let mut out = out.into_inner().unwrap();
    for v in out.iter_mut() { if v.is_null() { *v = json!({"error": "not run (bots died)"}); } }
    Ok(out)
}

pub fn is_error(v: &Value) -> bool { v.get("error").is_some() }

// ------------------------------------------------------------------ recording / replay

pub fn write_cases(path: &Path, cases: &[Value]) {
    if let Some(d) = path.parent() { let _ = std::fs::create_dir_all(d); }
    let s: String = cases.iter().map(|c| serde_json::to_string(c).unwrap() + "\n").collect();
    let _ = std::fs::write(path, s);
}

pub fn read_cases(path: &Path) -> Result<Vec<Value>> {
    let s = std::fs::read_to_string(path)?;
    s.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).map_err(Into::into)).collect()
}

/// Number of cases a run of `h` uses (without replay).
pub fn case_count(h: &Harness, o: &Opts) -> usize { if o.cases > 0 { o.cases } else if o.long { h.long_cases } else { h.default_cases } }

/// Cases for `h`: replayed from a file or generated from (seed, n); recorded either way.
pub fn make_cases(h: &Harness, o: &Opts) -> Result<(Vec<Value>, PathBuf)> {
    if let Some(p) = &o.replay { return Ok((read_cases(p)?, p.clone())); }
    let n = case_count(h, o);
    let cases: Vec<Value> = (0..n).map(|i| (h.gen)(&mut Rng::for_case(o.seed, i), i)).collect();
    let path = o.out.join("cases").join(h.id).join(format!("{}-{}.jsonl", o.seed, n));
    write_cases(&path, &cases);
    Ok((cases, path))
}

// ------------------------------------------------------------------ known divergences

/// `crates/check/known-divergences.txt`: `<harness> <class>` per line (# comments). Known classes are
/// reported but do not count towards `new=` (and so don't fail pr_ready).
pub fn known_divergences() -> std::collections::HashSet<String> {
    let p = std::env::var("CHECK_KNOWN").unwrap_or_else(|_| "crates/check/known-divergences.txt".into());
    std::fs::read_to_string(p).unwrap_or_default().lines().map(|l| l.trim()).filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from).collect()
}

// ------------------------------------------------------------------ driver

/// (case index, detail, failing?) for one mismatch class
type ClassItems = Vec<(usize, String, bool)>;

pub struct FuzzReport {
    pub id: String,
    pub cases: usize,
    pub mismatches: usize,
    pub new: usize,
    pub info: usize,
    pub errors: usize,
    pub skipped: Option<String>,
    pub secs: f64,
    pub text: String,
    pub new_classes: Vec<String>,
}

impl FuzzReport {
    pub fn result_line(&self) -> String {
        format!("RESULT fuzz={} cases={} mismatches={} new={} info={} errors={} skipped={} secs={:.0}", self.id, self.cases, self.mismatches, self.new, self.info, self.errors, self.skipped.is_some() as u8, self.secs)
    }
    pub fn pass(&self) -> bool { self.skipped.is_none() && self.new == 0 && self.errors * 5 <= self.cases.max(1) }
}

fn shorten(s: &str, n: usize) -> String { if s.chars().count() > n { s.chars().take(n).collect::<String>() + "..." } else { s.to_string() } }

/// Run the given harnesses against vanilla and a fresh Basalt, diff, report.
pub fn run_harnesses(ids: &[&'static Harness], o: Opts, vanilla: Option<(PathBuf, String)>, out_dir: &Path) -> Vec<FuzzReport> {
    let _ = OPTS.set(o.clone());
    let o = opts();
    let mut reports = Vec::new();
    let mut meta: Vec<(&'static Harness, Vec<Value>, PathBuf)> = Vec::new();
    for h in ids {
        match make_cases(h, o) {
            Ok((c, p)) => { cases_store().lock().unwrap().insert(h.id.to_string(), Arc::new(c.clone())); meta.push((h, c, p)); }
            Err(e) => reports.push(FuzzReport { id: h.id.into(), cases: 0, mismatches: 0, new: 0, info: 0, errors: 0, skipped: Some(format!("cannot load cases: {e:#}")), secs: 0.0, text: String::new(), new_classes: vec![] }),
        }
    }
    let ropts = Arc::new(RunOpts { out_dir: out_dir.into(), vanilla: vanilla.clone() });
    let specs: Vec<(usize, &'static Spec)> = meta.iter().map(|(h, _, _)| { let id = format!("fuzz_{}", h.id); let i = SPECS.iter().position(|s| s.id == id).unwrap(); (60 + i, &SPECS[i]) }).collect();
    let outcomes: Mutex<Vec<Outcome>> = Mutex::new(Vec::new());
    std::thread::scope(|sc| {
        if vanilla.is_some() {
            let (specs, ropts, outcomes) = (&specs, ropts.clone(), &outcomes);
            sc.spawn(move || for (i, s) in specs { let r = crate::runner::run_spec(s, *i, Kind::Vanilla, &ropts); eprintln!("{} vanilla  {:16} {:6.1}s {}", if r.pass { "ok  " } else { "FAIL" }, r.id, r.secs, r.failures.first().cloned().unwrap_or_default()); outcomes.lock().unwrap().push(r); });
        }
        let (specs, ropts, outcomes) = (&specs, ropts.clone(), &outcomes);
        sc.spawn(move || for (i, s) in specs { let r = crate::runner::run_spec(s, *i, Kind::Basalt, &ropts); eprintln!("{} basalt   {:16} {:6.1}s {}", if r.pass { "ok  " } else { "FAIL" }, r.id, r.secs, r.failures.first().cloned().unwrap_or_default()); outcomes.lock().unwrap().push(r); });
    });
    let outcomes = outcomes.into_inner().unwrap();
    let known = known_divergences();
    for (h, cases, path) in meta {
        let store = results_store().lock().unwrap();
        let get = |k: Kind| store.get(&(h.id.to_string(), k.name())).cloned();
        let (b, v) = (get(Kind::Basalt), get(Kind::Vanilla));
        let skip = if vanilla.is_none() { Some("no vanilla server (fuzz harnesses are differential; pass --vanilla ADDR)".to_string()) }
            else if let Some((_, _, Some(e))) = &v { Some(format!("vanilla run failed: {e}")) }
            else if let Some((_, _, Some(e))) = &b { Some(format!("basalt run failed: {e}")) } else { None };
        let (Some((br, bs, _)), Some((vr, vs, _)), None) = (b, v, skip.clone()) else {
            let why = skip.unwrap_or("not run".into());
            reports.push(FuzzReport { id: h.id.into(), cases: cases.len(), mismatches: 0, new: 0, info: 0, errors: 0, skipped: Some(why.clone()), secs: 0.0, text: format!("== fuzz_{}: SKIPPED ({why})\n", h.id), new_classes: vec![] });
            continue;
        };
        write_cases(&path.with_extension("vanilla.results.jsonl"), &vr);
        write_cases(&path.with_extension("basalt.results.jsonl"), &br);
        let mut classes: BTreeMap<String, ClassItems> = BTreeMap::new();
        let mut errors = 0usize; let mut failing_cases: Vec<usize> = Vec::new();
        let mut notes = String::new();
        for (i, c) in cases.iter().enumerate() {
            let (cv, cb) = (&vr[i], &br[i]);
            if is_error(cv) || is_error(cb) {
                errors += 1;
                if errors <= 3 { notes += &format!("  error case {i}: vanilla={} basalt={}\n", shorten(&cv.get("error").map(|e| e.to_string()).unwrap_or("ok".into()), 100), shorten(&cb.get("error").map(|e| e.to_string()).unwrap_or("ok".into()), 100)); }
                continue;
            }
            let ms = (h.compare)(c, cv, cb);
            if ms.iter().any(|m| m.fail) { failing_cases.push(i); }
            for m in ms { classes.entry(m.class).or_default().push((i, m.detail, m.fail)); }
        }
        drop(store);
        let (mut mism, mut new, mut info) = (0, 0, 0);
        let mut new_classes = Vec::new();
        let mut text = format!("== fuzz_{}: {} cases (seed {}), vanilla {:.0}s, basalt {:.0}s, cases file {}\n", h.id, cases.len(), o.seed, vs, bs, path.display());
        let mut rows: Vec<(&String, &ClassItems)> = classes.iter().collect();
        rows.sort_by_key(|(c, v)| (!v.iter().any(|x| x.2), std::cmp::Reverse(v.len()), (*c).clone()));
        for (class, items) in rows {
            let fail = items.iter().any(|x| x.2);
            let is_known = known.contains(&format!("{} {}", h.id, class));
            if fail { mism += items.len(); if !is_known { new += items.len(); new_classes.push(class.clone()); } } else { info += items.len(); }
            let tag = if !fail { "info " } else if is_known { "known" } else { "NEW  " };
            text += &format!("  [{tag}] {class}  x{}\n", items.len());
            let mut by_size: Vec<&(usize, String, bool)> = items.iter().collect();
            by_size.sort_by_key(|(i, _, _)| serde_json::to_string(&cases[*i]).map(|s| s.len()).unwrap_or(0));
            for (i, d, _) in by_size.into_iter().take(3) { text += &format!("      case {i}: {}\n        -> {}\n", shorten(&serde_json::to_string(&cases[*i]).unwrap(), 300), shorten(d, 300)); }
        }
        text += &notes;
        if !failing_cases.is_empty() {
            let fp = path.with_extension("fail.jsonl");
            write_cases(&fp, &failing_cases.iter().map(|&i| cases[i].clone()).collect::<Vec<_>>());
            text += &format!("  failing cases only: {} (replay: basalt-check --fuzz {} --replay {})\n", fp.display(), h.id, fp.display());
        }
        reports.push(FuzzReport { id: h.id.into(), cases: cases.len(), mismatches: mism, new, info, errors, skipped: None, secs: vs + bs, text, new_classes });
    }
    for oc in outcomes.iter().filter(|o| !o.pass && o.server == Kind::Basalt) { for r in reports.iter_mut().filter(|r| format!("fuzz_{}", r.id) == oc.id) { r.text += &format!("  basalt run problems: {}\n", oc.failures.join("; ")); } }
    reports
}

// ------------------------------------------------------------------ small shared helpers

/// A stack as a compact string (`"dirt*12"`, `""` for empty), damage and enchant aware.
pub fn stack_str(s: &crate::proto::Stack) -> String {
    if s.count <= 0 { return String::new(); }
    let mut r = format!("{}*{}", s.name(), s.count);
    if s.damage != 0 { r += &format!("~{}", s.damage); }
    if !s.ench.is_empty() { r += &format!("+e{:?}", s.ench); }
    r
}

fn is_sentinel(t: &crate::proto::Text) -> bool {
    t.key.as_deref().map(|k| k.starts_with("commands.list")).unwrap_or(false) || t.plain.contains("commands.list.players") || t.plain.contains(" online: ")
}

/// Send a command and a trailing `list` sentinel; return everything the server said in between
/// (commands run in order, so the sentinel's reply ends the command's output).
pub fn cmd_collect(bot: &Bot, cmd: &str) -> Result<Vec<crate::proto::Text>> {
    bot.alive()?;
    let mark = bot.st().chat.len();
    bot.cmd(cmd);
    bot.cmd("list");
    if !bot.wait_until(std::time::Duration::from_secs(5), |s| s.chat[mark..].iter().any(|l| is_sentinel(&l.text))) { anyhow::bail!("no reply to the `list` sentinel after /{cmd}"); }
    let all: Vec<crate::proto::Text> = bot.st().chat[mark..].iter().map(|l| l.text.clone()).collect();
    let end = all.iter().rposition(is_sentinel).unwrap_or(all.len());
    Ok(all[..end].iter().filter(|t| !t.key.as_deref().map(|k| k.starts_with("chat.type") || k.starts_with("multiplayer.")).unwrap_or(false) && !t.plain.ends_with("joined the game") && !t.plain.starts_with('<')).cloned().collect())
}

/// Wait until no new `cont_log` entry has arrived for `quiet_ms` (at most `max_ms`).
pub fn settle_quiet(bot: &Bot, quiet_ms: u64, max_ms: u64) {
    let t0 = Instant::now();
    let mut last = bot.st().cont_log.len();
    let mut last_change = Instant::now();
    while t0.elapsed().as_millis() < max_ms as u128 {
        std::thread::sleep(std::time::Duration::from_millis(3));
        let n = bot.st().cont_log.len();
        if n != last { last = n; last_change = Instant::now(); } else if last_change.elapsed().as_millis() as u64 >= quiet_ms { break; }
    }
}

/// Wait until the server has processed every command sent so far (vanilla runs chat commands on an
/// async chain, so they can overtake or lag behind other packets): send `list` and wait for its reply.
pub fn barrier(bot: &Bot) -> Result<()> {
    bot.alive()?;
    let mark = bot.st().chat.len();
    bot.cmd("list");
    if !bot.wait_until(std::time::Duration::from_secs(5), |s| s.chat[mark..].iter().any(|l| is_sentinel(&l.text))) { anyhow::bail!("barrier: no reply to `list`"); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_deterministic_and_independent_per_case() {
        let a: Vec<u64> = (0..5).map(|_| Rng::for_case(9, 3).next_u64()).collect();
        assert!(a.windows(2).all(|w| w[0] == w[1]));
        assert_ne!(Rng::for_case(9, 3).next_u64(), Rng::for_case(9, 4).next_u64());
        assert_ne!(Rng::for_case(9, 3).next_u64(), Rng::for_case(10, 3).next_u64());
        let mut r = Rng::new(1);
        for _ in 0..1000 { let v = r.range(-3, 3); assert!((-3..=3).contains(&v)); assert!(r.below(7) < 7); }
    }

    #[test]
    fn cases_roundtrip_through_files() {
        let dir = std::env::temp_dir().join(format!("basalt-fuzz-test-{}", std::process::id()));
        let o = Opts { seed: 5, cases: 12, out: dir.clone(), jobs: 1, ..Default::default() };
        let h = &commands::HARNESS;
        let (cases, path) = make_cases(h, &o).unwrap();
        assert_eq!(cases.len(), 12);
        assert!(path.ends_with("commands/5-12.jsonl"));
        assert_eq!(read_cases(&path).unwrap(), cases);
        // replaying gives exactly the recorded cases
        let o2 = Opts { replay: Some(path.clone()), ..o.clone() };
        assert_eq!(make_cases(h, &o2).unwrap().0, cases);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn every_harness_generates_deterministic_cases() {
        // dig's generator needs ./data (GameData); the other three are data-free
        for h in [&clicks::HARNESS, &commands::HARNESS, &place::HARNESS] {
            let a: Vec<Value> = (0..30).map(|i| (h.gen)(&mut Rng::for_case(2, i), i)).collect();
            let b: Vec<Value> = (0..30).map(|i| (h.gen)(&mut Rng::for_case(2, i), i)).collect();
            assert_eq!(a, b, "{}", h.id);
        }
    }
}

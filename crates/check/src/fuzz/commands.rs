//! Command parsing fuzzer: random valid / invalid command strings for the commands Basalt implements
//! (`crates/server/src/commands.rs` IMPLEMENTED) plus garbage, wrong arg counts, bad selectors,
//! coordinates and units. Both servers get the string; the outcome class (success / failure) must
//! match, and the translation key vanilla replies with is compared against Basalt's plain-English
//! reply mapped through [`basalt_key`] (key differences are reported as a known-divergence list, not
//! failures: `key/<command>/<vanilla key>`).
//!
//! Safety (vanilla is shared): selectors are limited to @s, @p, small-radius @a/@e filters; weather and
//! time are restored after each case; summoned entities are removed.
use super::{arena::worker_arena, barrier, cmd_collect, parallel, Harness, Mismatch, Rng};
use crate::bot::Bot;
use crate::runner::Ctx;
use anyhow::Result;
use serde_json::{json, Value};
use std::time::Duration;

pub static HARNESS: Harness = Harness { id: "commands", desc: "random valid/invalid command strings: outcome class + translation key", default_cases: 500, long_cases: 6000, gen, run, compare };

/// Commands exercised (kept in sync with crates/server/src/commands.rs IMPLEMENTED by a unit test below).
pub const COMMANDS: &[&str] = &["gamemode", "tp", "teleport", "give", "time", "say", "list", "save-all", "effect", "enchant", "setblock", "fill", "kill", "summon", "help", "weather", "clear", "locate"];

const SELECTORS_OK: &[&str] = &["@s", "@s", "@s", "@p", "@a[distance=..3]", "@e[type=minecraft:player,distance=..3]", "@s[gamemode=survival]", "@s[limit=1]"];
const SELECTORS_BAD: &[&str] = &["@x", "@e[", "@e[type=]", "@e[type=foo]", "@e[limit=0]", "nobody_here", "@s[", "@", "@a[distance=abc]", "@e[distance=..-1]", "@s[gamemode=nope]", "@e[sort=xyz]"];
const ITEMS: &[&str] = &["stone", "dirt", "oak_planks", "apple", "diamond_sword", "bread", "stick", "minecraft:cobblestone", "oak_log", "iron_pickaxe", "ender_pearl", "shield"];
const BLOCKS: &[&str] = &["stone", "air", "glass", "oak_planks", "oak_stairs[facing=north]", "oak_stairs[facing=south,half=top]", "oak_log[axis=x]", "minecraft:dirt", "oak_slab[type=top]", "cobblestone", "oak_door[half=lower]", "water", "chest"];
const BAD_BLOCKS: &[&str] = &["nonexist", "minecraft:", "stone[x=y]", "oak_stairs[facing=up]", "oak_log[axis]", "stone[", "Stone", "oak_stairs[facing=north", "[]", "oak_log[axis=x,axis=y]"];
const EFFECTS: &[&str] = &["speed", "slowness", "jump_boost", "regeneration", "strength", "haste", "night_vision", "minecraft:speed", "water_breathing", "resistance"];
const MOBS: &[&str] = &["pig", "cow", "sheep", "chicken", "minecraft:pig", "armor_stand", "villager", "zombie ~ ~2 ~ {NoAI:1b}"];
const COORD: &[&str] = &["~", "~1", "~-1", "~0.5", "~-2", "~2", "0.5", "~.5"];
const BAD_COORD: &[&str] = &["~a", "~~", "abc", "1e400", "99999999", "-99999999", "~1~", "^", "^1", "~ ~", "NaN", "~99999999"];
const WORDS: &[&str] = &["frobnicate", "GAMEMODE creative", "give", "//give @s stone", "tpp", "g", "minecraft:give @s stone 1", "kill -1", "list uuids extra", "stone", "gm c", "execute", "?", "help me please", "say"];

fn pos3(r: &mut Rng) -> String { format!("{} {} {}", r.pick(COORD), r.pick(COORD), r.pick(COORD)) }
fn count(r: &mut Rng) -> String {
    match r.weighted(&[50, 10, 8, 8, 8, 8, 8]) { 0 => r.range(1, 64).to_string(), 1 => "1".into(), 2 => "0".into(), 3 => "-1".into(), 4 => "abc".into(), 5 => "1.5".into(), _ => "99999".into() }
}
fn dur(r: &mut Rng) -> String {
    match r.weighted(&[20, 20, 10, 10, 10, 10, 10, 10]) { 0 => r.range(1, 300).to_string(), 1 => format!("{}s", r.range(1, 120)), 2 => format!("{}t", r.range(1, 400)), 3 => "1d".into(), 4 => "0".into(), 5 => "-5".into(), 6 => "1x".into(), _ => "infinite".into() }
}

fn gen(r: &mut Rng, _i: usize) -> Value {
    let kind = r.weighted(&[62, 18, 15, 5]); // 0 valid, 1 wrong arg count, 2 bad arg, 3 garbage
    let c = *r.pick(COMMANDS);
    let mut pre: Vec<String> = Vec::new();
    let cmd: String = if kind == 3 {
        match r.weighted(&[40, 15, 15, 15, 15]) {
            0 => r.pick(WORDS).to_string(),
            1 => String::new(),
            2 => " ".repeat(r.range(1, 4) as usize),
            3 => { let n = r.range(1, 6); (0..n).map(|_| ["give", "@s", "stone", "~", "1", "gamemode", "tp", "é", "ü", "日本", "-", "[", "]", "{", "}", "\"", "'"][r.below(17)]).collect::<Vec<_>>().join(" ") }
            _ => format!("{} {}", c, "a".repeat(r.range(100, 300) as usize)),
        }
    } else {
        match c {
            "gamemode" => match kind {
                0 => format!("gamemode {}{}", r.pick(&["survival", "creative", "adventure", "spectator"]), if r.chance(0.3) { " @s" } else { "" }),
                1 => r.pick(&["gamemode", "gamemode creative @s extra", "gamemode survival survival survival"]).to_string(),
                _ => match r.below(5) { 0 => "gamemode foo".into(), 1 => "gamemode 1".into(), 2 => "gamemode s".into(), 3 => format!("gamemode creative {}", r.pick(SELECTORS_BAD)), _ => "gamemode Creative".into() },
            },
            "tp" | "teleport" => match kind {
                0 => match r.below(5) { 0 => format!("{c} {}", pos3(r)), 1 => format!("{c} @s {}", pos3(r)), 2 => format!("{c} {} 90 0", pos3(r)), 3 => format!("{c} @s @s"), _ => format!("{c} {} {} {}", pos3(r), r.range(-180, 180), r.range(-90, 90)) },
                1 => r.pick(&["tp", "tp 1 2", "tp ~ ~", "tp ~ ~ ~ 90", "teleport @s ~ ~", "tp 0 0 0 0 0 0 0", "tp @s"]).to_string(),
                _ => match r.below(4) { 0 => format!("{c} {} {} {}", r.pick(BAD_COORD), r.pick(COORD), r.pick(COORD)), 1 => format!("{c} {} {}", r.pick(SELECTORS_BAD), pos3(r)), 2 => format!("{c} @s ~ ~ ~ {} 0", r.pick(&["abc", "1e99", "~", "NaN"])), _ => format!("{c} ~ {} ~", r.pick(BAD_COORD)) },
            },
            "give" => match kind {
                0 => format!("give {} {}{}", r.pick(SELECTORS_OK), r.pick(ITEMS), if r.chance(0.7) { format!(" {}", r.range(1, 64)) } else { String::new() }),
                1 => r.pick(&["give", "give @s", "give @s stone 1 2", "give stone"]).to_string(),
                _ => match r.below(3) { 0 => format!("give @s {} 1", r.pick(&["nonexist", "minecraft:", "Stone", "stone[", "stone{", "air", "minecraft:air"])), 1 => format!("give @s stone {}", count(r)), _ => format!("give {} stone 1", r.pick(SELECTORS_BAD)) },
            },
            "time" => match kind {
                0 => match r.below(5) { 0 => format!("time set {}", r.pick(&["day", "noon", "night", "midnight"])), 1 => format!("time set {}", r.range(0, 24000)), 2 => format!("time add {}", r.range(0, 500)), 3 => format!("time set {}", dur(r)), _ => format!("time query {}", r.pick(&["daytime", "gametime", "day"])) },
                1 => r.pick(&["time", "time set", "time add", "time query", "time set day extra"]).to_string(),
                _ => format!("time {} {}", r.pick(&["set", "add", "query", "foo"]), r.pick(&["foo", "-5", "1.5", "99999999999", "abc", ""])),
            },
            "say" => match kind { 0 => format!("say {}", r.pick(&["hello", "hello world", "@s hi", "a  b", "1 2 3"])), 1 => "say".into(), _ => format!("say {}", r.pick(&["@e", "\u{e9}\u{e8}", "[", "{\"a\":1}"])) },
            "list" => match kind { 0 => r.pick(&["list", "list uuids"]).to_string(), 1 => "list a b".into(), _ => "list foo".into() },
            "save-all" => match kind { 0 => r.pick(&["save-all", "save-all flush"]).to_string(), 1 => "save-all flush extra".into(), _ => "save-all foo".into() },
            "effect" => match kind {
                0 => match r.below(4) {
                    0 => format!("effect give @s {} {}", r.pick(EFFECTS), r.range(1, 60)),
                    1 => format!("effect give @s {} {} {}", r.pick(EFFECTS), r.range(1, 60), r.range(0, 5)),
                    2 => format!("effect give @s {} infinite {} {}", r.pick(EFFECTS), r.range(0, 3), r.pick(&["true", "false"])),
                    _ => format!("effect clear{}{}", r.pick(&["", " @s"]), if r.chance(0.4) { format!(" {}", r.pick(EFFECTS)) } else { String::new() }),
                },
                1 => r.pick(&["effect", "effect give", "effect give @s", "effect clear @s speed extra", "effect foo"]).to_string(),
                _ => match r.below(4) { 0 => "effect give @s foo".into(), 1 => format!("effect give @s {} {}", r.pick(EFFECTS), r.pick(&["-1", "0", "1000001", "abc", "1.5"])), 2 => format!("effect give @s {} 5 {}", r.pick(EFFECTS), r.pick(&["256", "-1", "x"])), _ => format!("effect clear {} speed", r.pick(SELECTORS_BAD)) },
            },
            "enchant" => {
                if r.chance(0.6) { pre.push("give @s minecraft:diamond_sword".into()); }
                match kind {
                    0 => format!("enchant @s {} {}", r.pick(&["sharpness", "minecraft:sharpness", "unbreaking", "mending", "looting", "protection", "knockback"]), r.range(1, 5)),
                    1 => r.pick(&["enchant", "enchant @s", "enchant sharpness"]).to_string(),
                    _ => format!("enchant @s {} {}", r.pick(&["foo", "sharpness", "minecraft:"]), r.pick(&["0", "-1", "6", "99999", "abc"])),
                }
            }
            "setblock" => match kind {
                0 => format!("setblock {} {} {} {}{}", r.pick(COORD), r.pick(&["~2", "~3", "~2.5"]), r.pick(COORD), r.pick(BLOCKS), r.pick(&["", "", " replace", " destroy", " keep", " strict"])),
                1 => r.pick(&["setblock", "setblock ~ ~2", "setblock ~ ~2 ~", "setblock ~ ~2 ~ stone replace extra"]).to_string(),
                _ => match r.below(3) { 0 => format!("setblock ~ ~2 ~ {}", r.pick(BAD_BLOCKS)), 1 => format!("setblock {} ~2 ~ stone", r.pick(BAD_COORD)), _ => format!("setblock ~ ~2 ~ stone {}", r.pick(&["foo", "REPLACE", "destroy keep"])) },
            },
            "fill" => match kind {
                0 => format!("fill ~-1 ~2 ~-1 ~{} ~{} ~{} {}{}", r.range(-1, 2), r.range(2, 3), r.range(-1, 2), r.pick(BLOCKS), r.pick(&["", "", " replace", " hollow", " outline", " destroy", " keep", " strict", " replace stone"])),
                1 => r.pick(&["fill", "fill ~ ~ ~", "fill ~ ~2 ~ ~1 ~3 ~1", "fill ~ ~2 ~ ~1 ~3 stone"]).to_string(),
                _ => match r.below(4) { 0 => "fill ~ ~ ~ ~100 ~100 ~100 air".into(), 1 => format!("fill ~ ~2 ~ ~1 ~3 ~1 {}", r.pick(BAD_BLOCKS)), 2 => format!("fill ~ ~2 ~ {} ~3 ~1 stone", r.pick(BAD_COORD)), _ => "fill ~ ~2 ~ ~1 ~3 ~1 stone foo".into() },
            },
            "kill" => match kind {
                0 => r.pick(&["kill", "kill @s", "kill @e[type=minecraft:item,distance=..4]", "kill @e[type=minecraft:pig,distance=..6]", "kill @e[type=minecraft:cow,distance=..6]", "kill @e[type=item,distance=..4]"]).to_string(),
                1 => "kill @s @s".into(),
                _ => format!("kill {}", r.pick(SELECTORS_BAD)),
            },
            "summon" => match kind {
                0 => format!("summon {}{}", r.pick(MOBS), if r.chance(0.5) { " ~ ~2 ~" } else { "" }),
                1 => r.pick(&["summon", "summon pig ~ ~2", "summon pig ~ ~2 ~ {} extra"]).to_string(),
                _ => match r.below(3) { 0 => format!("summon {} ~ ~2 ~", r.pick(&["foo", "minecraft:", "Pig", "player", "item"])), 1 => format!("summon pig {} ~2 ~", r.pick(BAD_COORD)), _ => "summon pig ~ ~2 ~ {NoAI:".into() },
            },
            "help" => match kind { 0 => format!("help{}", r.pick(&["", " give", " gamemode", " tp", " time", " summon"])), 1 => "help give extra".into(), _ => "help foo".into() },
            "weather" => match kind {
                0 => format!("weather {}{}", r.pick(&["clear", "rain", "thunder"]), if r.chance(0.5) { format!(" {}", dur(r)) } else { String::new() }),
                1 => r.pick(&["weather", "weather rain 10 extra"]).to_string(),
                _ => format!("weather {} {}", r.pick(&["foo", "Rain", "rain"]), r.pick(&["-5", "abc", "1x", ""])),
            },
            // the fuzz arena is a flat world (no structures there, vanilla's is generated): only the
            // world-independent error paths; positions are compared by m3_locate_structure
            "locate" => match kind {
                0 => format!("locate structure {}", r.pick(&["minecraft:not_a_structure", "foo", "minecraft:", "#minecraft:not_a_tag"])),
                1 => r.pick(&["locate", "locate structure", "locate structure minecraft:village_plains extra"]).to_string(),
                _ => format!("locate structure {}", r.pick(&["Minecraft:village", "village plains", "minecraft:village_plains["])),
            },
            "clear" => match kind {
                0 => r.pick(&["clear", "clear @s", "clear @s stone", "clear @s diamond_sword", "clear @s diamond_sword 1", "clear @s stone 0"]).to_string(),
                1 => "clear @s stone 1 2".into(),
                _ => match r.below(3) { 0 => "clear @s foo".into(), 1 => format!("clear {}", r.pick(SELECTORS_BAD)), _ => format!("clear @s stone {}", r.pick(&["-1", "abc", "1.5"])) },
            },
            _ => unreachable!(),
        }
    };
    json!({ "cmd": cmd.trim_end_matches('\n'), "pre": pre })
}

fn run(ctx: &mut Ctx, cases: &[Value]) -> Result<Vec<Value>> {
    let jobs = super::opts().jobs.clamp(1, 4);
    let base = ctx.arena_origin();
    let vanilla = ctx.is_vanilla();
    parallel(ctx, cases, jobs,
        |ctx, w| { let bot = ctx.bot()?; worker_arena(ctx, &bot, w, 9, 9, 6, "survival")?; Ok(bot) },
        move |bot, w, _i, case| {
            let home = [base[0] as f64 + 4.5, (base[1] + 1) as f64, (base[2] + w as i32 * 16) as f64 + 4.5];
            run_case(bot, home, vanilla, case)
        })
}

fn run_case(bot: &Bot, home: [f64; 3], _vanilla: bool, case: &Value) -> Result<Value> {
    if bot.st().dead { bot.respawn(); bot.wait_until(Duration::from_secs(5), |s| !s.dead); }
    for p in case["pre"].as_array().into_iter().flatten() { bot.cmd(p.as_str().unwrap_or("")); }
    if case["pre"].as_array().map(|a| !a.is_empty()).unwrap_or(false) { barrier(bot)?; }
    let cmd = case["cmd"].as_str().unwrap_or("");
    let lines = cmd_collect(bot, cmd)?;
    let out: Vec<Value> = lines.iter().map(|t| json!({ "key": t.key, "plain": t.plain, "args": t.args })).collect();
    // restore: position, mode, effects, inventory, entities, weather/time
    let head = cmd.split_whitespace().next().unwrap_or("");
    bot.cmd(&format!("tp @s {:.2} {:.2} {:.2} 0 0", home[0], home[1], home[2]));
    bot.cmd("gamemode survival");
    bot.cmd("effect clear @s");
    bot.cmd("clear");
    bot.cmd("kill @e[type=!minecraft:player,distance=..12]");
    if head == "weather" { bot.cmd("weather clear"); }
    if head == "time" { bot.cmd("time set day"); }
    barrier(bot)?;
    if bot.st().dead { bot.respawn(); bot.wait_until(Duration::from_secs(5), |s| !s.dead); bot.cmd(&format!("tp @s {:.2} {:.2} {:.2} 0 0", home[0], home[1], home[2])); barrier(bot)?; }
    Ok(json!({ "lines": out }))
}

// ------------------------------------------------------------------ outcome classification

#[derive(Debug, Clone, PartialEq)]
pub struct Outcome { pub fail: bool, pub key: String, pub plain: String }

fn is_failure_key(k: &str) -> bool {
    ["argument.", "arguments.", "command.", "parsing.", "snbt.", "entity.not_summonable"].iter().any(|p| k.starts_with(p))
        || [".failed", "invalid", "toobig", "no_time_marker", "notfound", "unknown", "too_low", "too_high", "toolow", "toohigh"].iter().any(|p| k.contains(p))
}

fn first_line(lines: &[Value]) -> Option<&Value> {
    // vanilla appends a `command.context.here` line (the echoed input) after parse errors
    lines.iter().find(|l| !l["plain"].as_str().unwrap_or("").ends_with("command.context.here"))
}

pub fn vanilla_outcome(lines: &[Value]) -> Outcome {
    // vanilla's own replies always carry a translation key, except /help's `/cmd usage` lines; keyless text is a late
    // `say` broadcast from an earlier case that arrived after its sentinel
    let kept: Vec<Value> = lines.iter().filter(|l| !l["key"].is_null() || l["plain"].as_str().unwrap_or("").starts_with('/') || l["plain"].as_str().unwrap_or("").ends_with("command.context.here")).cloned().collect();
    let lines = &kept[..];
    match first_line(lines) {
        None => Outcome { fail: false, key: String::new(), plain: String::new() },
        Some(l) => { let k = l["key"].as_str().unwrap_or("").to_string(); Outcome { fail: is_failure_key(&k), key: k, plain: l["plain"].as_str().unwrap_or("").into() } }
    }
}

/// Basalt's reply: its translation key when it sent one (it mirrors vanilla's), else plain English mapped
/// through [`basalt_key`] (failure?, vanilla-equivalent key or `basalt:<text>` when unmapped).
pub fn basalt_outcome(cmd: &str, lines: &[Value]) -> Outcome {
    let Some(l) = first_line(lines) else { return Outcome { fail: false, key: String::new(), plain: String::new() } };
    let p = l["plain"].as_str().unwrap_or("");
    // Basalt now answers with vanilla translation keys; only its own plain-text commands go through the table
    if let Some(k) = l["key"].as_str() { return Outcome { fail: is_failure_key(k), key: k.to_string(), plain: p.to_string() }; }
    if p.starts_with('/') { return Outcome { fail: false, key: String::new(), plain: p.to_string() }; }
    let c = cmd.split_whitespace().next().unwrap_or("");
    let (fail, key) = basalt_key(c, p);
    Outcome { fail, key, plain: p.to_string() }
}

/// The explicit mapping table: Basalt reply text prefix -> vanilla translation key.
pub fn basalt_key(cmd: &str, p: &str) -> (bool, String) {
    let cmd = if cmd == "teleport" { "tp" } else { cmd };
    // failures
    if p.starts_with("Unknown command") { return (true, "command.unknown.command".into()); }
    if p.starts_with("Usage:") { return (true, "command.unknown.argument".into()); }
    if p.starts_with("Unknown item") { return (true, "argument.item.id.invalid".into()); }
    if p.starts_with("Unknown effect") || p.starts_with("Unknown enchantment") || p.starts_with("Unknown mob") { return (true, "argument.resource.not_found".into()); }
    if p.starts_with("Too many blocks") { return (true, "commands.fill.toobig".into()); }
    if p.starts_with("Hold an item") { return (true, "commands.enchant.failed.itemless".into()); }
    if p.starts_with("Can't kill") { return (true, "basalt:can't-kill-creative".into()); }
    if p.starts_with("No items were found") { return (true, "clear.failed.single".into()); }
    // successes
    let table: &[(&str, &str, &str)] = &[
        ("gamemode", "Set game mode to", "commands.gamemode.success.self"),
        ("gm", "Set game mode to", "commands.gamemode.success.self"),
        ("tp", "Teleported to", "commands.teleport.success.location.single"),
        ("give", "Gave", "commands.give.success.single"),
        ("time", "Set the time to", "commands.time.set.absolute"),
        ("effect", "Applied effect", "commands.effect.give.success.single"),
        ("effect", "Removed effects", "commands.effect.clear.everything.success.single"),
        ("enchant", "Enchanted with", "commands.enchant.success.single"),
        ("setblock", "Changed", "commands.setblock.success"),
        ("fill", "Changed", "commands.fill.success"),
        ("summon", "Summoned new", "commands.summon.success"),
        ("kill", "Killed", "commands.kill.success.single"),
        ("save-all", "Saved the world", "commands.save.success"),
        ("list", "", "commands.list.players"),
        ("clear", "Removed", "commands.clear.success.single"),
    ];
    for (c, pre, key) in table { if *c == cmd && p.starts_with(pre) && (!pre.is_empty() || p.contains(" online")) { return (false, (*key).into()); } }
    if cmd == "weather" && p.starts_with("Set the weather to ") { return (false, format!("commands.weather.set.{}", p.trim_start_matches("Set the weather to "))); }
    if cmd == "help" && p.starts_with("Commands:") { return (false, "".into()); }
    if cmd == "say" { return (false, "".into()); }
    (false, format!("basalt:{}", p.chars().take(40).collect::<String>()))
}

fn cmd_name(case: &Value) -> String {
    let c = case["cmd"].as_str().unwrap_or("").split_whitespace().next().unwrap_or("").to_string();
    let c = if c == "teleport" { "tp".to_string() } else { c };
    if COMMANDS.contains(&c.as_str()) || c == "teleport" { c } else { "garbage".into() }
}

/// Keys that are equivalent for our purposes (vanilla distinguishes more cases than Basalt can).
fn same_key(v: &Outcome, b: &Outcome) -> bool {
    if v.key == b.key { return true; }
    // vanilla's plural/singular and per-target variants of one message
    let norm = |k: &str| k.replace(".single", "").replace(".plural", "").replace(".multiple", "");
    norm(&v.key) == norm(&b.key)
}

fn compare(case: &Value, v: &Value, b: &Value) -> Vec<Mismatch> {
    let (vl, bl) = (v["lines"].as_array().cloned().unwrap_or_default(), b["lines"].as_array().cloned().unwrap_or_default());
    let cmd = case["cmd"].as_str().unwrap_or("");
    let (vo, bo) = (vanilla_outcome(&vl), basalt_outcome(cmd, &bl));
    let name = cmd_name(case);
    if vo.fail != bo.fail {
        let s = |f: bool| if f { "fail" } else { "ok" };
        return vec![Mismatch::fail(format!("outcome/{name}/vanilla-{}-vs-basalt-{}", s(vo.fail), s(bo.fail)), format!("`{cmd}`: vanilla {} [{}] ({}) / basalt {} ({})", s(vo.fail), vo.key, vo.plain.chars().take(80).collect::<String>(), s(bo.fail), bo.plain.chars().take(80).collect::<String>()))];
    }
    if !same_key(&vo, &bo) {
        let class = if bo.key.starts_with("basalt:") { format!("key-unmapped/{name}") } else { format!("key/{name}/{}", vo.key) };
        return vec![Mismatch::info(class, format!("`{cmd}`: vanilla key {} / basalt `{}` (mapped to {})", vo.key, bo.plain.chars().take(80).collect::<String>(), bo.key))];
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn covers_implemented() {
        // every implemented command is exercised (IMPLEMENTED lives in crates/server/src/commands.rs)
        let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../server/src/commands.rs")).unwrap();
        let line = src.lines().find(|l| l.starts_with("pub const IMPLEMENTED")).unwrap();
        for name in line.split('"').skip(1).step_by(2) { assert!(COMMANDS.contains(&name), "fuzz_commands does not exercise /{name}"); }
    }
    #[test]
    fn deterministic() {
        let a: Vec<Value> = (0..50).map(|i| gen(&mut Rng::for_case(7, i), i)).collect();
        let b: Vec<Value> = (0..50).map(|i| gen(&mut Rng::for_case(7, i), i)).collect();
        assert_eq!(a, b);
    }
    #[test]
    fn mapping() {
        assert_eq!(basalt_key("gamemode", "Set game mode to Creative"), (false, "commands.gamemode.success.self".into()));
        assert!(basalt_key("tp", "Usage: /tp <x> <y> <z>").0);
    }
}

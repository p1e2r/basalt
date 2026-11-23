//! Scoreboard (`/scoreboard objectives|players`): objectives (all criteria count as dummy for now),
//! scores per holder (player names or plain names like `#t`), display slots; synced to clients and
//! saved in vanilla's data/minecraft/scoreboard.dat. Everything below was measured on vanilla 26.2
//! (basalt-probe packet dumps and command replies, 2026-11-24):
//! - set_objective: name, mode (0 add / 1 remove / 2 update), [display name NBT, varint render type
//!   (0 integer), bool number format]
//! - set_display_objective: varint slot (0 list, 1 sidebar, 2 below_name, 3.. sidebar.team.<colour>),
//!   objective name ("" clears)
//! - set_score: holder, objective, varint value, bool display name, bool number format
//! - reset_score: holder, optional objective (vanilla left it out when the holder's last score went)
//! - scoreboard.dat: data {Objectives [{Name, DisplayName}], PlayerScores [{Name, Objective, Score,
//!   Locked}], DisplaySlots {slot: name}}
use super::{err, plural, s, Target, R};
use crate::cmdparse::Parsed;
use crate::game::Game;
use basalt_nbt::{Compound, Tag};
use basalt_protocol::{ids::play::clientbound as cb, text, Writer};
use std::collections::BTreeMap;

#[derive(Default, Clone)]
pub struct Scoreboard {
    /// name -> (criteria, display name)
    pub objectives: BTreeMap<String, (String, Tag)>,
    /// (holder, objective) -> score
    pub scores: BTreeMap<(String, String), i32>,
    /// slot name -> objective
    pub display: BTreeMap<String, String>,
}

const COLOURS: [&str; 16] = [
    "black",
    "dark_blue",
    "dark_green",
    "dark_aqua",
    "dark_red",
    "dark_purple",
    "gold",
    "gray",
    "dark_gray",
    "blue",
    "green",
    "aqua",
    "red",
    "light_purple",
    "yellow",
    "white",
];

fn slot_id(slot: &str) -> Option<i32> {
    match slot {
        "list" => Some(0),
        "sidebar" => Some(1),
        "below_name" | "belowName" => Some(2),
        _ => slot
            .strip_prefix("sidebar.team.")
            .and_then(|c| COLOURS.iter().position(|x| *x == c))
            .map(|i| 3 + i as i32),
    }
}

fn objective_packet(name: &str, mode: u8, display: &Tag) -> Writer {
    let mut w = Writer::with_id(cb::SET_OBJECTIVE);
    w.str(name).u8(mode);
    if mode != 1 {
        w.nbt(display).varint(0).bool(false);
    }
    w
}
fn display_packet(slot: i32, name: &str) -> Writer {
    let mut w = Writer::with_id(cb::SET_DISPLAY_OBJECTIVE);
    w.varint(slot).str(name);
    w
}
fn score_packet(holder: &str, obj: &str, v: i32) -> Writer {
    let mut w = Writer::with_id(cb::SET_SCORE);
    w.str(holder).str(obj).varint(v).bool(false).bool(false);
    w
}

impl Scoreboard {
    /// Packets that bring a joining player up to date.
    pub fn join_packets(&self) -> Vec<Writer> {
        let mut out: Vec<Writer> = self
            .objectives
            .iter()
            .map(|(n, (_, d))| objective_packet(n, 0, d))
            .collect();
        for ((h, o), v) in &self.scores {
            out.push(score_packet(h, o, *v));
        }
        for (slot, o) in &self.display {
            if let Some(id) = slot_id(slot) {
                out.push(display_packet(id, o));
            }
        }
        out
    }

    pub fn load(world: &std::path::Path) -> Scoreboard {
        let mut sb = Scoreboard::default();
        let Some(t) = crate::level::read_gz(&world.join("data/minecraft/scoreboard.dat")) else {
            return sb;
        };
        let Some(d) = t.get("data") else { return sb };
        for o in d
            .get("Objectives")
            .and_then(|l| l.as_list())
            .into_iter()
            .flatten()
        {
            let Some(n) = o.get("Name").and_then(|v| v.as_str()) else {
                continue;
            };
            let crit = o
                .get("CriteriaName")
                .and_then(|v| v.as_str())
                .unwrap_or("dummy")
                .to_string();
            let disp = match o.get("DisplayName") {
                Some(Tag::String(s)) => crate::snbt::parse(s)
                    .filter(|t| !matches!(t, Tag::String(_)))
                    .unwrap_or(Tag::String(s.clone())),
                Some(t) => t.clone(),
                None => Tag::String(n.into()),
            };
            sb.objectives.insert(n.into(), (crit, disp));
        }
        for e in d
            .get("PlayerScores")
            .and_then(|l| l.as_list())
            .into_iter()
            .flatten()
        {
            if let (Some(n), Some(o), Some(v)) = (
                e.get("Name").and_then(|v| v.as_str()),
                e.get("Objective").and_then(|v| v.as_str()),
                e.get("Score").and_then(|v| v.as_i64()),
            ) {
                sb.scores.insert((n.into(), o.into()), v as i32);
            }
        }
        if let Some(c) = d.get("DisplaySlots").and_then(|c| c.as_compound()) {
            for (k, v) in c {
                if let Some(v) = v.as_str() {
                    sb.display.insert(k.clone(), v.into());
                }
            }
        }
        sb
    }

    pub fn save(&self, world: &std::path::Path) {
        let path = world.join("data/minecraft/scoreboard.dat");
        if self.objectives.is_empty() && !path.exists() {
            return;
        }
        let mut root = crate::level::read_gz(&path)
            .and_then(|t| t.as_compound().cloned())
            .unwrap_or_default();
        let mut d = Compound::new();
        let objs = self
            .objectives
            .iter()
            .map(|(n, (c, disp))| {
                let mut o = Compound::new();
                o.insert("Name".into(), Tag::String(n.clone()));
                o.insert("DisplayName".into(), disp.clone());
                if c != "dummy" {
                    o.insert("CriteriaName".into(), Tag::String(c.clone()));
                }
                Tag::Compound(o)
            })
            .collect();
        d.insert("Objectives".into(), Tag::List(objs));
        let scores = self
            .scores
            .iter()
            .map(|((h, o), v)| {
                basalt_nbt::compound([
                    ("Name", Tag::String(h.clone())),
                    ("Objective", Tag::String(o.clone())),
                    ("Score", Tag::Int(*v)),
                    ("Locked", Tag::Byte(1)),
                ])
            })
            .collect();
        d.insert("PlayerScores".into(), Tag::List(scores));
        d.insert(
            "DisplaySlots".into(),
            Tag::Compound(
                self.display
                    .iter()
                    .map(|(k, v)| (k.clone(), Tag::String(v.clone())))
                    .collect(),
            ),
        );
        root.insert("data".into(), Tag::Compound(d));
        root.insert("DataVersion".into(), Tag::Int(basalt_data::WORLD_VERSION));
        if let Err(e) = crate::level::write_gz(&path, &Tag::Compound(root)) {
            log::warn!("writing scoreboard.dat: {e}");
        }
    }
}

impl Game {
    fn sb_broadcast(&self, w: Writer) {
        for p in self.all_players() {
            p.send(Writer { buf: w.buf.clone() });
        }
    }

    /// Score holders named by an argument: selectors resolve to player names, anything else is a name.
    fn holders(&mut self, eid: i32, raw: &str) -> R<Vec<String>> {
        if raw.starts_with('@') {
            let t = self.resolve(eid, raw, false)?;
            Ok(t.iter()
                .filter_map(|t| {
                    if let Target::Player(e) = t {
                        self.players.get(e).map(|p| p.name.clone())
                    } else {
                        None
                    }
                })
                .collect())
        } else if raw == "*" {
            let mut v: Vec<String> = self.scoreboard.scores.keys().map(|k| k.0.clone()).collect();
            v.dedup();
            Ok(v)
        } else {
            Ok(vec![raw.to_string()])
        }
    }

    fn obj_text(&self, name: &str) -> Tag {
        let d = self
            .scoreboard
            .objectives
            .get(name)
            .map(|o| o.1.clone())
            .unwrap_or(Tag::String(name.into()));
        super::tr("chat.square_brackets", vec![d])
    }

    fn need_objective(&self, name: &str) -> R<()> {
        if self.scoreboard.objectives.contains_key(name) {
            Ok(())
        } else {
            Err(err("arguments.objective.notFound", &[name]))
        }
    }

    pub(super) fn cmd_scoreboard(&mut self, eid: i32, p: &Parsed) -> R<()> {
        let path = p.path_str();
        let obj = p.has("objective").then(|| p.arg("objective").to_string());
        match path.as_str() {
            "scoreboard objectives add objective criteria"
            | "scoreboard objectives add objective criteria displayName" => {
                let name = obj.unwrap();
                if self.scoreboard.objectives.contains_key(&name) {
                    return Err(err("commands.scoreboard.objectives.add.duplicate", &[]));
                }
                let disp = if p.has("displayName") {
                    crate::snbt::parse(p.arg("displayName"))
                        .ok_or_else(|| err("argument.component.invalid", &[p.arg("displayName")]))?
                } else {
                    Tag::String(name.clone())
                };
                self.sb_broadcast(objective_packet(&name, 0, &disp));
                self.scoreboard
                    .objectives
                    .insert(name.clone(), (p.arg("criteria").to_string(), disp));
                self.ok(
                    eid,
                    "commands.scoreboard.objectives.add.success",
                    vec![self.obj_text(&name)],
                );
            }
            "scoreboard objectives remove objective" => {
                let name = obj.unwrap();
                self.need_objective(&name)?;
                let t = self.obj_text(&name);
                self.scoreboard.objectives.remove(&name);
                self.scoreboard.scores.retain(|k, _| k.1 != name);
                self.scoreboard.display.retain(|_, v| *v != name);
                self.sb_broadcast(objective_packet(&name, 1, &Tag::End));
                self.ok(
                    eid,
                    "commands.scoreboard.objectives.remove.success",
                    vec![t],
                );
            }
            "scoreboard objectives list" => {
                let names: Vec<String> = self.scoreboard.objectives.keys().cloned().collect();
                if names.is_empty() {
                    self.ok(eid, "commands.scoreboard.objectives.list.empty", vec![]);
                } else {
                    let mut list = Vec::new();
                    for (i, n) in names.iter().enumerate() {
                        if i > 0 {
                            list.push(text::plain(", "));
                        }
                        list.push(self.obj_text(n));
                    }
                    let joined = basalt_nbt::compound([
                        ("text", Tag::String(String::new())),
                        ("extra", Tag::List(list)),
                    ]);
                    self.ok(
                        eid,
                        "commands.scoreboard.objectives.list.success",
                        vec![s(names.len()), joined],
                    );
                }
            }
            "scoreboard objectives setdisplay slot"
            | "scoreboard objectives setdisplay slot objective" => {
                let slot = p.arg("slot").to_string();
                let id = slot_id(&slot)
                    .ok_or_else(|| err("argument.scoreboardDisplaySlot.invalid", &[&slot]))?;
                match obj {
                    Some(name) => {
                        self.need_objective(&name)?;
                        if self.scoreboard.display.get(&slot) == Some(&name) {
                            return Err(err(
                                "commands.scoreboard.objectives.display.alreadySet",
                                &[],
                            ));
                        }
                        self.scoreboard.display.insert(slot.clone(), name.clone());
                        self.sb_broadcast(display_packet(id, &name));
                        self.ok(
                            eid,
                            "commands.scoreboard.objectives.display.set",
                            vec![s(&slot), s(&name)],
                        );
                    }
                    None => {
                        if self.scoreboard.display.remove(&slot).is_none() {
                            return Err(err(
                                "commands.scoreboard.objectives.display.alreadyEmpty",
                                &[],
                            ));
                        }
                        self.sb_broadcast(display_packet(id, ""));
                        self.ok(
                            eid,
                            "commands.scoreboard.objectives.display.cleared",
                            vec![s(&slot)],
                        );
                    }
                }
            }
            "scoreboard players set targets objective score"
            | "scoreboard players add targets objective score"
            | "scoreboard players remove targets objective score" => {
                let name = obj.unwrap();
                self.need_objective(&name)?;
                let op = p.path[2].clone();
                let n: i32 = p
                    .arg("score")
                    .parse()
                    .map_err(|_| err("parsing.int.invalid", &[p.arg("score")]))?;
                let hs = self.holders(eid, p.arg("targets"))?;
                if hs.is_empty() {
                    return Err(err("argument.entity.notfound.entity", &[]));
                }
                let mut last = 0;
                for h in &hs {
                    let e = self
                        .scoreboard
                        .scores
                        .entry((h.clone(), name.clone()))
                        .or_insert(0);
                    *e = match op.as_str() {
                        "set" => n,
                        "add" => e.wrapping_add(n),
                        _ => e.wrapping_sub(n),
                    };
                    last = *e;
                    let v = *e;
                    self.sb_broadcast(score_packet(h, &name, v));
                }
                let who = if hs.len() == 1 {
                    text::plain(&hs[0])
                } else {
                    s(hs.len())
                };
                let key = plural(
                    &format!("commands.scoreboard.players.{op}.success"),
                    hs.len(),
                );
                let args = match (op.as_str(), hs.len()) {
                    ("set", 1) => vec![self.obj_text(&name), who, s(last)],
                    ("set", _) => vec![self.obj_text(&name), who, s(n)],
                    (_, 1) => vec![s(n), self.obj_text(&name), who, s(last)],
                    _ => vec![s(n), self.obj_text(&name), who],
                };
                self.ok(eid, &key, args);
            }
            "scoreboard players get target objective" => {
                let name = obj.unwrap();
                self.need_objective(&name)?;
                let h = self
                    .holders(eid, p.arg("target"))?
                    .into_iter()
                    .next()
                    .ok_or_else(|| err("argument.entity.notfound.entity", &[]))?;
                match self.scoreboard.scores.get(&(h.clone(), name.clone())) {
                    Some(v) => {
                        let v = *v;
                        self.ok(
                            eid,
                            "commands.scoreboard.players.get.success",
                            vec![text::plain(&h), s(v), self.obj_text(&name)],
                        );
                    }
                    None => return Err(err("commands.scoreboard.players.get.null", &[&name, &h])),
                }
            }
            "scoreboard players list" => {
                let mut hs: Vec<String> =
                    self.scoreboard.scores.keys().map(|k| k.0.clone()).collect();
                hs.dedup();
                if hs.is_empty() {
                    self.ok(eid, "commands.scoreboard.players.list.empty", vec![]);
                } else {
                    self.ok(
                        eid,
                        "commands.scoreboard.players.list.success",
                        vec![s(hs.len()), s(hs.join(", "))],
                    );
                }
            }
            "scoreboard players list target" => {
                let h = self
                    .holders(eid, p.arg("target"))?
                    .into_iter()
                    .next()
                    .ok_or_else(|| err("argument.entity.notfound.entity", &[]))?;
                let mine: Vec<(String, i32)> = self
                    .scoreboard
                    .scores
                    .iter()
                    .filter(|(k, _)| k.0 == h)
                    .map(|(k, v)| (k.1.clone(), *v))
                    .collect();
                if mine.is_empty() {
                    self.ok(
                        eid,
                        "commands.scoreboard.players.list.entity.empty",
                        vec![text::plain(&h)],
                    );
                } else {
                    self.ok(
                        eid,
                        "commands.scoreboard.players.list.entity.success",
                        vec![text::plain(&h), s(mine.len())],
                    );
                    for (o, v) in mine {
                        self.ok(
                            eid,
                            "commands.scoreboard.players.list.entity.entry",
                            vec![self.obj_text(&o), s(v)],
                        );
                    }
                }
            }
            "scoreboard players reset targets" | "scoreboard players reset targets objective" => {
                if let Some(name) = &obj {
                    self.need_objective(name)?;
                }
                let hs = self.holders(eid, p.arg("targets"))?;
                for h in &hs {
                    self.scoreboard.scores.retain(|k, _| {
                        !(k.0 == *h && obj.as_ref().map(|o| *o == k.1).unwrap_or(true))
                    });
                    let left = self.scoreboard.scores.keys().any(|k| k.0 == *h);
                    let mut w = Writer::with_id(cb::RESET_SCORE);
                    w.str(h);
                    match (&obj, left) {
                        (Some(o), true) => {
                            w.bool(true).str(o);
                        }
                        _ => {
                            w.bool(false);
                        }
                    }
                    self.sb_broadcast(w);
                }
                let who = if hs.len() == 1 {
                    text::plain(&hs[0])
                } else {
                    s(hs.len())
                };
                match &obj {
                    Some(o) => {
                        let t = self.obj_text(o);
                        self.ok(
                            eid,
                            &plural("commands.scoreboard.players.reset.specific", hs.len()),
                            vec![t, who],
                        );
                    }
                    None => self.ok(
                        eid,
                        &plural("commands.scoreboard.players.reset.all", hs.len()),
                        vec![who],
                    ),
                }
            }
            "scoreboard players operation targets targetObjective operation source sourceObjective" => {
                // measured on vanilla 26.2 (2026-11-24): unset scores count as 0, /= and %= by 0 fail
                // with arguments.operation.div0, >< swaps, the reply shows the target's new value
                let (to, so) = (p.arg("targetObjective").to_string(), p.arg("sourceObjective").to_string());
                self.need_objective(&to)?;
                self.need_objective(&so)?;
                let op = p.arg("operation").to_string();
                let targets = self.holders(eid, p.arg("targets"))?;
                let sources = self.holders(eid, p.arg("source"))?;
                if targets.is_empty() || sources.is_empty() {
                    return Err(err("argument.entity.notfound.entity", &[]));
                }
                let mut last = 0;
                for t in &targets {
                    for src in &sources {
                        let a = *self.scoreboard.scores.get(&(t.clone(), to.clone())).unwrap_or(&0);
                        let b = *self.scoreboard.scores.get(&(src.clone(), so.clone())).unwrap_or(&0);
                        let floor_div = |x: i32, y: i32| { let q = x.wrapping_div(y); if (x % y != 0) && ((x < 0) != (y < 0)) { q - 1 } else { q } };
                        let v = match op.as_str() {
                            "=" => b,
                            "+=" => a.wrapping_add(b),
                            "-=" => a.wrapping_sub(b),
                            "*=" => a.wrapping_mul(b),
                            "/=" | "%=" if b == 0 => return Err(err("arguments.operation.div0", &[])),
                            "/=" => floor_div(a, b),
                            "%=" => a.wrapping_sub(floor_div(a, b).wrapping_mul(b)),
                            "<" => a.min(b),
                            ">" => a.max(b),
                            "><" => {
                                self.scoreboard.scores.insert((src.clone(), so.clone()), a);
                                self.sb_broadcast(score_packet(src, &so, a));
                                b
                            }
                            _ => return Err(err("arguments.operation.invalid", &[])),
                        };
                        self.scoreboard.scores.insert((t.clone(), to.clone()), v);
                        self.sb_broadcast(score_packet(t, &to, v));
                        last = v;
                    }
                }
                let who = if targets.len() == 1 { text::plain(&targets[0]) } else { s(targets.len()) };
                let key = plural("commands.scoreboard.players.operation.success", targets.len());
                let args = if targets.len() == 1 { vec![self.obj_text(&to), who, s(last)] } else { vec![self.obj_text(&to), who] };
                self.ok(eid, &key, args);
            }
            _ => return Err(err("command.unknown.command", &[])),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slots() {
        assert_eq!(slot_id("sidebar"), Some(1));
        assert_eq!(slot_id("sidebar.team.red"), Some(3 + 12));
        assert_eq!(slot_id("nope"), None);
    }
}

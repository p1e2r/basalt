//! `/bossbar` (custom boss bars): add/remove/list/get/set, synced to the players on each bar and saved in
//! vanilla's data/minecraft/custom_boss_events.dat. Measured on vanilla 26.2 (basalt-probe packet dumps
//! and console replies, 2026-11-24):
//! - boss_event: UUID, varint action; 0 add: name NBT, f32 progress, varint colour, varint overlay,
//!   u8 flags; 1 remove; 2 progress f32; 3 name NBT; 4 style: varint colour, varint overlay; 5 flags u8.
//!   Colours pink 0, blue 1, red 2, green 3, yellow 4, purple 5, white 6 (the default); overlays
//!   progress 0, notched_6 1, notched_10 2, notched_12 3, notched_20 4; flags (darken sky 1, boss music 2,
//!   fog 4) stay 0 for custom bars. progress = value / max. `set visible false` sends remove, `true` a
//!   full add again; `set players` with an empty selection sends remove to everyone on the bar.
//! - custom_boss_events.dat: data{"<ns:id>": {Name (string or component compound), Color (string),
//!   Overlay (string), Value, Max (ints), Visible (byte), Players [int-array UUID]}} + DataVersion.
//!   Fields equal to their default are left out: Value 0, Max 100, Color white, Overlay progress, no
//!   Visible when hidden (default visible = written as 1), no Players when empty.
use super::{err, s, R};
use crate::cmdparse::Parsed;
use crate::game::Game;
use basalt_nbt::{Compound, Tag};
use basalt_protocol::{ids::play::clientbound as cb, text, Writer};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

pub const COLORS: [&str; 7] = ["pink", "blue", "red", "green", "yellow", "purple", "white"];
pub const OVERLAYS: [&str; 5] = [
    "progress",
    "notched_6",
    "notched_10",
    "notched_12",
    "notched_20",
];

#[derive(Clone)]
pub struct Bar {
    pub uuid: Uuid,
    pub name: Tag,
    pub color: u8,
    pub overlay: u8,
    pub value: i32,
    pub max: i32,
    pub visible: bool,
    pub players: BTreeSet<Uuid>,
}

impl Bar {
    fn new(name: Tag) -> Bar {
        Bar {
            uuid: Uuid::new_v4(),
            name,
            color: 6,
            overlay: 0,
            value: 0,
            max: 100,
            visible: true,
            players: BTreeSet::new(),
        }
    }
    fn progress(&self) -> f32 {
        (self.value as f32 / self.max as f32).clamp(0.0, 1.0)
    }
    fn add_packet(&self) -> Writer {
        let mut w = Writer::with_id(cb::BOSS_EVENT);
        w.uuid(self.uuid)
            .varint(0)
            .nbt(&self.name)
            .f32(self.progress())
            .varint(self.color as i32)
            .varint(self.overlay as i32)
            .u8(0);
        w
    }
    fn simple(&self, action: i32) -> Writer {
        let mut w = Writer::with_id(cb::BOSS_EVENT);
        w.uuid(self.uuid).varint(action);
        w
    }
    fn remove_packet(&self) -> Writer {
        self.simple(1)
    }
}

#[derive(Default, Clone)]
pub struct BossBars {
    /// `namespace:path` -> bar
    pub bars: BTreeMap<String, Bar>,
}

fn full_id(raw: &str) -> String {
    if raw.contains(':') {
        raw.to_string()
    } else {
        format!("minecraft:{raw}")
    }
}

fn idx(list: &[&str], name: &str) -> u8 {
    list.iter().position(|x| *x == name).unwrap_or(0) as u8
}

impl BossBars {
    /// Packets that bring a joining player up to date.
    pub fn join_packets(&self, uuid: Uuid) -> Vec<Writer> {
        self.bars
            .values()
            .filter(|b| b.visible && b.players.contains(&uuid))
            .map(|b| b.add_packet())
            .collect()
    }

    pub fn load(world: &std::path::Path) -> BossBars {
        let mut out = BossBars::default();
        let Some(t) = crate::level::read_gz(&world.join("data/minecraft/custom_boss_events.dat"))
        else {
            return out;
        };
        let Some(d) = t.get("data").and_then(|d| d.as_compound()) else {
            return out;
        };
        for (id, e) in d {
            let name = match e.get("Name") {
                Some(Tag::String(s)) => crate::snbt::parse(s)
                    .filter(|t| !matches!(t, Tag::String(_)))
                    .unwrap_or(Tag::String(s.clone())),
                Some(t) => t.clone(),
                None => Tag::String(id.clone()),
            };
            let mut b = Bar::new(name);
            if let Some(c) = e.get("Color").and_then(|v| v.as_str()) {
                b.color = idx(&COLORS, c).min(6);
                if !COLORS.contains(&c) {
                    b.color = 6;
                }
            }
            if let Some(c) = e.get("Overlay").and_then(|v| v.as_str()) {
                b.overlay = idx(&OVERLAYS, c);
            }
            b.value = e.get("Value").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            b.max = e.get("Max").and_then(|v| v.as_i64()).unwrap_or(100) as i32;
            b.visible = e.get("Visible").and_then(|v| v.as_i64()).unwrap_or(0) != 0;
            for p in e
                .get("Players")
                .and_then(|l| l.as_list())
                .into_iter()
                .flatten()
            {
                if let Tag::IntArray(a) = p {
                    if a.len() == 4 {
                        let mut bytes = [0u8; 16];
                        for (i, v) in a.iter().enumerate() {
                            bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
                        }
                        b.players.insert(Uuid::from_bytes(bytes));
                    }
                }
            }
            out.bars.insert(id.clone(), b);
        }
        out
    }

    pub fn save(&self, world: &std::path::Path) {
        let path = world.join("data/minecraft/custom_boss_events.dat");
        if self.bars.is_empty() && !path.exists() {
            return;
        }
        let mut root = crate::level::read_gz(&path)
            .and_then(|t| t.as_compound().cloned())
            .unwrap_or_default();
        let mut d = Compound::new();
        for (id, b) in &self.bars {
            let mut c = Compound::new();
            c.insert("Name".into(), b.name.clone());
            if b.color != 6 {
                c.insert("Color".into(), Tag::String(COLORS[b.color as usize].into()));
            }
            if b.overlay != 0 {
                c.insert(
                    "Overlay".into(),
                    Tag::String(OVERLAYS[b.overlay as usize].into()),
                );
            }
            if b.value != 0 {
                c.insert("Value".into(), Tag::Int(b.value));
            }
            if b.max != 100 {
                c.insert("Max".into(), Tag::Int(b.max));
            }
            if b.visible {
                c.insert("Visible".into(), Tag::Byte(1));
            }
            if !b.players.is_empty() {
                let l = b
                    .players
                    .iter()
                    .map(|u| {
                        let by = u.as_bytes();
                        Tag::IntArray(
                            (0..4)
                                .map(|i| {
                                    i32::from_be_bytes(by[i * 4..i * 4 + 4].try_into().unwrap())
                                })
                                .collect(),
                        )
                    })
                    .collect();
                c.insert("Players".into(), Tag::List(l));
            }
            d.insert(id.clone(), Tag::Compound(c));
        }
        root.insert("data".into(), Tag::Compound(d));
        root.insert("DataVersion".into(), Tag::Int(basalt_data::WORLD_VERSION));
        if let Err(e) = crate::level::write_gz(&path, &Tag::Compound(root)) {
            log::warn!("writing custom_boss_events.dat: {e}");
        }
    }
}

impl Game {
    /// Send a packet built from the bar to every online player on it (hidden bars send nothing).
    fn bb_send(&self, id: &str, make: impl Fn(&Bar) -> Writer) {
        let Some(b) = self.bossbars.bars.get(id) else {
            return;
        };
        if !b.visible {
            return;
        }
        let w = make(b);
        for p in self.all_players() {
            if b.players.contains(&p.uuid) {
                p.send(Writer { buf: w.buf.clone() });
            }
        }
    }

    fn bb_name(&self, id: &str) -> Tag {
        let n = self
            .bossbars
            .bars
            .get(id)
            .map(|b| b.name.clone())
            .unwrap_or(Tag::String(id.into()));
        super::tr("chat.square_brackets", vec![n])
    }

    fn bb_need(&self, id: &str) -> R<()> {
        if self.bossbars.bars.contains_key(id) {
            Ok(())
        } else {
            Err(err("commands.bossbar.unknown", &[id]))
        }
    }

    fn bb_online_names(&self, b: &Bar) -> Vec<String> {
        let mut v: Vec<String> = self
            .all_players()
            .filter(|p| b.players.contains(&p.uuid))
            .map(|p| p.name.clone())
            .collect();
        v.sort();
        v
    }

    pub(super) fn cmd_bossbar(&mut self, eid: i32, p: &Parsed) -> R<()> {
        let path = p.path_str();
        let id = p.has("id").then(|| full_id(p.arg("id")));
        match path.as_str() {
            "bossbar add id name" => {
                let id = id.unwrap();
                if self.bossbars.bars.contains_key(&id) {
                    return Err(err("commands.bossbar.create.failed", &[&id]));
                }
                let name = crate::snbt::parse(p.arg("name"))
                    .ok_or_else(|| err("argument.component.invalid", &[p.arg("name")]))?;
                self.bossbars.bars.insert(id.clone(), Bar::new(name));
                let n = self.bb_name(&id);
                self.ok(eid, "commands.bossbar.create.success", vec![n]);
            }
            "bossbar remove id" => {
                let id = id.unwrap();
                self.bb_need(&id)?;
                let n = self.bb_name(&id);
                self.bb_send(&id, |b| b.remove_packet());
                self.bossbars.bars.remove(&id);
                self.ok(eid, "commands.bossbar.remove.success", vec![n]);
            }
            "bossbar list" => {
                let ids: Vec<String> = self.bossbars.bars.keys().cloned().collect();
                if ids.is_empty() {
                    self.ok(eid, "commands.bossbar.list.bars.none", vec![]);
                } else {
                    let mut list = Vec::new();
                    for (i, id) in ids.iter().enumerate() {
                        if i > 0 {
                            list.push(text::plain(", "));
                        }
                        list.push(self.bb_name(id));
                    }
                    let joined = basalt_nbt::compound([
                        ("text", Tag::String(String::new())),
                        ("extra", Tag::List(list)),
                    ]);
                    self.ok(
                        eid,
                        "commands.bossbar.list.bars.some",
                        vec![s(ids.len()), joined],
                    );
                }
            }
            "bossbar get id value"
            | "bossbar get id max"
            | "bossbar get id visible"
            | "bossbar get id players" => {
                let id = id.unwrap();
                self.bb_need(&id)?;
                let n = self.bb_name(&id);
                let b = self.bossbars.bars[&id].clone();
                match p.path[3].as_str() {
                    "value" => self.ok(eid, "commands.bossbar.get.value", vec![n, s(b.value)]),
                    "max" => self.ok(eid, "commands.bossbar.get.max", vec![n, s(b.max)]),
                    "visible" => {
                        let k = if b.visible { "visible" } else { "hidden" };
                        self.ok(eid, &format!("commands.bossbar.get.visible.{k}"), vec![n]);
                    }
                    _ => {
                        let names = self.bb_online_names(&b);
                        if names.is_empty() {
                            self.ok(eid, "commands.bossbar.get.players.none", vec![n]);
                        } else {
                            self.ok(
                                eid,
                                "commands.bossbar.get.players.some",
                                vec![n, s(names.len()), s(names.join(", "))],
                            );
                        }
                    }
                }
            }
            _ if path.starts_with("bossbar set ") => {
                self.bb_set(eid, p, &id.unwrap_or_default(), &path)?
            }
            _ => return Err(err("command.unknown.command", &[])),
        }
        Ok(())
    }

    fn bb_set(&mut self, eid: i32, p: &Parsed, id: &str, path: &str) -> R<()> {
        self.bb_need(id)?;
        let what = p.path[3].as_str();
        let n_before = self.bb_name(id);
        match what {
            "name" => {
                let name = crate::snbt::parse(p.arg("name"))
                    .ok_or_else(|| err("argument.component.invalid", &[p.arg("name")]))?;
                if self.bossbars.bars[id].name == name {
                    return Err(err("commands.bossbar.set.name.unchanged", &[]));
                }
                self.bossbars.bars.get_mut(id).unwrap().name = name.clone();
                self.bb_send(id, |_| {
                    let mut w = Writer::with_id(cb::BOSS_EVENT);
                    w.uuid(self.bossbars.bars[id].uuid).varint(3).nbt(&name);
                    w
                });
                let n = self.bb_name(id);
                self.ok(eid, "commands.bossbar.set.name.success", vec![n]);
            }
            "color" | "style" => {
                let word = p.path.get(4).cloned().unwrap_or_default();
                let b = self.bossbars.bars.get_mut(id).unwrap();
                if what == "color" {
                    let c = idx(&COLORS, &word);
                    if b.color == c {
                        return Err(err("commands.bossbar.set.color.unchanged", &[]));
                    }
                    b.color = c;
                } else {
                    let o = idx(&OVERLAYS, &word);
                    if b.overlay == o {
                        return Err(err("commands.bossbar.set.style.unchanged", &[]));
                    }
                    b.overlay = o;
                }
                self.bb_send(id, |b| {
                    let mut w = b.simple(4);
                    w.varint(b.color as i32).varint(b.overlay as i32);
                    w
                });
                let n = self.bb_name(id);
                self.ok(
                    eid,
                    &format!("commands.bossbar.set.{what}.success"),
                    vec![n],
                );
            }
            "value" | "max" => {
                let v: i32 = p
                    .arg(what)
                    .parse()
                    .map_err(|_| err("parsing.int.invalid", &[p.arg(what)]))?;
                let b = self.bossbars.bars.get_mut(id).unwrap();
                let before = b.progress();
                let slot = if what == "value" {
                    &mut b.value
                } else {
                    &mut b.max
                };
                if *slot == v {
                    return Err(err(&format!("commands.bossbar.set.{what}.unchanged"), &[]));
                }
                *slot = v;
                // vanilla only sends the progress packet when the clamped fraction changed (max 100 -> 20
                // with value 500 stays at 1.0 and sends nothing)
                if b.progress() != before {
                    self.bb_send(id, |b| {
                        let mut w = b.simple(2);
                        w.f32(b.progress());
                        w
                    });
                }
                let n = self.bb_name(id);
                self.ok(
                    eid,
                    &format!("commands.bossbar.set.{what}.success"),
                    vec![n, s(v)],
                );
            }
            "visible" => {
                let v = p.arg("visible") == "true";
                if self.bossbars.bars[id].visible == v {
                    let k = if v { "visible" } else { "hidden" };
                    return Err(err(
                        &format!("commands.bossbar.set.visibility.unchanged.{k}"),
                        &[],
                    ));
                }
                // the packet goes to the players before the flag flips for hiding, after it for showing
                if !v {
                    self.bb_send(id, |b| b.remove_packet());
                }
                self.bossbars.bars.get_mut(id).unwrap().visible = v;
                if v {
                    self.bb_send(id, |b| b.add_packet());
                }
                let k = if v { "visible" } else { "hidden" };
                self.ok(
                    eid,
                    &format!("commands.bossbar.set.visible.success.{k}"),
                    vec![n_before],
                );
            }
            "players" => {
                let new: BTreeSet<Uuid> = if p.has("targets") {
                    let t = self.resolve(eid, p.arg("targets"), true)?;
                    Self::players_of(&t)
                        .iter()
                        .filter_map(|e| self.all_players().find(|q| q.eid == *e).map(|q| q.uuid))
                        .collect()
                } else {
                    BTreeSet::new()
                };
                let b = self.bossbars.bars[id].clone();
                if new == b.players {
                    return Err(err("commands.bossbar.set.players.unchanged", &[]));
                }
                if b.visible {
                    for q in self.all_players() {
                        if b.players.contains(&q.uuid) && !new.contains(&q.uuid) {
                            q.send(b.remove_packet());
                        }
                        if new.contains(&q.uuid) && !b.players.contains(&q.uuid) {
                            q.send(b.add_packet());
                        }
                    }
                }
                self.bossbars.bars.get_mut(id).unwrap().players = new;
                let b = self.bossbars.bars[id].clone();
                let names = self.bb_online_names(&b);
                let n = self.bb_name(id);
                if names.is_empty() {
                    self.ok(eid, "commands.bossbar.set.players.success.none", vec![n]);
                } else {
                    self.ok(
                        eid,
                        "commands.bossbar.set.players.success.some",
                        vec![n, s(names.len()), s(names.join(", "))],
                    );
                }
            }
            _ => {
                let _ = path;
                return Err(err("command.unknown.command", &[]));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progress_and_ids() {
        let mut b = Bar::new(Tag::String("x".into()));
        b.value = 5;
        assert_eq!(b.progress(), 0.05);
        b.value = 500;
        assert_eq!(b.progress(), 1.0);
        assert_eq!(full_id("a"), "minecraft:a");
        assert_eq!(full_id("t:a"), "t:a");
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("basalt-bb-{}", std::process::id()));
        let mut bb = BossBars::default();
        let mut b = Bar::new(crate::snbt::parse("{\"text\":\"Hi\"}").unwrap());
        b.color = 3;
        b.overlay = 2;
        b.value = 7;
        b.max = 30;
        b.visible = false;
        b.players
            .insert(Uuid::from_u128(0x1234_5678_9abc_def0_1122_3344_5566_7788));
        bb.bars.insert("minecraft:x".into(), b);
        bb.bars
            .insert("t:plain".into(), Bar::new(Tag::String("plain".into())));
        bb.save(&dir);
        let l = BossBars::load(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        let (a, b) = (&bb.bars["minecraft:x"], &l.bars["minecraft:x"]);
        assert_eq!(
            (a.color, a.overlay, a.value, a.max, a.visible),
            (b.color, b.overlay, b.value, b.max, b.visible)
        );
        assert_eq!(a.players, b.players);
        assert_eq!(a.name, b.name);
        assert!(l.bars["t:plain"].visible && l.bars["t:plain"].max == 100);
    }
}

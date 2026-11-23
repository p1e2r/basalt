//! Chat commands: the input is parsed against the Brigadier tree Basalt sends to clients
//! (crates/server/src/cmdparse.rs) and the executor below acts on the parsed path. Replies are vanilla's
//! translation keys with their arguments (success keys and failure keys alike; failures are red), so the
//! real client renders them from its own language file. Basalt-only commands (`spawn`, `save`, `mobs`,
//! `basalt:*`) answer with plain text.
use super::{block_default, Game, Stack, NO_ENCH};
use crate::cmdparse::{self, CmdError, Coord, Parsed, PosKind, Sel, Selector, Sort};
use basalt_data::{registries as reg, BlockState};
use basalt_nbt::Tag;
use basalt_protocol::{ids::play::clientbound as cb, text, Writer};

type R<T> = Result<T, CmdError>;

fn err(key: &str, args: &[&str]) -> CmdError { CmdError::new(key, args) }
fn s(v: impl ToString) -> Tag { Tag::String(v.to_string()) }
/// NBT lists must be homogeneous: when arguments mix plain strings and components, wrap the strings as `{text: ..}`.
fn uniform(args: Vec<Tag>) -> Vec<Tag> {
    if args.iter().all(|a| matches!(a, Tag::String(_))) || args.iter().all(|a| matches!(a, Tag::Compound(_))) { return args; }
    args.into_iter().map(|a| match a { Tag::String(t) => { let mut c = basalt_nbt::Compound::new(); c.insert("text".into(), Tag::String(t)); Tag::Compound(c) } o => o }).collect()
}
fn tr(key: &str, args: Vec<Tag>) -> Tag { text::translate(key, uniform(args)) }
fn red(t: Tag) -> Tag { match t { Tag::Compound(mut c) => { c.insert("color".into(), Tag::String("red".into())); Tag::Compound(c) } o => o } }
/// `.single` when exactly one target matched, else `.multiple`.
fn plural(base: &str, n: usize) -> String { format!("{base}.{}", if n == 1 { "single" } else { "multiple" }) }

#[derive(Clone, Copy, Debug, PartialEq)]
enum Target { Player(i32), Mob(i32), Item(i32), Arrow(i32) }

/// Tick values of the vanilla overworld day markers (`time set day|noon|night|midnight`).
const TIME_MARKERS: &[(&str, i64)] = &[("day", 1000), ("noon", 6000), ("night", 13000), ("midnight", 18000)];

impl Game {
    fn resource_exists(&self, registry: &str, id: &str) -> bool {
        let short = registry.trim_start_matches("minecraft:");
        match short {
            "entity_type" => basalt_data::registry_id(reg::ENTITY_TYPE, id).is_some(),
            "item" => basalt_data::registry_id(reg::ITEM, id).is_some(),
            "mob_effect" => super::env::effect_id(id).is_some(),
            "enchantment" => self.sh.registries.id("minecraft:enchantment", id).is_some(),
            // other data-driven registries (timeline, world_clock, ...): a data file with that id
            _ => self.sh.cfg.data_dir.join("data/minecraft").join(short).join(format!("{}.json", id.trim_start_matches("minecraft:"))).exists(),
        }
    }

    fn fail(&self, eid: i32, e: CmdError) {
        let msg = if let Some(rest) = e.key.strip_prefix("basalt:") { text::plain(&format!("{rest}{}", e.args.first().map(|a| format!(" {a}")).unwrap_or_default())) } else { tr(&e.key, e.args.iter().map(s).collect()) };
        self.tell(eid, red(msg));
    }

    fn ok(&self, eid: i32, key: &str, args: Vec<Tag>) { self.tell(eid, tr(key, args)); }

    pub(super) fn command(&mut self, eid: i32, cmd: &str) {
        let name = self.players.get(&eid).map(|p| p.name.clone()).unwrap_or_default();
        log::info!("{name} issued /{cmd}");
        let first = cmd.split(' ').next().unwrap_or("");
        // Basalt-only commands (plain text, not in vanilla)
        match first {
            "spawn" if cmd == "spawn" => { let sp = self.spawn; let p = self.players.get_mut(&eid).unwrap(); p.pos = sp; Self::teleport(p); self.stream_chunks(eid); return; }
            "save" if cmd == "save" => { self.save_level(); self.tell(eid, text::plain("Saved the world")); return; }
            "mobs" => { self.cmd_mobs(eid, cmd.split_whitespace().nth(1)); return; }
            c if c.starts_with("basalt:") && self.sh.cfg.debug => { let args: Vec<&str> = cmd.split_whitespace().collect(); self.debug_command(eid, &args); return; }
            _ => {}
        }
        let Some(tree) = cmdparse::tree(&self.sh.cfg.data_dir) else { self.fail(eid, err("basalt:Command tree unavailable (reports/commands.json missing)", &[])); return };
        let parsed = tree.parse(cmd, &|r, id| self.resource_exists(r, id));
        match parsed.and_then(|p| self.run_command(eid, &p)) { Ok(()) => {} Err(e) => self.fail(eid, e) }
    }

    fn cmd_mobs(&mut self, eid: i32, radius: Option<&str>) {
        // debug: mobs near the sender — kind, eid, position, health, AI state
        let r: f64 = radius.and_then(|a| a.parse().ok()).unwrap_or(32.0);
        let me = self.players[&eid].pos;
        let mut v: Vec<&super::mob::Mob> = self.mobs.values().filter(|m| (m.pos[0] - me[0]).powi(2) + (m.pos[2] - me[2]).powi(2) <= r * r).collect();
        v.sort_by_key(|m| m.eid);
        let lines: Vec<String> = v.iter().map(|m| format!("{} #{} {:.1} {:.1} {:.1} hp {:.1}{}{}{}{}", m.kind.name(), m.eid, m.pos[0], m.pos[1], m.pos[2], m.health,
            m.ai.target.map(|t| format!(" target {t}")).unwrap_or_default(), if m.fire_ticks > 0 { " burning" } else { "" }, if m.baby { " baby" } else { "" }, if m.dead.is_some() { " dying" } else { "" })).collect();
        self.tell(eid, text::plain(&format!("{} mobs within {r}: {}", lines.len(), lines.join("; "))));
    }

    // ------------------------------------------------------------ selectors

    fn selector(raw: &str) -> Selector { cmdparse::parse_selector(raw, 0, false, false, &|_, _| true).map(|x| x.0).unwrap_or_else(|_| unreachable!("validated by the tree parser")) }

    fn entity_pos(&self, t: Target) -> [f64; 3] {
        match t {
            Target::Player(e) => self.players.get(&e).map(|p| p.pos).unwrap_or_default(),
            Target::Mob(e) => self.mobs.get(&e).map(|m| m.pos).unwrap_or_default(),
            Target::Item(e) => self.items.get(&e).map(|i| i.pos).unwrap_or_default(),
            Target::Arrow(e) => self.arrows.get(&e).map(|a| a.pos).unwrap_or_default(),
        }
    }

    fn entity_type_name(&self, t: Target) -> String {
        match t {
            Target::Player(_) => "minecraft:player".into(),
            Target::Mob(e) => self.mobs.get(&e).map(|m| format!("minecraft:{}", m.kind.name())).unwrap_or_default(),
            Target::Item(_) => "minecraft:item".into(),
            Target::Arrow(_) => "minecraft:arrow".into(),
        }
    }

    /// Display name component: a player's name, an entity type's translatable name, an item entity's item name.
    fn display_name(&self, t: Target) -> Tag {
        match t {
            Target::Player(e) => text::plain(self.players.get(&e).map(|p| p.name.as_str()).unwrap_or("")),
            Target::Item(e) => self.items.get(&e).map(|i| item_name(i.stack.item)).unwrap_or_else(|| text::plain("")),
            _ => tr(&format!("entity.minecraft.{}", self.entity_type_name(t).trim_start_matches("minecraft:")), vec![]),
        }
    }

    fn resolve(&mut self, sender: i32, raw: &str, players_only: bool) -> R<Vec<Target>> {
        let sel = Self::selector(raw);
        let me = self.players[&sender].pos;
        let mut all: Vec<Target> = Vec::new();
        let mut pids: Vec<i32> = self.players.keys().copied().collect(); pids.sort();
        match sel.kind {
            Sel::Name => {
                let n = sel.name.to_lowercase();
                let id = uuid::Uuid::parse_str(&sel.name).ok();
                if let Some(p) = self.players.values().find(|p| p.name.to_lowercase() == n || Some(p.uuid) == id) { all.push(Target::Player(p.eid)); }
                else if let Some(u) = id { if let Some(m) = self.mobs.values().find(|m| m.uuid == u) { all.push(Target::Mob(m.eid)); } }
            }
            Sel::Me => all.push(Target::Player(sender)),
            Sel::Nearest | Sel::Random | Sel::All => all.extend(pids.iter().map(|e| Target::Player(*e))),
            Sel::Entities | Sel::NearestEntity => {
                all.extend(pids.iter().map(|e| Target::Player(*e)));
                let mut m: Vec<i32> = self.mobs.values().filter(|m| m.alive()).map(|m| m.eid).collect(); m.sort(); all.extend(m.into_iter().map(Target::Mob));
                let mut i: Vec<i32> = self.items.keys().copied().collect(); i.sort(); all.extend(i.into_iter().map(Target::Item));
                let mut a: Vec<i32> = self.arrows.keys().copied().collect(); a.sort(); all.extend(a.into_iter().map(Target::Arrow));
            }
        }
        let dist2 = |g: &Game, t: Target| { let p = g.entity_pos(t); (0..3).map(|i| (p[i] - me[i]).powi(2)).sum::<f64>() };
        all.retain(|&t| {
            let ty = self.entity_type_name(t);
            if !sel.types.iter().all(|(neg, id)| (ty == *id) != *neg) { return false; }
            if let Some((lo, hi)) = sel.distance { let d = dist2(self, t); if lo.is_some_and(|l| d < l * l) || hi.is_some_and(|h| d > h * h) { return false; } }
            if !sel.gamemodes.is_empty() {
                let Target::Player(e) = t else { return false };
                let gm = ["survival", "creative", "adventure", "spectator"][self.players[&e].gamemode as usize];
                if !sel.gamemodes.iter().all(|(neg, g)| (gm == g) != *neg) { return false; }
            }
            if !sel.names.is_empty() {
                let n = match t { Target::Player(e) => self.players[&e].name.clone(), _ => String::new() };
                if !sel.names.iter().all(|(neg, w)| (n == *w) != *neg) { return false; }
            }
            true
        });
        let sort = sel.sort.clone().unwrap_or(Sort::Arbitrary);
        match sort {
            Sort::Nearest => all.sort_by(|a, b| dist2(self, *a).partial_cmp(&dist2(self, *b)).unwrap_or(std::cmp::Ordering::Equal)),
            Sort::Furthest => all.sort_by(|a, b| dist2(self, *b).partial_cmp(&dist2(self, *a)).unwrap_or(std::cmp::Ordering::Equal)),
            Sort::Random => { use rand::seq::SliceRandom; all.shuffle(&mut self.rng); }
            Sort::Arbitrary => {}
        }
        all.truncate(sel.max_results());
        if players_only { all.retain(|t| matches!(t, Target::Player(_))); }
        if all.is_empty() { return Err(err(if players_only { "argument.entity.notfound.player" } else { "argument.entity.notfound.entity" }, &[])); }
        Ok(all)
    }

    fn players_of(targets: &[Target]) -> Vec<i32> { targets.iter().filter_map(|t| if let Target::Player(e) = t { Some(*e) } else { None }).collect() }

    // ------------------------------------------------------------ coordinates

    /// Resolve parsed coordinates against the sender (`~` = offset from its feet, `^` = along its look direction).
    fn pos_from(&self, sender: i32, coords: &[Coord], center: bool) -> [f64; 3] {
        let p = &self.players[&sender];
        if coords[0].local {
            let (yaw, pitch) = ((p.yaw as f64).to_radians(), (p.pitch as f64).to_radians());
            let look = |y: f64, pi: f64| [-y.sin() * pi.cos(), -pi.sin(), y.cos() * pi.cos()];
            let (left, up, fwd) = (look(yaw - std::f64::consts::FRAC_PI_2, 0.0), look(yaw, pitch - std::f64::consts::FRAC_PI_2), look(yaw, pitch));
            return std::array::from_fn(|i| p.pos[i] + left[i] * coords[0].v + up[i] * coords[1].v + fwd[i] * coords[2].v);
        }
        std::array::from_fn(|i| {
            let c = coords[i];
            let v = if c.rel { p.pos[i] + c.v } else { c.v };
            // absolute integer x/z in a vec3 argument address the block centre
            if center && !c.rel && !c.dotted && i != 1 { v + 0.5 } else { v }
        })
    }

    fn block_pos_from(&self, sender: i32, raw: &str) -> R<[i32; 3]> {
        let (c, _) = cmdparse::parse_pos(raw, 0, PosKind::Block)?;
        let p = self.pos_from(sender, &c, false);
        Ok([p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32])
    }

    fn check_block_pos(&mut self, b: [i32; 3]) -> R<()> {
        if b[0].abs() > 30_000_000 || b[2].abs() > 30_000_000 { return Err(err("argument.pos.unloaded", &[])); }
        if b[1] < basalt_world::MIN_Y || b[1] >= basalt_world::MIN_Y + basalt_world::HEIGHT { return Err(err("argument.pos.outofworld", &[])); }
        self.ensure_chunk(b[0] >> 4, b[2] >> 4);
        Ok(())
    }

    // ------------------------------------------------------------ dispatch

    fn run_command(&mut self, eid: i32, p: &Parsed) -> R<()> {
        let sender_name = self.players[&eid].name.clone();
        match p.path_str().as_str() {
            "gamemode gamemode" | "gamemode gamemode target" => {
                let gm = ["survival", "creative", "adventure", "spectator"].iter().position(|m| *m == p.arg("gamemode")).unwrap_or(0) as u8;
                let targets = if p.has("target") { self.resolve(eid, p.arg("target"), true)? } else { vec![Target::Player(eid)] };
                let ids = Self::players_of(&targets);
                let mode = tr(&format!("gameMode.{}", p.arg("gamemode")), vec![]);
                let mut changed = Vec::new();
                for e in ids { if self.players[&e].gamemode != gm { self.set_gamemode(e, gm); changed.push(e); } }
                // vanilla says nothing when nobody's mode changed
                match changed.as_slice() {
                    [] => {}
                    [e] if *e == eid => self.ok(eid, "commands.gamemode.success.self", vec![mode.clone()]),
                    _ => {
                        for &e in &changed {
                            if e != eid { self.tell(e, tr("gameMode.changed", vec![mode.clone()])); }
                        }
                        if changed.len() == 1 { let n = text::plain(&self.players[&changed[0]].name); self.ok(eid, "commands.gamemode.success.other", vec![n, mode]); }
                        else { self.ok(eid, "commands.gamemode.success.other", vec![s(changed.len()), mode]); }
                    }
                }
            }
            path if path.starts_with("teleport") => self.cmd_teleport(eid, p)?,
            "give targets item" | "give targets item count" => {
                let targets = self.resolve(eid, p.arg("targets"), true)?;
                let item_raw = p.arg("item");
                let id_text = item_raw.split('[').next().unwrap_or("");
                let item = basalt_data::registry_id(reg::ITEM, id_text).ok_or_else(|| err("argument.item.id.invalid", &[id_text]))?;
                let count: i32 = if p.has("count") { p.arg("count").parse().unwrap_or(1) } else { 1 };
                let max = self.max_stack(item);
                if count as i64 > max as i64 * 100 { return Err(CmdError { key: "commands.give.failed.toomanyitems".into(), args: vec![(max * 100).to_string(), format!("{id_text}")] }); }
                let ids = Self::players_of(&targets);
                for &e in &ids {
                    let left = self.give_stack(e, Stack { item, count, damage: 0, ench: NO_ENCH });
                    if left > 0 { self.throw_item(e, Stack { item, count: left, damage: 0, ench: NO_ENCH }, false); }
                }
                let comp = tr("chat.square_brackets", vec![item_name(item)]);
                let who = if ids.len() == 1 { text::plain(&self.players[&ids[0]].name) } else { s(ids.len()) };
                self.ok(eid, &plural("commands.give.success", ids.len()), vec![s(count), comp, who]);
            }
            path if path.starts_with("time") => self.cmd_time(eid, p, path)?,
            "say message" => {
                let m = p.arg("message").to_string();
                self.system_chat(tr("chat.type.announcement", vec![text::plain(&sender_name), text::plain(&m)]));
            }
            "list" | "list uuids" => {
                let mut ps: Vec<&super::Player> = self.players.values().collect();
                ps.sort_by_key(|p| p.eid);
                let names: Tag = if p.path.len() > 1 {
                    // `commands.list.nameAndId` entries separated by ", "
                    let mut parts: Vec<Tag> = Vec::new();
                    for (i, pl) in ps.iter().enumerate() { if i > 0 { parts.push(text::plain(", ")); } parts.push(tr("commands.list.nameAndId", vec![text::plain(&pl.name), s(pl.uuid)])); }
                    let mut c = basalt_nbt::Compound::new(); c.insert("text".into(), s("")); c.insert("extra".into(), Tag::List(uniform(parts))); Tag::Compound(c)
                } else { text::plain(&ps.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")) };
                self.ok(eid, "commands.list.players", vec![s(ps.len()), s(self.sh.cfg.max_players), names]);
            }
            "save-all" | "save-all flush" => {
                self.ok(eid, "commands.save.saving", vec![]);
                self.save_level();
                self.ok(eid, "commands.save.success", vec![]);
            }
            path if path.starts_with("effect") => self.cmd_effect(eid, p, path)?,
            "enchant targets enchantment" | "enchant targets enchantment level" => self.cmd_enchant(eid, p)?,
            "setblock pos block" | "setblock pos block destroy" | "setblock pos block keep" | "setblock pos block replace" | "setblock pos block strict" => {
                let b = self.block_pos_from(eid, p.arg("pos"))?;
                self.check_block_pos(b)?;
                let state = BlockState::parse(p.arg("block").split('{').next().unwrap_or("").trim_start_matches("minecraft:")).map(|b| b.0).ok_or_else(|| err("argument.block.id.invalid", &[p.arg("block")]))?;
                let old = self.world.get_block(b[0], b[1], b[2]);
                let keep = p.path.last().map(|l| l == "keep").unwrap_or(false);
                if (keep && !BlockState(old).is_air()) || old == state { return Err(err("commands.setblock.failed", &[])); }
                self.set_block(b[0], b[1], b[2], state);
                self.ok(eid, "commands.setblock.success", vec![s(b[0]), s(b[1]), s(b[2])]);
            }
            path if path.starts_with("fill") => self.cmd_fill(eid, p)?,
            "kill" | "kill targets" => {
                let targets = if p.has("targets") { self.resolve(eid, p.arg("targets"), false)? } else { vec![Target::Player(eid)] };
                for &t in &targets {
                    match t {
                        Target::Player(e) => { let h = self.players[&e].health; self.hurt_bypass(e, h.max(1.0) + 1000.0, "generic_kill"); }
                        Target::Mob(e) => { self.damage_mob(e, 1.0e6, "generic_kill", None, None, None); }
                        Target::Item(e) => self.remove_item(e),
                        Target::Arrow(e) => { self.arrows.remove(&e); self.untrack(e); }
                    }
                }
                if targets.len() == 1 { let n = self.display_name(targets[0]); self.ok(eid, "commands.kill.success.single", vec![n]); }
                else { self.ok(eid, "commands.kill.success.multiple", vec![s(targets.len())]); }
            }
            "summon entity" | "summon entity pos" | "summon entity pos nbt" => self.cmd_summon(eid, p)?,
            "item replace block pos slot with item" | "item replace block pos slot with item count" => self.cmd_item_replace_block(eid, p)?,
            "help" | "help command" => self.cmd_help(eid, p)?,
            path if path.starts_with("weather") => {
                let kind = p.path[1].as_str();
                let ticks = if p.has("duration") { Some(cmdparse::parse_time(p.arg("duration"), 0, 1).map(|t| t.0 as i32).unwrap_or(1)) } else { None };
                self.set_weather(kind, ticks);
                self.ok(eid, &format!("commands.weather.set.{kind}"), vec![]);
            }
            "clear" | "clear targets" | "clear targets item" | "clear targets item maxCount" => self.cmd_clear(eid, p)?,
            other => return Err(err("basalt:Not supported by Basalt yet:", &[other])),
        }
        Ok(())
    }

    fn set_gamemode(&mut self, eid: i32, gm: u8) {
        let p = self.players.get_mut(&eid).unwrap();
        p.gamemode = gm; p.flying = gm == 3 || (gm == 1 && p.flying);
        let mut w = Writer::with_id(cb::GAME_EVENT); w.u8(3).f32(gm as f32); p.send(w);
        Self::send_abilities(p);
        let uuid = p.uuid;
        self.broadcast(None, || { let mut w = Writer::with_id(cb::PLAYER_INFO_UPDATE); w.u8(4).varint(1).uuid(uuid).varint(gm as i32); w });
    }

    // ------------------------------------------------------------ /tp

    fn cmd_teleport(&mut self, eid: i32, p: &Parsed) -> R<()> {
        // `tp <dest>` / `tp <x y z>` act on the sender; with a leading `targets` it acts on those entities
        let targets = if p.has("targets") { self.resolve(eid, p.arg("targets"), false)? } else { vec![Target::Player(eid)] };
        let valid = |v: [f64; 3]| v[0].abs() <= 30_000_000.0 && v[2].abs() <= 30_000_000.0 && v[1].abs() <= 20_000_000.0 && v.iter().all(|x| x.is_finite());
        let (mut yaw, mut pitch): (Option<f32>, Option<f32>) = (None, None);
        let dest_entity: Option<Target>;
        let dest: [f64; 3];
        if p.has("destination") {
            let d = self.resolve(eid, p.arg("destination"), false)?[0];
            dest = self.entity_pos(d); dest_entity = Some(d);
            if let Target::Player(e) = d { yaw = Some(self.players[&e].yaw); pitch = Some(self.players[&e].pitch); }
            else if let Target::Mob(e) = d { yaw = Some(self.mobs[&e].yaw); pitch = Some(self.mobs[&e].pitch); }
        } else {
            let (c, _) = cmdparse::parse_pos(p.arg("location"), 0, PosKind::Vec3)?;
            dest = self.pos_from(eid, &c, true); dest_entity = None;
            if !valid(dest) { return Err(err("commands.teleport.invalidPosition", &[])); }
            if p.has("rotation") {
                let (r, _) = cmdparse::parse_pos(p.arg("rotation"), 0, PosKind::Rot)?;
                                let (cy, cp) = { let t = &self.players[&eid]; (t.yaw as f64, t.pitch as f64) };
                let y = if r[0].rel { cy + r[0].v } else { r[0].v };
                let pi = if r[1].rel { cp + r[1].v } else { r[1].v };
                yaw = Some(((y + 180.0).rem_euclid(360.0) - 180.0) as f32); pitch = Some(pi.clamp(-90.0, 90.0) as f32);
            } else if p.has("facingLocation") {
                let (c, _) = cmdparse::parse_pos(p.arg("facingLocation"), 0, PosKind::Vec3)?;
                let f = self.pos_from(eid, &c, true);
                let (y, pi) = look_angles(dest, f); yaw = y; pitch = pi;
            } else if p.has("facingEntity") {
                let f = self.resolve(eid, p.arg("facingEntity"), false)?[0];
                let mut fp = self.entity_pos(f);
                if p.arg("facingAnchor") == "eyes" { fp[1] += if matches!(f, Target::Player(_)) { 1.62 } else { 0.0 }; }
                let (y, pi) = look_angles(dest, fp); yaw = y; pitch = pi;
            }
        }
        for &t in &targets {
            match t {
                Target::Player(e) => {
                    let pl = self.players.get_mut(&e).unwrap();
                    pl.pos = dest; if let Some(y) = yaw { pl.yaw = y; } if let Some(pi) = pitch { pl.pitch = pi; }
                    Self::teleport(pl); self.stream_chunks(e);
                }
                Target::Mob(e) => { if let Some(m) = self.mobs.get_mut(&e) { m.pos = dest; m.vel = [0.0; 3]; if let Some(y) = yaw { m.yaw = y; } } }
                Target::Item(e) => { if let Some(i) = self.items.get_mut(&e) { i.pos = dest; } }
                Target::Arrow(e) => { if let Some(a) = self.arrows.get_mut(&e) { a.pos = dest; } }
            }
        }
        let who = if targets.len() == 1 { self.display_name(targets[0]) } else { s(targets.len()) };
        let key = if dest_entity.is_some() { "commands.teleport.success.entity" } else { "commands.teleport.success.location" };
        let mut args = vec![who];
        match dest_entity { Some(d) => args.push(self.display_name(d)), None => args.extend(dest.iter().map(|v| s(format!("{v:.6}")))) }
        self.ok(eid, &plural(key, targets.len()), args);
        Ok(())
    }

    // ------------------------------------------------------------ /time

    fn cmd_time(&mut self, eid: i32, p: &Parsed, path: &str) -> R<()> {
        let clock = "minecraft:overworld";
        match path {
            "time set time" | "time add time" => {
                let (t, _) = cmdparse::parse_time(p.arg("time"), 0, if path == "time set time" { 0 } else { i32::MIN as i64 })?;
                self.day_time = if path == "time set time" { t } else { self.day_time + t };
                self.broadcast_time();
                self.ok(eid, "commands.time.set.absolute", vec![s(clock), s(self.day_time)]);
            }
            "time set timemarker" => {
                let raw = p.arg("timemarker");
                let id = if raw.contains(':') { raw.to_string() } else { format!("minecraft:{raw}") };
                let Some((_, t)) = TIME_MARKERS.iter().find(|(n, _)| format!("minecraft:{n}") == id) else {
                    return Err(err("commands.time.no_time_marker_found", &[&format!("ResourceKey[minecraft:clock_time_marker / {id}]"), clock]));
                };
                self.day_time = *t;
                self.broadcast_time();
                self.ok(eid, "commands.time.set.time_marker", vec![s(clock), s(id)]);
            }
            "time query gametime" => self.ok(eid, "commands.time.query.gametime", vec![s(self.tick)]),
            "time query time" => self.ok(eid, "commands.time.query.time", vec![s(clock), s(self.day_time)]),
            other => return Err(err("basalt:Not supported by Basalt yet:", &[other])),
        }
        Ok(())
    }

    fn broadcast_time(&mut self) { let w = self.set_time_packet(); self.broadcast(None, || Writer { buf: w.buf.clone() }); }

    // ------------------------------------------------------------ /effect

    fn cmd_effect(&mut self, eid: i32, p: &Parsed, path: &str) -> R<()> {
        let targets = if p.has("targets") { self.resolve(eid, p.arg("targets"), false)? } else { vec![Target::Player(eid)] };
        let ids = Self::players_of(&targets);
        if path.starts_with("effect give") {
            let (name, _) = cmdparse::read_id(p.arg("effect"), 0)?;
            let short = name.trim_start_matches("minecraft:").to_string();
            let infinite = p.path.iter().any(|n| n == "infinite");
            let secs: i32 = if infinite { 0 } else if p.has("seconds") { p.arg("seconds").parse().unwrap_or(30) } else { 30 };
            let amp: u8 = p.arg("amplifier").parse().unwrap_or(0);
            let hide = p.arg("hideParticles") == "true";
            if ids.is_empty() { return Err(err("commands.effect.give.failed", &[])); }
            for &e in &ids { self.add_effect(e, &short, amp, if infinite { -1 } else { secs * 20 }, !hide); }
            let eff = tr(&format!("effect.minecraft.{short}"), vec![]);
            let who = if ids.len() == 1 { text::plain(&self.players[&ids[0]].name) } else { s(ids.len()) };
            self.ok(eid, &plural("commands.effect.give.success", ids.len()), vec![eff, who, s(secs)]);
            return Ok(());
        }
        // clear
        let only = if p.has("effect") { let (n, _) = cmdparse::read_id(p.arg("effect"), 0)?; Some(n.trim_start_matches("minecraft:").to_string()) } else { None };
        let mut cleared = 0;
        for &e in &ids {
            let have: Vec<i32> = self.players[&e].effects.keys().copied().collect();
            for id in have {
                if only.as_deref().and_then(super::env::effect_id).map(|o| o == id).unwrap_or(only.is_none()) { self.clear_effect(e, id); cleared += 1; }
            }
        }
        match &only {
            None if cleared == 0 => return Err(err("commands.effect.clear.everything.failed", &[])),
            Some(_) if cleared == 0 => return Err(err("commands.effect.clear.specific.failed", &[])),
            None => { let who = if ids.len() == 1 { text::plain(&self.players[&ids[0]].name) } else { s(ids.len()) }; self.ok(eid, &plural("commands.effect.clear.everything.success", ids.len()), vec![who]); }
            Some(n) => { let who = if ids.len() == 1 { text::plain(&self.players[&ids[0]].name) } else { s(ids.len()) }; self.ok(eid, &plural("commands.effect.clear.specific.success", ids.len()), vec![tr(&format!("effect.minecraft.{n}"), vec![]), who]); }
        }
        Ok(())
    }

    // ------------------------------------------------------------ /enchant

    fn tag_values(&self, tag: &str) -> Vec<String> {
        let rel = tag.trim_start_matches('#').trim_start_matches("minecraft:");
        let path = self.sh.cfg.data_dir.join("data/minecraft/tags/enchantment").join(format!("{rel}.json"));
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| v["values"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())).unwrap_or_default()
    }

    fn cmd_enchant(&mut self, eid: i32, p: &Parsed) -> R<()> {
        let targets = self.resolve(eid, p.arg("targets"), false)?;
        let (name, _) = cmdparse::read_id(p.arg("enchantment"), 0)?;
        let short = name.trim_start_matches("minecraft:").to_string();
        let level: u8 = if p.has("level") { p.arg("level").parse::<u32>().unwrap_or(1).min(255) as u8 } else { 1 };
        let id = self.sh.registries.id("minecraft:enchantment", &name).ok_or_else(|| err("argument.resource.not_found", &[&name, "minecraft:enchantment"]))?;
        let info: serde_json::Value = std::fs::read(self.sh.cfg.data_dir.join(format!("data/minecraft/enchantment/{short}.json"))).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let max = info["max_level"].as_u64().unwrap_or(1) as u8;
        let mut done = 0;
        let mut last_err: Option<CmdError> = None;
        let mut last_name = Tag::String(String::new());
        for &t in &targets {
            let Target::Player(e) = t else { last_err = Some(CmdError { key: "commands.enchant.failed.entity".into(), args: vec![String::new()] }); continue };
            let pname = self.players[&e].name.clone();
            let slot = 36 + self.players[&e].held;
            let st = self.players[&e].inv[slot];
            if level > max { last_err = Some(err("commands.enchant.failed.level", &[&level.to_string(), &max.to_string()])); continue; }
            if st.count <= 0 { last_err = Some(err("commands.enchant.failed.itemless", &[&pname])); continue; }
            let item_name_s = reg::ITEM.get(st.item as usize).copied().unwrap_or("").to_string();
            let supported = match &info["supported_items"] {
                serde_json::Value::String(t) => self.data.tags.item_has_name(t, &item_name_s),
                serde_json::Value::Array(a) => a.iter().any(|x| x.as_str().map(|n| n.trim_start_matches("minecraft:") == item_name_s).unwrap_or(false)),
                _ => true,
            };
            let excl: Vec<String> = info["exclusive_set"].as_str().map(|t| self.tag_values(t)).unwrap_or_default();
            let en = self.sh.registries.entries("minecraft:enchantment").to_vec();
            let conflicts = st.ench.iter().any(|x| x.1 > 0 && x.0 as i32 != id && en.get(x.0 as usize).map(|n| excl.contains(n)).unwrap_or(false));
            if !supported || conflicts { last_err = Some(CmdError { key: "commands.enchant.failed.incompatible".into(), args: vec![item_name_s.clone()] }); continue; }
            let pl = self.players.get_mut(&e).unwrap();
            let ench = &mut pl.inv[slot].ench;
            if let Some(i) = ench.iter().position(|x| x.1 > 0 && x.0 as i32 == id).or_else(|| ench.iter().position(|x| x.1 == 0)) { ench[i] = (id as u16, level); }
            Self::send_window(pl);
            done += 1; last_name = text::plain(&pname);
        }
        if done == 0 { return Err(last_err.unwrap_or_else(|| err("commands.enchant.failed", &[]))); }
        let mut comp = basalt_nbt::Compound::new();
        comp.insert("translate".into(), s(format!("enchantment.minecraft.{short}")));
        if level != 1 || max != 1 { comp.insert("extra".into(), Tag::List(uniform(vec![text::plain(" "), tr(&format!("enchantment.level.{level}"), vec![])]))); }
        let who = if done == 1 { last_name } else { s(done) };
        self.ok(eid, &plural("commands.enchant.success", done), vec![Tag::Compound(comp), who]);
        Ok(())
    }

    // ------------------------------------------------------------ /fill

    fn cmd_fill(&mut self, eid: i32, p: &Parsed) -> R<()> {
        let (a, b) = (self.block_pos_from(eid, p.arg("from"))?, self.block_pos_from(eid, p.arg("to"))?);
        let (lo, hi) = (std::array::from_fn::<i32, 3, _>(|i| a[i].min(b[i])), std::array::from_fn::<i32, 3, _>(|i| a[i].max(b[i])));
        let vol = (hi[0] - lo[0] + 1) as i64 * (hi[1] - lo[1] + 1) as i64 * (hi[2] - lo[2] + 1) as i64;
        for pt in [lo, hi] { if pt[1] < basalt_world::MIN_Y || pt[1] >= basalt_world::MIN_Y + basalt_world::HEIGHT { return Err(err("argument.pos.outofworld", &[])); } }
        for pt in [lo, hi] { if pt[0].abs() > 30_000_000 || pt[2].abs() > 30_000_000 { return Err(err("argument.pos.unloaded", &[])); } }
        if vol > 32768 { return Err(err("commands.fill.toobig", &["32768", &vol.to_string()])); }
        let state = BlockState::parse(p.arg("block").split('{').next().unwrap_or("").trim_start_matches("minecraft:")).map(|b| b.0).ok_or_else(|| err("argument.block.id.invalid", &[p.arg("block")]))?;
        let mode = p.path.iter().rev().find(|n| matches!(n.as_str(), "destroy" | "hollow" | "keep" | "outline" | "replace" | "strict")).map(|s| s.as_str()).unwrap_or("replace");
        let filter = p.has("filter").then(|| p.arg("filter").to_string());
        for cx in (lo[0] >> 4)..=(hi[0] >> 4) { for cz in (lo[2] >> 4)..=(hi[2] >> 4) { self.ensure_chunk(cx, cz); } }
        let air = block_default("air").unwrap_or(0);
        let mut changed = 0;
        for x in lo[0]..=hi[0] { for y in lo[1]..=hi[1] { for z in lo[2]..=hi[2] {
            let edge = x == lo[0] || x == hi[0] || y == lo[1] || y == hi[1] || z == lo[2] || z == hi[2];
            let old = self.world.get_block(x, y, z);
            let new = match mode {
                "hollow" => if edge { state } else { air },
                "outline" => if edge { state } else { continue },
                "keep" => if BlockState(old).is_air() { state } else { continue },
                _ => if filter.as_ref().map(|f| self.block_matches(f, old)).unwrap_or(true) { state } else { continue },
            };
            if new == old { continue; }
            self.set_block(x, y, z, new);
            changed += 1;
        } } }
        if changed == 0 { return Err(err("commands.fill.failed", &[])); }
        self.ok(eid, "commands.fill.success", vec![s(changed)]);
        Ok(())
    }

    /// `id`, `id[prop=value,...]` or `#tag` against a block state (the `fill ... replace <filter>` predicate).
    fn block_matches(&self, raw: &str, state: u32) -> bool {
        let raw = raw.split('{').next().unwrap_or("");
        let st = BlockState(state);
        if let Some(tag) = raw.strip_prefix('#') { return self.data.tags.block_has(tag.split('[').next().unwrap_or(""), st.name()); }
        let (id, props) = match raw.split_once('[') { Some((n, pr)) => (n, pr.trim_end_matches(']')), None => (raw, "") };
        if id.trim_start_matches("minecraft:") != st.name() { return false; }
        props.split(',').filter(|kv| !kv.is_empty()).all(|kv| kv.split_once('=').map(|(k, v)| st.get(k.trim()) == Some(v.trim())).unwrap_or(true))
    }

    // ------------------------------------------------------------ /summon

    fn cmd_summon(&mut self, eid: i32, p: &Parsed) -> R<()> {
        let (id, _) = cmdparse::read_id(p.arg("entity"), 0)?;
        if id == "minecraft:player" { return Err(err("entity.not_summonable", &[&id])); }
        let pos = if p.has("pos") { let (c, _) = cmdparse::parse_pos(p.arg("pos"), 0, PosKind::Vec3)?; self.pos_from(eid, &c, true) } else { self.players[&eid].pos };
        if pos[1] < basalt_world::MIN_Y as f64 || pos[1] >= (basalt_world::MIN_Y + basalt_world::HEIGHT) as f64 || pos[0].abs() > 30_000_000.0 || pos[2].abs() > 30_000_000.0 { return Err(err("commands.summon.invalidPosition", &[])); }
        let Some(kind) = super::mob::MobKind::from_name(&id) else { return Err(err("commands.summon.failed", &[])) };
        let yaw = self.players[&eid].yaw;
        let extra = p.arg("nbt").to_lowercase();
        let color = extra.split("color:").nth(1).and_then(|c| c.trim_matches(|c: char| !c.is_ascii_digit()).split(|c: char| !c.is_ascii_digit()).next().and_then(|n| n.parse::<u8>().ok()));
        self.ensure_chunk((pos[0].floor() as i32) >> 4, (pos[2].floor() as i32) >> 4);
        self.spawn_mob(kind, pos, yaw + 180.0, |m| {
            m.persistent = true;
            m.baby = kind.can_be_baby() && (extra.contains("baby") || extra.contains("age:-"));
            m.no_ai = extra.contains("noai");
            m.sheared = extra.contains("sheared");
            m.ai.ignited = extra.contains("ignited");
            m.ai.custom_fuse = extra.split("fuse:").nth(1).and_then(|f| f.split(|c: char| !c.is_ascii_digit()).next().and_then(|n| n.parse().ok()));
            if let Some(c) = color { m.sheep_color = c & 15; }
        });
        self.ok(eid, "commands.summon.success", vec![tr(&format!("entity.minecraft.{}", kind.name()), vec![])]);
        Ok(())
    }

    // ------------------------------------------------------------ /item replace block

    /// `/item replace block <pos> container.<n> with <item> [count]`: set a slot of a chest/barrel/furnace
    /// (block entity). Not in the tree sent to clients; the check scenarios use it to fill containers.
    fn cmd_item_replace_block(&mut self, eid: i32, p: &Parsed) -> R<()> {
        let pos = self.block_pos_from(eid, p.arg("pos"))?;
        self.check_block_pos(pos)?;
        let slot: usize = p.arg("slot").strip_prefix("container.").and_then(|n| n.parse().ok()).ok_or_else(|| err("slot.unknown", &[p.arg("slot")]))?;
        let id_text = p.arg("item").split('[').next().unwrap_or("");
        let id = basalt_data::registry_id(reg::ITEM, id_text).ok_or_else(|| err("argument.item.id.invalid", &[id_text]))?;
        let count: i32 = if p.has("count") { p.arg("count").parse().unwrap_or(1) } else { 1 };
        let kind = self.be_ensure(pos).ok_or_else(|| err("commands.item.target.not_a_container", &[&pos[0].to_string(), &pos[1].to_string(), &pos[2].to_string()]))?;
        if slot >= kind.slots() { return Err(err("slot.unknown", &[p.arg("slot")])); }
        let st = if count > 0 { Stack { item: id, count: count.min(self.max_stack(id)), damage: 0, ench: NO_ENCH } } else { Stack::default() };
        if let Some(be) = self.bes.get_mut(&pos) { be.items[slot] = st; }
        self.cont_changed(pos);
        self.ok(eid, "commands.item.block.set.success", vec![s(pos[0]), s(pos[1]), s(pos[2]), item_name(id)]);
        Ok(())
    }

    // ------------------------------------------------------------ /help

    fn cmd_help(&mut self, eid: i32, p: &Parsed) -> R<()> {
        let Some(tree) = cmdparse::tree(&self.sh.cfg.data_dir) else { return Err(err("commands.help.failed", &[])) };
        let names: Vec<String> = if p.has("command") {
            let c = p.arg("command").split(' ').next().unwrap_or("").to_string();
            if !tree.has_command(&c) { return Err(err("commands.help.failed", &[])); }
            vec![c]
        } else { tree.command_names() };
        for n in names { let usage = tree.usage(&n); self.tell(eid, text::plain(&format!("/{n}{}", if usage.is_empty() { String::new() } else { format!(" {usage}") }))); }
        Ok(())
    }

    // ------------------------------------------------------------ /clear

    fn cmd_clear(&mut self, eid: i32, p: &Parsed) -> R<()> {
        let targets = if p.has("targets") { self.resolve(eid, p.arg("targets"), true)? } else { vec![Target::Player(eid)] };
        let ids = Self::players_of(&targets);
        let pred = p.arg("item").to_string();
        let max: Option<i32> = p.has("maxCount").then(|| p.arg("maxCount").parse().unwrap_or(0));
        let matches = |g: &Game, st: &Stack| -> bool {
            if st.count <= 0 { return false; }
            if pred.is_empty() || pred.starts_with('*') { return true; }
            let name = reg::ITEM.get(st.item as usize).copied().unwrap_or("");
            match pred.strip_prefix('#') { Some(t) => g.data.tags.item_has_name(t.split('[').next().unwrap_or(""), name), None => pred.split('[').next().unwrap_or("").trim_start_matches("minecraft:") == name }
        };
        let mut total = 0;
        for &e in &ids {
            let mut remaining = max.map(|m| if m == 0 { 0 } else { m }).unwrap_or(i32::MAX);
            let test_only = max == Some(0);
            let n_slots = self.players[&e].inv.len();
            let mut found = 0;
            for i in 0..n_slots {
                let st = self.players[&e].inv[i];
                if !matches(self, &st) { continue; }
                found += st.count;
                if !test_only && remaining > 0 { let take = st.count.min(remaining); remaining -= take; let pl = self.players.get_mut(&e).unwrap(); pl.inv[i].count -= take; if pl.inv[i].count <= 0 { pl.inv[i] = Stack::default(); } }
            }
            // carried stack and crafting grids count too (as the previous Basalt implementation did)
            let cur = self.players[&e].cursor;
            if matches(self, &cur) { found += cur.count; if !test_only && remaining > 0 { let take = cur.count.min(remaining); remaining -= take; let pl = self.players.get_mut(&e).unwrap(); pl.cursor.count -= take; if pl.cursor.count <= 0 { pl.cursor = Stack::default(); } } }
            let removed = if test_only { found } else { found.min(max.unwrap_or(i32::MAX)) };
            total += removed;
            if !test_only && removed > 0 { Self::send_window(self.players.get_mut(&e).unwrap()); }
        }
        let who = |g: &Game| if ids.len() == 1 { text::plain(&g.players[&ids[0]].name) } else { s(ids.len()) };
        if total == 0 {
            let name = if ids.len() == 1 { self.players[&ids[0]].name.clone() } else { ids.len().to_string() };
            return Err(err(&plural("clear.failed", ids.len()), &[&name]));
        }
        let key = if max == Some(0) { "commands.clear.test" } else { "commands.clear.success" };
        self.ok(eid, &plural(key, ids.len()), vec![s(total), who(self)]);
        Ok(())
    }
}

/// `[<item name>]`-style name of an item: block items use the block's key (as vanilla does), others the item's.
fn item_name(item: i32) -> Tag {
    let n = reg::ITEM.get(item as usize).copied().unwrap_or("air");
    tr(&if basalt_data::blocks::BLOCKS.iter().any(|b| b.name == n) { format!("block.minecraft.{n}") } else { format!("item.minecraft.{n}") }, vec![])
}

/// Yaw/pitch (degrees) for looking from `from` at `to`.
fn look_angles(from: [f64; 3], to: [f64; 3]) -> (Option<f32>, Option<f32>) {
    let (dx, dy, dz) = (to[0] - from[0], to[1] - from[1], to[2] - from[2]);
    let h = (dx * dx + dz * dz).sqrt();
    (Some((-dx.atan2(dz).to_degrees()) as f32), Some((-dy.atan2(h).to_degrees()) as f32))
}

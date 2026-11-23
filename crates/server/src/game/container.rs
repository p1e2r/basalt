//! Container block entities: chests (single/double/trapped), barrels, furnaces (furnace, blast
//! furnace, smoker).
//!
//! Live state is `Game::bes` (typed); every change is mirrored into `Chunk::block_entities` (vanilla
//! block-entity NBT: `id,x,y,z,Items:[{Slot,id,count,components?}]`, furnaces also `lit_time_remaining`,
//! `lit_total_time`, `cooking_time_spent`, `cooking_total_time`, `RecipesUsed`), so region files, the
//! chunk network encoding and unloading need no extra bookkeeping.
//!
//! Wire facts measured against vanilla 26.2 (NOTES.md "Containers"): open_screen menu ids 2 (generic_9x3),
//! 5 (generic_9x6, double chest), 14 furnace, 10 blast_furnace, 22 smoker; titles `container.chest`,
//! `container.chestDouble`, `container.furnace`..; lid animation = block_event(type 1, value = viewer count)
//! on each chest half, repeated every 5 ticks while open, plus `block.chest.open/close` sounds; furnaces send
//! container_set_data (id 0 lit time, 1 lit duration, 2 cook progress, 3 cook total) for all four ids at open,
//! then only the ones that changed each tick.
use super::inv::Window;
use super::{Game, Stack, NO_ENCH};
use basalt_data::{registries as reg, registry_id, BlockState};
use basalt_game::CookKind;
use basalt_nbt::{Compound, Tag};
use basalt_protocol::{ids::play::clientbound as cb, text, Writer};
use rand::Rng;
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Chest,
    Trapped,
    Barrel,
    Furnace,
    Blast,
    Smoker,
}

impl Kind {
    pub fn of_block(name: &str) -> Option<Kind> {
        Some(match name {
            "chest" => Kind::Chest,
            "trapped_chest" => Kind::Trapped,
            "barrel" => Kind::Barrel,
            "furnace" => Kind::Furnace,
            "blast_furnace" => Kind::Blast,
            "smoker" => Kind::Smoker,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            Kind::Chest => "chest",
            Kind::Trapped => "trapped_chest",
            Kind::Barrel => "barrel",
            Kind::Furnace => "furnace",
            Kind::Blast => "blast_furnace",
            Kind::Smoker => "smoker",
        }
    }
    pub fn slots(self) -> usize {
        if self.is_furnace() {
            3
        } else {
            27
        }
    }
    pub fn is_furnace(self) -> bool {
        matches!(self, Kind::Furnace | Kind::Blast | Kind::Smoker)
    }
    pub fn is_chest(self) -> bool {
        matches!(self, Kind::Chest | Kind::Trapped)
    }
    pub fn cook(self) -> Option<CookKind> {
        match self {
            Kind::Furnace => Some(CookKind::Smelting),
            Kind::Blast => Some(CookKind::Blasting),
            Kind::Smoker => Some(CookKind::Smoking),
            _ => None,
        }
    }
    fn menu(self, double: bool) -> &'static str {
        match self {
            Kind::Chest | Kind::Trapped => {
                if double {
                    "generic_9x6"
                } else {
                    "generic_9x3"
                }
            }
            Kind::Barrel => "generic_9x3",
            Kind::Furnace => "furnace",
            Kind::Blast => "blast_furnace",
            Kind::Smoker => "smoker",
        }
    }
    fn title(self, double: bool) -> &'static str {
        match self {
            Kind::Chest | Kind::Trapped => {
                if double {
                    "container.chestDouble"
                } else {
                    "container.chest"
                }
            }
            Kind::Barrel => "container.barrel",
            Kind::Furnace => "container.furnace",
            Kind::Blast => "container.blast_furnace",
            Kind::Smoker => "container.smoker",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Be {
    pub kind: Kind,
    pub items: Vec<Stack>,
    pub lit: i32,
    pub lit_total: i32,
    pub cook: i32,
    pub cook_total: i32,
    /// RecipesUsed: recipe id -> times used (experience is paid out when the result is taken).
    pub used: BTreeMap<String, i32>,
    /// Players with this container open (eids).
    pub viewers: Vec<i32>,
    recheck: u32,
    /// Unknown NBT keys (CustomName, Lock, ...) carried through load/save.
    extra: Compound,
}

impl Be {
    pub fn new(kind: Kind) -> Be {
        Be {
            kind,
            items: vec![Stack::default(); kind.slots()],
            lit: 0,
            lit_total: 0,
            cook: 0,
            cook_total: 200,
            used: BTreeMap::new(),
            viewers: Vec::new(),
            recheck: 0,
            extra: Compound::new(),
        }
    }
}

/// An open container window: `pos[0]` holds slots 0..n, `pos[1]` (double chests) the next n.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Cw {
    pub id: i32,
    pub kind: Kind,
    pub pos: [[i32; 3]; 2],
    pub halves: u8,
}

impl Cw {
    pub fn n_cont(&self) -> usize {
        self.kind.slots() * self.halves as usize
    }
    pub fn total(&self) -> usize {
        self.n_cont() + 36
    }
}

fn cw_dir(f: &str) -> &'static str {
    match f {
        "north" => "east",
        "east" => "south",
        "south" => "west",
        _ => "north",
    }
}
fn ccw_dir(f: &str) -> &'static str {
    match f {
        "north" => "west",
        "west" => "south",
        "south" => "east",
        _ => "north",
    }
}
fn dir_vec(f: &str) -> [i32; 3] {
    match f {
        "north" => [0, 0, -1],
        "south" => [0, 0, 1],
        "west" => [-1, 0, 0],
        "east" => [1, 0, 0],
        "up" => [0, 1, 0],
        _ => [0, -1, 0],
    }
}
fn add(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// Direction from a chest half to its partner (measured: facing south, the west chest is `right` and the east one `left`,
/// so the left half pairs clockwise of its facing and the right half counter-clockwise).
fn partner_dir(state: u32) -> Option<&'static str> {
    let s = BlockState(state);
    let f = s.get("facing")?;
    match s.get("type")? {
        "left" => Some(cw_dir(f)),
        "right" => Some(ccw_dir(f)),
        _ => None,
    }
}

fn with(state: u32, k: &str, v: &str) -> u32 {
    BlockState(state).with(k, v).map(|s| s.0).unwrap_or(state)
}

/// Experience level and progress (0..1) of a point total (public formulas: 2n+7 / 5n-38 / 9n-158 per level).
pub fn xp_level(total: i32) -> (i32, f32) {
    let (mut level, mut rem) = (0, total.max(0));
    loop {
        let need = if level < 15 {
            2 * level + 7
        } else if level < 30 {
            5 * level - 38
        } else {
            9 * level - 158
        };
        if rem < need {
            return (level, rem as f32 / need as f32);
        }
        rem -= need;
        level += 1;
    }
}

impl Game {
    // ------------------------------------------------------------ item <-> NBT

    pub(super) fn stack_to_nbt(&self, slot: Option<usize>, s: &Stack) -> Tag {
        let mut c = Compound::new();
        if let Some(slot) = slot {
            c.insert("Slot".into(), Tag::Byte(slot as i8));
        }
        c.insert(
            "id".into(),
            Tag::String(format!(
                "minecraft:{}",
                reg::ITEM.get(s.item as usize).copied().unwrap_or("air")
            )),
        );
        c.insert("count".into(), Tag::Int(s.count));
        let mut comps = Compound::new();
        if s.damage > 0 {
            comps.insert("minecraft:damage".into(), Tag::Int(s.damage));
        }
        let enchs = self.sh.registries.entries("minecraft:enchantment");
        let e: Compound = s
            .ench
            .iter()
            .filter(|e| e.1 > 0)
            .filter_map(|e| Some((enchs.get(e.0 as usize)?.clone(), Tag::Int(e.1 as i32))))
            .collect();
        if !e.is_empty() {
            comps.insert("minecraft:enchantments".into(), Tag::Compound(e));
        }
        if !comps.is_empty() {
            c.insert("components".into(), Tag::Compound(comps));
        }
        Tag::Compound(c)
    }

    pub(super) fn stack_from_nbt(&self, t: &Tag) -> Option<Stack> {
        let id = t.get("id")?.as_str()?;
        let item = registry_id(reg::ITEM, id)?;
        let count = t
            .get("count")
            .or_else(|| t.get("Count"))
            .and_then(|c| c.as_i64())
            .unwrap_or(1) as i32;
        if count <= 0 {
            return None;
        }
        let (mut damage, mut ench) = (0, NO_ENCH);
        if let Some(Tag::Compound(c)) = t.get("components") {
            if let Some(d) = c.get("minecraft:damage").and_then(|d| d.as_i64()) {
                damage = d as i32;
            }
            if let Some(e) = c.get("minecraft:enchantments") {
                let levels = e.get("levels").unwrap_or(e);
                let enchs = self.sh.registries.entries("minecraft:enchantment");
                if let Tag::Compound(m) = levels {
                    for (k, (name, lvl)) in m.iter().take(3).enumerate() {
                        if let (Some(id), Some(l)) =
                            (enchs.iter().position(|e| e == name), lvl.as_i64())
                        {
                            ench[k] = (id as u16, l.clamp(0, 255) as u8);
                        }
                    }
                }
            }
        }
        Some(Stack {
            item,
            count,
            damage,
            ench,
        })
    }

    fn be_from_nbt(&self, kind: Kind, t: &Tag) -> Be {
        let mut be = Be::new(kind);
        let mut extra = t.as_compound().cloned().unwrap_or_default();
        for k in [
            "id",
            "x",
            "y",
            "z",
            "Items",
            "components",
            "lit_time_remaining",
            "lit_total_time",
            "cooking_time_spent",
            "cooking_total_time",
            "RecipesUsed",
            "keepPacked",
        ] {
            extra.remove(k);
        }
        be.extra = extra;
        for it in t
            .get("Items")
            .and_then(|i| i.as_list())
            .into_iter()
            .flatten()
        {
            let slot = it.get("Slot").and_then(|s| s.as_i64()).unwrap_or(-1);
            if slot >= 0 && (slot as usize) < be.items.len() {
                if let Some(s) = self.stack_from_nbt(it) {
                    be.items[slot as usize] = s;
                }
            }
        }
        let n = |k: &str| t.get(k).and_then(|v| v.as_i64()).map(|v| v as i32);
        be.lit = n("lit_time_remaining").unwrap_or(0);
        be.lit_total = n("lit_total_time").unwrap_or(0);
        be.cook = n("cooking_time_spent").unwrap_or(0);
        be.cook_total = n("cooking_total_time").unwrap_or(200);
        if let Some(Tag::Compound(u)) = t.get("RecipesUsed") {
            for (k, v) in u {
                if let Some(v) = v.as_i64() {
                    be.used
                        .insert(k.trim_start_matches("minecraft:").to_string(), v as i32);
                }
            }
        }
        be
    }

    fn be_to_nbt(&self, pos: [i32; 3], be: &Be) -> Tag {
        let mut c = be.extra.clone();
        c.insert(
            "id".into(),
            Tag::String(format!("minecraft:{}", be.kind.name())),
        );
        c.insert("x".into(), Tag::Int(pos[0]));
        c.insert("y".into(), Tag::Int(pos[1]));
        c.insert("z".into(), Tag::Int(pos[2]));
        c.insert("components".into(), Tag::Compound(Compound::new()));
        c.insert(
            "Items".into(),
            Tag::List(
                be.items
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.count > 0)
                    .map(|(i, s)| self.stack_to_nbt(Some(i), s))
                    .collect(),
            ),
        );
        if be.kind.is_furnace() {
            c.insert("lit_time_remaining".into(), Tag::Short(be.lit as i16));
            c.insert("lit_total_time".into(), Tag::Short(be.lit_total as i16));
            c.insert("cooking_time_spent".into(), Tag::Short(be.cook as i16));
            c.insert(
                "cooking_total_time".into(),
                Tag::Short(be.cook_total as i16),
            );
            c.insert(
                "RecipesUsed".into(),
                Tag::Compound(
                    be.used
                        .iter()
                        .map(|(k, v)| (format!("minecraft:{k}"), Tag::Int(*v)))
                        .collect(),
                ),
            );
        }
        Tag::Compound(c)
    }

    /// Mirror a block entity into its chunk's NBT list (so saving and chunk packets see it).
    pub(super) fn be_save(&mut self, pos: [i32; 3]) {
        let Some(be) = self.bes.get(&pos) else { return };
        let tag = self.be_to_nbt(pos, be);
        let Some(c) = self.world.chunk_mut(pos[0] >> 4, pos[2] >> 4) else {
            return;
        };
        let at = |t: &Tag| {
            t.get("x").and_then(|v| v.as_i64()) == Some(pos[0] as i64)
                && t.get("y").and_then(|v| v.as_i64()) == Some(pos[1] as i64)
                && t.get("z").and_then(|v| v.as_i64()) == Some(pos[2] as i64)
        };
        match c.block_entities.iter().position(at) {
            Some(i) => c.block_entities[i] = tag,
            None => c.block_entities.push(tag),
        }
        c.dirty = true;
    }

    /// Load the container block entities of a chunk that just entered memory.
    pub(super) fn be_adopt_chunk(&mut self, cx: i32, cz: i32) {
        let Some(c) = self.world.chunk(cx, cz) else {
            return;
        };
        let mut found = Vec::new();
        for t in &c.block_entities {
            let Some(id) = t.get("id").and_then(|i| i.as_str()) else {
                continue;
            };
            let Some(kind) = Kind::of_block(id.trim_start_matches("minecraft:")) else {
                continue;
            };
            let g = |k: &str| t.get(k).and_then(|v| v.as_i64()).map(|v| v as i32);
            let (Some(x), Some(y), Some(z)) = (g("x"), g("y"), g("z")) else {
                continue;
            };
            found.push(([x, y, z], kind, t.clone()));
        }
        for (pos, kind, t) in found {
            if Kind::of_block(BlockState(self.world.get_block(pos[0], pos[1], pos[2])).name())
                != Some(kind)
            {
                continue;
            }
            let be = self.be_from_nbt(kind, &t);
            self.bes.insert(pos, be);
        }
    }

    /// Forget the block entities of a chunk that is leaving memory (the NBT in the chunk is current).
    pub(super) fn be_drop_chunk(&mut self, cx: i32, cz: i32) {
        self.bes.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
    }

    /// A block changed: create / drop the block entity to match the new state. Never drops items.
    pub(super) fn be_block_changed(&mut self, pos: [i32; 3], state: u32) {
        let new = Kind::of_block(BlockState(state).name());
        let old = self.bes.get(&pos).map(|b| b.kind);
        match (old, new) {
            (Some(a), Some(b)) if a == b => {}
            (Some(_), Some(b)) => {
                self.be_remove(pos);
                self.bes.insert(pos, Be::new(b));
                self.be_save(pos);
            }
            (Some(_), None) => self.be_remove(pos),
            (None, Some(b)) => {
                self.bes.insert(pos, Be::new(b));
                self.be_save(pos);
            }
            (None, None) => {}
        }
    }

    /// Drop a block entity (closing its viewers' windows) without spilling items.
    pub(super) fn be_remove(&mut self, pos: [i32; 3]) {
        if let Some(be) = self.bes.remove(&pos) {
            for v in be.viewers {
                self.force_close(v);
            }
        }
        if let Some(c) = self.world.chunk_mut(pos[0] >> 4, pos[2] >> 4) {
            let before = c.block_entities.len();
            c.block_entities.retain(|t| {
                !(t.get("x").and_then(|v| v.as_i64()) == Some(pos[0] as i64)
                    && t.get("y").and_then(|v| v.as_i64()) == Some(pos[1] as i64)
                    && t.get("z").and_then(|v| v.as_i64()) == Some(pos[2] as i64))
            });
            if c.block_entities.len() != before {
                c.dirty = true;
            }
        }
    }

    /// Make sure a container block at `pos` has a block entity (blocks placed by commands or generated without one).
    pub(super) fn be_ensure(&mut self, pos: [i32; 3]) -> Option<Kind> {
        let kind = Kind::of_block(BlockState(self.world.get_block(pos[0], pos[1], pos[2])).name())?;
        if self.bes.get(&pos).map(|b| b.kind) != Some(kind) {
            self.bes.insert(pos, Be::new(kind));
            self.be_save(pos);
        }
        Some(kind)
    }

    // ------------------------------------------------------------ chest pairing

    /// Positions of the chest halves that make up the container at `pos`, slot order: [first (slots 0..27), second].
    /// The right half (as seen from the front) is first.
    fn chest_halves(&self, pos: [i32; 3]) -> ([[i32; 3]; 2], u8) {
        let st = self.world.get_block(pos[0], pos[1], pos[2]);
        let Some(d) = partner_dir(st) else {
            return ([pos, pos], 1);
        };
        let other = add(pos, dir_vec(d));
        let os = self.world.get_block(other[0], other[1], other[2]);
        let (a, b) = (BlockState(st), BlockState(os));
        if a.name() != b.name()
            || a.get("facing") != b.get("facing")
            || partner_dir(os).map(|d2| add(other, dir_vec(d2))) != Some(pos)
        {
            return ([pos, pos], 1);
        }
        if a.get("type") == Some("right") {
            ([pos, other], 2)
        } else {
            ([other, pos], 2)
        }
    }

    /// Chest placement: pick `type` from neighbours (same facing, single) and update the partner. Returns the new state.
    pub(super) fn chest_placed(
        &mut self,
        pos: [i32; 3],
        mut state: u32,
        sneaking: bool,
        face: usize,
    ) -> u32 {
        let s = BlockState(state);
        let (Some(facing), name) = (s.get("facing"), s.name()) else {
            return state;
        };
        let single_with_facing = |g: &Game, p: [i32; 3], f: &str| -> bool {
            let o = BlockState(g.world.get_block(p[0], p[1], p[2]));
            o.name() == name && o.get("type") == Some("single") && o.get("facing") == Some(f)
        };
        let mut facing = facing;
        let mut ty = "single";
        let clicked = ["down", "up", "north", "south", "west", "east"][face];
        if face >= 2 && sneaking {
            // sneak-placing against a chest side: pair with that chest if it is a single one facing across the click axis
            let opp = opposite_dir(clicked);
            let p = add(pos, dir_vec(opp));
            let o = BlockState(self.world.get_block(p[0], p[1], p[2]));
            if o.name() == name && o.get("type") == Some("single") {
                if let Some(f) = o.get("facing") {
                    let axis_clicked = if matches!(clicked, "north" | "south") {
                        'z'
                    } else {
                        'x'
                    };
                    let axis_f = if matches!(f, "north" | "south") {
                        'z'
                    } else {
                        'x'
                    };
                    if axis_clicked != axis_f {
                        facing = f;
                        ty = if ccw_dir(facing) == opp {
                            "right"
                        } else {
                            "left"
                        };
                    }
                }
            }
        } else if !sneaking {
            // measured: a chest placed east of a south-facing single chest (the neighbour is clockwise of the facing) becomes `left`
            if single_with_facing(self, add(pos, dir_vec(cw_dir(facing))), facing) {
                ty = "left";
            } else if single_with_facing(self, add(pos, dir_vec(ccw_dir(facing))), facing) {
                ty = "right";
            }
        }
        state = with(with(state, "facing", facing), "type", ty);
        if ty != "single" {
            let d = partner_dir(state).unwrap_or("north");
            let other = add(pos, dir_vec(d));
            let os = self.world.get_block(other[0], other[1], other[2]);
            let new_other = with(os, "type", if ty == "left" { "right" } else { "left" });
            if new_other != os {
                self.set_block(other[0], other[1], other[2], new_other);
            }
        }
        state
    }

    /// A chest half was removed: its partner becomes single.
    pub(super) fn chest_removed(&mut self, pos: [i32; 3], old: u32) {
        let Some(d) = partner_dir(old) else { return };
        let other = add(pos, dir_vec(d));
        let os = self.world.get_block(other[0], other[1], other[2]);
        if BlockState(os).name() == BlockState(old).name()
            && BlockState(os).get("type") != Some("single")
        {
            self.set_block(other[0], other[1], other[2], with(os, "type", "single"));
        }
    }

    // ------------------------------------------------------------ opening / closing

    fn chest_blocked(&self, pos: [i32; 3]) -> bool {
        let a = self.world.get_block(pos[0], pos[1] + 1, pos[2]);
        let p = basalt_world::props::props(a);
        p.opacity >= 15 && !p.is_air() && !BlockState(a).name().ends_with("leaves")
    }

    pub(super) fn open_container(&mut self, eid: i32, pos: [i32; 3]) {
        let Some(kind) = self.be_ensure(pos) else {
            return;
        };
        let st = self.world.get_block(pos[0], pos[1], pos[2]);
        let (halves, n) = if kind.is_chest() {
            self.chest_halves(pos)
        } else {
            ([pos, pos], 1)
        };
        if kind.is_chest() && (0..n as usize).any(|i| self.chest_blocked(halves[i])) {
            return;
        }
        for &h in halves.iter().take(n as usize) {
            self.be_ensure(h);
        }
        if self
            .players
            .get(&eid)
            .map(|p| p.window != Window::Inventory)
            .unwrap_or(true)
        {
            self.close_window(eid);
        }
        let Some(p) = self.players.get_mut(&eid) else {
            return;
        };
        p.next_window = p.next_window % 100 + 1;
        let id = p.next_window;
        let cw = Cw {
            id,
            kind,
            pos: halves,
            halves: n,
        };
        p.window = Window::Cont(cw);
        p.data_sent = [-1; 4];
        let was_empty = halves.iter().take(n as usize).all(|h| {
            self.bes
                .get(h)
                .map(|b| b.viewers.is_empty())
                .unwrap_or(true)
        });
        for h in halves.iter().take(n as usize) {
            if let Some(b) = self.bes.get_mut(h) {
                b.viewers.push(eid);
                b.recheck = 0;
            }
        }
        // sound first (as vanilla), then the screen
        if was_empty {
            let c = [
                (halves[0][0] + halves[n as usize - 1][0]) as f64 / 2.0 + 0.5,
                (halves[0][1]) as f64 + 0.5,
                (halves[0][2] + halves[n as usize - 1][2]) as f64 / 2.0 + 0.5,
            ];
            let pitch = 0.9 + self.rng.gen::<f32>() * 0.1;
            match kind {
                Kind::Barrel => {
                    let f = dir_vec(BlockState(st).get("facing").unwrap_or("up"));
                    self.play_sound(
                        "block.barrel.open",
                        4,
                        [
                            pos[0] as f64 + 0.5 + f[0] as f64 * 0.5,
                            pos[1] as f64 + 0.5 + f[1] as f64 * 0.5,
                            pos[2] as f64 + 0.5 + f[2] as f64 * 0.5,
                        ],
                        0.5,
                        pitch,
                    );
                    self.set_block(pos[0], pos[1], pos[2], with(st, "open", "true"));
                }
                Kind::Chest | Kind::Trapped => {
                    self.play_sound("block.chest.open", 4, c, 0.5, pitch)
                }
                _ => {}
            }
        }
        self.cont_refresh_mirror(eid);
        let (menu, title) = (
            registry_id(reg::MENU, kind.menu(n == 2)).unwrap_or(2),
            kind.title(n == 2),
        );
        if let Some(p) = self.players.get(&eid) {
            let mut w = Writer::with_id(cb::OPEN_SCREEN);
            w.varint(id)
                .varint(menu)
                .nbt(&text::translate(title, vec![]));
            p.send(w);
        }
        if let Some(p) = self.players.get_mut(&eid) {
            Self::send_window(p);
        }
        if kind.is_furnace() {
            self.send_furnace_data(eid);
        }
        if kind.is_chest() {
            self.lid_event(&cw);
        }
    }

    fn lid_event(&self, cw: &Cw) {
        let block = registry_id(reg::BLOCK, cw.kind.name()).unwrap_or(0);
        for i in 0..cw.halves as usize {
            let p = cw.pos[i];
            let count = self.bes.get(&p).map(|b| b.viewers.len()).unwrap_or(0) as i32;
            self.broadcast_chunk((p[0] >> 4, p[2] >> 4), None, || {
                let mut w = Writer::with_id(cb::BLOCK_EVENT);
                w.position(p[0], p[1], p[2])
                    .u8(1)
                    .u8(count as u8)
                    .varint(block);
                w
            });
        }
    }

    /// A player's container window is going away (closed, left, container destroyed).
    pub(super) fn cont_closed(&mut self, eid: i32, cw: Cw) {
        let mut emptied = true;
        for i in 0..cw.halves as usize {
            if let Some(b) = self.bes.get_mut(&cw.pos[i]) {
                b.viewers.retain(|v| *v != eid);
                if !b.viewers.is_empty() {
                    emptied = false;
                }
            }
        }
        if cw.kind.is_chest() {
            self.lid_event(&cw);
        }
        if emptied {
            let n = cw.halves as usize;
            let c = [
                (cw.pos[0][0] + cw.pos[n - 1][0]) as f64 / 2.0 + 0.5,
                cw.pos[0][1] as f64 + 0.5,
                (cw.pos[0][2] + cw.pos[n - 1][2]) as f64 / 2.0 + 0.5,
            ];
            let pitch = 0.9 + self.rng.gen::<f32>() * 0.1;
            let st = self
                .world
                .get_block(cw.pos[0][0], cw.pos[0][1], cw.pos[0][2]);
            match cw.kind {
                Kind::Barrel => {
                    if BlockState(st).name() == "barrel" {
                        let f = dir_vec(BlockState(st).get("facing").unwrap_or("up"));
                        let p = cw.pos[0];
                        self.play_sound(
                            "block.barrel.close",
                            4,
                            [
                                p[0] as f64 + 0.5 + f[0] as f64 * 0.5,
                                p[1] as f64 + 0.5 + f[1] as f64 * 0.5,
                                p[2] as f64 + 0.5 + f[2] as f64 * 0.5,
                            ],
                            0.5,
                            pitch,
                        );
                        self.set_block(p[0], p[1], p[2], with(st, "open", "false"));
                    }
                }
                Kind::Chest | Kind::Trapped => {
                    self.play_sound("block.chest.close", 4, c, 0.5, pitch)
                }
                _ => {}
            }
        }
    }

    /// Close a player's window from the server side (container destroyed): tells the client.
    pub(super) fn force_close(&mut self, eid: i32) {
        let Some(p) = self.players.get(&eid) else {
            return;
        };
        let id = p.window_id();
        if id == 0 {
            return;
        }
        let mut w = Writer::with_id(cb::CONTAINER_CLOSE);
        w.varint(id);
        p.send(w);
        self.close_window(eid);
    }

    /// Copy the container's slots into the player's mirror (used by send_window).
    fn cont_refresh_mirror(&mut self, eid: i32) {
        let Some(Window::Cont(cw)) = self.players.get(&eid).map(|p| p.window) else {
            return;
        };
        let mut items = Vec::with_capacity(cw.n_cont());
        for h in 0..cw.halves as usize {
            match self.bes.get(&cw.pos[h]) {
                Some(b) => items.extend_from_slice(&b.items),
                None => items.extend(std::iter::repeat_n(Stack::default(), cw.kind.slots())),
            }
        }
        if let Some(p) = self.players.get_mut(&eid) {
            p.cont_items = items;
        }
    }

    /// After a container changed: refresh and resend every viewer's window.
    pub(super) fn cont_changed(&mut self, pos: [i32; 3]) {
        self.be_save(pos);
        let viewers = self
            .bes
            .get(&pos)
            .map(|b| b.viewers.clone())
            .unwrap_or_default();
        for v in viewers {
            self.cont_refresh_mirror(v);
            if let Some(p) = self.players.get_mut(&v) {
                Self::send_window(p);
            }
        }
    }

    // ------------------------------------------------------------ slot access (window coordinates)

    fn win_slot(&self, cw: &Cw, i: usize) -> Stack {
        let (n, per) = (cw.n_cont(), cw.kind.slots());
        if i < n {
            self.bes
                .get(&cw.pos[i / per])
                .map(|b| b.items[i % per])
                .unwrap_or_default()
        } else {
            Stack::default()
        }
    }

    pub(super) fn win_get(&self, eid: i32, cw: &Cw, i: usize) -> Stack {
        let n = cw.n_cont();
        if i < n {
            return self.win_slot(cw, i);
        }
        let j = i - n;
        self.players
            .get(&eid)
            .map(|p| {
                if j < 27 {
                    p.inv[9 + j]
                } else {
                    p.inv[36 + j - 27]
                }
            })
            .unwrap_or_default()
    }

    pub(super) fn win_set(&mut self, eid: i32, cw: &Cw, i: usize, s: Stack) {
        let s = if s.count <= 0 { Stack::default() } else { s };
        let (n, per) = (cw.n_cont(), cw.kind.slots());
        if i < n {
            let pos = cw.pos[i / per];
            let reset_cook = if cw.kind.is_furnace() && i == 0 {
                self.bes
                    .get(&pos)
                    .map(|b| !(s.count > 0 && b.items[0].item == s.item && b.items[0].count > 0))
                    .unwrap_or(false)
            } else {
                false
            };
            let total = if reset_cook && s.count > 0 {
                cw.kind
                    .cook()
                    .and_then(|k| self.data.cook(k, s.item))
                    .map(|r| r.2 as i32)
            } else {
                None
            };
            if let Some(b) = self.bes.get_mut(&pos) {
                b.items[i % per] = s;
                if reset_cook {
                    b.cook = 0;
                    b.cook_total = total.unwrap_or(200);
                }
            }
        } else if let Some(p) = self.players.get_mut(&eid) {
            let j = i - n;
            if j < 27 {
                p.inv[9 + j] = s
            } else {
                p.inv[36 + j - 27] = s
            }
        }
    }

    /// Can `s` go into window slot `i` (container-side rules).
    fn win_may_place(&self, cw: &Cw, i: usize, s: &Stack) -> bool {
        if i >= cw.n_cont() {
            return true;
        }
        if !cw.kind.is_furnace() {
            return true;
        }
        match i {
            0 => true,
            1 => self.data.fuel_ticks(s.item).is_some() || self.item_is(s.item, "bucket"),
            _ => false,
        }
    }

    fn item_is(&self, item: i32, name: &str) -> bool {
        reg::ITEM.get(item as usize).copied() == Some(name)
    }

    fn win_slot_max(&self, cw: &Cw, i: usize, s: &Stack) -> i32 {
        let m = self.max_stack(s.item);
        if cw.kind.is_furnace() && i == 1 && self.item_is(s.item, "bucket") {
            1
        } else {
            m
        }
    }

    /// Vanilla's moveItemStackTo: merge into matching stacks, then fill empty slots, over `range` (optionally reversed).
    fn win_move_to(
        &mut self,
        eid: i32,
        cw: &Cw,
        mut s: Stack,
        lo: usize,
        hi: usize,
        reverse: bool,
    ) -> Stack {
        let idx: Vec<usize> = if reverse {
            (lo..hi).rev().collect()
        } else {
            (lo..hi).collect()
        };
        if self.max_stack(s.item) > 1 {
            for &i in &idx {
                if s.count <= 0 {
                    break;
                }
                let t = self.win_get(eid, cw, i);
                if t.count > 0 && same(&t, &s) {
                    let room = self.win_slot_max(cw, i, &s) - t.count;
                    if room > 0 {
                        let k = room.min(s.count);
                        self.win_set(
                            eid,
                            cw,
                            i,
                            Stack {
                                count: t.count + k,
                                ..t
                            },
                        );
                        s.count -= k;
                    }
                }
            }
        }
        for &i in &idx {
            if s.count <= 0 {
                break;
            }
            if self.win_get(eid, cw, i).count == 0 && self.win_may_place(cw, i, &s) {
                let k = s.count.min(self.win_slot_max(cw, i, &s));
                self.win_set(eid, cw, i, Stack { count: k, ..s });
                s.count -= k;
            }
        }
        if s.count <= 0 {
            Stack::default()
        } else {
            s
        }
    }

    // ------------------------------------------------------------ container_click

    /// container_click for container windows (chest/barrel/furnace): slot layout = container slots, then 27 main + 9 hotbar.
    pub(super) fn cont_click(&mut self, eid: i32, cw: Cw, slot: i16, button: i8, mode: i32) {
        let n = cw.total();
        let valid = slot >= 0 && (slot as usize) < n;
        let creative = self.players[&eid].gamemode == 1;
        let result_slot = |i: usize| cw.kind.is_furnace() && i == 2;
        let mut touched_result = false;
        match mode {
            0 if slot == -999 => {
                let c = self.players[&eid].cursor;
                if c.count > 0 {
                    let k = if button == 0 { c.count } else { 1 };
                    self.players.get_mut(&eid).unwrap().cursor = Stack {
                        count: c.count - k,
                        ..c
                    };
                    if self.players[&eid].cursor.count <= 0 {
                        self.players.get_mut(&eid).unwrap().cursor = Stack::default();
                    }
                    self.throw_item(eid, Stack { count: k, ..c }, false);
                }
            }
            0 if valid => {
                let i = slot as usize;
                let (s, c) = (self.win_get(eid, &cw, i), self.players[&eid].cursor);
                let place_ok = c.count > 0 && self.win_may_place(&cw, i, &c);
                let slot_max = if c.count > 0 {
                    self.win_slot_max(&cw, i, &c)
                } else {
                    64
                };
                let set_cursor = |g: &mut Game, c: Stack| {
                    g.players.get_mut(&eid).unwrap().cursor =
                        if c.count > 0 { c } else { Stack::default() }
                };
                if s.count == 0 && c.count == 0 {
                } else if s.count == 0 {
                    if place_ok {
                        let k = (if button == 0 { c.count } else { 1 }).min(slot_max);
                        self.win_set(eid, &cw, i, Stack { count: k, ..c });
                        set_cursor(
                            self,
                            Stack {
                                count: c.count - k,
                                ..c
                            },
                        );
                    }
                } else if c.count == 0 {
                    let k = if button == 0 {
                        s.count
                    } else {
                        (s.count + 1) / 2
                    };
                    self.win_set(
                        eid,
                        &cw,
                        i,
                        Stack {
                            count: s.count - k,
                            ..s
                        },
                    );
                    set_cursor(self, Stack { count: k, ..s });
                    touched_result |= result_slot(i);
                } else if place_ok {
                    if same(&s, &c) {
                        let k = (if button == 0 { c.count } else { 1 })
                            .min(slot_max - s.count)
                            .max(0);
                        self.win_set(
                            eid,
                            &cw,
                            i,
                            Stack {
                                count: s.count + k,
                                ..s
                            },
                        );
                        set_cursor(
                            self,
                            Stack {
                                count: c.count - k,
                                ..c
                            },
                        );
                    } else if c.count <= slot_max {
                        self.win_set(eid, &cw, i, c);
                        set_cursor(self, s);
                    }
                } else if same(&s, &c) {
                    let k = s.count.min(self.max_stack(c.item) - c.count).max(0);
                    if k > 0 {
                        self.win_set(
                            eid,
                            &cw,
                            i,
                            Stack {
                                count: s.count - k,
                                ..s
                            },
                        );
                        set_cursor(
                            self,
                            Stack {
                                count: c.count + k,
                                ..c
                            },
                        );
                        touched_result |= result_slot(i);
                    }
                }
            }
            1 if valid => {
                let i = slot as usize;
                let s = self.win_get(eid, &cw, i);
                if s.count > 0 {
                    let nc = cw.n_cont();
                    let left = if cw.kind.is_furnace() {
                        if i == 2 {
                            self.win_move_to(eid, &cw, s, nc, n, true)
                        } else if i < 2 {
                            self.win_move_to(eid, &cw, s, nc, n, false)
                        } else if cw
                            .kind
                            .cook()
                            .map(|k| self.data.cook(k, s.item).is_some())
                            .unwrap_or(false)
                        {
                            self.win_move_to(eid, &cw, s, 0, 1, false)
                        } else if self.data.fuel_ticks(s.item).is_some() {
                            self.win_move_to(eid, &cw, s, 1, 2, false)
                        } else if i < nc + 27 {
                            self.win_move_to(eid, &cw, s, nc + 27, n, false)
                        } else {
                            self.win_move_to(eid, &cw, s, nc, nc + 27, false)
                        }
                    } else if i < nc {
                        self.win_move_to(eid, &cw, s, nc, n, true)
                    } else {
                        self.win_move_to(eid, &cw, s, 0, nc, false)
                    };
                    if left.count != s.count {
                        self.win_set(eid, &cw, i, left);
                        touched_result |= result_slot(i);
                    }
                }
            }
            2 if valid => {
                let hb = if button == 40 {
                    Some(45usize)
                } else if (0..9).contains(&button) {
                    Some(36 + button as usize)
                } else {
                    None
                };
                if let Some(hb) = hb {
                    let i = slot as usize;
                    let (a, b) = (self.win_get(eid, &cw, i), self.players[&eid].inv[hb]);
                    if a.count > 0 || b.count > 0 {
                        if b.count == 0 {
                            self.win_set(eid, &cw, i, Stack::default());
                            self.players.get_mut(&eid).unwrap().inv[hb] = a;
                            touched_result |= result_slot(i);
                        } else if self.win_may_place(&cw, i, &b) {
                            let max = self.win_slot_max(&cw, i, &b);
                            if a.count == 0 {
                                let k = b.count.min(max);
                                self.win_set(eid, &cw, i, Stack { count: k, ..b });
                                self.players.get_mut(&eid).unwrap().inv[hb] = Stack {
                                    count: b.count - k,
                                    ..b
                                };
                                if self.players[&eid].inv[hb].count <= 0 {
                                    self.players.get_mut(&eid).unwrap().inv[hb] = Stack::default();
                                }
                            } else if b.count <= max {
                                self.win_set(eid, &cw, i, b);
                                self.players.get_mut(&eid).unwrap().inv[hb] = a;
                                touched_result |= result_slot(i);
                            }
                        }
                    }
                }
            }
            3 if valid && creative => {
                let s = self.win_get(eid, &cw, slot as usize);
                if s.count > 0 && self.players[&eid].cursor.count == 0 {
                    let m = self.max_stack(s.item);
                    self.players.get_mut(&eid).unwrap().cursor = Stack { count: m, ..s };
                }
            }
            4 if valid => {
                let i = slot as usize;
                let s = self.win_get(eid, &cw, i);
                if s.count > 0 {
                    let k = if button == 1 { s.count } else { 1 };
                    self.win_set(
                        eid,
                        &cw,
                        i,
                        Stack {
                            count: s.count - k,
                            ..s
                        },
                    );
                    self.throw_item(eid, Stack { count: k, ..s }, false);
                    touched_result |= result_slot(i);
                }
            }
            5 => {
                let p = self.players.get_mut(&eid).unwrap();
                match button {
                    0 | 4 | 8 => p.drag = (button, Vec::new()),
                    1 | 5 | 9 => {
                        if valid && !p.drag.1.contains(&slot) {
                            p.drag.1.push(slot);
                        }
                    }
                    2 | 6 | 10 => {
                        let (start, slots) = std::mem::take(&mut p.drag);
                        let c = p.cursor;
                        if c.count > 0 && !slots.is_empty() {
                            let targets: Vec<usize> = slots
                                .iter()
                                .map(|&s| s as usize)
                                .filter(|&i| i < n)
                                .filter(|&i| {
                                    let t = self.win_get(eid, &cw, i);
                                    self.win_may_place(&cw, i, &c) && (t.count == 0 || same(&t, &c))
                                })
                                .collect();
                            let per = match start {
                                0 => c.count / targets.len().max(1) as i32,
                                4 => 1,
                                _ => self.max_stack(c.item),
                            };
                            let mut left = c.count;
                            for i in targets {
                                let t = self.win_get(eid, &cw, i);
                                let max = self.win_slot_max(&cw, i, &c);
                                let have = if t.count > 0 { t.count } else { 0 };
                                let put = (per + have).min(max) - have;
                                if put <= 0 {
                                    continue;
                                }
                                let put = if start == 8 && creative {
                                    max - have
                                } else {
                                    put
                                };
                                self.win_set(
                                    eid,
                                    &cw,
                                    i,
                                    Stack {
                                        count: have + put,
                                        ..c
                                    },
                                );
                                if !(start == 8 && creative) {
                                    left -= put;
                                }
                            }
                            self.players.get_mut(&eid).unwrap().cursor = if left > 0 {
                                Stack { count: left, ..c }
                            } else {
                                Stack::default()
                            };
                        }
                    }
                    _ => {}
                }
            }
            6 => {
                // measured: double-click collecting only happens when the clicked slot is empty (or its stack can't be taken)
                let c = self.players[&eid].cursor;
                let clicked_busy = valid
                    && self.win_get(eid, &cw, slot as usize).count > 0
                    && !result_slot(slot as usize);
                if c.count > 0 && !clicked_busy {
                    let max = self.max_stack(c.item);
                    let mut cur = c.count;
                    for pass in 0..2 {
                        for i in (0..n).map(|k| if button == 0 { k } else { n - 1 - k }) {
                            if cur >= max {
                                break;
                            }
                            if result_slot(i) {
                                continue;
                            }
                            let t = self.win_get(eid, &cw, i);
                            if t.count > 0
                                && same(&t, &c)
                                && ((pass == 0) == (t.count != self.max_stack(t.item)))
                            {
                                let k = (max - cur).min(t.count);
                                cur += k;
                                self.win_set(
                                    eid,
                                    &cw,
                                    i,
                                    Stack {
                                        count: t.count - k,
                                        ..t
                                    },
                                );
                            }
                        }
                    }
                    self.players.get_mut(&eid).unwrap().cursor.count = cur;
                }
            }
            _ => {}
        }
        if touched_result {
            self.furnace_result_taken(eid, &cw);
        }
        for h in 0..cw.halves as usize {
            self.cont_changed(cw.pos[h]);
        }
        if let Some(p) = self.players.get_mut(&eid) {
            Self::send_window(p);
        }
    }

    // ------------------------------------------------------------ furnaces

    /// Experience stored by the furnace is paid out whenever anything is taken from the result slot.
    fn furnace_result_taken(&mut self, eid: i32, cw: &Cw) {
        let pos = cw.pos[0];
        let Some(b) = self.bes.get_mut(&pos) else {
            return;
        };
        let used = std::mem::take(&mut b.used);
        let mut xp = 0.0f32;
        for (id, n) in &used {
            xp += self.data.cook_xp(id).unwrap_or(0.0) * *n as f32;
        }
        let whole = xp.floor() as i32;
        let points = whole + (self.rng.gen::<f32>() < xp - whole as f32) as i32;
        if points > 0 {
            self.give_xp(eid, points);
        }
    }

    pub(super) fn give_xp(&mut self, eid: i32, points: i32) {
        let Some(p) = self.players.get_mut(&eid) else {
            return;
        };
        p.xp += points;
        Self::send_xp(p);
    }

    pub(super) fn send_xp(p: &super::Player) {
        let (level, progress) = xp_level(p.xp);
        let mut w = Writer::with_id(cb::SET_EXPERIENCE);
        w.f32(progress).varint(level).varint(p.xp);
        p.send(w);
    }

    /// Send the furnace properties that changed since last sent to this viewer.
    fn send_furnace_data(&mut self, eid: i32) {
        let Some(p) = self.players.get(&eid) else {
            return;
        };
        let Window::Cont(cw) = p.window else { return };
        let Some(b) = self.bes.get(&cw.pos[0]) else {
            return;
        };
        let vals = [b.lit, b.lit_total, b.cook, b.cook_total];
        let p = self.players.get_mut(&eid).unwrap();
        for (k, &v) in vals.iter().enumerate() {
            if p.data_sent[k] != v {
                p.data_sent[k] = v;
                let mut w = Writer::with_id(cb::CONTAINER_SET_DATA);
                w.varint(cw.id).i16(k as i16).i16(v as i16);
                p.send(w);
            }
        }
    }

    fn can_burn(&self, be: &Be) -> Option<(i32, f32, u32)> {
        let kind = be.kind.cook()?;
        let input = be.items[0];
        if input.count <= 0 {
            return None;
        }
        let r = self.data.cook(kind, input.item)?;
        let out = be.items[2];
        if out.count > 0
            && (out.item != r.0
                || out.damage != 0
                || out.count + 1 > self.max_stack(out.item).min(64))
        {
            return None;
        }
        Some(r)
    }

    /// One furnace tick (order measured against vanilla: the tick that lights the fuel already counts one cook tick;
    /// progress resets to 0 the tick it reaches the total; without fuel progress decays 2 per tick).
    fn tick_furnace(&mut self, pos: [i32; 3]) {
        let Some(kind) = self.be_ensure(pos) else {
            return;
        };
        let Some(mut be) = self.bes.remove(&pos) else {
            return;
        };
        let was_lit = be.lit > 0;
        let mut changed = false;
        // the total follows the input item whenever no progress is pending (also for contents set by commands)
        // (an emptied input keeps the old total: only taking the stack out by hand resets it, see win_set)
        if be.cook == 0 && be.items[0].count > 0 {
            let want = kind
                .cook()
                .and_then(|k| self.data.cook(k, be.items[0].item))
                .map(|r| r.2 as i32)
                .unwrap_or(200);
            if be.cook_total != want {
                be.cook_total = want;
                changed = true;
            }
        }
        if was_lit {
            be.lit -= 1;
        }
        let has_fuel = be.items[1].count > 0;
        let has_input = be.items[0].count > 0;
        if be.lit > 0 || (has_fuel && has_input) {
            let recipe = self.can_burn(&be);
            if be.lit <= 0 && recipe.is_some() {
                let t = self.data.fuel_ticks(be.items[1].item).unwrap_or(0) as i32;
                be.lit = t;
                be.lit_total = t;
                if be.lit > 0 {
                    changed = true;
                    let f = be.items[1];
                    // buckets of lava leave an empty bucket behind
                    if self.item_is(f.item, "lava_bucket") {
                        be.items[1] = Stack {
                            item: registry_id(reg::ITEM, "bucket").unwrap_or(f.item),
                            count: 1,
                            damage: 0,
                            ench: NO_ENCH,
                        };
                    } else {
                        be.items[1].count -= 1;
                        if be.items[1].count <= 0 {
                            be.items[1] = Stack::default();
                        }
                    }
                }
            }
            if let Some((res, _xp, time)) = recipe.filter(|_| be.lit > 0) {
                be.cook += 1;
                if be.cook >= be.cook_total {
                    be.cook = 0;
                    be.cook_total = time as i32;
                    let out = be.items[2];
                    be.items[2] = if out.count > 0 {
                        Stack {
                            count: out.count + 1,
                            ..out
                        }
                    } else {
                        Stack {
                            item: res,
                            count: 1,
                            damage: 0,
                            ench: NO_ENCH,
                        }
                    };
                    let input = be.items[0].item;
                    if let Some(id) = self.data.cook_id(kind.cook().unwrap(), input) {
                        *be.used.entry(id.to_string()).or_insert(0) += 1;
                    }
                    be.items[0].count -= 1;
                    if be.items[0].count <= 0 {
                        be.items[0] = Stack::default();
                    }
                    changed = true;
                }
            } else {
                be.cook = 0;
            }
        } else if be.lit <= 0 && be.cook > 0 {
            be.cook = (be.cook - 2).clamp(0, be.cook_total);
        }
        let lit_now = be.lit > 0;
        self.bes.insert(pos, be);
        if was_lit != lit_now {
            let st = self.world.get_block(pos[0], pos[1], pos[2]);
            self.set_block(
                pos[0],
                pos[1],
                pos[2],
                with(st, "lit", if lit_now { "true" } else { "false" }),
            );
            changed = true;
        }
        self.be_save(pos);
        let viewers = self
            .bes
            .get(&pos)
            .map(|b| b.viewers.clone())
            .unwrap_or_default();
        for v in viewers {
            if changed {
                self.cont_refresh_mirror(v);
                if let Some(p) = self.players.get_mut(&v) {
                    Self::send_window(p);
                }
            }
            self.send_furnace_data(v);
        }
    }

    /// Per-tick container work: furnaces and the 5-tick lid re-broadcast.
    pub(super) fn tick_containers(&mut self) {
        if self.bes.is_empty() {
            return;
        }
        let furnaces: Vec<[i32; 3]> = self
            .bes
            .iter()
            .filter(|(_, b)| b.kind.is_furnace())
            .map(|(p, _)| *p)
            .collect();
        for p in furnaces {
            self.tick_furnace(p);
        }
        let open: Vec<[i32; 3]> = self
            .bes
            .iter()
            .filter(|(_, b)| b.kind.is_chest() && !b.viewers.is_empty())
            .map(|(p, _)| *p)
            .collect();
        for p in open {
            let Some(b) = self.bes.get_mut(&p) else {
                continue;
            };
            b.recheck += 1;
            if b.recheck >= 5 {
                b.recheck = 0;
                let (count, kind) = (b.viewers.len() as u8, b.kind);
                let block = registry_id(reg::BLOCK, kind.name()).unwrap_or(0);
                self.broadcast_chunk((p[0] >> 4, p[2] >> 4), None, || {
                    let mut w = Writer::with_id(cb::BLOCK_EVENT);
                    w.position(p[0], p[1], p[2]).u8(1).u8(count).varint(block);
                    w
                });
            }
        }
    }

    // ------------------------------------------------------------ breaking

    /// A container block is being destroyed: spill its contents as item entities and drop the block entity.
    pub(super) fn spill_container(&mut self, pos: [i32; 3]) {
        let Some(be) = self.bes.get(&pos) else { return };
        let items: Vec<Stack> = be.items.iter().copied().filter(|s| s.count > 0).collect();
        self.be_remove(pos);
        for s in items {
            let mut left = s.count;
            while left > 0 {
                // stacks come out in pieces of 10..=30, scattered inside the block
                let k = left.min(self.rng.gen_range(10..=30));
                left -= k;
                let o = [
                    self.rng.gen_range(0.125..0.875),
                    self.rng.gen_range(0.0..0.75),
                    self.rng.gen_range(0.125..0.875),
                ];
                let vel = [
                    self.rng.gen_range(-0.1..0.1),
                    self.rng.gen_range(0.1..0.3),
                    self.rng.gen_range(-0.1..0.1),
                ];
                self.spawn_item(
                    [
                        pos[0] as f64 + o[0],
                        pos[1] as f64 + o[1],
                        pos[2] as f64 + o[2],
                    ],
                    vel,
                    Stack { count: k, ..s },
                    10,
                );
            }
        }
    }

    /// The block at `pos` (state `old`) is about to be removed by gameplay: spill contents, fix chest pairing.
    pub(super) fn container_broken(&mut self, pos: [i32; 3], old: u32) {
        if Kind::of_block(BlockState(old).name()).is_none() {
            return;
        }
        self.spill_container(pos);
        if BlockState(old).get("type").is_some() {
            self.chest_removed(pos, old);
        }
    }
}

fn opposite_dir(f: &str) -> &'static str {
    match f {
        "south" => "north",
        "north" => "south",
        "east" => "west",
        "west" => "east",
        "up" => "down",
        _ => "up",
    }
}

fn same(a: &Stack, b: &Stack) -> bool {
    a.item == b.item && a.damage == b.damage && a.ench == b.ench
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn xp_levels() {
        assert_eq!(xp_level(0), (0, 0.0));
        assert_eq!(xp_level(7).0, 1);
        assert_eq!(xp_level(15).0, 1); // level 1 -> 2 needs 9 points
        assert_eq!(xp_level(16).0, 2);
        assert_eq!(xp_level(17).0, 2);
        assert_eq!(xp_level(352).0, 16); // 0..15 cost 352 points in total
        assert_eq!(xp_level(1507).0, 31);
    }
    #[test]
    fn chest_pair_directions() {
        let south_left = BlockState::parse("chest[type=left,facing=south]")
            .unwrap()
            .0;
        let south_right = BlockState::parse("chest[type=right,facing=south]")
            .unwrap()
            .0;
        assert_eq!(partner_dir(south_left), Some("west")); // measured: the west chest of a south-facing pair is `right`, its partner east
        assert_eq!(partner_dir(south_right), Some("east"));
        assert_eq!(
            partner_dir(
                BlockState::parse("chest[type=single,facing=south]")
                    .unwrap()
                    .0
            ),
            None
        );
    }
}

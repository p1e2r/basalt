//! Menu stations: enchanting table, anvil, grindstone, smithing table (transient per-player slots) and the
//! brewing stand's recipe table. Rules are driven by `./data` (enchantments, smithing recipes, repairable items);
//! the anvil / grindstone / table maths follow the public gameplay rules and are checked against vanilla by the
//! m10_* scenarios in crates/check.
use super::container::{Cw, Kind};
use super::inv::Window;
use super::stackx::{intern, Ench, NO_ENCH};
use super::{Game, Stack};
use basalt_data::{registries as reg, registry_id, BlockState};
use basalt_game::{offer_cost, SmithKind};
use basalt_protocol::{ids::play::clientbound as cb, Writer};
use rand::{Rng, SeedableRng};

/// Per-player station state (slots are returned to the player when the window closes).
#[derive(Clone, Debug)]
pub struct St {
    pub items: Vec<Stack>,
    pub rename: Option<String>,
    pub cost: i32,
    /// Anvil: material units that taking the result consumes from slot 1.
    pub units: i32,
    /// Enchantment seed (kept across windows, renewed after each enchant).
    pub seed: i32,
    pub offers: [i32; 3],
    pub clue: [i32; 3],
    pub clue_lvl: [i32; 3],
    pub sent: [i32; 10],
}

impl Default for St {
    fn default() -> St {
        St {
            items: Vec::new(),
            rename: None,
            cost: 0,
            units: 0,
            seed: 0,
            offers: [0; 3],
            clue: [-1; 3],
            clue_lvl: [-1; 3],
            sent: [i32::MIN; 10],
        }
    }
}

impl Kind {
    pub fn is_station(self) -> bool {
        matches!(self, Kind::Enchant | Kind::Anvil | Kind::Grind | Kind::Smith)
    }
    pub fn station_of_block(name: &str) -> Option<Kind> {
        Some(match name {
            "enchanting_table" => Kind::Enchant,
            "anvil" | "chipped_anvil" | "damaged_anvil" => Kind::Anvil,
            "grindstone" => Kind::Grind,
            "smithing_table" => Kind::Smith,
            _ => return None,
        })
    }
    /// Result slot index of the window (take-only).
    pub fn result_slot(self) -> Option<usize> {
        match self {
            Kind::Furnace | Kind::Blast | Kind::Smoker | Kind::Anvil | Kind::Grind => Some(2),
            Kind::Smith => Some(3),
            _ => None,
        }
    }
}

/// Brewing recipes: (input potion name or "water", ingredient item, output potion name). Observed table of vanilla's
/// mixes (wiki + verified with the m10_brew_table scenario); water-bottle forms are handled by the caller.
const BREW: &[(&str, &str, &str)] = &[
    ("water", "nether_wart", "awkward"),
    ("water", "glowstone_dust", "thick"),
    ("water", "redstone", "mundane"),
    ("water", "fermented_spider_eye", "weakness"),
    ("water", "sugar", "mundane"),
    ("water", "ghast_tear", "mundane"),
    ("water", "spider_eye", "mundane"),
    ("water", "blaze_powder", "mundane"),
    ("water", "magma_cream", "mundane"),
    ("water", "glistering_melon_slice", "mundane"),
    ("water", "rabbit_foot", "mundane"),
    ("water", "stone", "mundane"),
    ("water", "slime_block", "mundane"),
    ("water", "cobweb", "mundane"),
    ("water", "breeze_rod", "mundane"),
    ("awkward", "golden_carrot", "night_vision"),
    ("awkward", "magma_cream", "fire_resistance"),
    ("awkward", "rabbit_foot", "leaping"),
    ("awkward", "sugar", "swiftness"),
    ("awkward", "pufferfish", "water_breathing"),
    ("awkward", "glistering_melon_slice", "healing"),
    ("awkward", "spider_eye", "poison"),
    ("awkward", "ghast_tear", "regeneration"),
    ("awkward", "blaze_powder", "strength"),
    ("awkward", "turtle_helmet", "turtle_master"),
    ("awkward", "phantom_membrane", "slow_falling"),
    ("awkward", "stone", "infested"),
    ("awkward", "slime_block", "oozing"),
    ("awkward", "cobweb", "weaving"),
    ("awkward", "breeze_rod", "wind_charged"),
    ("night_vision", "fermented_spider_eye", "invisibility"),
    ("long_night_vision", "fermented_spider_eye", "long_invisibility"),
    ("fire_resistance", "fermented_spider_eye", "slowness"),
    ("long_fire_resistance", "fermented_spider_eye", "long_slowness"),
    ("leaping", "fermented_spider_eye", "slowness"),
    ("long_leaping", "fermented_spider_eye", "long_slowness"),
    ("swiftness", "fermented_spider_eye", "slowness"),
    ("long_swiftness", "fermented_spider_eye", "long_slowness"),
    ("healing", "fermented_spider_eye", "harming"),
    ("strong_healing", "fermented_spider_eye", "strong_harming"),
    ("poison", "fermented_spider_eye", "harming"),
    ("long_poison", "fermented_spider_eye", "harming"),
    ("strong_poison", "fermented_spider_eye", "strong_harming"),
    ("water_breathing", "fermented_spider_eye", "harming"),
    ("long_water_breathing", "fermented_spider_eye", "harming"),
    ("weakness", "redstone", "long_weakness"),
];
/// Potions that extend with redstone (base -> long_*) and strengthen with glowstone dust (base -> strong_*).
const LONGER: &[&str] = &[
    "night_vision",
    "invisibility",
    "fire_resistance",
    "leaping",
    "swiftness",
    "slowness",
    "turtle_master",
    "water_breathing",
    "poison",
    "regeneration",
    "strength",
    "slow_falling",
];
const STRONGER: &[&str] = &[
    "leaping",
    "swiftness",
    "slowness",
    "turtle_master",
    "healing",
    "harming",
    "poison",
    "regeneration",
    "strength",
];

/// Result of brewing `ingredient` onto a bottle of kind `bottle` ("potion" / "splash_potion" / "lingering_potion") holding
/// `potion` (name), as (new bottle item name, new potion name).
pub fn brew(bottle: &str, potion: &str, ingredient: &str) -> Option<(String, String)> {
    if ingredient == "gunpowder" && bottle == "potion" {
        return Some(("splash_potion".into(), potion.into()));
    }
    if ingredient == "dragon_breath" && bottle == "splash_potion" {
        return Some(("lingering_potion".into(), potion.into()));
    }
    if ingredient == "redstone" && LONGER.contains(&potion) {
        return Some((bottle.into(), format!("long_{potion}")));
    }
    if ingredient == "glowstone_dust" && STRONGER.contains(&potion) {
        return Some((bottle.into(), format!("strong_{potion}")));
    }
    BREW.iter()
        .find(|r| r.0 == potion && r.1 == ingredient)
        .map(|r| (bottle.to_string(), r.2.to_string()))
}

pub fn is_brew_ingredient(name: &str) -> bool {
    matches!(name, "gunpowder" | "dragon_breath" | "redstone" | "glowstone_dust") || BREW.iter().any(|r| r.1 == name)
}

impl Game {
    fn en_names(&self) -> &[String] {
        self.sh.registries.entries("minecraft:enchantment")
    }
    fn en_short(&self, id: u16) -> String {
        self.en_names()
            .get(id as usize)
            .map(|n| n.trim_start_matches("minecraft:").to_string())
            .unwrap_or_default()
    }
    fn en_id(&self, name: &str) -> Option<u16> {
        let full = format!("minecraft:{name}");
        self.en_names().iter().position(|n| *n == full).map(|i| i as u16)
    }
    fn item_name(&self, item: i32) -> &'static str {
        reg::ITEM.get(item as usize).copied().unwrap_or("air")
    }

    // ------------------------------------------------------------ opening / closing

    pub(super) fn open_station(&mut self, eid: i32, pos: [i32; 3], kind: Kind) {
        if self.players.get(&eid).map(|p| p.window != Window::Inventory).unwrap_or(true) {
            self.close_window(eid);
        }
        let seed: i32 = self.rng.gen();
        let Some(p) = self.players.get_mut(&eid) else { return };
        p.next_window = p.next_window % 100 + 1;
        let id = p.next_window;
        let cw = Cw {
            id,
            kind,
            pos: [pos, pos],
            halves: 1,
        };
        p.window = Window::Cont(cw);
        p.st.items = vec![Stack::default(); kind.slots()];
        p.st.rename = None;
        p.st.sent = [i32::MIN; 10];
        p.st.cost = 0;
        let _ = seed; // vanilla players start with enchantment seed 0 until their first enchant
        p.cont_items = p.st.items.clone();
        let menu = registry_id(reg::MENU, kind.menu_name()).unwrap_or(2);
        let mut w = Writer::with_id(cb::OPEN_SCREEN);
        w.varint(id)
            .varint(menu)
            .nbt(&basalt_protocol::text::translate(kind.title_key(), vec![]));
        p.send(w);
        Self::send_window(p);
        self.station_update(eid);
    }

    /// Window closed: slots go back to the player (the result slot is discarded).
    pub(super) fn station_closed(&mut self, eid: i32, cw: Cw) {
        let Some(p) = self.players.get_mut(&eid) else { return };
        let items = std::mem::take(&mut p.st.items);
        for (i, s) in items.into_iter().enumerate() {
            if s.count > 0 && Some(i) != cw.kind.result_slot() {
                let left = self.give_stack(eid, s);
                if left > 0 {
                    self.throw_item(eid, Stack { count: left, ..s }, false);
                }
            }
        }
    }

    // ------------------------------------------------------------ slot rules

    pub(super) fn station_may_place(&self, kind: Kind, i: usize, s: &Stack) -> bool {
        let name = self.item_name(s.item);
        match kind {
            Kind::Enchant => i == 1 && name == "lapis_lazuli" || i == 0,
            Kind::Anvil => i < 2,
            Kind::Grind => {
                i < 2
                    && (self.data.item(s.item).map(|x| x.max_damage > 0).unwrap_or(false)
                        || name == "enchanted_book"
                        || !s.ench.list().is_empty())
            }
            Kind::Smith => {
                let r = &self.data.ench.smithing;
                match i {
                    0 => r.iter().any(|x| x.template.matches(s.item)),
                    1 => r.iter().any(|x| x.base.matches(s.item)),
                    2 => r.iter().any(|x| x.addition.matches(s.item)),
                    _ => false,
                }
            }
            _ => true,
        }
    }

    pub(super) fn station_slot_max(&self, kind: Kind, i: usize, s: &Stack) -> i32 {
        if kind == Kind::Enchant && i == 0 {
            1
        } else {
            self.max_stack(s.item)
        }
    }

    // ------------------------------------------------------------ recompute

    pub(super) fn station_update(&mut self, eid: i32) {
        let Some(Window::Cont(cw)) = self.players.get(&eid).map(|p| p.window) else {
            return;
        };
        if !cw.kind.is_station() {
            return;
        }
        match cw.kind {
            Kind::Enchant => self.enchant_update(eid, cw),
            Kind::Anvil => {
                let creative = self.players[&eid].gamemode == 1;
                let (a, b) = (self.players[&eid].st.items[0], self.players[&eid].st.items[1]);
                let rename = self.players[&eid].st.rename.clone();
                let (res, cost, units) = self.anvil_compute(a, b, &rename, creative);
                let p = self.players.get_mut(&eid).unwrap();
                p.st.items[2] = res;
                p.st.cost = cost;
                p.st.units = units;
            }
            Kind::Grind => {
                let (a, b) = (self.players[&eid].st.items[0], self.players[&eid].st.items[1]);
                let r = self.grind_compute(a, b);
                self.players.get_mut(&eid).unwrap().st.items[2] = r;
            }
            Kind::Smith => {
                let it = self.players[&eid].st.items.clone();
                let r = self.smith_compute(it[0], it[1], it[2]);
                self.players.get_mut(&eid).unwrap().st.items[3] = r;
            }
            _ => {}
        }
        let Some(p) = self.players.get_mut(&eid) else { return };
        p.cont_items = p.st.items.clone();
        let vals: Vec<i32> = match cw.kind {
            Kind::Enchant => {
                let s = &p.st;
                vec![
                    s.offers[0],
                    s.offers[1],
                    s.offers[2],
                    s.seed & !0xf,
                    s.clue[0],
                    s.clue[1],
                    s.clue[2],
                    s.clue_lvl[0],
                    s.clue_lvl[1],
                    s.clue_lvl[2],
                ]
            }
            Kind::Anvil => vec![p.st.cost],
            Kind::Smith => vec![(p.st.items[..3].iter().any(|x| x.count > 0) && p.st.items[3].count == 0) as i32],
            _ => vec![],
        };
        for (k, v) in vals.into_iter().enumerate() {
            if p.st.sent[k] != v {
                p.st.sent[k] = v;
                let mut w = Writer::with_id(cb::CONTAINER_SET_DATA);
                w.varint(cw.id).i16(k as i16).i16(v as i16);
                p.send(w);
            }
        }
    }

    // ------------------------------------------------------------ enchanting table

    /// Bookshelf power at the table: shelves in the 5x5 ring (two layers) whose gap block towards the table is replaceable.
    pub fn enchant_power(&self, pos: [i32; 3]) -> i32 {
        let mut n = 0;
        for dx in -2i32..=2 {
            for dz in -2i32..=2 {
                if dx.abs() < 2 && dz.abs() < 2 {
                    continue;
                }
                for dy in 0..=1 {
                    let shelf = self.world.get_block(pos[0] + dx, pos[1] + dy, pos[2] + dz);
                    let gap = self.world.get_block(pos[0] + dx / 2, pos[1] + dy, pos[2] + dz / 2);
                    if self.data.tags.block_has("enchantment_power_provider", BlockState(shelf).name())
                        && self.data.tags.block_has("enchantment_power_transmitter", BlockState(gap).name())
                    {
                        n += 1;
                    }
                }
            }
        }
        n.min(15)
    }

    fn enchant_update(&mut self, eid: i32, cw: Cw) {
        let item = self.players[&eid].st.items[0];
        let seed = self.players[&eid].st.seed;
        let info = self.data.item(item.item).map(|i| i.enchantable).unwrap_or(0);
        let mut offers = [0; 3];
        let mut clue = [-1; 3];
        let mut clue_lvl = [-1; 3];
        if item.count > 0 && info > 0 && item.ench.list().is_empty() {
            let power = self.enchant_power(cw.pos[0]);
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed as u32 as u64);
            for (i, o) in offers.iter_mut().enumerate() {
                *o = offer_cost(&mut rng, power, i);
            }
            let is_book = self.item_name(item.item) == "book";
            for i in 0..3 {
                if offers[i] > 0 {
                    let mut r = rand::rngs::StdRng::seed_from_u64((seed as u32 as u64) + i as u64);
                    let l = self.data.ench.roll(&mut r, item.item, is_book, info, offers[i]);
                    if let Some(pick) = l.get(r.gen_range(0..l.len().max(1))) {
                        if let Some(id) = self.en_id(&pick.0) {
                            clue[i] = id as i32;
                            clue_lvl[i] = pick.1;
                        }
                    }
                }
            }
        }
        let p = self.players.get_mut(&eid).unwrap();
        p.st.offers = offers;
        p.st.clue = clue;
        p.st.clue_lvl = clue_lvl;
    }

    pub(super) fn station_button(&mut self, eid: i32, button: i32) {
        let Some(Window::Cont(cw)) = self.players.get(&eid).map(|p| p.window) else {
            return;
        };
        if cw.kind != Kind::Enchant || !(0..3).contains(&button) {
            return;
        }
        let b = button as usize;
        let p = &self.players[&eid];
        let (item, lapis) = (p.st.items[0], p.st.items[1]);
        let cost = p.st.offers[b];
        let creative = p.gamemode == 1;
        let need = button + 1;
        if cost <= 0 || item.count <= 0 || !item.ench.list().is_empty() {
            return;
        }
        if !creative && (p.xp_level < cost || p.xp_level < need || lapis.count < need || self.item_name(lapis.item) != "lapis_lazuli") {
            return;
        }
        let info = self.data.item(item.item).map(|i| i.enchantable).unwrap_or(0);
        let seed = p.st.seed;
        let is_book = self.item_name(item.item) == "book";
        let mut r = rand::rngs::StdRng::seed_from_u64((seed as u32 as u64) + b as u64);
        let mut list = self.data.ench.roll(&mut r, item.item, is_book, info, cost);
        if list.is_empty() {
            return;
        }
        if is_book && list.len() > 1 {
            let k = r.gen_range(0..list.len());
            list.remove(k);
        }
        let mut res = item;
        if is_book {
            if let Some(eb) = registry_id(reg::ITEM, "enchanted_book") {
                res.item = eb;
                res.ench.stored = true;
            }
        }
        for (n, l) in &list {
            if let Some(id) = self.en_id(n) {
                res.ench.set(id, *l as u8);
            }
        }
        let new_seed: i32 = self.rng.gen();
        let p = self.players.get_mut(&eid).unwrap();
        p.st.items[0] = res;
        if !creative {
            p.st.items[1].count -= need;
            if p.st.items[1].count <= 0 {
                p.st.items[1] = Stack::default();
            }
            self.give_levels(eid, -need);
        }
        self.players.get_mut(&eid).unwrap().st.seed = new_seed;
        let pos = cw.pos[0];
        let pitch = self.rng.gen::<f32>() * 0.1 + 0.9;
        self.play_sound(
            "block.enchantment_table.use",
            4,
            [pos[0] as f64 + 0.5, pos[1] as f64 + 0.5, pos[2] as f64 + 0.5],
            1.0,
            pitch,
        );
        self.station_update(eid);
        if let Some(p) = self.players.get_mut(&eid) {
            Self::send_window(p);
        }
    }

    /// Experience levels change (not points): the point total stays as in vanilla's `giveExperienceLevels`.
    pub(super) fn give_levels(&mut self, eid: i32, n: i32) {
        let Some(p) = self.players.get_mut(&eid) else { return };
        p.xp_level += n;
        if p.xp_level < 0 {
            p.xp_level = 0;
            p.xp_progress = 0.0;
            p.xp = 0;
        }
        Self::send_xp(p);
    }

    // ------------------------------------------------------------ anvil

    pub(super) fn station_rename(&mut self, eid: i32, name: String) {
        let Some(Window::Cont(cw)) = self.players.get(&eid).map(|p| p.window) else {
            return;
        };
        if cw.kind != Kind::Anvil {
            return;
        }
        let name: String = name.chars().take(50).collect();
        self.players.get_mut(&eid).unwrap().st.rename = Some(name);
        self.station_update(eid);
        if let Some(p) = self.players.get_mut(&eid) {
            Self::send_window(p);
        }
    }

    fn stack_enchants(&self, s: &Stack) -> Vec<(String, i32)> {
        s.ench.list().into_iter().map(|(i, l)| (self.en_short(i), l as i32)).collect()
    }

    fn default_name_matches(&self, s: &Stack, name: &str) -> bool {
        // the item's own name (translation) is not known server-side; vanilla compares against the hover name, so only
        // an existing custom name can match
        s.ench.name_str().as_deref() == Some(name)
    }

    /// (result, cost shown in container data, repair-material units consumed). Empty result when nothing can be made.
    pub fn anvil_compute(&self, a: Stack, b: Stack, rename: &Option<String>, creative: bool) -> (Stack, i32, i32) {
        let none = (Stack::default(), 0, 0);
        if a.count <= 0 {
            return none;
        }
        let mut res = Stack { count: 1, ..a };
        let mut total = 0i32;
        let mut base = a.ench.repair as i64;
        let mut units = 0;
        let mut rename_cost = 0;
        let info = self.data.item(a.item);
        let max_damage = info.map(|i| i.max_damage).unwrap_or(0);
        if b.count > 0 {
            base += b.ench.repair as i64;
            let b_book = self.item_name(b.item) == "enchanted_book" && !b.ench.list().is_empty();
            let repairable = info.and_then(|i| i.repairable.as_ref()).map(|r| r.matches(b.item)).unwrap_or(false);
            if max_damage > 0 && repairable {
                let mut d = res.damage.min(max_damage / 4);
                if d <= 0 {
                    return none;
                }
                let mut n = 0;
                while d > 0 && n < b.count {
                    res.damage -= d;
                    total += 1;
                    d = res.damage.min(max_damage / 4);
                    n += 1;
                }
                units = n;
            } else {
                if !b_book && (res.item != b.item || max_damage <= 0) {
                    return none;
                }
                if max_damage > 0 && !b_book {
                    let remaining = (max_damage - res.damage) + (max_damage - b.damage) + max_damage * 12 / 100;
                    let nd = (max_damage - remaining).max(0);
                    if nd < res.damage {
                        res.damage = nd;
                        total += 2;
                    }
                }
                let a_book = self.item_name(a.item) == "enchanted_book";
                let mut applied = false;
                let mut incompatible = false;
                let mut cur = self.stack_enchants(&res);
                for (name, lvl) in self.stack_enchants(&b) {
                    let Some(def) = self.data.ench.by_name(&name) else { continue };
                    let have = cur.iter().find(|e| e.0 == name).map(|e| e.1).unwrap_or(0);
                    let mut level = if have == lvl { lvl + 1 } else { have.max(lvl) };
                    let mut can = def.supported.matches(res.item) || a_book || creative;
                    for (o, _) in &cur {
                        if *o != name && self.data.ench.conflicts(&name, o) {
                            can = false;
                            if !creative {
                                total += 1;
                            }
                        }
                    }
                    if !can {
                        incompatible = true;
                        continue;
                    }
                    level = level.min(def.max_level);
                    match cur.iter_mut().find(|e| e.0 == name) {
                        Some(e) => e.1 = level,
                        None => cur.push((name.clone(), level)),
                    }
                    applied = true;
                    let mut rate = def.anvil_cost;
                    if b_book {
                        rate = (rate / 2).max(1);
                    }
                    total += rate * level;
                }
                if incompatible && !applied {
                    return none;
                }
                let list: Vec<(u16, u8)> = cur.iter().filter_map(|(n, l)| Some((self.en_id(n)?, *l as u8))).collect();
                res.ench.set_list(&list);
            }
        }
        match rename.as_deref() {
            Some("") | None if rename.as_deref() == Some("") => {
                if a.ench.name != 0 {
                    rename_cost = 1;
                    total += 1;
                    res.ench.name = 0;
                }
            }
            Some(n) if !n.is_empty() => {
                if !self.default_name_matches(&a, n) {
                    rename_cost = 1;
                    total += 1;
                    res.ench.name = intern(n);
                }
            }
            _ => {}
        }
        let mut cost = (base + total as i64).min(i32::MAX as i64) as i32;
        if rename_cost > 0 && rename_cost == total && cost >= 40 {
            cost = 39;
        }
        if total <= 0 {
            return (Stack::default(), 0, 0);
        }
        let shown = cost;
        if cost >= 40 && !creative {
            return (Stack::default(), shown, 0);
        }
        let mut pen = a.ench.repair.max(if b.count > 0 { b.ench.repair } else { 0 });
        if rename_cost != total || rename_cost == 0 {
            pen = pen.saturating_mul(2).saturating_add(1);
        }
        res.ench.repair = pen;
        (res, shown, units)
    }

    // ------------------------------------------------------------ grindstone

    fn is_curse(&self, name: &str) -> bool {
        self.data.ench.by_name(name).map(|d| d.curse).unwrap_or(false)
    }

    fn strip_non_curses(&self, mut s: Stack) -> Stack {
        let keep: Vec<(u16, u8)> = s
            .ench
            .list()
            .into_iter()
            .filter(|(i, _)| self.is_curse(&self.en_short(*i)))
            .collect();
        s.ench.set_list(&keep);
        if self.item_name(s.item) == "enchanted_book" && keep.is_empty() {
            if let Some(b) = registry_id(reg::ITEM, "book") {
                s.item = b;
                s.ench.stored = false;
            }
        }
        let mut pen = 0;
        for _ in &keep {
            pen = pen * 2 + 1;
        }
        s.ench.repair = pen;
        s
    }

    pub fn grind_compute(&self, a: Stack, b: Stack) -> Stack {
        let (ea, eb) = (a.count > 0, b.count > 0);
        if !ea && !eb {
            return Stack::default();
        }
        if ea && eb {
            if a.item != b.item {
                return Stack::default();
            }
            let max = self.data.item(a.item).map(|i| i.max_damage).unwrap_or(0);
            let mut r = Stack { count: 1, ..a };
            if max > 0 {
                let rem = (max - a.damage) + (max - b.damage) + max * 5 / 100;
                r.damage = (max - rem).max(0);
            }
            // curses of both survive (higher level wins)
            let mut list = a.ench.list();
            for (i, l) in b.ench.list() {
                match list.iter_mut().find(|e| e.0 == i) {
                    Some(e) => e.1 = e.1.max(l),
                    None => list.push((i, l)),
                }
            }
            r.ench.set_list(&list);
            return self.strip_non_curses(r);
        }
        let s = if ea { a } else { b };
        let has_ench = !s.ench.list().is_empty();
        if !has_ench && !(self.data.item(s.item).map(|i| i.max_damage > 0).unwrap_or(false)) {
            return Stack::default();
        }
        if !has_ench {
            return Stack::default();
        }
        self.strip_non_curses(Stack { count: 1, ..s })
    }

    fn grind_xp(&self, s: &Stack) -> i32 {
        s.ench
            .list()
            .into_iter()
            .filter_map(|(i, l)| {
                let n = self.en_short(i);
                let d = self.data.ench.by_name(&n)?;
                (!d.curse).then(|| d.min_cost_at(l as i32))
            })
            .sum()
    }

    // ------------------------------------------------------------ smithing

    pub fn smith_compute(&self, t: Stack, base: Stack, add: Stack) -> Stack {
        if t.count <= 0 || base.count <= 0 || add.count <= 0 {
            return Stack::default();
        }
        let Some(r) = self.data.ench.smithing_for(t.item, base.item, add.item) else {
            return Stack::default();
        };
        let mut out = Stack { count: 1, ..base };
        match &r.kind {
            SmithKind::Transform { result } => out.item = *result,
            SmithKind::Trim { pattern } => {
                let Some(mat) = self.data.item(add.item).and_then(|i| i.trim_material.clone()) else {
                    return Stack::default();
                };
                let m = self.sh.registries.entries("minecraft:trim_material");
                let p = self.sh.registries.entries("minecraft:trim_pattern");
                let (Some(mi), Some(pi)) = (
                    m.iter().position(|x| *x == format!("minecraft:{mat}")),
                    p.iter().position(|x| *x == format!("minecraft:{pattern}")),
                ) else {
                    return Stack::default();
                };
                if base.ench.trim_mat == mi as u8 + 1 && base.ench.trim_pat == pi as u8 + 1 {
                    return Stack::default();
                }
                out.ench.trim_mat = mi as u8 + 1;
                out.ench.trim_pat = pi as u8 + 1;
            }
        }
        out
    }

    // ------------------------------------------------------------ taking the result

    /// Click on the result slot of a station. Returns false when the click is not a plain take (falls back to generic).
    pub(super) fn station_result_click(&mut self, eid: i32, cw: &Cw, mode: i32, button: i8) -> bool {
        let r = cw.kind.result_slot().unwrap_or(0);
        let res = self.players[&eid].st.items[r];
        if !matches!(mode, 0 | 1 | 2 | 4) || res.count <= 0 {
            return true;
        }
        let creative = self.players[&eid].gamemode == 1;
        if cw.kind == Kind::Anvil {
            let p = &self.players[&eid];
            if !(creative || p.xp_level >= p.st.cost) || p.st.cost <= 0 {
                return true;
            }
        }
        let n = cw.n_cont();
        let total = cw.total();
        match mode {
            0 => {
                let c = self.players[&eid].cursor;
                if c.count > 0
                    && !(c.item == res.item
                        && c.ench == res.ench
                        && c.damage == res.damage
                        && c.count + res.count <= self.max_stack(c.item))
                {
                    return true;
                }
                self.station_consume(eid, cw);
                let p = self.players.get_mut(&eid).unwrap();
                p.cursor = Stack {
                    count: c.count + res.count,
                    ..res
                };
            }
            1 => {
                // must fit entirely (hotbar end first)
                let left = self.win_move_to(eid, cw, res, n, total, true);
                if left.count > 0 {
                    return true;
                }
                self.station_consume(eid, cw);
            }
            2 => {
                let hb = if button == 40 {
                    45usize
                } else if (0..9).contains(&button) {
                    36 + button as usize
                } else {
                    return true;
                };
                if self.players[&eid].inv[hb].count > 0 {
                    return true;
                }
                self.station_consume(eid, cw);
                self.players.get_mut(&eid).unwrap().inv[hb] = res;
            }
            _ => {
                self.station_consume(eid, cw);
                self.throw_item(eid, res, false);
            }
        }
        true
    }

    /// Apply the cost / input consumption of taking the result.
    fn station_consume(&mut self, eid: i32, cw: &Cw) {
        let pos = cw.pos[0];
        let creative = self.players[&eid].gamemode == 1;
        let ctr = [pos[0] as f64 + 0.5, pos[1] as f64 + 0.5, pos[2] as f64 + 0.5];
        match cw.kind {
            Kind::Anvil => {
                let (cost, units) = (self.players[&eid].st.cost, self.players[&eid].st.units);
                if !creative {
                    self.give_levels(eid, -cost);
                }
                let p = self.players.get_mut(&eid).unwrap();
                p.st.items[0] = Stack::default();
                if units > 0 && p.st.items[1].count > units {
                    p.st.items[1].count -= units;
                } else if units > 0 || p.st.items[1].count > 0 {
                    p.st.items[1] = Stack::default();
                }
                p.st.items[2] = Stack::default();
                let st = self.world.get_block(pos[0], pos[1], pos[2]);
                let mut event = 1030;
                if !creative && self.rng.gen::<f32>() < 0.12 {
                    let name = BlockState(st).name();
                    let next = match name {
                        "anvil" => Some("chipped_anvil"),
                        "chipped_anvil" => Some("damaged_anvil"),
                        _ => None,
                    };
                    match next {
                        Some(n) => {
                            if let Some(s2) = BlockState(st)
                                .get("facing")
                                .and_then(|f| BlockState::parse(&format!("{n}[facing={f}]")))
                            {
                                self.set_block(pos[0], pos[1], pos[2], s2.0);
                            }
                        }
                        None => {
                            self.set_block(pos[0], pos[1], pos[2], 0);
                            event = 1029;
                        }
                    }
                }
                self.level_event(event, pos);
            }
            Kind::Grind => {
                let (a, b) = (self.players[&eid].st.items[0], self.players[&eid].st.items[1]);
                let xp = self.grind_xp(&a) + self.grind_xp(&b);
                if xp > 0 {
                    let j = (xp as f64 / 2.0).ceil() as i32;
                    let amount = j + self.rng.gen_range(0..j);
                    self.spawn_xp(ctr, amount);
                }
                let p = self.players.get_mut(&eid).unwrap();
                p.st.items[0] = Stack::default();
                p.st.items[1] = Stack::default();
                p.st.items[2] = Stack::default();
                self.level_event(1042, pos);
            }
            Kind::Smith => {
                let p = self.players.get_mut(&eid).unwrap();
                for i in 0..3 {
                    p.st.items[i].count -= 1;
                    if p.st.items[i].count <= 0 {
                        p.st.items[i] = Stack::default();
                    }
                }
                p.st.items[3] = Stack::default();
                self.level_event(1044, pos);
            }
            _ => {}
        }
    }

    fn level_event(&self, ty: i32, pos: [i32; 3]) {
        self.broadcast_chunk((pos[0] >> 4, pos[2] >> 4), None, || {
            let mut w = Writer::with_id(cb::LEVEL_EVENT);
            w.i32(ty).position(pos[0], pos[1], pos[2]).i32(0).bool(false);
            w
        });
    }

    /// Shift-click routing from the player's inventory into a station (None = use the main/hotbar swap).
    pub(super) fn station_quick_target_any(&self, cw: &Cw, s: &Stack) -> Option<(usize, usize)> {
        let name = self.item_name(s.item);
        match cw.kind {
            Kind::Brewing => {
                if name == "blaze_powder" {
                    Some((4, 5))
                } else if matches!(name, "potion" | "splash_potion" | "lingering_potion" | "glass_bottle") {
                    Some((0, 3))
                } else if is_brew_ingredient(name) {
                    Some((3, 4))
                } else {
                    None
                }
            }
            Kind::Enchant => {
                if self.item_name(s.item) == "lapis_lazuli" {
                    Some((1, 2))
                } else {
                    Some((0, 1))
                }
            }
            Kind::Anvil | Kind::Grind => Some((0, 2)),
            Kind::Smith => (0..3).find(|&i| self.station_may_place(Kind::Smith, i, s)).map(|i| (i, i + 1)),
            _ => None,
        }
    }
}

impl Game {
    fn potion_of(&self, s: &Stack) -> Option<&'static str> {
        if s.ench.potion == 0 {
            return Some("water");
        }
        reg::POTION.get(s.ench.potion as usize - 1).copied()
    }

    fn brewable(&self, items: &[Stack]) -> bool {
        let ing = self.item_name(items[3].item);
        if items[3].count <= 0 {
            return false;
        }
        (0..3).any(|i| {
            let b = items[i];
            let bn = self.item_name(b.item);
            b.count > 0
                && matches!(bn, "potion" | "splash_potion" | "lingering_potion")
                && self.potion_of(&b).map(|p| brew(bn, p, ing).is_some()).unwrap_or(false)
        })
    }

    /// One brewing stand tick: fuel is taken from the blaze powder slot (20 charges), a brew takes 400 ticks and uses
    /// one charge, finishes by converting every bottle and consuming one ingredient.
    pub(super) fn tick_brewing(&mut self, pos: [i32; 3]) {
        let Some(be) = self.bes.get(&pos) else { return };
        let mut be = be.clone();
        let before = (be.brew, be.fuel);
        let items_before = be.items.clone();
        if be.fuel <= 0 && be.items[4].count > 0 && self.item_name(be.items[4].item) == "blaze_powder" {
            be.fuel = 20;
            be.items[4].count -= 1;
            if be.items[4].count <= 0 {
                be.items[4] = Stack::default();
            }
        }
        let ok = self.brewable(&be.items);
        if be.brew > 0 {
            be.brew -= 1;
            if be.brew == 0 && ok {
                let ing = self.item_name(be.items[3].item);
                for i in 0..3 {
                    let b = be.items[i];
                    let bn = self.item_name(b.item);
                    if b.count <= 0 || !matches!(bn, "potion" | "splash_potion" | "lingering_potion") {
                        continue;
                    }
                    let Some(pn) = self.potion_of(&b) else { continue };
                    if let Some((nb, np)) = brew(bn, pn, ing) {
                        if let (Some(item), Some(pi)) = (registry_id(reg::ITEM, &nb), reg::POTION.iter().position(|x| *x == np)) {
                            be.items[i] = Stack {
                                item,
                                count: 1,
                                damage: 0,
                                ench: Ench {
                                    potion: pi as u8 + 1,
                                    ..NO_ENCH
                                },
                            };
                        }
                    }
                }
                be.items[3].count -= 1;
                if be.items[3].count <= 0 {
                    be.items[3] = Stack::default();
                }
                be.brew_item = -1;
                self.play_sound(
                    "block.brewing_stand.brew",
                    4,
                    [pos[0] as f64 + 0.5, pos[1] as f64 + 0.5, pos[2] as f64 + 0.5],
                    1.0,
                    1.0,
                );
            } else if !ok || be.items[3].item != be.brew_item {
                be.brew = 0;
            }
        } else if ok && be.fuel > 0 {
            be.fuel -= 1;
            be.brew = 400;
            be.brew_item = be.items[3].item;
        }
        let changed = before != (be.brew, be.fuel) || items_before != be.items;
        if !changed {
            return;
        }
        // has_bottle_N block state follows the three bottle slots
        let st = self.world.get_block(pos[0], pos[1], pos[2]);
        let mut ns = st;
        for i in 0..3 {
            let v = if be.items[i].count > 0 { "true" } else { "false" };
            if let Some(x) = BlockState(ns).with(&format!("has_bottle_{i}"), v) {
                ns = x.0;
            }
        }
        let viewers = be.viewers.clone();
        self.bes.insert(pos, be);
        if ns != st {
            self.set_block(pos[0], pos[1], pos[2], ns);
        }
        self.cont_changed(pos);
        for v in viewers {
            self.send_furnace_data(v);
        }
    }
}

#[allow(dead_code)]
fn _keep(_: Ench) -> Ench {
    NO_ENCH
}

/// Split on commas outside braces / quotes.
fn split_top(s: &str) -> Vec<String> {
    let (mut out, mut depth, mut q, mut cur) = (Vec::new(), 0, None::<char>, String::new());
    for c in s.chars() {
        match (c, q) {
            ('"' | '\'', None) => {
                q = Some(c);
                cur.push(c)
            }
            (c2, Some(qc)) if c2 == qc => {
                q = None;
                cur.push(c)
            }
            ('{' | '[', None) => {
                depth += 1;
                cur.push(c)
            }
            ('}' | ']', None) => {
                depth -= 1;
                cur.push(c)
            }
            (',', None) if depth == 0 => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

impl Game {
    /// Apply the item components of a command item argument (`[damage=5,enchantments={sharpness:3},...]`) to a stack.
    /// Supports damage, repair_cost, custom_name, enchantments, stored_enchantments and potion_contents.potion.
    pub(super) fn apply_components(&self, raw: &str, s: &mut Stack) {
        let Some(body) = raw.split_once('[').map(|x| x.1.trim_end_matches(']')) else {
            return;
        };
        for part in split_top(body) {
            let Some((k, v)) = part.split_once('=') else { continue };
            let (k, v) = (k.trim().trim_start_matches("minecraft:"), v.trim());
            let unq = |x: &str| {
                x.trim()
                    .trim_matches(|c| c == '"' || c == '\'')
                    .trim_start_matches("minecraft:")
                    .to_string()
            };
            match k {
                "damage" => s.damage = v.parse().unwrap_or(0),
                "repair_cost" => s.ench.repair = v.parse().unwrap_or(0),
                "custom_name" => s.ench.name = intern(v.trim_matches(|c| c == '"' || c == '\'')),
                "enchantments" | "stored_enchantments" => {
                    s.ench.stored = k == "stored_enchantments";
                    let inner = v.trim_start_matches('{').trim_end_matches('}');
                    for e in split_top(inner) {
                        if let Some((n, l)) = e.split_once(':') {
                            if let (Some(id), Ok(l)) = (self.en_id(&unq(n)), l.trim().parse::<u8>()) {
                                s.ench.set(id, l);
                            }
                        }
                    }
                }
                "potion_contents" => {
                    let inner = v.trim_start_matches('{').trim_end_matches('}');
                    for e in split_top(inner) {
                        if let Some((n, l)) = e.split_once(':') {
                            if n.trim() == "potion" {
                                let pn = unq(l);
                                if let Some(i) = reg::POTION.iter().position(|x| *x == pn) {
                                    s.ench.potion = i as u8 + 1;
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Measured on vanilla 26.2 (m10_brew_table, 2026-11-24): base x ingredient on a potion bottle -> (bottle, potion).
    /// Only entries where the result differs from the base are listed; everything else must be unchanged.
    #[test]
    fn brew_table_matches_vanilla_measurement() {
        let m: &[(&str, &str, &str, &str)] = &[
            ("water", "nether_wart", "potion", "awkward"),
            ("water", "redstone", "potion", "mundane"),
            ("water", "glowstone_dust", "potion", "thick"),
            ("water", "fermented_spider_eye", "potion", "weakness"),
            ("water", "gunpowder", "splash_potion", "water"),
            ("water", "sugar", "potion", "mundane"),
            ("water", "magma_cream", "potion", "mundane"),
            ("water", "rabbit_foot", "potion", "mundane"),
            ("water", "glistering_melon_slice", "potion", "mundane"),
            ("water", "spider_eye", "potion", "mundane"),
            ("water", "ghast_tear", "potion", "mundane"),
            ("water", "blaze_powder", "potion", "mundane"),
            ("water", "stone", "potion", "mundane"),
            ("water", "slime_block", "potion", "mundane"),
            ("water", "cobweb", "potion", "mundane"),
            ("water", "breeze_rod", "potion", "mundane"),
            ("awkward", "gunpowder", "splash_potion", "awkward"),
            ("awkward", "sugar", "potion", "swiftness"),
            ("awkward", "golden_carrot", "potion", "night_vision"),
            ("awkward", "magma_cream", "potion", "fire_resistance"),
            ("awkward", "rabbit_foot", "potion", "leaping"),
            ("awkward", "pufferfish", "potion", "water_breathing"),
            ("awkward", "glistering_melon_slice", "potion", "healing"),
            ("awkward", "spider_eye", "potion", "poison"),
            ("awkward", "ghast_tear", "potion", "regeneration"),
            ("awkward", "blaze_powder", "potion", "strength"),
            ("awkward", "turtle_helmet", "potion", "turtle_master"),
            ("awkward", "phantom_membrane", "potion", "slow_falling"),
            ("awkward", "stone", "potion", "infested"),
            ("awkward", "slime_block", "potion", "oozing"),
            ("awkward", "cobweb", "potion", "weaving"),
            ("awkward", "breeze_rod", "potion", "wind_charged"),
            ("healing", "glowstone_dust", "potion", "strong_healing"),
            ("healing", "fermented_spider_eye", "potion", "harming"),
            ("healing", "gunpowder", "splash_potion", "healing"),
            ("poison", "redstone", "potion", "long_poison"),
            ("poison", "glowstone_dust", "potion", "strong_poison"),
            ("poison", "fermented_spider_eye", "potion", "harming"),
            ("poison", "gunpowder", "splash_potion", "poison"),
            ("swiftness", "redstone", "potion", "long_swiftness"),
            ("swiftness", "glowstone_dust", "potion", "strong_swiftness"),
            ("swiftness", "fermented_spider_eye", "potion", "slowness"),
            ("swiftness", "gunpowder", "splash_potion", "swiftness"),
            ("night_vision", "redstone", "potion", "long_night_vision"),
            ("night_vision", "fermented_spider_eye", "potion", "invisibility"),
            ("night_vision", "gunpowder", "splash_potion", "night_vision"),
            ("weakness", "redstone", "potion", "long_weakness"),
            ("weakness", "gunpowder", "splash_potion", "weakness"),
            ("strong_swiftness", "gunpowder", "splash_potion", "strong_swiftness"),
            ("long_leaping", "fermented_spider_eye", "potion", "long_slowness"),
            ("long_leaping", "gunpowder", "splash_potion", "long_leaping"),
        ];
        let bases = [
            "water",
            "awkward",
            "healing",
            "poison",
            "swiftness",
            "night_vision",
            "weakness",
            "strong_swiftness",
            "long_leaping",
        ];
        let ings = [
            "nether_wart",
            "redstone",
            "glowstone_dust",
            "fermented_spider_eye",
            "gunpowder",
            "sugar",
            "golden_carrot",
            "magma_cream",
            "rabbit_foot",
            "pufferfish",
            "glistering_melon_slice",
            "spider_eye",
            "ghast_tear",
            "blaze_powder",
            "turtle_helmet",
            "phantom_membrane",
            "stone",
            "slime_block",
            "cobweb",
            "breeze_rod",
        ];
        for b in bases {
            for i in ings {
                let want = m.iter().find(|x| x.0 == b && x.1 == i).map(|x| (x.2.to_string(), x.3.to_string()));
                let got = brew("potion", b, i);
                assert_eq!(got, want, "{b} + {i}");
            }
        }
    }
    #[test]
    fn brew_table() {
        assert_eq!(brew("potion", "water", "nether_wart"), Some(("potion".into(), "awkward".into())));
        assert_eq!(
            brew("potion", "strength", "redstone"),
            Some(("potion".into(), "long_strength".into()))
        );
        assert_eq!(
            brew("potion", "healing", "glowstone_dust"),
            Some(("potion".into(), "strong_healing".into()))
        );
        assert_eq!(
            brew("splash_potion", "poison", "dragon_breath"),
            Some(("lingering_potion".into(), "poison".into()))
        );
        assert_eq!(brew("potion", "luck", "redstone"), None);
    }
}

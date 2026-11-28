//! Items that change the block they are used on (use_item_on after the block's own use):
//! hoes till, shovels flatten paths and douse campfires, axes strip logs / scrape copper / remove
//! wax, shears carve pumpkins, honeycomb waxes copper, bone meal grows plants.
//!
//! Rules measured on vanilla 26.2 with scenario tool_actions (2026-11-26):
//! * hoe: grass_block / dirt / dirt_path -> farmland[moisture=0], coarse_dirt -> dirt, rooted_dirt ->
//!   dirt + 1 hanging_roots item; any face but the bottom, only with air above (a flower or short
//!   grass on top blocks it); podzol / mycelium untouched.
//! * shovel: grass_block / dirt / coarse_dirt / rooted_dirt / podzol / mycelium -> dirt_path (any face
//!   but the bottom, air above; farmland stays); a lit (soul) campfire goes out from any face, level
//!   event 1009 heard by everyone.
//! * axe: log/wood/stem/hyphae/bamboo_block -> stripped_ variant (axis kept), copper one oxidation
//!   stage back (other properties kept), waxed_ copper -> unwaxed. Doors/trapdoors open instead (the
//!   block's own use comes first).
//! * shears on a pumpkin: carved_pumpkin facing the clicked side (top/bottom: towards the player),
//!   4 pumpkin_seeds, sound block.pumpkin.carve to everyone.
//! * honeycomb on unwaxed copper -> waxed_ variant, one honeycomb used (not in creative).
//! * tools lose 1 durability per successful use (not in creative); failed uses cost nothing.
//! * bone meal: crops age +2..5 (capped), saplings, grass blocks, short grass / fern; one used per
//!   successful application (not on a fully grown crop or stone), level event 1505 for everyone.
//!   The player's own sounds (hoe till, shovel flatten, axe strip/scrape/wax off, honeycomb wax on)
//!   are played by their client; onlookers get them from the server (not measured: single bot).
use super::*;
use basalt_data::BlockState;
use rand::Rng;

fn bs(name: &str) -> Option<BlockState> {
    BlockState::from_name(name)
}

/// `to` with every property of `from` that `to` also has.
fn rename(from: BlockState, to: &str) -> Option<u32> {
    let mut t = bs(to)?;
    for (k, v) in from.props() {
        if let Some(n) = t.with(k, v) {
            t = n;
        }
    }
    Some(t.0)
}

/// Copper block one oxidation stage back (None for unoxidized / non-copper).
fn scraped(name: &str) -> Option<String> {
    if name.starts_with("waxed_") {
        return None;
    }
    let (from, to) = [
        ("exposed_", ""),
        ("weathered_", "exposed_"),
        ("oxidized_", "weathered_"),
    ]
    .into_iter()
    .find(|(p, _)| name.starts_with(p))?;
    let rest = &name[from.len()..];
    let out = if rest == "copper" && to.is_empty() {
        "copper_block".to_string()
    } else {
        format!("{to}{rest}")
    };
    bs(&out).map(|_| out)
}

fn is_copper(name: &str) -> bool {
    name.contains("copper") || name == "lightning_rod"
}

fn stripped(name: &str) -> Option<String> {
    if name.starts_with("stripped_") {
        return None;
    }
    let ok = name.ends_with("_log")
        || name.ends_with("_wood")
        || name.ends_with("_stem")
        || name.ends_with("_hyphae")
        || name == "bamboo_block";
    if !ok {
        return None;
    }
    let out = format!("stripped_{name}");
    bs(&out).map(|_| out)
}

fn max_age(s: BlockState) -> i32 {
    let mut m = 0;
    while s.with("age", &(m + 1).to_string()).is_some() {
        m += 1;
    }
    m
}

impl Game {
    pub(super) fn level_event_data(&self, ty: i32, pos: [i32; 3], data: i32, except: Option<i32>) {
        self.broadcast_chunk((pos[0] >> 4, pos[2] >> 4), except, || {
            let mut w = Writer::with_id(cb::LEVEL_EVENT);
            w.i32(ty)
                .position(pos[0], pos[1], pos[2])
                .i32(data)
                .bool(false);
            w
        });
    }

    /// Use one of the held stack (survival / adventure).
    pub(super) fn consume_held(&mut self, eid: i32) {
        let Some(p) = self.players.get_mut(&eid) else {
            return;
        };
        if p.gamemode == 1 {
            return;
        }
        let slot = 36 + p.held;
        p.inv[slot].count -= 1;
        if p.inv[slot].count <= 0 {
            p.inv[slot] = Stack::default();
        }
        Self::send_window(p);
    }

    /// Tool / item action on the clicked block. True when the item did something (or is an item
    /// that never places a block, so placement should not be tried).
    pub(super) fn use_tool_on(&mut self, eid: i32, b: [i32; 3], face: usize, item: &str) -> bool {
        let s = BlockState(self.world.get_block(b[0], b[1], b[2]));
        let name = s.name();
        let up = BlockState(self.world.get_block(b[0], b[1] + 1, b[2]));
        let top_free = face != 0 && up.name() == "air";
        let center = [b[0] as f64 + 0.5, b[1] as f64 + 0.5, b[2] as f64 + 0.5];
        if item.ends_with("_hoe") {
            let (to, drop) = match name {
                "grass_block" | "dirt" | "dirt_path" => ("farmland", None),
                "coarse_dirt" => ("dirt", None),
                "rooted_dirt" => ("dirt", Some("hanging_roots")),
                _ => return true,
            };
            if !top_free {
                return true;
            }
            self.set_block(b[0], b[1], b[2], bs(to).map(|s| s.0).unwrap_or(0));
            self.update_neighbors(b, 0);
            if let Some(d) = drop {
                if let Some(id) = basalt_data::registry_id(reg::ITEM, d) {
                    self.drop_from_face(b, face, id);
                }
            }
            self.play_sound_except("item.hoe.till", 4, center, 1.0, 1.0, Some(eid));
            self.wear_held(eid, 1);
            return true;
        }
        if item.ends_with("_shovel") {
            if matches!(name, "campfire" | "soul_campfire") {
                if s.get("lit") == Some("true") {
                    let ns = s.with("lit", "false").map(|s| s.0).unwrap_or(s.0);
                    self.set_block(b[0], b[1], b[2], ns);
                    self.level_event_data(1009, b, 0, None);
                    self.wear_held(eid, 1);
                }
                return true;
            }
            let pathable = matches!(
                name,
                "grass_block" | "dirt" | "coarse_dirt" | "rooted_dirt" | "podzol" | "mycelium"
            );
            if pathable && top_free {
                self.set_block(b[0], b[1], b[2], bs("dirt_path").map(|s| s.0).unwrap_or(0));
                self.update_neighbors(b, 0);
                self.play_sound_except("item.shovel.flatten", 4, center, 1.0, 1.0, Some(eid));
                self.wear_held(eid, 1);
            }
            return true;
        }
        if item.ends_with("_axe") {
            let (to, sound, event) = if let Some(t) = stripped(name) {
                (t, "item.axe.strip", None)
            } else if let Some(t) = scraped(name) {
                (t, "item.axe.scrape", Some(3005))
            } else if let Some(t) = name
                .strip_prefix("waxed_")
                .filter(|t| bs(t).is_some())
                .map(str::to_string)
            {
                (t, "item.axe.wax_off", Some(3004))
            } else {
                return true;
            };
            let Some(ns) = rename(s, &to) else {
                return true;
            };
            self.set_block(b[0], b[1], b[2], ns);
            self.retype_other_half(b, s, &to);
            self.play_sound_except(sound, 4, center, 1.0, 1.0, Some(eid));
            if let Some(e) = event {
                self.level_event_data(e, b, 0, Some(eid));
            }
            self.wear_held(eid, 1);
            return true;
        }
        match item {
            // fluids are emptied / picked up by use_item (bucket.rs), never placed like blocks
            "bucket" | "water_bucket" | "lava_bucket" => true,
            "shears" => {
                if name != "pumpkin" {
                    return false;
                }
                let facing = if face >= 2 {
                    face
                } else {
                    // towards the player
                    let yaw = self.players.get(&eid).map(|p| p.yaw).unwrap_or(0.0);
                    let d = ((yaw / 90.0).round() as i32).rem_euclid(4);
                    // yaw 0 looks south (+z): the carved face points north, at the player
                    [2, 5, 3, 4][d as usize]
                };
                let fname = ["down", "up", "north", "south", "west", "east"][facing];
                let ns = bs("carved_pumpkin")
                    .and_then(|c| c.with("facing", fname))
                    .map(|c| c.0)
                    .unwrap_or(0);
                self.set_block(b[0], b[1], b[2], ns);
                if let Some(id) = basalt_data::registry_id(reg::ITEM, "pumpkin_seeds") {
                    let pos = [
                        center[0] + FACE[facing][0] as f64 * 0.65,
                        b[1] as f64 + 0.1,
                        center[2] + FACE[facing][2] as f64 * 0.65,
                    ];
                    let vel = [
                        0.05 * FACE[facing][0] as f64 + self.rng.gen_range(0.0..0.02),
                        0.05,
                        0.05 * FACE[facing][2] as f64 + self.rng.gen_range(0.0..0.02),
                    ];
                    self.spawn_item(
                        pos,
                        vel,
                        Stack {
                            item: id,
                            count: 4,
                            ..Stack::default()
                        },
                        10,
                    );
                }
                self.play_sound("block.pumpkin.carve", 4, center, 1.0, 1.0);
                self.wear_held(eid, 1);
                true
            }
            "honeycomb" => {
                if name.starts_with("waxed_") || !is_copper(name) {
                    return true;
                }
                let to = if name == "copper_block" {
                    "waxed_copper_block".to_string()
                } else {
                    format!("waxed_{name}")
                };
                let Some(ns) = rename(s, &to) else {
                    return true;
                };
                self.set_block(b[0], b[1], b[2], ns);
                self.retype_other_half(b, s, &to);
                self.play_sound_except("item.honeycomb.wax_on", 4, center, 1.0, 1.0, Some(eid));
                self.level_event_data(3003, b, 0, Some(eid));
                self.consume_held(eid);
                true
            }
            "bone_meal" => {
                if self.bone_meal(b) {
                    self.level_event_data(1505, b, 15, None);
                    self.consume_held(eid);
                }
                true
            }
            _ => false,
        }
    }

    /// Doors keep both halves the same block.
    fn retype_other_half(&mut self, b: [i32; 3], s: BlockState, to: &str) {
        let Some(half) = s.get("half") else {
            return;
        };
        if s.block().kind != "door" && s.block().kind != "weathering_copper_door" {
            return;
        }
        let o = [b[0], b[1] + if half == "upper" { -1 } else { 1 }, b[2]];
        let os = BlockState(self.world.get_block(o[0], o[1], o[2]));
        if os.name() == s.name() {
            if let Some(n) = rename(os, to) {
                self.set_block(o[0], o[1], o[2], n);
            }
        }
    }

    fn drop_from_face(&mut self, b: [i32; 3], face: usize, item: i32) {
        // measured (till_rooted): first seen exactly on the clicked face, up to ±0.25 across it
        let f = FACE[face];
        let mut pos = [0.0; 3];
        for i in 0..3 {
            pos[i] = b[i] as f64
                + 0.5
                + if f[i] != 0 {
                    f[i] as f64 * 0.5
                } else {
                    self.rng.gen_range(-0.25..0.25)
                };
        }
        let vel = [
            self.rng.gen_range(-0.1..0.1),
            0.2,
            self.rng.gen_range(-0.1..0.1),
        ];
        self.spawn_item(
            pos,
            vel,
            Stack {
                item,
                count: 1,
                ..Stack::default()
            },
            10,
        );
    }

    /// Bone meal on `b`; true when it applied (the item is used up even if a random growth step
    /// did nothing, e.g. a sapling that didn't advance).
    fn bone_meal(&mut self, b: [i32; 3]) -> bool {
        let s = BlockState(self.world.get_block(b[0], b[1], b[2]));
        let n = s.name();
        let up = [b[0], b[1] + 1, b[2]];
        match n {
            "wheat" | "carrots" | "potatoes" | "beetroots" | "torchflower_crop" => {
                let a: i32 = s.get("age").and_then(|v| v.parse().ok()).unwrap_or(0);
                let m = max_age(s);
                if a >= m {
                    return false;
                }
                let na = (a + self.rng.gen_range(2..=5)).min(m);
                if let Some(ns) = s.with("age", &na.to_string()) {
                    self.set_block(b[0], b[1], b[2], ns.0);
                }
                true
            }
            "melon_stem" | "pumpkin_stem" => {
                let a: i32 = s.get("age").and_then(|v| v.parse().ok()).unwrap_or(0);
                if a >= 7 {
                    return false;
                }
                let na = (a + self.rng.gen_range(2..=5)).min(7);
                if let Some(ns) = s.with("age", &na.to_string()) {
                    self.set_block(b[0], b[1], b[2], ns.0);
                }
                true
            }
            "oak_sapling" => {
                if self.rng.gen::<f32>() < 0.45 {
                    if s.get("stage") == Some("0") {
                        if let Some(ns) = s.with("stage", "1") {
                            self.set_block(b[0], b[1], b[2], ns.0);
                        }
                    } else {
                        self.grow_oak(b);
                    }
                }
                true
            }
            "short_grass" | "fern" => {
                let tall = if n == "fern" {
                    "large_fern"
                } else {
                    "tall_grass"
                };
                if self.world.get_block(up[0], up[1], up[2]) != 0 {
                    return false;
                }
                if let (Some(lo), Some(hi)) = (
                    bs(tall).and_then(|t| t.with("half", "lower")),
                    bs(tall).and_then(|t| t.with("half", "upper")),
                ) {
                    self.set_block(b[0], b[1], b[2], lo.0);
                    self.set_block(up[0], up[1], up[2], hi.0);
                }
                true
            }
            "grass_block" => {
                if self.world.get_block(up[0], up[1], up[2]) != 0 {
                    return false;
                }
                self.grass_bonemeal(up);
                true
            }
            _ => false,
        }
    }

    /// Random walks from the cell above a bone-mealed grass block: empty cells over grass get short
    /// grass (sometimes a flower), existing short grass sometimes grows tall.
    fn grass_bonemeal(&mut self, start: [i32; 3]) {
        let short = bs("short_grass").map(|s| s.0).unwrap_or(0);
        let flowers = ["dandelion", "poppy"];
        'outer: for i in 0..128 {
            let mut p = start;
            for _ in 0..i / 16 {
                p[0] += self.rng.gen_range(-1..=1);
                p[1] += self.rng.gen_range(-1..=1) * self.rng.gen_range(0..3) / 2;
                p[2] += self.rng.gen_range(-1..=1);
                let below = BlockState(self.world.get_block(p[0], p[1] - 1, p[2]));
                let here = self.world.get_block(p[0], p[1], p[2]);
                if below.name() != "grass_block" || basalt_world::props::props(here).blocks_motion()
                {
                    continue 'outer;
                }
            }
            let here = BlockState(self.world.get_block(p[0], p[1], p[2]));
            if here.name() == "short_grass" {
                if self.rng.gen_range(0..10) == 0 {
                    self.bone_meal(p);
                }
                continue;
            }
            if here.0 != 0 {
                continue;
            }
            let st = if self.rng.gen_range(0..8) == 0 {
                bs(flowers[self.rng.gen_range(0..flowers.len())])
                    .map(|s| s.0)
                    .unwrap_or(short)
            } else {
                short
            };
            self.set_block(p[0], p[1], p[2], st);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copper_names() {
        assert_eq!(scraped("exposed_copper").as_deref(), Some("copper_block"));
        assert_eq!(
            scraped("oxidized_cut_copper_stairs").as_deref(),
            Some("weathered_cut_copper_stairs")
        );
        assert_eq!(scraped("exposed_cut_copper").as_deref(), Some("cut_copper"));
        assert_eq!(scraped("copper_block"), None);
        assert_eq!(scraped("waxed_exposed_copper"), None);
        assert_eq!(stripped("oak_log").as_deref(), Some("stripped_oak_log"));
        assert_eq!(
            stripped("bamboo_block").as_deref(),
            Some("stripped_bamboo_block")
        );
        assert_eq!(stripped("stripped_oak_log"), None);
        assert!(bs("waxed_copper_block").is_some());
    }
}

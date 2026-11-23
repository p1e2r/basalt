//! Player data in vanilla 26.2's format, `players/data/<uuid>.dat` (gzipped NBT), written next to
//! Basalt's `players/<uuid>.json` so vanilla servers see the same player; tags Basalt does not use
//! (recipe book, attributes, brain, ...) are kept from the existing file. A world that only has
//! vanilla's .dat (a world made by vanilla) is loaded from it. Layout as in the files the 26.2
//! server wrote in vanilla/srv/world/players/data (2026-11-24): Inventory entries {Slot, id,
//! count, components}, Slot 0-8 hotbar / 9-35 main, armor and offhand in `equipment`
//! {head, chest, legs, feet, offhand}, UUID as 4 ints.
use super::{dim, Game, Player, Stack, NO_ENCH};
use basalt_data::registries as reg;
use basalt_nbt::{Compound, Tag};
use uuid::Uuid;

const EQUIP: [(&str, usize); 5] = [
    ("head", 5),
    ("chest", 6),
    ("legs", 7),
    ("feet", 8),
    ("offhand", 45),
];

fn path(dir: &std::path::Path, uuid: Uuid) -> std::path::PathBuf {
    dir.join("players/data").join(format!("{uuid}.dat"))
}

fn uuid_ints(u: Uuid) -> Vec<i32> {
    u.as_bytes()
        .chunks(4)
        .map(|c| i32::from_be_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Window slot of the player inventory (Basalt's index) for a vanilla Inventory `Slot`.
fn window_slot(slot: i32) -> Option<usize> {
    match slot {
        0..=8 => Some(36 + slot as usize),
        9..=35 => Some(slot as usize),
        _ => None,
    }
}

impl Game {
    fn stack_nbt(&self, s: &Stack) -> Tag {
        let mut c = Compound::new();
        c.insert(
            "id".into(),
            Tag::String(format!(
                "minecraft:{}",
                reg::ITEM.get(s.item as usize).copied().unwrap_or("air")
            )),
        );
        c.insert("count".into(), Tag::Int(s.count));
        let mut comp = Compound::new();
        if s.damage > 0 {
            comp.insert("minecraft:damage".into(), Tag::Int(s.damage));
        }
        let names = self.sh.registries.entries("minecraft:enchantment");
        let mut en = Compound::new();
        for (id, lvl) in s.ench.iter().filter(|e| e.1 > 0) {
            if let Some(n) = names.get(*id as usize) {
                en.insert(n.clone(), Tag::Int(*lvl as i32));
            }
        }
        if !en.is_empty() {
            comp.insert("minecraft:enchantments".into(), Tag::Compound(en));
        }
        if !comp.is_empty() {
            c.insert("components".into(), Tag::Compound(comp));
        }
        Tag::Compound(c)
    }

    fn nbt_stack(&self, t: &Tag) -> Option<Stack> {
        let id = t.get("id")?.as_str()?.trim_start_matches("minecraft:");
        let item = reg::ITEM.iter().position(|n| *n == id)? as i32;
        let count = t.get("count").and_then(|c| c.as_i64()).unwrap_or(1) as i32;
        let comp = t.get("components");
        let damage = comp
            .and_then(|c| c.get("minecraft:damage"))
            .and_then(|d| d.as_i64())
            .unwrap_or(0) as i32;
        let mut ench = NO_ENCH;
        if let Some(e) = comp
            .and_then(|c| c.get("minecraft:enchantments"))
            .and_then(|e| e.as_compound())
        {
            for (k, (n, l)) in e.iter().take(3).enumerate() {
                if let Some(id) = self.sh.registries.id("minecraft:enchantment", n) {
                    ench[k] = (id as u16, l.as_i64().unwrap_or(1).clamp(0, 255) as u8);
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

    /// Write `players/data/<uuid>.dat`, keeping the tags Basalt doesn't manage.
    pub(super) fn save_player_dat(&self, p: &Player) {
        let Some(d) = &self.dir else { return };
        let file = path(d, p.uuid);
        let mut root = crate::level::read_gz(&file)
            .and_then(|t| t.as_compound().cloned())
            .unwrap_or_default();
        let mut ins = |k: &str, v: Tag| {
            root.insert(k.into(), v);
        };
        ins("DataVersion", Tag::Int(basalt_data::WORLD_VERSION));
        ins("UUID", Tag::IntArray(uuid_ints(p.uuid)));
        ins(
            "Pos",
            Tag::List(p.pos.iter().map(|v| Tag::Double(*v)).collect()),
        );
        ins(
            "Rotation",
            Tag::List(vec![Tag::Float(p.yaw), Tag::Float(p.pitch)]),
        );
        ins(
            "Dimension",
            Tag::String(dim::DIMS[self.dim_of(p.eid).unwrap_or(self.cur)].id.into()),
        );
        ins("playerGameType", Tag::Int(p.gamemode as i32));
        ins("SelectedItemSlot", Tag::Int(p.held as i32));
        ins("Health", Tag::Float(p.health));
        ins("foodLevel", Tag::Int(p.food));
        ins("foodSaturationLevel", Tag::Float(p.saturation));
        ins(
            "Air",
            Tag::Short(p.air.clamp(i16::MIN as i32, i16::MAX as i32) as i16),
        );
        ins(
            "Fire",
            Tag::Short(if p.fire_ticks > 0 {
                p.fire_ticks.min(i16::MAX as i32) as i16
            } else {
                -20
            }),
        );
        ins("XpLevel", Tag::Int(p.xp_level));
        ins("XpP", Tag::Float(p.xp_progress));
        ins("XpTotal", Tag::Int(p.xp));
        let mut inv = Vec::new();
        for slot in 0..36 {
            let w = window_slot(slot).unwrap();
            let s = &p.inv[w];
            if s.count <= 0 {
                continue;
            }
            if let Tag::Compound(mut c) = self.stack_nbt(s) {
                c.insert("Slot".into(), Tag::Byte(slot as i8));
                inv.push(Tag::Compound(c));
            }
        }
        ins("Inventory", Tag::List(inv));
        let mut eq = Compound::new();
        for (k, w) in EQUIP {
            if p.inv[w].count > 0 {
                eq.insert(k.into(), self.stack_nbt(&p.inv[w]));
            }
        }
        ins("equipment", Tag::Compound(eq));
        let mut ab = root
            .get("abilities")
            .and_then(|a| a.as_compound())
            .cloned()
            .unwrap_or_default();
        let creative = p.gamemode == 1;
        ab.insert("flying".into(), Tag::Byte(p.flying as i8));
        ab.insert(
            "mayfly".into(),
            Tag::Byte((creative || p.gamemode == 3) as i8),
        );
        ab.insert("instabuild".into(), Tag::Byte(creative as i8));
        ab.insert(
            "invulnerable".into(),
            Tag::Byte((creative || p.gamemode == 3) as i8),
        );
        ab.entry("mayBuild".into()).or_insert(Tag::Byte(1));
        ab.entry("walkSpeed".into()).or_insert(Tag::Float(0.1));
        ab.entry("flySpeed".into()).or_insert(Tag::Float(0.05));
        root.insert("abilities".into(), Tag::Compound(ab));
        if let Err(e) = crate::level::write_gz(&file, &Tag::Compound(root)) {
            log::warn!("writing {}: {e}", file.display());
        }
    }

    /// Dimension saved in a vanilla player file.
    pub(super) fn dat_dim(&self, uuid: Uuid) -> Option<usize> {
        let t = crate::level::read_gz(&path(self.dir.as_ref()?, uuid))?;
        dim::dim_index(t.get("Dimension")?.as_str()?)
    }

    /// Load a player from vanilla's .dat (used when Basalt has no json for them). True if found.
    pub(super) fn load_player_dat(&self, p: &mut Player) -> bool {
        let Some(d) = &self.dir else { return false };
        let Some(t) = crate::level::read_gz(&path(d, p.uuid)) else {
            return false;
        };
        let f = |k: &str| t.get(k).and_then(|v| v.as_f64());
        if let Some(Tag::List(l)) = t.get("Pos") {
            if l.len() == 3 {
                p.pos = [
                    l[0].as_f64().unwrap_or(0.0),
                    l[1].as_f64().unwrap_or(0.0),
                    l[2].as_f64().unwrap_or(0.0),
                ];
            }
        }
        if let Some(Tag::List(l)) = t.get("Rotation") {
            if l.len() == 2 {
                p.yaw = l[0].as_f64().unwrap_or(0.0) as f32;
                p.pitch = l[1].as_f64().unwrap_or(0.0) as f32;
            }
        }
        if let Some(g) = f("playerGameType") {
            p.gamemode = g.clamp(0.0, 3.0) as u8;
        }
        if let Some(h) = f("SelectedItemSlot") {
            p.held = (h as usize).min(8);
        }
        if let Some(h) = f("Health") {
            p.health = h as f32;
        }
        if let Some(v) = f("foodLevel") {
            p.food = v as i32;
        }
        if let Some(v) = f("foodSaturationLevel") {
            p.saturation = v as f32;
        }
        if let Some(v) = f("Air") {
            p.air = v as i32;
        }
        if let Some(v) = f("Fire") {
            p.fire_ticks = (v as i32).max(0);
        }
        if let Some(v) = f("XpLevel") {
            p.xp_level = v as i32;
        }
        if let Some(v) = f("XpP") {
            p.xp_progress = v as f32;
        }
        if let Some(v) = f("XpTotal") {
            p.xp = v as i32;
        }
        p.flying = t
            .get("abilities")
            .and_then(|a| a.get("flying"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            != 0;
        p.dead = p.health <= 0.0;
        for it in t
            .get("Inventory")
            .and_then(|l| l.as_list())
            .into_iter()
            .flatten()
        {
            let Some(w) = it
                .get("Slot")
                .and_then(|s| s.as_i64())
                .and_then(|s| window_slot(s as i32))
            else {
                continue;
            };
            if let Some(s) = self.nbt_stack(it) {
                p.inv[w] = s;
            }
        }
        for (k, w) in EQUIP {
            if let Some(s) = t
                .get("equipment")
                .and_then(|e| e.get(k))
                .and_then(|it| self.nbt_stack(it))
            {
                p.inv[w] = s;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn slots() {
        assert_eq!(super::window_slot(0), Some(36));
        assert_eq!(super::window_slot(8), Some(44));
        assert_eq!(super::window_slot(9), Some(9));
        assert_eq!(super::window_slot(36), None);
        let u = uuid::Uuid::parse_str("6b05e283-c815-4af8-b00a-ab75a2156e8b").unwrap();
        assert_eq!(super::uuid_ints(u)[0], 0x6b05e283u32 as i32);
    }
}

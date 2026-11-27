//! Item-stack data beyond item/count/damage: enchantments (up to `MAX_ENCH`), plus the anvil / brewing / smithing
//! components (custom name, repair cost, enchanted-book storage flag, potion, armor trim).
//!
//! `Ench` is `Copy` and derefs to its enchantment slice, so existing code that copies `stack.ench` around keeps
//! every extra component. Custom names live in a process-wide interner (an id keeps `Ench` `Copy`).
use std::ops::{Deref, DerefMut};
use std::sync::Mutex;

pub const MAX_ENCH: usize = 8;

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Ench {
    /// (enchantment registry id, level); level 0 = unused slot.
    pub e: [(u16, u8); MAX_ENCH],
    /// `minecraft:repair_cost` (anvil prior-work penalty).
    pub repair: i32,
    /// Interned `minecraft:custom_name` (0 = none).
    pub name: u32,
    /// The list is `stored_enchantments` (enchanted books) instead of `enchantments`.
    pub stored: bool,
    /// `minecraft:potion_contents` potion: POTION registry id + 1 (0 = none).
    pub potion: u8,
    /// `minecraft:trim`: TRIM_MATERIAL registry id + 1, TRIM_PATTERN registry id + 1 (0 = none).
    pub trim_mat: u8,
    pub trim_pat: u8,
    /// `minecraft:charged_projectiles` (loaded crossbow): ITEM registry id + 1 of the projectile, and
    /// how many copies (multishot loads 3).
    pub charged: u16,
    pub charged_n: u8,
    /// every loaded copy is intangible_projectile (loaded in creative: no arrow was used)
    pub charged_free: bool,
}

pub const NO_ENCH: Ench = Ench {
    e: [(0, 0); MAX_ENCH],
    repair: 0,
    name: 0,
    stored: false,
    potion: 0,
    trim_mat: 0,
    trim_pat: 0,
    charged: 0,
    charged_n: 0,
    charged_free: false,
};

impl Deref for Ench {
    type Target = [(u16, u8)];
    fn deref(&self) -> &[(u16, u8)] {
        &self.e
    }
}
impl DerefMut for Ench {
    fn deref_mut(&mut self) -> &mut [(u16, u8)] {
        &mut self.e
    }
}

impl Ench {
    /// Used enchantments as (id, level).
    pub fn list(&self) -> Vec<(u16, u8)> {
        self.e.iter().copied().filter(|e| e.1 > 0).collect()
    }
    #[allow(dead_code)]
    pub fn level_of(&self, id: u16) -> u8 {
        self.e
            .iter()
            .find(|e| e.1 > 0 && e.0 == id)
            .map(|e| e.1)
            .unwrap_or(0)
    }
    /// Set (or add) an enchantment; false when all slots are used.
    pub fn set(&mut self, id: u16, level: u8) -> bool {
        if let Some(i) = self.e.iter().position(|e| e.1 > 0 && e.0 == id) {
            self.e[i].1 = level;
            return true;
        }
        match self.e.iter().position(|e| e.1 == 0) {
            Some(i) => {
                self.e[i] = (id, level);
                true
            }
            None => false,
        }
    }
    /// Replace the enchantment list (keeps the other components).
    pub fn set_list(&mut self, list: &[(u16, u8)]) {
        self.e = [(0, 0); MAX_ENCH];
        for (i, l) in list.iter().filter(|l| l.1 > 0).take(MAX_ENCH).enumerate() {
            self.e[i] = *l;
        }
    }
    pub fn name_str(&self) -> Option<String> {
        name_of(self.name)
    }
    pub fn has_extras(&self) -> bool {
        self.repair != 0
            || self.name != 0
            || self.potion != 0
            || self.trim_mat != 0
            || self.charged != 0
    }
}

static NAMES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Id (>= 1) of a custom-name string.
pub fn intern(s: &str) -> u32 {
    let mut v = NAMES.lock().unwrap();
    if let Some(i) = v.iter().position(|x| x == s) {
        return i as u32 + 1;
    }
    v.push(s.to_string());
    v.len() as u32
}

pub fn name_of(id: u32) -> Option<String> {
    if id == 0 {
        return None;
    }
    NAMES.lock().unwrap().get(id as usize - 1).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_and_enchants() {
        let a = intern("Excalibur");
        assert_eq!(intern("Excalibur"), a);
        assert_eq!(name_of(a).as_deref(), Some("Excalibur"));
        let mut e = NO_ENCH;
        assert!(e.set(3, 2));
        e.set(3, 4);
        e.set(7, 1);
        assert_eq!(e.list(), vec![(3, 4), (7, 1)]);
        assert_eq!(e.level_of(7), 1);
    }
}

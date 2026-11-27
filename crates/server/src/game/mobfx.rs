//! Status effects on mobs (tipped/spectral arrows, splash and lingering potions). Measured on vanilla 26.2
//! (2026-11-27, scenario bow_enchants): a spectral arrow sets the glowing shared flag (0x40) on the target, a
//! tipped poison arrow poisons a cow for 1/8 of the potion's duration (damage every 25 >> amplifier ticks, never
//! below 1 health), instant effects act at once. Undead mobs ignore poison and regeneration and swap
//! harming and healing (vanilla behaviour from the game's documented rules; not yet diffed for undead).
use super::env::effect_id;
use super::mob::Mob;
use super::Game;

impl Game {
    /// Give a mob an effect (keeps the stronger / longer one like players' effects).
    pub(super) fn add_mob_effect(&mut self, eid: i32, name: &str, amp: u8, ticks: i32) {
        let Some(id) = effect_id(name) else { return };
        let Some(m) = self.mobs.get_mut(&eid) else {
            return;
        };
        if !m.alive() {
            return;
        }
        if m.kind.undead() && matches!(name, "poison" | "regeneration") {
            return;
        }
        match m.effects.iter_mut().find(|e| e.0 == id) {
            Some(e) => {
                if amp > e.1 || (amp == e.1 && ticks > e.2) {
                    e.1 = amp;
                    e.2 = ticks;
                }
            }
            None => m.effects.push((id, amp, ticks)),
        }
    }

    /// Instant health / damage on a mob (undead swap them): heal 4 << amp, harm 6 << amp, scaled.
    pub(super) fn instant_on_mob(
        &mut self,
        eid: i32,
        harm: bool,
        amp: u8,
        strength: f64,
        cause: Option<i32>,
        direct: Option<i32>,
    ) {
        let Some(m) = self.mobs.get(&eid) else {
            return;
        };
        let harm = harm != m.kind.undead();
        if harm {
            let dmg = (strength * (6u32 << amp) as f64 + 0.5) as i32 as f32;
            self.damage_mob(eid, dmg, "indirect_magic", cause, direct, None);
        } else {
            let heal = (strength * (4u32 << amp) as f64 + 0.5) as i32 as f32;
            self.heal_mob(eid, heal);
        }
    }

    fn heal_mob(&mut self, eid: i32, amount: f32) {
        let Some(m) = self.mobs.get_mut(&eid) else {
            return;
        };
        let max = m.max_health();
        if !m.alive() || m.health >= max {
            return;
        }
        m.health = (m.health + amount).min(max);
        let h = m.health;
        self.send_mob_data(eid, |w| {
            w.u8(9).varint(3).f32(h);
        });
    }

    /// One tick of a mob's effects (called from tick_mob with the mob taken out of the map).
    /// Returns true when the mob died.
    pub(super) fn tick_mob_effects(&mut self, m: &mut Mob) -> bool {
        if m.effects.is_empty() || !m.alive() {
            return false;
        }
        let (poison, wither, regen) = (
            effect_id("poison").unwrap_or(-1),
            effect_id("wither").unwrap_or(-1),
            effect_id("regeneration").unwrap_or(-1),
        );
        let mut hurt: Vec<(f32, &'static str)> = Vec::new();
        let mut heal = 0.0f32;
        for e in m.effects.iter_mut() {
            let (id, amp, left) = (e.0, e.1, e.2);
            let every = |base: i32| {
                let iv = base >> amp.min(5);
                iv <= 0 || left % iv == 0
            };
            if id == poison && every(25) && m.health > 1.0 {
                hurt.push((1.0, "magic"));
            } else if id == wither && every(40) {
                hurt.push((1.0, "wither"));
            } else if id == regen && every(50) {
                heal += 1.0;
            }
            e.2 -= 1;
        }
        m.effects.retain(|e| e.2 > 0);
        if heal > 0.0 {
            let max = m.max_health();
            if m.health < max {
                m.health = (m.health + heal).min(max);
                let (eid, h) = (m.eid, m.health);
                self.send_mob_data(eid, |w| {
                    w.u8(9).varint(3).f32(h);
                });
            }
        }
        for (amount, kind) in hurt {
            self.hurt_held_mob(m, amount, kind);
            if !m.alive() {
                return true;
            }
        }
        false
    }
}

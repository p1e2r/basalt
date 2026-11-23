//! Breeding: feeding an adult animal its tempt item puts it in love (600 ticks, entity event 18 =
//! hearts); two in-love animals of a kind within 8 blocks walk together and after 60 ticks within
//! 3 blocks make a baby (parents then wait 6000 ticks); the breeder gets 1-7 XP. Babies grow up
//! after 24000 ticks, faster when fed. Checked against vanilla 26.2 with scenario m5_breeding
//! (two cows 1.5 apart: a baby ~3.3 s after feeding).
use super::mob::{Mob, MobKind};
use super::{Game, Stack};
use basalt_data::registries as reg;
use basalt_protocol::{ids::play::clientbound as cb, Writer};
use rand::Rng;

const LOVE: u32 = 600;
const COOLDOWN: u32 = 6000;
const GROW: i32 = 24000;

impl Game {
    /// Right-click with an item on an animal: feed it if the item breeds it. Returns true if used.
    pub(super) fn try_feed(&mut self, eid: i32, target: i32, slot: usize) -> bool {
        let Some(p) = self.players.get(&eid) else {
            return false;
        };
        let held = p.inv[slot];
        let creative = p.gamemode == 1;
        let item = reg::ITEM.get(held.item as usize).copied().unwrap_or("air");
        let Some(m) = self.mobs.get_mut(&target) else {
            return false;
        };
        if !m.kind.tempt_items().contains(&item)
            || m.kind == MobKind::Zombie
            || item == "carrot_on_a_stick"
        {
            return false;
        }
        if m.baby {
            m.grow += (GROW - m.grow).max(0) / 10;
        } else if m.love == 0 && m.breed_cooldown == 0 {
            m.love = LOVE;
            m.lover = Some(eid);
            let mut w = Writer::with_id(cb::ENTITY_EVENT);
            w.i32(target).u8(18);
            self.send_to_viewers(target, &w);
        } else {
            return false;
        }
        if !creative {
            let p = self.players.get_mut(&eid).unwrap();
            p.inv[slot].count -= 1;
            if p.inv[slot].count <= 0 {
                p.inv[slot] = Stack::default();
            }
            Self::send_window(p);
        }
        true
    }

    /// Per tick for an animal: timers, growing up, and the breed goal. Returns the partner to walk
    /// to while in love (the caller makes it the movement goal).
    pub(super) fn breed_tick(&mut self, m: &mut Mob) -> Option<[f64; 3]> {
        if m.breed_cooldown > 0 {
            m.breed_cooldown -= 1;
        }
        if m.baby && m.kind != MobKind::Zombie {
            m.grow += 1;
            if m.grow >= GROW {
                m.baby = false;
                let eid = m.eid;
                self.send_mob_data(eid, |w| {
                    w.u8(16).varint(8).bool(false);
                });
            }
        }
        if m.love == 0 || m.baby {
            return None;
        }
        m.love -= 1;
        let (kind, pos) = (m.kind, m.pos);
        let partner = self
            .mobs
            .values()
            .filter(|o| o.kind == kind && o.love > 0 && !o.baby && o.alive())
            .map(|o| {
                (
                    o.eid,
                    o.pos,
                    (o.pos[0] - pos[0]).powi(2)
                        + (o.pos[1] - pos[1]).powi(2)
                        + (o.pos[2] - pos[2]).powi(2),
                )
            })
            .filter(|o| o.2 < 64.0)
            .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap());
        let Some((pid, ppos, d2)) = partner else {
            m.love_with = 0;
            return None;
        };
        if d2 < 9.0 {
            m.love_with += 1;
        } else {
            m.love_with = 0;
        }
        if m.love_with >= 60 {
            m.love = 0;
            m.love_with = 0;
            m.breed_cooldown = COOLDOWN;
            if let Some(o) = self.mobs.get_mut(&pid) {
                o.love = 0;
                o.love_with = 0;
                o.breed_cooldown = COOLDOWN;
            }
            let color = m.sheep_color;
            let at = [(pos[0] + ppos[0]) / 2.0, pos[1], (pos[2] + ppos[2]) / 2.0];
            self.spawn_mob(kind, at, m.yaw, |b| {
                b.baby = true;
                b.grow = 0;
                b.sheep_color = color;
            });
            let xp = self.rng.gen_range(1..=7);
            self.spawn_xp(pos, xp);
            return None;
        }
        Some(ppos)
    }
}

//! Player bows and tridents: hold use_item to draw, release (player_action 5) to shoot. Measured on vanilla
//! 26.2 (2026-11-24): a full draw (20 ticks) fires an arrow at speed 3.0 (critical flag set), spawned at eye height - 0.1
//! with gaussian inaccuracy 1.0, sound entity.arrow.shoot (category 7, volume 1.0, pitch
//! 1 / (1.2 + 0.4 r) + power / 2 at the shooter's feet); a thrown trident leaves at speed 2.5 with
//! sound_entity item.trident.throw (category 7, volume 1, pitch 1). Draw power (pull = ticks / 20, capped at 1):
//! power = (pull^2 + 2 pull) / 3, nothing below 0.1; tridents need a 10-tick draw.
use super::{Game, Stack};
use basalt_data::{registries as reg, registry_id};
use basalt_protocol::{ids::play::clientbound as cb, Writer};
use rand::Rng;

impl Game {
    /// use_item with a bow or trident in hand: start drawing. Returns true when the item is one of those.
    pub(super) fn try_draw(&mut self, eid: i32, hand: i32) -> bool {
        let Some(p) = self.players.get(&eid) else {
            return false;
        };
        if p.dead || p.gamemode == 3 {
            return false;
        }
        let slot = if hand == 0 { 36 + p.held } else { 45 };
        let s = p.inv[slot];
        if s.count <= 0 {
            return false;
        }
        let name = reg::ITEM.get(s.item as usize).copied().unwrap_or("air");
        let creative = p.gamemode == 1;
        match name {
            "bow" => {
                if !creative && self.find_arrow(eid).is_none() {
                    return true;
                }
            }
            "trident" => {}
            _ => return false,
        }
        let tick = self.tick;
        if let Some(p) = self.players.get_mut(&eid) {
            p.draw = Some((slot, tick));
        }
        true
    }

    /// Inventory slot of the first arrow stack (hotbar, then main inventory; the offhand first).
    fn find_arrow(&self, eid: i32) -> Option<usize> {
        let p = self.players.get(&eid)?;
        let is_arrow = |i: usize| {
            p.inv[i].count > 0
                && matches!(
                    reg::ITEM.get(p.inv[i].item as usize).copied(),
                    Some("arrow" | "spectral_arrow" | "tipped_arrow")
                )
        };
        std::iter::once(45usize)
            .chain(36..45)
            .chain(9..36)
            .find(|&i| is_arrow(i))
    }

    /// release_use_item: shoot if a draw was in progress.
    pub(super) fn release_draw(&mut self, eid: i32) {
        let Some(p) = self.players.get_mut(&eid) else {
            return;
        };
        let Some((slot, start)) = p.draw.take() else {
            return;
        };
        if p.dead {
            return;
        }
        let s = p.inv[slot];
        let name = reg::ITEM.get(s.item as usize).copied().unwrap_or("air");
        let ticks = self.tick.saturating_sub(start) as f64;
        let creative = p.gamemode == 1;
        let (yaw, pitch, sneaking, feet) = (p.yaw as f64, p.pitch as f64, p.sneaking, p.pos);
        let eye = feet[1] + if sneaking { 1.27 } else { 1.62 };
        let from = [feet[0], eye - 0.1, feet[2]];
        let (yr, xr) = (yaw.to_radians(), pitch.to_radians());
        let dir = [-yr.sin() * xr.cos(), -xr.sin(), yr.cos() * xr.cos()];
        let max_damage = self.data.item(s.item).map(|i| i.max_damage).unwrap_or(0);
        match name {
            "bow" => {
                let pull = (ticks / 20.0).min(1.0);
                let power = ((pull * pull + pull * 2.0) / 3.0).min(1.0);
                if power < 0.1 {
                    return;
                }
                let arrow_slot = self.find_arrow(eid);
                if !creative && arrow_slot.is_none() {
                    return;
                }
                let mut item = None;
                if !creative {
                    let p = self.players.get_mut(&eid).unwrap();
                    let a = arrow_slot.unwrap();
                    item = Some(Stack {
                        count: 1,
                        ..p.inv[a]
                    });
                    p.inv[a].count -= 1;
                    if p.inv[a].count <= 0 {
                        p.inv[a] = Stack::default();
                    }
                    p.inv[slot].damage += 1;
                    if max_damage > 0 && p.inv[slot].damage >= max_damage {
                        p.inv[slot] = Stack::default();
                    }
                    Self::send_window(p);
                }
                self.shoot_projectile(
                    eid,
                    from,
                    dir,
                    power * 3.0,
                    1.0,
                    2.0,
                    "arrow",
                    power >= 1.0,
                    item,
                );
                let pitch = 1.0 / (self.rng.gen::<f32>() * 0.4 + 1.2) + power as f32 * 0.5;
                self.play_sound("entity.arrow.shoot", 7, feet, 1.0, pitch);
            }
            "trident" => {
                if ticks < 10.0 {
                    return;
                }
                let mut item = None;
                let mut thrown = s;
                thrown.count = 1;
                if !creative {
                    thrown.damage += 1;
                    let p = self.players.get_mut(&eid).unwrap();
                    p.inv[slot] = Stack::default();
                    Self::send_window(p);
                    item = Some(thrown);
                }
                let id =
                    self.shoot_projectile(eid, from, dir, 2.5, 1.0, 8.0, "trident", false, item);
                let sid = Self::sound_id("item.trident.throw");
                let seed: i64 = self.rng.gen();
                let mut w = Writer::with_id(cb::SOUND_ENTITY);
                w.varint(sid)
                    .varint(7)
                    .varint(id)
                    .f32(1.0)
                    .f32(1.0)
                    .i64(seed);
                self.broadcast(None, || Writer { buf: w.buf.clone() });
                let _ = registry_id(reg::ITEM, "trident");
            }
            _ => {}
        }
    }
}

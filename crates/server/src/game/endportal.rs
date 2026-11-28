//! End portals (M8): eyes of ender on end portal frames, the 3x3 portal they open, travel to the
//! End's arrival platform and the way home (win-game event, then the client's respawn command).
//!
//! Measured on vanilla 26.2 with scenario m8_end_portal (NOTES.md "The End"):
//! * an eye used on a frame without one sets `eye=true` (survival: the eye is used up); when the
//!   12 frames around a 3x3 hole all hold eyes and face into the hole, the hole fills with
//!   end_portal. A complete ring facing outward stays empty. /setblock alone never opens one.
//! * touching an end portal outside the End: to the End at 100.5 49 0.5, yaw 90, pitch kept, on a
//!   rebuilt 5x5 obsidian platform (y 48) with a 5x5x3 air pocket above.
//! * touching one in the End: game_event 4 (win game; 1 = roll the credits the first time) and the
//!   player stays put until the client sends "perform respawn"; then back to the overworld spawn
//!   keeping inventory and experience (a dimension change, not a death).
use super::dim::{END, OVERWORLD};
use super::*;

/// Arrival point in the End (vanilla's fixed platform position).
const END_SPAWN: [i32; 3] = [100, 49, 0];

fn st(s: &str) -> u32 {
    BlockState::parse(s).map(|b| b.0).unwrap_or(0)
}

/// Unit step of a horizontal `facing` value.
fn facing_step(f: &str) -> Option<[i32; 2]> {
    Some(match f {
        "north" => [0, -1],
        "south" => [0, 1],
        "west" => [-1, 0],
        "east" => [1, 0],
        _ => return None,
    })
}

impl Game {
    /// Ender eye used on a block. True when the click was handled (frame filled).
    pub(super) fn use_ender_eye(&mut self, eid: i32, b: [i32; 3]) -> bool {
        let s = BlockState(self.world.get_block(b[0], b[1], b[2]));
        if s.name() != "end_portal_frame" || s.get("eye") == Some("true") {
            return false;
        }
        let facing = s.get("facing").unwrap_or("north").to_string();
        self.set_block(
            b[0],
            b[1],
            b[2],
            st(&format!("end_portal_frame[eye=true,facing={facing}]")),
        );
        self.consume_held(eid);
        // world event 1503: eye placed (smoke + sound)
        self.broadcast_chunk((b[0] >> 4, b[2] >> 4), None, || {
            let mut w = Writer::with_id(cb::LEVEL_EVENT);
            w.i32(1503).position(b[0], b[1], b[2]).i32(0).bool(false);
            w
        });
        // the hole this frame belongs to lies 2 blocks in front of it, 1 to either side at most
        let Some(d) = facing_step(&facing) else {
            return true;
        };
        for k in -1..=1 {
            let c = [
                b[0] + 2 * d[0] + k * d[1].abs(),
                b[1],
                b[2] + 2 * d[1] + k * d[0].abs(),
            ];
            if self.end_ring_complete(c) {
                for dx in -1..=1 {
                    for dz in -1..=1 {
                        self.set_block(c[0] + dx, c[1], c[2] + dz, st("end_portal"));
                    }
                }
                // world event 1038: end portal opened (heard everywhere)
                let make = || {
                    let mut w = Writer::with_id(cb::LEVEL_EVENT);
                    w.i32(1038).position(c[0], c[1], c[2]).i32(0).bool(true);
                    w
                };
                self.broadcast_all(None, make);
                break;
            }
        }
        true
    }

    /// The 12 frames around the 3x3 hole centred on `c` all hold eyes and face the hole.
    fn end_ring_complete(&self, c: [i32; 3]) -> bool {
        for i in -1..=1 {
            for (p, want) in [
                ([c[0] + i, c[2] - 2], "south"),
                ([c[0] + i, c[2] + 2], "north"),
                ([c[0] - 2, c[2] + i], "east"),
                ([c[0] + 2, c[2] + i], "west"),
            ] {
                let s = BlockState(self.world.get_block(p[0], c[1], p[1]));
                if s.name() != "end_portal_frame"
                    || s.get("eye") != Some("true")
                    || s.get("facing") != Some(want)
                {
                    return false;
                }
            }
        }
        true
    }

    /// The player's box touches an end portal block (the slab between 6/16 and 12/16 of the block).
    fn end_portal_touched(&self, p: &Player) -> bool {
        let lo = [p.pos[0] - 0.3, p.pos[1], p.pos[2] - 0.3];
        let hi = [p.pos[0] + 0.3, p.pos[1] + 1.8, p.pos[2] + 0.3];
        for x in (lo[0] + 1e-7).floor() as i32..=(hi[0] - 1e-7).floor() as i32 {
            for y in (lo[1] + 1e-7).floor() as i32..=(hi[1] - 1e-7).floor() as i32 {
                for z in (lo[2] + 1e-7).floor() as i32..=(hi[2] - 1e-7).floor() as i32 {
                    if BlockState(self.world.get_block(x, y, z)).name() != "end_portal" {
                        continue;
                    }
                    let (a, b) = (y as f64 + 0.375, y as f64 + 0.75);
                    if lo[1] < b && hi[1] > a {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Per tick: players of the current dimension standing in an end portal travel at once.
    pub(super) fn tick_end_portals(&mut self) {
        // (the nether-portal tick counts the cooldown down in the overworld and the nether)
        if self.cur == END {
            for p in self.players.values_mut() {
                p.portal_cooldown = (p.portal_cooldown - 1).max(0);
            }
        }
        let ids: Vec<i32> = self
            .players
            .iter()
            .filter(|(_, p)| !p.dead && p.gamemode != 3 && !p.won_game && p.portal_cooldown <= 0)
            .map(|(e, _)| *e)
            .collect();
        for eid in ids {
            if !self.end_portal_touched(&self.players[&eid]) {
                continue;
            }
            if self.cur == END {
                self.win_game(eid);
            } else {
                self.travel_to_end(eid);
            }
        }
    }

    fn travel_to_end(&mut self, eid: i32) {
        let from = self.cur;
        let Some(p) = self.players.get(&eid) else {
            return;
        };
        let pitch = p.pitch;
        self.enter(END);
        let [x, y, z] = END_SPAWN;
        self.ensure_chunk(x >> 4, z >> 4);
        let obsidian = st("obsidian");
        for dx in -2..=2 {
            for dz in -2..=2 {
                for dy in -1..=2 {
                    let want = if dy == -1 { obsidian } else { 0 };
                    if self.world.get_block(x + dx, y + dy, z + dz) != want {
                        self.set_block(x + dx, y + dy, z + dz, want);
                    }
                }
            }
        }
        self.enter(from);
        self.change_dimension(
            eid,
            END,
            [x as f64 + 0.5, y as f64, z as f64 + 0.5],
            90.0,
            pitch,
        );
        if let Some(p) = self.players.get_mut(&eid) {
            p.portal_cooldown = 20;
        }
    }

    /// Leaving the End through its portal: the win-game event; the trip home waits for the
    /// client's respawn command ([`Game::finish_win`]).
    fn win_game(&mut self, eid: i32) {
        let Some(p) = self.players.get_mut(&eid) else {
            return;
        };
        p.won_game = true;
        let credits = !p.seen_credits;
        p.seen_credits = true;
        let mut w = Writer::with_id(cb::GAME_EVENT);
        w.u8(4).f32(if credits { 1.0 } else { 0.0 });
        p.send(w);
    }

    /// "Perform respawn" after the credits: back to the overworld spawn, keeping everything.
    /// False when the player hadn't left the End.
    pub(super) fn finish_win(&mut self, eid: i32) -> bool {
        if !self.enter_player(eid) {
            return false;
        }
        let Some(p) = self.players.get_mut(&eid) else {
            return false;
        };
        if !p.won_game {
            return false;
        }
        p.won_game = false;
        let point = p.spawn_point.filter(|(_, d)| *d == OVERWORLD);
        let spawn = point
            .map(|(b, _)| [b[0] as f64 + 0.5, b[1] as f64, b[2] as f64 + 0.5])
            .unwrap_or(self.spawn);
        self.change_dimension(eid, OVERWORLD, spawn, 0.0, 0.0);
        if let Some(p) = self.players.get_mut(&eid) {
            p.portal_cooldown = 20;
        }
        true
    }
}

//! End crystals (M8): one on every spike top when an End chunk generates, placed with the
//! end_crystal item on obsidian / bedrock, exploding with power 6 when hit (measured on vanilla
//! 26.2 with scenario m8_end_crystals; NOTES.md "The End").
//! * spike crystals stand at (x + 0.5, height + 1, z + 0.5) in the fire on the bedrock top and
//!   keep the default show_bottom (no entity data sent); placed ones sit at the clicked block's top
//!   centre with show_bottom = false (entity data index 9);
//! * a hit removes the crystal and explodes at its position (feet): on obsidian the blast is
//!   shadowed by the block and leaves the floor around it intact, like vanilla;
//! * explosions remove crystals without a second blast (not measured).
use super::mob::write_lpvec3;
use super::*;
use uuid::Uuid;

pub struct Crystal {
    pub eid: i32,
    pub uuid: Uuid,
    pub pos: [f64; 3],
    pub show_bottom: bool,
}

impl Crystal {
    pub fn spawn_packets(&self) -> Vec<Writer> {
        let mut a = Writer::with_id(cb::ADD_ENTITY);
        a.varint(self.eid)
            .uuid(self.uuid)
            .varint(registry_id(reg::ENTITY_TYPE, "end_crystal").unwrap_or(0))
            .f64(self.pos[0])
            .f64(self.pos[1])
            .f64(self.pos[2]);
        write_lpvec3(&mut a, [0.0; 3]);
        a.u8(0).u8(0).u8(0).varint(0);
        let mut v = vec![a];
        if !self.show_bottom {
            let mut w = Writer::with_id(cb::SET_ENTITY_DATA);
            w.varint(self.eid).u8(9).varint(8).bool(false).u8(0xff);
            v.push(w);
        }
        v
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({ "kind": "end_crystal", "uuid": self.uuid.to_string(), "pos": self.pos, "show_bottom": self.show_bottom })
    }

    pub fn from_json(eid: i32, v: &serde_json::Value) -> Option<Crystal> {
        if v["kind"].as_str()? != "end_crystal" {
            return None;
        }
        let a = v["pos"].as_array()?;
        Some(Crystal {
            eid,
            uuid: v["uuid"]
                .as_str()
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(Uuid::new_v4),
            pos: [
                a.first()?.as_f64()?,
                a.get(1)?.as_f64()?,
                a.get(2)?.as_f64()?,
            ],
            show_bottom: v["show_bottom"].as_bool().unwrap_or(true),
        })
    }
}

impl Game {
    pub(super) fn spawn_crystal(&mut self, pos: [f64; 3], show_bottom: bool) -> i32 {
        let eid = self
            .sh
            .next_eid
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.crystals.insert(
            eid,
            Crystal {
                eid,
                uuid: Uuid::new_v4(),
                pos,
                show_bottom,
            },
        );
        self.sync_viewers_for(eid);
        eid
    }

    /// A freshly generated End chunk: crystals on the spikes whose centre is in it.
    pub(super) fn crystals_for_new_chunk(&mut self, cx: i32, cz: i32) {
        if self.cur != super::dim::END {
            return;
        }
        for s in basalt_worldgen::features::end::spikes(self.sh.cfg.seed) {
            if s.x >> 4 == cx && s.z >> 4 == cz {
                let pos = [s.x as f64 + 0.5, (s.height + 1) as f64, s.z as f64 + 0.5];
                self.spawn_crystal(pos, true);
            }
        }
    }

    /// end_crystal used on a block: obsidian or bedrock with air above and no entity there.
    /// True when a crystal was placed.
    pub(super) fn use_end_crystal(&mut self, eid: i32, b: [i32; 3]) -> bool {
        let n = BlockState(self.world.get_block(b[0], b[1], b[2]));
        if !matches!(n.name(), "obsidian" | "bedrock") {
            return false;
        }
        let up = [b[0], b[1] + 1, b[2]];
        if !BlockState(self.world.get_block(up[0], up[1], up[2])).is_air() {
            return false;
        }
        let pos = [up[0] as f64 + 0.5, up[1] as f64, up[2] as f64 + 0.5];
        let blocked = self
            .crystals
            .values()
            .map(|c| c.pos)
            .chain(self.mobs.values().map(|m| m.pos))
            .chain(self.players.values().filter(|p| !p.dead).map(|p| p.pos))
            .any(|q| {
                (q[0] - pos[0]).abs() < 1.0
                    && (q[2] - pos[2]).abs() < 1.0
                    && q[1] > pos[1] - 1.8
                    && q[1] < pos[1] + 2.0
            });
        if blocked {
            return false;
        }
        self.spawn_crystal(pos, false);
        self.consume_held(eid);
        true
    }

    /// A player hit a crystal: it breaks and explodes (power 6, blocks destroyed).
    pub(super) fn hit_crystal(&mut self, eid: i32, target: i32) -> bool {
        let Some(c) = self.crystals.remove(&target) else {
            return false;
        };
        self.untrack(target);
        let name = self
            .players
            .get(&eid)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        self.explode(c.pos, 6.0, Some((eid, name)), true);
        true
    }

    /// Crystals caught in an explosion at `c` vanish (no chain blast).
    pub(super) fn crystals_in_blast(&mut self, c: [f64; 3], radius: f64) {
        let gone: Vec<i32> = self
            .crystals
            .values()
            .filter(|k| {
                let d2: f64 = (0..3).map(|i| (k.pos[i] - c[i]).powi(2)).sum();
                d2 > 0.0 && d2 <= (2.0 * radius).powi(2)
            })
            .map(|k| k.eid)
            .collect();
        for e in gone {
            self.crystals.remove(&e);
            self.untrack(e);
        }
    }
}

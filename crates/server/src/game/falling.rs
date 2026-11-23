//! Gravity blocks (sand, gravel, concrete powder, anvils, suspicious sand/gravel): a block change
//! schedules a check 2 ticks later; a gravity block with nothing solid below turns into a
//! falling_block entity (gravity 0.04, drag 0.98) that becomes a block again where it lands, or an
//! item when the cell it lands in is taken (e.g. a torch). Concrete powder hardens on touching
//! water. Timing matched to vanilla 26.2 with scenario m6_falling_blocks (10-block drop: 24 ticks
//! from the setblock).
use super::mob::write_lpvec3;
use super::{Game, Stack, NO_ENCH};
use basalt_data::{registries as reg, registry_id, BlockState};
use basalt_protocol::{ids::play::clientbound as cb, Writer};
use basalt_world::props;
use uuid::Uuid;

pub struct Faller {
    pub eid: i32,
    pub uuid: Uuid,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub state: u32,
    pub age: u32,
}

pub fn is_gravity(state: u32) -> bool {
    let n = BlockState(state).name();
    matches!(
        n,
        "sand"
            | "red_sand"
            | "gravel"
            | "suspicious_sand"
            | "suspicious_gravel"
            | "anvil"
            | "chipped_anvil"
            | "damaged_anvil"
    ) || n.ends_with("_concrete_powder")
}

/// Cells a falling block drops through (and may land in).
fn passable(state: u32) -> bool {
    if state == 0 {
        return true;
    }
    matches!(
        BlockState(state).name(),
        "air"
            | "cave_air"
            | "void_air"
            | "water"
            | "lava"
            | "fire"
            | "soul_fire"
            | "short_grass"
            | "fern"
            | "dead_bush"
            | "seagrass"
            | "tall_seagrass"
            | "vine"
            | "structure_void"
            | "light"
            | "tall_grass"
            | "large_fern"
            | "bubble_column"
    )
}

fn is_water(state: u32) -> bool {
    matches!(BlockState(state).name(), "water" | "bubble_column")
        || BlockState(state).get("waterlogged") == Some("true")
}

impl Faller {
    pub fn spawn_packets(&self) -> Vec<Writer> {
        let mut a = Writer::with_id(cb::ADD_ENTITY);
        a.varint(self.eid)
            .uuid(self.uuid)
            .varint(registry_id(reg::ENTITY_TYPE, "falling_block").unwrap_or(0))
            .f64(self.pos[0])
            .f64(self.pos[1])
            .f64(self.pos[2]);
        write_lpvec3(&mut a, self.vel);
        a.u8(0).u8(0).u8(0).varint(self.state as i32);
        vec![a]
    }
}

impl Game {
    /// A block changed at `b`: re-check it and the block above for falling in 2 ticks.
    pub(super) fn schedule_gravity(&mut self, b: [i32; 3]) {
        let due = self.tick + 2;
        for p in [b, [b[0], b[1] + 1, b[2]]] {
            if !self.gravity_checks.iter().any(|(_, q)| *q == p) {
                self.gravity_checks.push((due, p));
            }
        }
    }

    pub(super) fn tick_falling(&mut self) {
        let now = self.tick;
        let due: Vec<[i32; 3]> = self
            .gravity_checks
            .iter()
            .filter(|(t, _)| *t <= now)
            .map(|(_, p)| *p)
            .collect();
        self.gravity_checks.retain(|(t, _)| *t > now);
        for b in due {
            let st = self.world.get_block(b[0], b[1], b[2]);
            if !is_gravity(st)
                || b[1] <= basalt_world::MIN_Y
                || !passable(self.world.get_block(b[0], b[1] - 1, b[2]))
            {
                continue;
            }
            self.set_block(b[0], b[1], b[2], 0);
            let eid = self
                .sh
                .next_eid
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.fallers.insert(
                eid,
                Faller {
                    eid,
                    uuid: Uuid::new_v4(),
                    pos: [b[0] as f64 + 0.5, b[1] as f64, b[2] as f64 + 0.5],
                    vel: [0.0; 3],
                    state: st,
                    age: 0,
                },
            );
            self.sync_viewers_for(eid);
        }
        let ids: Vec<i32> = self.fallers.keys().copied().collect();
        for id in ids {
            let Some(mut f) = self.fallers.remove(&id) else {
                continue;
            };
            f.age += 1;
            f.vel[1] -= 0.04;
            let (x, z) = (f.pos[0].floor() as i32, f.pos[2].floor() as i32);
            let ny = f.pos[1] + f.vel[1];
            let cell_y = ny.floor() as i32;
            let powder = BlockState(f.state).name().ends_with("_concrete_powder");
            // powder entering water hardens there
            if powder && is_water(self.world.get_block(x, cell_y, z)) {
                self.land(&f, [x, cell_y, z]);
                continue;
            }
            if f.vel[1] < 0.0
                && !passable(self.world.get_block(x, cell_y, z))
                && props::props(self.world.get_block(x, cell_y, z)).blocks_motion()
            {
                // landed on the block below: settle in the cell above it
                self.land(&f, [x, cell_y + 1, z]);
                continue;
            }
            f.pos[1] = ny;
            f.vel[1] *= 0.98;
            if f.pos[1] < (basalt_world::MIN_Y - 64) as f64 || f.age > 600 {
                self.drop_faller(&f);
                continue;
            }
            if f.age % 20 == 0 {
                self.broadcast_faller_sync(&f);
            }
            self.fallers.insert(id, f);
        }
    }

    fn broadcast_faller_sync(&self, f: &Faller) {
        let (eid, pos, vel) = (f.eid, f.pos, f.vel);
        for p in self.players.values().filter(|p| p.seen.contains(&eid)) {
            let mut w = Writer::with_id(cb::ENTITY_POSITION_SYNC);
            w.varint(eid)
                .f64(pos[0])
                .f64(pos[1])
                .f64(pos[2])
                .f64(vel[0])
                .f64(vel[1])
                .f64(vel[2])
                .f32(0.0)
                .f32(0.0)
                .bool(false);
            p.send(w);
        }
    }

    fn remove_faller_viewers(&mut self, eid: i32) {
        for p in self.players.values_mut() {
            if p.seen.remove(&eid) {
                let mut w = Writer::with_id(cb::REMOVE_ENTITIES);
                w.varint(1).varint(eid);
                p.send(w);
            }
        }
    }

    /// Become a block at `cell` if it is free, else drop as an item.
    fn land(&mut self, f: &Faller, cell: [i32; 3]) {
        self.remove_faller_viewers(f.eid);
        let here = self.world.get_block(cell[0], cell[1], cell[2]);
        if !passable(here) {
            self.drop_faller(f);
            return;
        }
        let mut st = f.state;
        let name = BlockState(st).name();
        if let Some(color) = name.strip_suffix("_concrete_powder") {
            let wet = is_water(here)
                || [[1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1], [0, 1, 0]]
                    .iter()
                    .any(|d| {
                        is_water(self.world.get_block(
                            cell[0] + d[0],
                            cell[1] + d[1],
                            cell[2] + d[2],
                        ))
                    });
            if wet {
                if let Some(c) = BlockState::from_name(&format!("{color}_concrete")) {
                    st = c.0;
                }
            }
        }
        self.set_block(cell[0], cell[1], cell[2], st);
    }

    fn drop_faller(&mut self, f: &Faller) {
        self.remove_faller_viewers(f.eid);
        if let Some(item) = registry_id(reg::ITEM, BlockState(f.state).name()) {
            self.spawn_item(
                [f.pos[0], f.pos[1] + 0.5, f.pos[2]],
                [0.0, 0.1, 0.0],
                Stack {
                    item,
                    count: 1,
                    damage: 0,
                    ench: NO_ENCH,
                },
                10,
            );
        }
    }
}

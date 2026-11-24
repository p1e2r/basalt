//! Dimensions: one world (chunks, generator pool, players, entities) per dimension.
//!
//! The game code works on "the current dimension": `Game::world`, `gen`, `players`, `mobs`,
//! `items`, ... always hold the state of `Game::cur`. The other dimensions are parked in
//! `Game::parked` and swapped in with [`Game::enter`] before anything touches them (a packet
//! from a player, a tick of that dimension, a dimension change). Truly global things (chat,
//! the tab list, time, keepalive) go through [`Game::all_players`] / [`Game::broadcast_all`].
use super::*;
use basalt_world::Layout;

pub(super) const OVERWORLD: usize = 0;
pub(super) const NETHER: usize = 1;

/// Static description of a dimension Basalt runs.
pub(super) struct DimInfo {
    /// Dimension (level) id.
    pub id: &'static str,
    /// dimension_type registry entry.
    pub ty: &'static str,
    pub layout: Layout,
    /// Sea level sent in login/respawn (nether: noise settings sea_level 32).
    pub sea_level: i32,
    /// Coordinate scale of the dimension type (nether 8).
    pub scale: f64,
    /// Region folder relative to the world directory (vanilla 26.2 layout; see `region_dir` for the
    /// overworld's older Basalt `region/`).
    pub region: &'static str,
    /// Mob persistence file relative to the world directory.
    pub entities: &'static str,
    /// Portal registry file (Basalt-only; vanilla keeps portals in its POI files).
    pub portals: &'static str,
}

pub(super) const DIMS: [DimInfo; 2] = [
    DimInfo {
        id: "minecraft:overworld",
        ty: "minecraft:overworld",
        layout: Layout::OVERWORLD,
        sea_level: 63,
        scale: 1.0,
        region: "dimensions/minecraft/overworld/region",
        entities: "entities.json",
        portals: "basalt-portals.json",
    },
    DimInfo {
        id: "minecraft:the_nether",
        ty: "minecraft:the_nether",
        layout: Layout::NETHER,
        sea_level: 32,
        scale: 8.0,
        region: "dimensions/minecraft/the_nether/region",
        entities: "dimensions/minecraft/the_nether/basalt-entities.json",
        portals: "dimensions/minecraft/the_nether/basalt-portals.json",
    },
];

/// Region folder of dimension `d` in world `w`: vanilla 26.2's layout, except an overworld that
/// Basalt saved before the layout change (`region/` with chunks, nothing in the vanilla folder)
/// keeps using `region/`.
pub(super) fn region_dir(w: &std::path::Path, d: usize) -> std::path::PathBuf {
    let vanilla = w.join(DIMS[d].region);
    if d == OVERWORLD {
        let old = w.join("region");
        let has = |p: &std::path::Path| {
            std::fs::read_dir(p)
                .map(|r| {
                    r.flatten()
                        .any(|e| e.path().extension().map(|x| x == "mca").unwrap_or(false))
                })
                .unwrap_or(false)
        };
        if has(&old) && !has(&vanilla) {
            return old;
        }
    }
    vanilla
}

/// Dimension index for a level id (`minecraft:the_nether`, `the_nether`, `nether`, ...).
pub(super) fn dim_index(name: &str) -> Option<usize> {
    match name.trim_start_matches("minecraft:") {
        "overworld" => Some(OVERWORLD),
        "the_nether" | "nether" => Some(NETHER),
        _ => None,
    }
}

/// Everything that belongs to one dimension.
pub(super) struct DimState {
    pub world: World,
    pub gen: GenPool,
    pub players: HashMap<i32, Player>,
    pub items: HashMap<i32, items::ItemEntity>,
    pub mobs: HashMap<i32, mob::Mob>,
    pub arrows: HashMap<i32, arrow::Arrow>,
    pub projs: HashMap<i32, proj::Proj>,
    pub fallers: HashMap<i32, falling::Faller>,
    pub gravity_checks: Vec<(u64, [i32; 3])>,
    pub orbs: HashMap<i32, xporb::Orb>,
    pub bes: HashMap<[i32; 3], container::Be>,
    pub portals: HashSet<[i32; 3]>,
}

impl DimState {
    pub fn new(d: usize, dir: Option<&std::path::Path>, gen: Arc<dyn Generator>) -> DimState {
        let info = &DIMS[d];
        DimState {
            world: World::new_in(dir.map(|w| region_dir(w, d)), info.layout),
            gen: GenPool::new(gen),
            players: HashMap::new(),
            items: HashMap::new(),
            mobs: HashMap::new(),
            arrows: HashMap::new(),
            projs: HashMap::new(),
            fallers: HashMap::new(),
            gravity_checks: Vec::new(),
            orbs: HashMap::new(),
            bes: HashMap::new(),
            portals: HashSet::new(),
        }
    }
}

impl Game {
    fn swap_state(&mut self, s: &mut DimState) {
        std::mem::swap(&mut self.world, &mut s.world);
        std::mem::swap(&mut self.gen, &mut s.gen);
        std::mem::swap(&mut self.players, &mut s.players);
        std::mem::swap(&mut self.items, &mut s.items);
        std::mem::swap(&mut self.mobs, &mut s.mobs);
        std::mem::swap(&mut self.arrows, &mut s.arrows);
        std::mem::swap(&mut self.projs, &mut s.projs);
        std::mem::swap(&mut self.fallers, &mut s.fallers);
        std::mem::swap(&mut self.gravity_checks, &mut s.gravity_checks);
        std::mem::swap(&mut self.orbs, &mut s.orbs);
        std::mem::swap(&mut self.bes, &mut s.bes);
        std::mem::swap(&mut self.portals, &mut s.portals);
    }

    /// Make dimension `d` the current one.
    pub(super) fn enter(&mut self, d: usize) {
        if d == self.cur || d >= DIMS.len() {
            return;
        }
        let Some(mut s) = self.parked[d].take() else {
            return;
        };
        self.swap_state(&mut s);
        self.parked[self.cur] = Some(s);
        self.cur = d;
    }

    /// Dimension a player is in (None: not online).
    pub(super) fn dim_of(&self, eid: i32) -> Option<usize> {
        if self.players.contains_key(&eid) {
            return Some(self.cur);
        }
        self.parked
            .iter()
            .enumerate()
            .find(|(_, s)| {
                s.as_ref()
                    .map(|s| s.players.contains_key(&eid))
                    .unwrap_or(false)
            })
            .map(|(i, _)| i)
    }

    /// Enter the player's dimension; false if the player is not online.
    pub(super) fn enter_player(&mut self, eid: i32) -> bool {
        match self.dim_of(eid) {
            Some(d) => {
                self.enter(d);
                true
            }
            None => false,
        }
    }

    /// Every online player, in all dimensions.
    pub(super) fn all_players(&self) -> impl Iterator<Item = &Player> {
        self.players.values().chain(
            self.parked
                .iter()
                .flatten()
                .flat_map(|s| s.players.values()),
        )
    }

    /// Send to every online player in every dimension (chat, tab list, time).
    pub(super) fn broadcast_all(&self, except: Option<i32>, make: impl Fn() -> Writer) {
        for p in self.all_players() {
            if Some(p.eid) != except {
                p.send(make());
            }
        }
    }

    /// The dimension has players or loaded chunks (so it needs ticking).
    pub(super) fn dim_active(&self, d: usize) -> bool {
        if d == self.cur {
            return d == OVERWORLD || !self.players.is_empty() || !self.world.chunks.is_empty();
        }
        match &self.parked[d] {
            Some(s) => d == OVERWORLD || !s.players.is_empty() || !s.world.chunks.is_empty(),
            None => false,
        }
    }

    /// Common spawn info of login/respawn for the current dimension.
    pub(super) fn spawn_info(&self, w: &mut Writer, gamemode: u8) {
        let info = &DIMS[self.cur];
        let dims = self.sh.registries.entries("minecraft:dimension_type");
        let dim_type = dims.iter().position(|d| d == info.ty).unwrap_or(0) as i32;
        let flat = self.sh.cfg.flat && self.cur == OVERWORLD;
        w.varint(dim_type)
            .str(info.id)
            .i64(self.hashed_seed())
            .u8(gamemode)
            .i8(-1)
            .bool(false)
            .bool(flat)
            .bool(false)
            .varint(0)
            .varint(info.sea_level);
    }

    /// Move a player to another dimension at `pos` (the current dimension must be the player's).
    /// Sends respawn (keeping attributes and entity data) and the level info, then streams the
    /// new dimension's chunks.
    pub(super) fn change_dimension(
        &mut self,
        eid: i32,
        to: usize,
        pos: [f64; 3],
        yaw: f32,
        pitch: f32,
    ) {
        if !self.enter_player(eid) {
            return;
        }
        let from = self.cur;
        if from == to {
            if let Some(p) = self.players.get_mut(&eid) {
                p.pos = pos;
                p.yaw = yaw;
                p.pitch = pitch;
                Self::teleport(p);
            }
            return;
        }
        if self
            .players
            .get(&eid)
            .map(|p| !matches!(p.window, inv::Window::Inventory))
            .unwrap_or(false)
        {
            self.close_window(eid);
        }
        let Some(mut p) = self.players.remove(&eid) else {
            return;
        };
        // the others in the old dimension forget the player
        for o in self.players.values() {
            let mut w = Writer::with_id(cb::REMOVE_ENTITIES);
            w.varint(1).varint(eid);
            o.send(w);
        }
        self.enter(to);
        p.pos = pos;
        p.yaw = yaw;
        p.pitch = pitch;
        p.loaded.clear();
        p.seen_items.clear();
        p.seen.clear();
        p.center = (i32::MAX, i32::MAX);
        p.batch_outstanding = false;
        p.fall_start = None;
        p.dig_start = None;
        p.delayed_dig = None;
        let mut w = Writer::with_id(cb::RESPAWN);
        self.spawn_info(&mut w, p.gamemode);
        w.u8(3); // keep attributes + entity data (a dimension change, not a death)
        p.send(w);
        let mut w = Writer::with_id(cb::CHANGE_DIFFICULTY);
        w.u8(self.difficulty).bool(false);
        p.send(w);
        // vanilla order (measured over the wire, 26.2): respawn, difficulty, position, abilities, border, spawn, game event 13, ticking, inventory, held slot, health, xp
        Self::teleport(&mut p);
        Self::send_abilities(&p);
        let mut w = Writer::with_id(cb::INITIALIZE_BORDER);
        w.f64(0.0)
            .f64(0.0)
            .f64(59999968.0)
            .f64(59999968.0)
            .varlong(0)
            .varint(29999984)
            .varint(5)
            .varint(300);
        p.send(w);
        p.send(self.set_time_packet());
        let mut w = Writer::with_id(cb::SET_DEFAULT_SPAWN_POSITION);
        w.str("minecraft:overworld")
            .position(
                self.spawn[0].floor() as i32,
                self.spawn[1] as i32,
                self.spawn[2].floor() as i32,
            )
            .f32(0.0)
            .f32(0.0);
        p.send(w);
        let mut w = Writer::with_id(cb::GAME_EVENT);
        w.u8(13).f32(0.0);
        p.send(w);
        let mut w = Writer::with_id(cb::TICKING_STATE);
        w.f32(20.0).bool(false);
        p.send(w);
        let mut w = Writer::with_id(cb::TICKING_STEP);
        w.varint(0);
        p.send(w);
        if to == OVERWORLD && self.weather.rain > 0.0 {
            for (id, v) in [
                (1u8, 0.0f32),
                (7, self.weather.rain),
                (8, self.weather.thunder),
            ] {
                let mut w = Writer::with_id(cb::GAME_EVENT);
                w.u8(id).f32(v);
                p.send(w);
            }
        }
        Self::send_window(&mut p);
        let mut w = Writer::with_id(cb::SET_HELD_SLOT);
        w.varint(p.held as i32);
        p.send(w);
        let mut w = Writer::with_id(cb::SET_HEALTH);
        w.f32(p.health).varint(p.food).f32(p.saturation);
        p.send(w);
        Self::send_xp(&p);
        // players already in the new dimension and the newcomer see each other
        let add = self.add_entity(&p);
        for o in self.players.values() {
            o.send(Writer {
                buf: add.buf.clone(),
            });
            p.send(self.add_entity(o));
        }
        log::info!(
            "{} moved to {} at {:.1} {:.1} {:.1}",
            p.name,
            DIMS[to].id,
            pos[0],
            pos[1],
            pos[2]
        );
        self.players.insert(eid, p);
        self.send_effects(eid);
        self.stream_chunks(eid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dim_names() {
        assert_eq!(dim_index("minecraft:the_nether"), Some(NETHER));
        assert_eq!(dim_index("overworld"), Some(OVERWORLD));
        assert_eq!(dim_index("minecraft:the_end"), None);
        assert_eq!(DIMS[NETHER].layout.min_y(), 0);
        assert_eq!(DIMS[NETHER].layout.max_y(), 256);
    }
}

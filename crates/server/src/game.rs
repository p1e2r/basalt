//! Game thread: owns the world and all players; runs at 20 TPS.
mod ai;
mod arrow;
mod bow;
mod breed;
mod bucket;
mod cmd;
mod combat;
mod container;
mod debug;
pub(crate) mod dim;
mod env;
mod explosion;
mod falling;
mod fluid;
mod health;
mod inv;
mod items;
mod mob;
mod path;
mod place;
mod playerdat;
mod portal;
mod potion;
mod potions;
mod proj;
mod randomtick;
mod spawn;
mod stackx;
mod station;
mod tools;
mod track;
mod wind;
mod xporb;

use crate::conn::Shared;
use crate::gen::{GenPool, Generator};
use anyhow::{anyhow, Result};
use basalt_data::{registries as reg, registry_id, BlockState};
use basalt_protocol::{
    ids::play::clientbound as cb, ids::play::serverbound as sb, text, Reader, Writer,
};
use basalt_world::World;
use rand::SeedableRng;
use std::collections::{HashMap, HashSet};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

pub enum ToGame {
    Join {
        eid: i32,
        uuid: Uuid,
        name: String,
        out: UnboundedSender<Vec<u8>>,
        view: u8,
    },
    Packet {
        eid: i32,
        id: i32,
        data: Vec<u8>,
    },
    Leave {
        eid: i32,
    },
}

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Stack {
    pub item: i32,
    pub count: i32,
    pub damage: i32,
    /// enchantments + anvil/brewing/smithing components (see stackx.rs)
    pub ench: stackx::Ench,
}

pub use stackx::NO_ENCH;

struct Player {
    eid: i32,
    uuid: Uuid,
    name: String,
    out: UnboundedSender<Vec<u8>>,
    view: i32,
    pos: [f64; 3],
    yaw: f32,
    pitch: f32,
    on_ground: bool,
    gamemode: u8,
    /// /spawnpoint: block position and dimension index
    spawn_point: Option<([i32; 3], usize)>,
    /// /tag
    tags: std::collections::BTreeSet<String>,
    flying: bool,
    inv: [Stack; 46],
    held: usize,
    loaded: HashSet<(i32, i32)>,
    center: (i32, i32),
    pending_tp: Option<i32>,
    next_tp: i32,
    batch_outstanding: bool,
    chunks_per_tick: f32,
    keepalive: Option<(i64, Instant)>,
    last_keepalive: Instant,
    last_sent_pos: ([f64; 3], f32, f32),
    sneaking: bool,
    // survival
    dead: bool,
    health: f32,
    food: i32,
    saturation: f32,
    exhaustion: f32,
    fall_start: Option<f64>,
    hurt_cooldown: u32,
    regen_timer: u32,
    using: Option<(usize, u32)>,
    sprinting: bool,
    jump_held: bool,
    dig_start: Option<([i32; 3], u64)>,
    /// a rejected stop vanilla keeps tracking: (block, start tick); the block breaks when its progress reaches 1.0
    delayed_dig: Option<([i32; 3], u64)>,
    // containers
    cursor: Stack,
    craft_result: Stack,
    table_grid: [Stack; 9],
    window: inv::Window,
    next_window: i32,
    state_id: i32,
    drag: (i8, Vec<i16>),
    seen_items: HashSet<i32>,
    /// container slots of the open container window (mirror of the block entity, for send_window)
    cont_items: Vec<Stack>,
    /// ender chest inventory (27 slots; vanilla's EnderItems)
    ender: Vec<Stack>,
    st: station::St,
    /// furnace properties last sent to this player (container_set_data diffing)
    data_sent: [i32; 4],
    /// experience points (total)
    /// experience: level, progress into it (0..1, vanilla keeps a float), points ever gained (the
    /// total set_experience reports; level commands leave it alone — measured, m5_xp_command)
    xp_level: i32,
    xp_progress: f32,
    xp: i32,
    // environment
    air: i32,
    fire_ticks: i32,
    ticks_frozen: i32,
    /// consecutive ticks standing in fire (vanilla ignites you after 20)
    fire_exposure: i32,
    /// effect id -> (amplifier, ticks left; -1 = infinite)
    effects: HashMap<i32, (u8, i32)>,
    prev_tick_pos: [f64; 3],
    /// Non-item entities (mobs, arrows) this player's client knows about.
    seen: HashSet<i32>,
    last_attack_tick: u64,
    /// nether portal: ticks inside, cooldown after a dimension change, last portal block touched
    portal_ticks: i32,
    portal_cooldown: i32,
    portal_entry: Option<[i32; 3]>,
    /// item cooldown groups ("ender_pearl") -> game tick they end
    cooldowns: HashMap<String, u64>,
    /// bow / trident draw in progress: (inventory slot, start tick)
    draw: Option<(usize, u64)>,
}

impl Player {
    fn send(&self, w: Writer) {
        let _ = self.out.send(w.buf);
    }
    fn chunk(&self) -> (i32, i32) {
        (
            (self.pos[0].floor() as i32) >> 4,
            (self.pos[2].floor() as i32) >> 4,
        )
    }
    fn sees_chunk(&self, c: (i32, i32)) -> bool {
        self.loaded.contains(&c)
    }
}

/// Rain / thunder: vanilla-style timers (clear / rain / thunder, in ticks) and 0..1 levels that move 0.01 per tick.
#[derive(Default)]
pub struct Weather {
    raining: bool,
    thundering: bool,
    clear_time: i32,
    rain_time: i32,
    thunder_time: i32,
    pub rain: f32,
    pub thunder: f32,
    sent_raining: bool,
}

pub struct Game {
    sh: Arc<Shared>,
    world: World,
    gen: GenPool,
    players: HashMap<i32, Player>,
    tick: u64,
    day_time: i64,
    spawn: [f64; 3],
    player_type: i32,
    dir: Option<std::path::PathBuf>,
    data: Arc<basalt_game::GameData>,
    /// `commands` packet body, built from reports/commands.json (None → the small built-in tree)
    commands_body: Option<Vec<u8>>,
    weather: Weather,
    items: HashMap<i32, items::ItemEntity>,
    mobs: HashMap<i32, mob::Mob>,
    arrows: HashMap<i32, arrow::Arrow>,
    /// thrown snowballs / eggs / pearls / bottles / potions / wind charges
    projs: HashMap<i32, proj::Proj>,
    /// falling_block entities and pending gravity checks (due tick, block)
    fallers: HashMap<i32, falling::Faller>,
    gravity_checks: Vec<(u64, [i32; 3])>,
    /// scheduled fluid ticks (due tick, cell) + the cells already queued
    fluid_ticks: Vec<(u64, [i32; 3])>,
    fluid_pending: HashSet<[i32; 3]>,
    /// gamerule random_tick_speed (vanilla default 3)
    random_tick_speed: i32,
    /// /scoreboard state (data/minecraft/scoreboard.dat)
    scoreboard: cmd::score::Scoreboard,
    /// /bossbar state (data/minecraft/custom_boss_events.dat)
    bossbars: cmd::bossbar::BossBars,
    /// game mode new players get (/defaultgamemode, --gamemode)
    default_gamemode: u8,
    /// 0 peaceful, 1 easy, 2 normal, 3 hard (saved in basalt.json; Basalt's default is Easy)
    pub(crate) difficulty: u8,
    orbs: HashMap<i32, xporb::Orb>,
    /// container block entities of loaded chunks (chests, barrels, furnaces)
    bes: HashMap<[i32; 3], container::Be>,
    path_budget: u32,
    no_spawning: bool,
    rng: rand::rngs::StdRng,
    /// Index (dim::DIMS) of the dimension whose state is in the fields above.
    cur: usize,
    /// The other dimensions (`parked[cur]` is None).
    parked: Vec<Option<dim::DimState>>,
    /// Dimension set by `execute in <dim> run ...` while the inner command runs.
    cmd_dim: Option<usize>,
    /// nether portal blocks of the current dimension (vanilla keeps them as POIs)
    portals: HashSet<[i32; 3]>,
    /// set while portal blocks are placed / removed in bulk (no frame re-checks)
    portal_quiet: bool,
}

fn angle(deg: f32) -> u8 {
    ((deg / 360.0 * 256.0).round() as i32 & 0xFF) as u8
}

/// Default block state for a block name ("stone" or "minecraft:stone").
pub fn block_default(name: &str) -> Option<u32> {
    let n = name.trim_start_matches("minecraft:");
    basalt_data::blocks::BLOCKS
        .iter()
        .find(|b| b.name == n)
        .map(|b| b.default_state)
}

fn with_prop(state: u32, key: &str, val: &str) -> u32 {
    let st = BlockState(state);
    let b = st.block();
    // state index = sum(prop_value_index * stride), last property varies fastest
    let mut stride = 1u32;
    let mut idx = state - b.first_state;
    let mut out = b.first_state;
    let mut parts: Vec<u32> = Vec::new();
    for (_, vals) in b.props.iter().rev() {
        parts.push(idx % vals.len() as u32);
        idx /= vals.len() as u32;
    }
    parts.reverse();
    for (i, (k, vals)) in b.props.iter().enumerate() {
        if *k == key {
            if let Some(p) = vals.iter().position(|v| *v == val) {
                parts[i] = p as u32;
            }
        }
    }
    for (i, (_, vals)) in b.props.iter().enumerate().rev() {
        out += parts[i] * stride;
        stride *= vals.len() as u32;
    }
    out
}

const FACE: [[i32; 3]; 6] = [
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
    [-1, 0, 0],
    [1, 0, 0],
];

/// Vanilla's `time` argument: a number with an optional unit — `t` ticks (default), `s` seconds, `d` days (24000).
fn horizontal_facing(yaw: f32) -> &'static str {
    match (((yaw.rem_euclid(360.0) + 45.0) / 90.0) as i32) & 3 {
        0 => "south",
        1 => "west",
        2 => "north",
        _ => "east",
    }
}

impl Game {
    /// `gens`: one generator per dimension (index as in `dim::DIMS`).
    pub fn new(sh: Arc<Shared>, gens: Vec<Arc<dyn Generator>>) -> Game {
        let dir = sh.cfg.world_dir.clone();
        let mut states: Vec<Option<dim::DimState>> = gens
            .into_iter()
            .enumerate()
            .map(|(d, g)| Some(dim::DimState::new(d, dir.as_deref(), g)))
            .collect();
        let s = states[dim::OVERWORLD].take().expect("overworld generator");
        let player_type = registry_id(reg::ENTITY_TYPE, "player").unwrap_or(0);
        let data = sh.data.clone();
        let sh_difficulty = sh.cfg.difficulty;
        let mut g = Game {
            sh,
            world: s.world,
            gen: s.gen,
            players: s.players,
            tick: 0,
            day_time: 1000,
            spawn: [0.5, 0.0, 0.5],
            player_type,
            dir,
            data,
            commands_body: None,
            weather: Weather::default(),
            items: s.items,
            mobs: s.mobs,
            arrows: s.arrows,
            projs: s.projs,
            fallers: s.fallers,
            gravity_checks: s.gravity_checks,
            fluid_ticks: s.fluid_ticks,
            fluid_pending: s.fluid_pending,
            random_tick_speed: 3,
            scoreboard: Default::default(),
            bossbars: Default::default(),
            default_gamemode: 0,
            difficulty: sh_difficulty,
            orbs: s.orbs,
            bes: s.bes,
            path_budget: 0,
            no_spawning: false,
            rng: rand::rngs::StdRng::from_entropy(),
            cur: dim::OVERWORLD,
            parked: states,
            cmd_dim: None,
            portals: s.portals,
            portal_quiet: false,
        };
        g.no_spawning = g.sh.cfg.no_spawning;
        g.default_gamemode = g.sh.cfg.gamemode;
        g.commands_body =
            crate::commands::build_packet(&g.sh.cfg.data_dir, crate::commands::IMPLEMENTED);
        if g.commands_body.is_none() {
            log::warn!("no reports/commands.json: using the built-in command tree");
        }
        g.load_level();
        for d in (0..g.parked.len()).rev() {
            g.enter(d);
            g.load_mobs();
            g.load_portals();
        }
        // the world's own spawn (level.dat) when it has one, else search a spot
        match g.dir.as_deref().and_then(|d| crate::level::load(d).spawn) {
            Some(sp) => g.spawn = [sp[0] as f64 + 0.5, sp[1] as f64, sp[2] as f64 + 0.5],
            None => g.spawn = g.find_spawn(),
        }
        log::info!(
            "spawn at {:.1} {:.1} {:.1}",
            g.spawn[0],
            g.spawn[1],
            g.spawn[2]
        );
        g
    }

    fn load_level(&mut self) {
        let Some(d) = &self.dir else { return };
        if let Ok(s) = std::fs::read_to_string(d.join("basalt.json")) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                self.day_time = v["day_time"].as_i64().unwrap_or(1000);
                self.difficulty = v["difficulty"]
                    .as_u64()
                    .unwrap_or(self.sh.cfg.difficulty as u64)
                    .min(3) as u8;
            }
        }
        self.scoreboard = cmd::score::Scoreboard::load(d);
        self.bossbars = cmd::bossbar::BossBars::load(d);
        // vanilla level files (level.dat + data/minecraft/*.dat) win when present
        let l = crate::level::load(d);
        if let Some(t) = l.day_time {
            self.day_time = t;
        }
        if let Some(t) = l.game_time {
            self.tick = t.max(0) as u64;
        }
        if let Some(df) = l.difficulty {
            self.difficulty = df;
        }
        if let Some(r) = l.random_tick_speed {
            self.random_tick_speed = r;
        }
    }
    fn save_level(&mut self) {
        let back = self.cur;
        for d in 0..self.parked.len() {
            self.enter(d);
            if let Err(e) = self.world.save_all() {
                log::error!("saving {}: {e:#}", dim::DIMS[d].id);
            }
            self.save_mobs();
            self.save_portals();
        }
        self.enter(back);
        let Some(d) = &self.dir else { return };
        let _ = std::fs::create_dir_all(d.join("players"));
        let _ = std::fs::write(d.join("basalt.json"), serde_json::json!({ "day_time": self.day_time, "seed": self.sh.cfg.seed, "difficulty": self.difficulty }).to_string());
        self.scoreboard.save(d);
        self.bossbars.save(d);
        // and vanilla's level files, so a vanilla 26.2 server can open the world (unknown tags kept)
        let sp = self.spawn;
        crate::level::save(
            d,
            &crate::level::Level {
                seed: Some(self.sh.cfg.seed),
                spawn: Some([
                    sp[0].floor() as i32,
                    sp[1].floor() as i32,
                    sp[2].floor() as i32,
                ]),
                difficulty: Some(self.difficulty),
                day_time: Some(self.day_time),
                game_time: Some(self.tick as i64),
                random_tick_speed: Some(self.random_tick_speed),
                raining: None,
            },
            self.sh.cfg.flat,
        );
        for p in self.all_players() {
            self.save_player(p);
        }
    }
    fn save_player(&self, p: &Player) {
        let Some(d) = &self.dir else { return };
        let inv: Vec<_> = p
            .inv
            .iter()
            .enumerate()
            .filter(|(_, s)| s.count > 0)
            .map(|(i, s)| {
                serde_json::json!([
                    i,
                    s.item,
                    s.count,
                    s.damage,
                    s.ench
                        .iter()
                        .filter(|e| e.1 > 0)
                        .map(|e| [e.0 as u32, e.1 as u32])
                        .collect::<Vec<_>>(),
                    stackx_json(&s.ench)
                ])
            })
            .collect();
        let j = serde_json::json!({ "pos": p.pos, "yaw": p.yaw, "pitch": p.pitch, "gamemode": p.gamemode, "held": p.held, "inv": inv, "flying": p.flying,
            "xp": p.xp, "xp_level": p.xp_level, "xp_progress": p.xp_progress, "health": p.health, "food": p.food, "saturation": p.saturation, "dead": p.dead, "air": p.air, "fire": p.fire_ticks,
            "effects": p.effects.iter().map(|(id, (a, t))| [*id as i64, *a as i64, *t as i64]).collect::<Vec<_>>(),
            "dimension": dim::DIMS[self.dim_of(p.eid).unwrap_or(self.cur)].id,
            "ender": self.ender_items(p).iter().enumerate().filter(|(_, s)| s.count > 0).map(|(i, s)| serde_json::json!([i, s.item, s.count, s.damage])).collect::<Vec<_>>() });
        let _ = std::fs::create_dir_all(d.join("players"));
        let _ = std::fs::write(
            d.join("players").join(format!("{}.json", p.uuid)),
            j.to_string(),
        );
        self.save_player_dat(p);
    }
    /// Dimension a player was saved in (overworld if unknown).
    fn saved_dim(&self, uuid: Uuid) -> usize {
        let Some(d) = &self.dir else {
            return dim::OVERWORLD;
        };
        let Ok(s) = std::fs::read_to_string(d.join("players").join(format!("{uuid}.json"))) else {
            return self.dat_dim(uuid).unwrap_or(dim::OVERWORLD);
        };
        serde_json::from_str::<serde_json::Value>(&s)
            .ok()
            .and_then(|v| v["dimension"].as_str().and_then(dim::dim_index))
            .unwrap_or(dim::OVERWORLD)
    }
    fn load_player(&self, p: &mut Player) {
        let Some(d) = &self.dir else { return };
        let Ok(s) = std::fs::read_to_string(d.join("players").join(format!("{}.json", p.uuid)))
        else {
            self.load_player_dat(p);
            return;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) else {
            return;
        };
        if let Some(a) = v["pos"].as_array() {
            if a.len() == 3 {
                p.pos = [
                    a[0].as_f64().unwrap_or(0.0),
                    a[1].as_f64().unwrap_or(0.0),
                    a[2].as_f64().unwrap_or(0.0),
                ];
            }
        }
        p.yaw = v["yaw"].as_f64().unwrap_or(0.0) as f32;
        p.pitch = v["pitch"].as_f64().unwrap_or(0.0) as f32;
        p.gamemode = v["gamemode"].as_u64().unwrap_or(p.gamemode as u64) as u8;
        p.held = v["held"].as_u64().unwrap_or(0).min(8) as usize;
        p.flying = v["flying"].as_bool().unwrap_or(false);
        p.xp = v["xp"].as_i64().unwrap_or(0) as i32;
        match (v["xp_level"].as_i64(), v["xp_progress"].as_f64()) {
            (Some(l), Some(pr)) => {
                p.xp_level = l as i32;
                p.xp_progress = pr as f32;
            }
            _ => {
                let (l, pr) = container::xp_level(p.xp);
                p.xp_level = l;
                p.xp_progress = pr;
            }
        }
        p.health = v["health"].as_f64().unwrap_or(20.0) as f32;
        p.food = v["food"].as_i64().unwrap_or(20) as i32;
        p.saturation = v["saturation"].as_f64().unwrap_or(5.0) as f32;
        p.dead = v["dead"].as_bool().unwrap_or(false);
        p.air = v["air"].as_i64().unwrap_or(env::MAX_AIR as i64) as i32;
        p.fire_ticks = v["fire"].as_i64().unwrap_or(0) as i32;
        for e in v["effects"].as_array().into_iter().flatten() {
            if let (Some(id), Some(a), Some(t)) = (e[0].as_i64(), e[1].as_i64(), e[2].as_i64()) {
                p.effects.insert(id as i32, (a as u8, t as i32));
            }
        }
        for e in v["ender"].as_array().into_iter().flatten() {
            if let (Some(i), Some(item), Some(count)) =
                (e[0].as_u64(), e[1].as_i64(), e[2].as_i64())
            {
                if (i as usize) < 27 {
                    p.ender[i as usize] = Stack {
                        item: item as i32,
                        count: count as i32,
                        damage: e[3].as_i64().unwrap_or(0) as i32,
                        ench: NO_ENCH,
                    };
                }
            }
        }
        for e in v["inv"].as_array().into_iter().flatten() {
            if let (Some(i), Some(item), Some(count)) =
                (e[0].as_u64(), e[1].as_i64(), e[2].as_i64())
            {
                if (i as usize) < 46 {
                    let mut ench = NO_ENCH;
                    for (k, en) in e[4]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .take(stackx::MAX_ENCH)
                        .enumerate()
                    {
                        ench[k] = (
                            en[0].as_u64().unwrap_or(0) as u16,
                            en[1].as_u64().unwrap_or(0) as u8,
                        );
                    }
                    stackx_from_json(&mut ench, &e[5]);
                    p.inv[i as usize] = Stack {
                        item: item as i32,
                        count: count as i32,
                        damage: e[3].as_i64().unwrap_or(0) as i32,
                        ench,
                    };
                }
            }
        }
    }

    // ------------------------------------------------------------ world helpers

    /// Load or generate a chunk synchronously (startup / spawn search only).
    fn ensure_chunk(&mut self, cx: i32, cz: i32) {
        if self.chunk_ready(cx, cz) {
            return;
        }
        let c = self.gen.gen.generate(cx, cz);
        self.add_generated(c);
    }

    /// True if the chunk is in memory (loading it from disk if saved); otherwise queues generation.
    fn chunk_ready(&mut self, cx: i32, cz: i32) -> bool {
        if self.world.chunk(cx, cz).is_some() {
            return true;
        }
        match self.world.load(cx, cz) {
            Ok(true) => {
                self.be_adopt_chunk(cx, cz);
                return true;
            }
            Ok(false) => {}
            Err(e) => log::warn!("loading chunk {cx},{cz}: {e:#}; regenerating"),
        }
        false
    }

    fn add_generated(&mut self, c: basalt_world::Chunk) {
        let (cx, cz) = (c.x, c.z);
        if self.world.chunk(cx, cz).is_some() {
            return;
        }
        self.world.insert(c);
        self.be_adopt_chunk(cx, cz);
        // neighbours that players already have may get new light across the border
        for (nx, nz) in self.world.light_chunk_changed(cx, cz) {
            if (nx, nz) == (cx, cz) {
                continue;
            }
            let Some(ch) = self.world.chunk(nx, nz) else {
                continue;
            };
            let mut body = Vec::new();
            ch.write_light(&mut body);
            self.broadcast_chunk((nx, nz), None, || {
                let mut w = Writer::with_id(cb::LIGHT_UPDATE);
                w.varint(nx).varint(nz).bytes(&body);
                w
            });
        }
    }

    /// Nearest dry land to the origin (vanilla also searches for a non-ocean spawn).
    fn find_spawn(&mut self) -> [f64; 3] {
        for r in 0..24i32 {
            for dx in -r..=r {
                for dz in -r..=r {
                    if dx.abs() != r && dz.abs() != r {
                        continue;
                    }
                    let (x, z) = (dx * 16 + 8, dz * 16 + 8);
                    let y = self.surface_y(x, z);
                    let top = BlockState(self.world.get_block(x, y - 1, z)).name();
                    if y > 62
                        && !matches!(
                            top,
                            "water" | "ice" | "lava" | "seagrass" | "kelp" | "kelp_plant"
                        )
                        && !top.ends_with("leaves")
                    {
                        return [x as f64 + 0.5, y as f64, z as f64 + 0.5];
                    }
                }
            }
        }
        [0.5, self.surface_y(0, 0) as f64, 0.5]
    }

    fn surface_y(&mut self, x: i32, z: i32) -> i32 {
        self.ensure_chunk(x >> 4, z >> 4);
        let mut y = basalt_world::MIN_Y + basalt_world::HEIGHT - 1;
        while y > basalt_world::MIN_Y && self.world.get_block(x, y, z) == 0 {
            y -= 1;
        }
        y + 1
    }

    fn chunk_packet(&self, cx: i32, cz: i32) -> Option<Writer> {
        let c = self.world.chunk(cx, cz)?;
        let mut w = Writer::with_id(cb::LEVEL_CHUNK_WITH_LIGHT);
        w.i32(cx).i32(cz);
        c.write_network(&mut w.buf);
        Some(w)
    }

    fn broadcast(&self, except: Option<i32>, make: impl Fn() -> Writer) {
        for p in self.players.values() {
            if Some(p.eid) != except {
                p.send(make());
            }
        }
    }
    fn broadcast_chunk(&self, c: (i32, i32), except: Option<i32>, make: impl Fn() -> Writer) {
        for p in self.players.values() {
            if Some(p.eid) != except && p.sees_chunk(c) {
                p.send(make());
            }
        }
    }
    fn world_spawn_packet(&self) -> Writer {
        let mut w = Writer::with_id(cb::SET_DEFAULT_SPAWN_POSITION);
        w.str("minecraft:overworld")
            .position(
                self.spawn[0].floor() as i32,
                self.spawn[1] as i32,
                self.spawn[2].floor() as i32,
            )
            .f32(0.0)
            .f32(0.0);
        w
    }
    /// /setworldspawn: tell every player in the overworld (the compass target).
    fn send_world_spawn(&self) {
        for p in self.all_players() {
            p.send(self.world_spawn_packet());
        }
    }
    fn system_chat(&self, msg: basalt_nbt::Tag) {
        log::info!("[chat] {}", text::to_plain(&msg));
        self.broadcast_all(None, || {
            let mut w = Writer::with_id(cb::SYSTEM_CHAT);
            w.nbt(&msg).bool(false);
            w
        });
    }
    fn tell(&self, eid: i32, msg: basalt_nbt::Tag) {
        if let Some(p) = self.all_players().find(|p| p.eid == eid) {
            let mut w = Writer::with_id(cb::SYSTEM_CHAT);
            w.nbt(&msg).bool(false);
            p.send(w);
        }
    }

    /// Change a block, notify viewers and resend light for affected chunks.
    fn set_block(&mut self, x: i32, y: i32, z: i32, state: u32) {
        let old = self.world.get_block(x, y, z);
        let changed = self.world.set_block(x, y, z, state);
        self.be_block_changed([x, y, z], state);
        self.schedule_gravity([x, y, z]);
        if old != state {
            self.fluid_block_changed([x, y, z]);
        }
        self.broadcast_chunk((x >> 4, z >> 4), None, || {
            let mut w = Writer::with_id(cb::BLOCK_UPDATE);
            w.position(x, y, z).varint(state as i32);
            w
        });
        for (cx, cz) in changed {
            let Some(c) = self.world.chunk(cx, cz) else {
                continue;
            };
            let mut body = Vec::new();
            c.write_light(&mut body);
            self.broadcast_chunk((cx, cz), None, || {
                let mut w = Writer::with_id(cb::LIGHT_UPDATE);
                w.varint(cx).varint(cz).bytes(&body);
                w
            });
        }
        if old != state && self.world.get_block(x, y, z) == state {
            if BlockState(old).name() == "nether_portal"
                || BlockState(state).name() == "nether_portal"
            {
                self.portal_registry([x, y, z], state);
            }
            self.portal_block_changed([x, y, z], old, state);
        }
    }

    // ------------------------------------------------------------ packets to players

    fn info_add(p: &Player) -> Writer {
        let mut w = Writer::with_id(cb::PLAYER_INFO_UPDATE);
        w.u8(0xff)
            .varint(1)
            .uuid(p.uuid)
            .str(&p.name)
            .varint(0)
            .bool(false)
            .varint(p.gamemode as i32)
            .bool(true)
            .varint(0)
            .bool(false)
            .varint(0)
            .bool(true);
        w
    }
    fn add_entity(&self, p: &Player) -> Writer {
        let mut w = Writer::with_id(cb::ADD_ENTITY);
        w.varint(p.eid)
            .uuid(p.uuid)
            .varint(self.player_type)
            .f64(p.pos[0])
            .f64(p.pos[1])
            .f64(p.pos[2])
            .u8(0)
            .angle(p.pitch)
            .angle(p.yaw)
            .angle(p.yaw)
            .varint(0);
        w
    }
    fn sync_entity(p: &Player) -> Writer {
        let mut w = Writer::with_id(cb::ENTITY_POSITION_SYNC);
        w.varint(p.eid)
            .f64(p.pos[0])
            .f64(p.pos[1])
            .f64(p.pos[2])
            .f64(0.0)
            .f64(0.0)
            .f64(0.0)
            .f32(p.yaw)
            .f32(p.pitch)
            .bool(p.on_ground);
        w
    }

    fn send_abilities(p: &Player) {
        let (inv, can_fly, instabuild) = match p.gamemode {
            1 => (true, true, true),
            3 => (true, true, false),
            _ => (false, false, false),
        };
        let flags =
            inv as u8 | (p.flying as u8) << 1 | (can_fly as u8) << 2 | (instabuild as u8) << 3;
        let mut w = Writer::with_id(cb::PLAYER_ABILITIES);
        w.u8(flags).f32(0.05).f32(0.1);
        p.send(w);
    }
    fn teleport(p: &mut Player) {
        p.next_tp += 1;
        p.pending_tp = Some(p.next_tp);
        let mut w = Writer::with_id(cb::PLAYER_POSITION);
        w.varint(p.next_tp)
            .f64(p.pos[0])
            .f64(p.pos[1])
            .f64(p.pos[2])
            .f64(0.0)
            .f64(0.0)
            .f64(0.0)
            .f32(p.yaw)
            .f32(p.pitch)
            .i32(0);
        p.send(w);
    }
    fn hashed_seed(&self) -> i64 {
        self.sh.cfg.seed.wrapping_mul(0x9E3779B97F4A7C15u64 as i64)
    }

    /// Natural weather cycle + level interpolation; broadcasts begin/end rain and level changes.
    fn tick_weather(&mut self) {
        let w = &mut self.weather;
        let rng = &mut self.rng;
        if w.clear_time > 0 {
            w.clear_time -= 1;
            w.thunder_time = if w.thundering { 0 } else { 1 };
            w.rain_time = if w.raining { 0 } else { 1 };
            w.thundering = false;
            w.raining = false;
        } else {
            if w.thunder_time > 0 {
                w.thunder_time -= 1;
                if w.thunder_time == 0 {
                    w.thundering = !w.thundering;
                }
            } else if w.thundering {
                w.thunder_time = rand::Rng::gen_range(rng, 3600..15600);
            } else {
                w.thunder_time = rand::Rng::gen_range(rng, 12000..180000);
            }
            if w.rain_time > 0 {
                w.rain_time -= 1;
                if w.rain_time == 0 {
                    w.raining = !w.raining;
                }
            } else if w.raining {
                w.rain_time = rand::Rng::gen_range(rng, 12000..24000);
            } else {
                w.rain_time = rand::Rng::gen_range(rng, 12000..180000);
            }
        }
        let (old_rain, old_thunder) = (w.rain, w.thunder);
        w.thunder = (w.thunder + if w.thundering { 0.01 } else { -0.01 }).clamp(0.0, 1.0);
        w.rain = (w.rain + if w.raining { 0.01 } else { -0.01 }).clamp(0.0, 1.0);
        let (rain, thunder, raining, sent) = (w.rain, w.thunder, w.raining, w.sent_raining);
        let ev = |id: u8, v: f32| {
            let mut p = Writer::with_id(cb::GAME_EVENT);
            p.u8(id).f32(v);
            p
        };
        if raining != sent {
            self.weather.sent_raining = raining;
            self.broadcast(None, || ev(if raining { 1 } else { 2 }, 0.0));
        }
        if rain != old_rain {
            self.broadcast(None, || ev(7, rain));
        }
        if thunder != old_thunder {
            self.broadcast(None, || ev(8, thunder));
        }
    }

    /// `/weather clear|rain|thunder [time]` (vanilla's default durations when omitted; plain numbers are ticks).
    fn set_weather(&mut self, kind: &str, duration_ticks: Option<i32>) {
        let r = &mut self.rng;
        let (rain_d, thunder_d, clear_d) = (
            rand::Rng::gen_range(r, 12000..24000),
            rand::Rng::gen_range(r, 3600..15600),
            rand::Rng::gen_range(r, 12000..180000),
        );
        let w = &mut self.weather;
        let ticks = duration_ticks;
        match kind {
            "clear" => {
                w.clear_time = ticks.unwrap_or(clear_d);
                w.rain_time = 0;
                w.thunder_time = 0;
                w.raining = false;
                w.thundering = false;
            }
            "rain" => {
                w.clear_time = 0;
                w.rain_time = ticks.unwrap_or(rain_d);
                w.thunder_time = ticks.unwrap_or(rain_d);
                w.raining = true;
                w.thundering = false;
            }
            _ => {
                w.clear_time = 0;
                w.rain_time = ticks.unwrap_or(thunder_d);
                w.thunder_time = ticks.unwrap_or(thunder_d);
                w.raining = true;
                w.thundering = true;
            }
        }
    }

    fn set_time_packet(&self) -> Writer {
        let mut w = Writer::with_id(cb::SET_TIME);
        w.i64(self.tick as i64).varint(2);
        for clock in 0..2 {
            w.varint(clock).varlong(self.day_time).f32(0.0).f32(1.0);
        }
        w
    }

    // ------------------------------------------------------------ join / leave

    fn join(
        &mut self,
        eid: i32,
        uuid: Uuid,
        name: String,
        out: UnboundedSender<Vec<u8>>,
        view: u8,
    ) {
        let old = self.all_players().find(|p| p.uuid == uuid).map(|p| p.eid);
        if let Some(old) = old {
            self.enter_player(old);
            if let Some(p) = self.players.get(&old) {
                let mut w = Writer::with_id(cb::DISCONNECT);
                w.nbt(&text::plain("You logged in from another location"));
                p.send(w);
                let _ = p.out.send(Vec::new());
            }
            self.leave(old);
        }
        let mut p = Player {
            eid,
            uuid,
            name: name.clone(),
            out,
            view: (view as i32).min(self.sh.cfg.view_distance as i32),
            pos: self.spawn,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            gamemode: self.default_gamemode,
            spawn_point: None,
            tags: Default::default(),
            flying: false,
            inv: [Stack::default(); 46],
            held: 0,
            loaded: HashSet::new(),
            center: (i32::MAX, i32::MAX),
            pending_tp: None,
            next_tp: 0,
            batch_outstanding: false,
            chunks_per_tick: 9.0,
            keepalive: None,
            last_keepalive: Instant::now(),
            last_sent_pos: ([0.0; 3], 0.0, 0.0),
            sneaking: false,
            dead: false,
            health: 20.0,
            food: 20,
            saturation: 5.0,
            exhaustion: 0.0,
            fall_start: None,
            hurt_cooldown: 0,
            regen_timer: 0,
            using: None,
            sprinting: false,
            jump_held: false,
            dig_start: None,
            delayed_dig: None,
            cursor: Stack::default(),
            craft_result: Stack::default(),
            table_grid: [Stack::default(); 9],
            window: inv::Window::Inventory,
            next_window: 0,
            state_id: 0,
            drag: (0, Vec::new()),
            seen_items: HashSet::new(),
            cont_items: Vec::new(),
            ender: vec![Stack::default(); 27],
            st: station::St::default(),
            data_sent: [-1; 4],
            xp_level: 0,
            xp_progress: 0.0,
            xp: 0,
            air: env::MAX_AIR,
            fire_ticks: 0,
            ticks_frozen: 0,
            fire_exposure: 0,
            effects: HashMap::new(),
            prev_tick_pos: [0.0; 3],
            seen: HashSet::new(),
            last_attack_tick: 0,
            portal_ticks: 0,
            portal_cooldown: 0,
            portal_entry: None,
            cooldowns: HashMap::new(),
            draw: None,
        };
        self.load_player(&mut p);
        let d = self.saved_dim(p.uuid);
        self.enter(d);
        if d != dim::OVERWORLD && p.pos == self.spawn {
            p.pos = [0.5, 64.0, 0.5];
        }
        let mut w = Writer::with_id(cb::LOGIN);
        w.i32(eid).bool(false).varint(dim::DIMS.len() as i32);
        for di in &dim::DIMS {
            w.str(di.id);
        }
        w.varint(self.sh.cfg.max_players as i32)
            .varint(p.view)
            .varint(p.view)
            .bool(false)
            .bool(true)
            .bool(false);
        self.spawn_info(&mut w, p.gamemode);
        w.bool(false).u8(0);
        p.send(w);
        let mut w = Writer::with_id(cb::CHANGE_DIFFICULTY);
        w.u8(self.difficulty).bool(false);
        p.send(w);
        // op level (entity event 24 + level), from ops.json (admin.rs)
        let lvl = self
            .sh
            .admin
            .lock()
            .unwrap()
            .level(&p.uuid.to_string(), &p.name);
        let mut w = Writer::with_id(cb::ENTITY_EVENT);
        w.i32(p.eid).u8(24 + lvl.min(4));
        p.send(w);
        Self::send_abilities(&p);
        let mut w = Writer::with_id(cb::SET_HELD_SLOT);
        w.varint(p.held as i32);
        p.send(w);
        let mut w = Writer::with_id(cb::UPDATE_RECIPES);
        w.varint(0).varint(0);
        p.send(w);
        match &self.commands_body {
            Some(b) => {
                let mut w = Writer::with_id(cb::COMMANDS);
                w.bytes(b);
                p.send(w);
            }
            None => p.send(commands_packet()),
        }
        Self::teleport(&mut p);
        let mut w = Writer::with_id(cb::SERVER_DATA);
        w.nbt(&text::plain(&self.sh.cfg.motd)).bool(false);
        p.send(w);
        // tab list: everyone to the newcomer, newcomer to everyone
        for o in self.all_players() {
            p.send(Self::info_add(o));
        }
        p.send(Self::info_add(&p));
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
        p.send(self.world_spawn_packet());
        for w in self.scoreboard.join_packets() {
            p.send(w);
        }
        for w in self.bossbars.join_packets(p.uuid) {
            p.send(w);
        }
        let mut w = Writer::with_id(cb::GAME_EVENT);
        w.u8(13).f32(0.0);
        p.send(w);
        if self.cur == dim::OVERWORLD && self.weather.rain > 0.0 {
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
        let mut w = Writer::with_id(cb::TICKING_STATE);
        w.f32(20.0).bool(false);
        p.send(w);
        let mut w = Writer::with_id(cb::TICKING_STEP);
        w.varint(0);
        p.send(w);
        Self::send_window(&mut p);
        let mut w = Writer::with_id(cb::SET_HEALTH);
        w.f32(p.health).varint(p.food).f32(p.saturation);
        p.send(w);
        Self::send_xp(&p);
        self.broadcast_all(None, || Self::info_add(&p));
        let add = self.add_entity(&p);
        for o in self.players.values() {
            o.send(Writer {
                buf: add.buf.clone(),
            });
            p.send(self.add_entity(o));
        }
        log::info!(
            "{name} joined (eid {eid}) at {:.1} {:.1} {:.1}",
            p.pos[0],
            p.pos[1],
            p.pos[2]
        );
        self.players.insert(eid, p);
        self.system_chat(text::colored(&format!("{name} joined the game"), "yellow"));
        self.stream_chunks(eid);
        self.send_effects(eid);
        if self.players[&eid].dead {
            let p = &self.players[&eid];
            let mut w = Writer::with_id(cb::PLAYER_COMBAT_KILL);
            w.varint(eid).nbt(&text::plain("You died"));
            p.send(w);
        }
    }

    fn leave(&mut self, eid: i32) {
        if !self.enter_player(eid) {
            return;
        }
        if self.players.get(&eid).map(|p| !p.dead).unwrap_or(false) {
            self.close_window(eid);
        }
        let Some(p) = self.players.remove(&eid) else {
            return;
        };
        self.save_player(&p);
        log::info!("{} left", p.name);
        self.broadcast(None, || {
            let mut w = Writer::with_id(cb::REMOVE_ENTITIES);
            w.varint(1).varint(eid);
            w
        });
        self.broadcast_all(None, || {
            let mut w = Writer::with_id(cb::PLAYER_INFO_REMOVE);
            w.varint(1).uuid(p.uuid);
            w
        });
        self.system_chat(text::colored(
            &format!("{} left the game", p.name),
            "yellow",
        ));
    }

    // ------------------------------------------------------------ chunk streaming

    fn stream_chunks(&mut self, eid: i32) {
        let Some(p) = self.players.get(&eid) else {
            return;
        };
        let (cx, cz) = p.chunk();
        let view = p.view;
        let mut want: Vec<(i32, i32)> = Vec::new();
        // vanilla's shape (measured: view 4 -> 117 chunks): the square of radius view+1 without its four corner chunks
        let r = view + 1;
        for dx in -r..=r {
            for dz in -r..=r {
                if !(dx.abs() == r && dz.abs() == r) {
                    want.push((cx + dx, cz + dz));
                }
            }
        }
        let want_set: HashSet<_> = want.iter().copied().collect();
        let to_unload: Vec<_> = p
            .loaded
            .iter()
            .copied()
            .filter(|c| !want_set.contains(c))
            .collect();
        let center_changed = p.center != (cx, cz);
        let outstanding = p.batch_outstanding;
        let budget = p.chunks_per_tick.clamp(1.0, 64.0).ceil() as usize;
        let mut missing: Vec<_> = want.into_iter().filter(|c| !p.loaded.contains(c)).collect();
        missing.sort_by_key(|(x, z)| (x - cx).pow(2) + (z - cz).pow(2));
        let p = self.players.get_mut(&eid).unwrap();
        if center_changed {
            p.center = (cx, cz);
            let mut w = Writer::with_id(cb::SET_CHUNK_CACHE_CENTER);
            w.varint(cx).varint(cz);
            p.send(w);
        }
        for c in &to_unload {
            p.loaded.remove(c);
            let mut w = Writer::with_id(cb::FORGET_LEVEL_CHUNK);
            w.i32(c.1).i32(c.0);
            p.send(w);
        }
        if outstanding || missing.is_empty() {
            return;
        }
        // send what's ready (nearest first); queue generation for what isn't
        let mut batch = Vec::new();
        for &(x, z) in &missing {
            if batch.len() >= budget {
                break;
            }
            if self.chunk_ready(x, z) {
                batch.push((x, z));
            } else {
                self.gen.request(x, z);
            }
        }
        for &(x, z) in missing.iter().take(budget * 4) {
            if self.world.chunk(x, z).is_none() {
                self.gen.request(x, z);
            }
        }
        if batch.is_empty() {
            return;
        }
        let packets: Vec<Writer> = batch
            .iter()
            .filter_map(|&(x, z)| self.chunk_packet(x, z))
            .collect();
        let p = self.players.get_mut(&eid).unwrap();
        p.send(Writer::with_id(cb::CHUNK_BATCH_START));
        let n = packets.len();
        for w in packets {
            p.send(w);
        }
        for c in batch {
            p.loaded.insert(c);
        }
        let mut w = Writer::with_id(cb::CHUNK_BATCH_FINISHED);
        w.varint(n as i32);
        p.send(w);
        p.batch_outstanding = true;
    }

    // ------------------------------------------------------------ inbound packets

    fn packet(&mut self, eid: i32, id: i32, data: &[u8]) -> Result<()> {
        let mut r = Reader::new(data);
        match id {
            sb::KEEP_ALIVE => {
                let v = r.i64()?;
                if let Some(p) = self.players.get_mut(&eid) {
                    if p.keepalive.map(|k| k.0) == Some(v) {
                        p.keepalive = None;
                    }
                }
            }
            sb::ACCEPT_TELEPORTATION => {
                let t = r.varint()?;
                if let Some(p) = self.players.get_mut(&eid) {
                    if p.pending_tp == Some(t) {
                        p.pending_tp = None;
                    }
                }
            }
            sb::CHUNK_BATCH_RECEIVED => {
                let rate = r.f32()?;
                if let Some(p) = self.players.get_mut(&eid) {
                    p.batch_outstanding = false;
                    if rate.is_finite() && rate > 0.0 {
                        p.chunks_per_tick = rate;
                    }
                }
            }
            sb::MOVE_PLAYER_POS
            | sb::MOVE_PLAYER_POS_ROT
            | sb::MOVE_PLAYER_ROT
            | sb::MOVE_PLAYER_STATUS_ONLY => {
                let has_pos = id == sb::MOVE_PLAYER_POS || id == sb::MOVE_PLAYER_POS_ROT;
                let has_rot = id == sb::MOVE_PLAYER_ROT || id == sb::MOVE_PLAYER_POS_ROT;
                let pos = if has_pos {
                    Some([r.f64()?, r.f64()?, r.f64()?])
                } else {
                    None
                };
                let rot = if has_rot {
                    Some((r.f32()?, r.f32()?))
                } else {
                    None
                };
                let flags = r.u8()?;
                let Some(p) = self.players.get_mut(&eid) else {
                    return Ok(());
                };
                if p.pending_tp.is_some() {
                    return Ok(());
                } // ignore until the client confirms our teleport
                if let Some(np) = pos {
                    if np.iter().any(|v| !v.is_finite() || v.abs() > 3.0e7) {
                        return Err(anyhow!("invalid position"));
                    }
                    let d2: f64 = (0..3).map(|i| (np[i] - p.pos[i]).powi(2)).sum();
                    if d2 > 100.0 * 100.0 {
                        log::warn!("{} moved too quickly ({:.1})", p.name, d2.sqrt());
                        Self::teleport(p);
                        return Ok(());
                    }
                    if p.dead {
                        return Ok(());
                    }
                    let old = p.pos;
                    p.pos = np;
                    p.on_ground = flags & 1 != 0;
                    let jumped = np[1] > old[1] + 0.2 && !p.on_ground && p.jump_held;
                    let hd = ((np[0] - old[0]).powi(2) + (np[2] - old[2]).powi(2)).sqrt();
                    self.track_fall(eid, old[1]);
                    self.add_move_exhaustion(eid, hd, jumped);
                    let Some(p) = self.players.get_mut(&eid) else {
                        return Ok(());
                    };
                    if jumped {
                        p.jump_held = false;
                    }
                }
                let Some(p) = self.players.get_mut(&eid) else {
                    return Ok(());
                };
                if let Some((y, pi)) = rot {
                    if y.is_finite() && pi.is_finite() {
                        p.yaw = y;
                        p.pitch = pi.clamp(-90.0, 90.0);
                    }
                }
                p.on_ground = flags & 1 != 0;
                if pos.is_some() && p.chunk() != p.center {
                    self.stream_chunks(eid);
                }
            }
            sb::PLAYER_ABILITIES => {
                let f = r.u8()?;
                if let Some(p) = self.players.get_mut(&eid) {
                    p.flying = f & 2 != 0 && matches!(p.gamemode, 1 | 3);
                }
            }
            sb::PLAYER_COMMAND => {
                let _id = r.varint()?;
                let action = r.varint()?;
                if let Some(p) = self.players.get_mut(&eid) {
                    match action {
                        1 => p.sprinting = true,
                        2 => p.sprinting = false,
                        _ => {}
                    }
                }
            }
            sb::PLAYER_INPUT => {
                let f = r.u8()?;
                if let Some(p) = self.players.get_mut(&eid) {
                    p.sneaking = f & 32 != 0;
                    p.jump_held = f & 16 != 0;
                    p.sprinting = p.sprinting || f & 64 != 0 && f & 1 != 0;
                    if f & 1 == 0 {
                        p.sprinting = false;
                    }
                }
            }
            sb::CONTAINER_CLICK => {
                self.container_click(eid, &mut r)?;
            }
            sb::CONTAINER_BUTTON_CLICK => {
                let w = r.varint()?;
                let b = r.varint()?;
                if self.players.get(&eid).map(|p| p.window_id()) == Some(w) {
                    self.station_button(eid, b);
                }
            }
            sb::RENAME_ITEM => {
                let n = r.str()?;
                self.station_rename(eid, n);
            }
            sb::CONTAINER_CLOSE => {
                let id = r.varint()?;
                if self.players.get(&eid).map(|p| p.window_id()) == Some(id) {
                    self.close_window(eid);
                }
            }
            sb::CLIENT_COMMAND => {
                if r.varint()? == 0 {
                    self.respawn(eid);
                }
            }
            sb::SET_CARRIED_ITEM => {
                let s = r.i16()?;
                if let Some(p) = self.players.get_mut(&eid) {
                    if (0..9).contains(&s) {
                        if p.held != s as usize {
                            p.last_attack_tick = self.tick;
                        }
                        p.held = s as usize;
                    }
                }
            }
            // attack: varint entity id; interact: varint entity id, varint hand, LpVec3 offset, bool sneaking
            sb::ATTACK => {
                let target = r.varint()?;
                self.player_attack(eid, target);
            }
            sb::INTERACT => {
                let target = r.varint()?;
                let hand = r.varint().unwrap_or(0);
                self.player_interact(eid, target, hand);
            }
            sb::SWING => {
                let hand = r.varint()?;
                self.broadcast(Some(eid), || {
                    let mut w = Writer::with_id(cb::ANIMATE);
                    w.varint(eid).u8(if hand == 0 { 0 } else { 3 });
                    w
                });
            }
            sb::SET_CREATIVE_MODE_SLOT => {
                let slot = r.i16()?;
                let st = read_slot(&mut r)?;
                if let Some(p) = self.players.get_mut(&eid) {
                    if p.gamemode == 1 && (1..46).contains(&slot) {
                        p.inv[slot as usize] = st;
                    }
                }
            }
            sb::PLAYER_ACTION => {
                let status = r.varint()?;
                let (x, y, z) = r.position()?;
                let _face = r.u8()?;
                let seq = r.varint()?;
                self.dig(eid, status, [x, y, z]);
                if let Some(p) = self.players.get(&eid) {
                    let mut w = Writer::with_id(cb::BLOCK_CHANGED_ACK);
                    w.varint(seq);
                    p.send(w);
                }
            }
            sb::USE_ITEM_ON => {
                let hand = r.varint()?;
                let (x, y, z) = r.position()?;
                let face = r.varint()?;
                let cursor = [r.f32()?, r.f32()?, r.f32()?];
                let _inside = r.bool()?;
                let _border = r.bool()?;
                let seq = r.varint()?;
                if hand == 0 {
                    self.place(eid, [x, y, z], face.clamp(0, 5) as usize, cursor);
                }
                if let Some(p) = self.players.get(&eid) {
                    let mut w = Writer::with_id(cb::BLOCK_CHANGED_ACK);
                    w.varint(seq);
                    p.send(w);
                }
            }
            sb::USE_ITEM => {
                let hand = r.varint()?;
                let seq = r.varint()?;
                let (yaw, pitch) = (r.f32().unwrap_or(0.0), r.f32().unwrap_or(0.0));
                if !self.try_bucket(eid, hand, yaw, pitch)
                    && !self.try_throw(eid, hand, yaw, pitch)
                    && !self.try_draw(eid, hand)
                {
                    self.start_using(eid, hand);
                }
                if let Some(p) = self.players.get(&eid) {
                    let mut w = Writer::with_id(cb::BLOCK_CHANGED_ACK);
                    w.varint(seq);
                    p.send(w);
                }
            }
            sb::CHAT => {
                let msg = r.str()?;
                if msg.len() > 256 || msg.chars().any(|c| c.is_control()) {
                    return Err(anyhow!("illegal chat"));
                }
                let name = self
                    .players
                    .get(&eid)
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                self.system_chat(text::plain(&format!("<{name}> {msg}")));
            }
            sb::COMMAND_SUGGESTION => {
                let id = r.varint()?;
                let text = r.str()?;
                let (start, len, list) = crate::commands::suggest(&text);
                if let Some(p) = self.players.get(&eid) {
                    let mut w = Writer::with_id(cb::COMMAND_SUGGESTIONS);
                    w.varint(id)
                        .varint(start)
                        .varint(len)
                        .varint(list.len() as i32);
                    for s in &list {
                        w.str(s).bool(false);
                    }
                    p.send(w);
                }
            }
            sb::CHAT_COMMAND | sb::CHAT_COMMAND_SIGNED => {
                let cmd = r.str()?;
                self.command(eid, &cmd);
            }
            sb::CLIENT_INFORMATION => {
                let _l = r.str()?;
                let v = r.i8()?;
                if let Some(p) = self.players.get_mut(&eid) {
                    p.view = (v.clamp(2, 32) as i32).min(self.sh.cfg.view_distance as i32);
                }
                self.stream_chunks(eid);
            }
            _ => {}
        }
        Ok(())
    }

    /// Server-side "standing on the floor": a motion-blocking block directly under the feet (any corner of the
    /// 0.6-wide box) with the feet within 1/1000 of a full block top, or of a half-block top (slabs).
    fn standing_on_floor(&self, pos: [f64; 3]) -> bool {
        let below = pos[1] - 0.001;
        let by = below.floor() as i32;
        let frac = pos[1] - by as f64;
        if !(frac >= 0.999 || (frac - 0.5).abs() <= 0.001) {
            return false;
        }
        [(-0.3, -0.3), (0.3, -0.3), (-0.3, 0.3), (0.3, 0.3)]
            .iter()
            .any(|(dx, dz)| {
                basalt_world::props::props(self.world.get_block(
                    (pos[0] + dx).floor() as i32,
                    by,
                    (pos[2] + dz).floor() as i32,
                ))
                .blocks_motion()
            })
    }

    fn dig(&mut self, eid: i32, status: i32, b: [i32; 3]) {
        match status {
            3 | 4 => {
                // drop held stack / one item
                let Some(p) = self.players.get_mut(&eid) else {
                    return;
                };
                let slot = 36 + p.held;
                let s = p.inv[slot];
                if s.count <= 0 || p.dead {
                    return;
                }
                let n = if status == 3 { s.count } else { 1 };
                p.inv[slot].count -= n;
                if p.inv[slot].count <= 0 {
                    p.inv[slot] = Stack::default();
                }
                Self::send_window(p);
                self.throw_item(
                    eid,
                    Stack {
                        item: s.item,
                        count: n,
                        damage: 0,
                        ench: NO_ENCH,
                    },
                    false,
                );
                return;
            }
            5 => {
                if let Some(p) = self.players.get_mut(&eid) {
                    p.using = None;
                }
                self.release_draw(eid);
                return;
            }
            6 => {
                // swap main/off hand
                if let Some(p) = self.players.get_mut(&eid) {
                    let slot = 36 + p.held;
                    p.inv.swap(slot, 45);
                    Self::send_window(p);
                }
                return;
            }
            _ => {}
        }
        let Some(p) = self.players.get(&eid) else {
            return;
        };
        if p.dead {
            return;
        }
        let eye = [p.pos[0], p.pos[1] + 1.62, p.pos[2]];
        let d2: f64 = (0..3).map(|i| (b[i] as f64 + 0.5 - eye[i]).powi(2)).sum();
        if d2 > 8.0 * 8.0 {
            return;
        }
        let creative = p.gamemode == 1;
        if p.gamemode == 2 || p.gamemode == 3 {
            return;
        }
        let old = self.world.get_block(b[0], b[1], b[2]);
        if old == 0 {
            return;
        }
        let held = {
            let s = p.inv[36 + p.held];
            (s.count > 0).then_some(s.item)
        };
        let feet = self.world.get_block(
            p.pos[0].floor() as i32,
            (p.pos[1] + 1.62).floor() as i32,
            p.pos[2].floor() as i32,
        );
        let in_water = basalt_world::props::props(feet).has_fluid();
        // vanilla ignores the client's on_ground flag here: a bot that claims "airborne" while standing on the floor
        // digs at the ground rate, one that is really above the floor digs 5x slower (measured by crates/check --fuzz dig)
        let on_ground = self.standing_on_floor(p.pos);
        let delta = self.data.dig_progress(old, held, on_ground, in_water);
        let breaks = if creative {
            status == 0
        } else {
            match (status, delta) {
                (_, None) => false,
                (0, Some(d)) if d >= 1.0 => true,
                (0, Some(_)) => {
                    self.players.get_mut(&eid).unwrap().dig_start = Some((b, self.tick));
                    false
                }
                (1, _) => {
                    self.players.get_mut(&eid).unwrap().dig_start = None;
                    false
                }
                (2, Some(d)) => {
                    let started = self.players.get_mut(&eid).unwrap().dig_start.take();
                    let ok = matches!(started, Some((sb, t0)) if sb == b && basalt_game::GameData::dig_accepts(d, self.tick - t0));
                    if !ok {
                        let st = old;
                        self.players[&eid].send({
                            let mut w = Writer::with_id(cb::BLOCK_UPDATE);
                            w.position(b[0], b[1], b[2]).varint(st as i32);
                            w
                        });
                        // like vanilla, keep tracking the attempt: the block still breaks once its progress reaches 1.0
                        if let Some((sb, t0)) = started {
                            if sb == b {
                                let p = self.players.get_mut(&eid).unwrap();
                                if p.delayed_dig.is_none() {
                                    p.delayed_dig = Some((b, t0));
                                }
                            }
                        }
                    }
                    ok
                }
                _ => false,
            }
        };
        if !breaks {
            return;
        }
        self.finish_dig(eid, b, old, creative, held);
    }

    /// Dig progress per tick for `eid` on `state` (server-side ground / water), plus the held item.
    fn dig_context(&self, eid: i32, state: u32) -> Option<(Option<i32>, Option<f32>)> {
        let p = self.players.get(&eid)?;
        let held = {
            let s = p.inv[36 + p.held];
            (s.count > 0).then_some(s.item)
        };
        let feet = self.world.get_block(
            p.pos[0].floor() as i32,
            (p.pos[1] + 1.62).floor() as i32,
            p.pos[2].floor() as i32,
        );
        let in_water = basalt_world::props::props(feet).has_fluid();
        Some((
            held,
            self.data
                .dig_progress(state, held, self.standing_on_floor(p.pos), in_water),
        ))
    }

    /// Complete vanilla's delayed destroys: a stop the server rejected (too early) is still tracked from the
    /// start tick and the block breaks by itself once `progress * (ticks + 1) >= 1`.
    fn tick_delayed_dig(&mut self) {
        let pending: Vec<(i32, [i32; 3], u64)> = self
            .players
            .values()
            .filter_map(|p| p.delayed_dig.map(|(b, t0)| (p.eid, b, t0)))
            .collect();
        for (eid, b, t0) in pending {
            let old = self.world.get_block(b[0], b[1], b[2]);
            let done = match self.dig_context(eid, old) {
                _ if old == 0 => true,
                Some((held, Some(d))) if ((self.tick - t0 + 1) as f32) * d >= 1.0 => {
                    self.finish_dig(eid, b, old, false, held);
                    true
                }
                Some((_, Some(_))) => false,
                _ => true,
            };
            if done {
                if let Some(p) = self.players.get_mut(&eid) {
                    p.delayed_dig = None;
                }
            }
        }
    }

    /// Remove a block a player broke (accepted stop, instant break or a delayed destroy completing): particles,
    /// tool wear, drops, the other half of a door.
    fn finish_dig(&mut self, eid: i32, b: [i32; 3], old: u32, creative: bool, held: Option<i32>) {
        self.container_broken(b, old);
        let below = self.world.get_block(b[0], b[1] - 1, b[2]);
        let above = self.world.get_block(b[0], b[1] + 1, b[2]);
        let os = BlockState(old);
        // creative: breaking the upper half of a double block takes the lower half away first, without a drop
        if creative
            && os.get("half") == Some("upper")
            && os.name() != "tall_seagrass"
            && BlockState(below).block().name == os.block().name
            && BlockState(below).get("half") == Some("lower")
        {
            self.set_block(b[0], b[1] - 1, b[2], Self::empty_state(BlockState(below)));
        }
        self.set_block(b[0], b[1], b[2], Self::empty_state(os));
        // world event 2001: block break particles + sound for others
        self.broadcast_chunk((b[0] >> 4, b[2] >> 4), Some(eid), || {
            let mut w = Writer::with_id(cb::LEVEL_EVENT);
            w.i32(2001)
                .position(b[0], b[1], b[2])
                .i32(old as i32)
                .bool(false);
            w
        });
        if !creative {
            if let Some(p) = self.players.get_mut(&eid) {
                p.exhaustion += 0.005;
            }
            if basalt_game::hardness(os.name()) > 0.0 {
                self.wear_held(eid, 1);
            }
            let harvest = self.data.can_harvest(old, held);
            let drops = if harvest {
                self.data
                    .block_drops_around(old, held, below, above, &mut self.rng)
            } else {
                Vec::new()
            };
            self.drop_at_block(b, drops);
            if harvest {
                let xp = xporb::ore_xp(old, &mut self.rng);
                if xp > 0 {
                    self.spawn_xp(
                        [b[0] as f64 + 0.5, b[1] as f64 + 0.5, b[2] as f64 + 0.5],
                        xp,
                    );
                }
            }
        }
        // whatever depended on this block (plants, torches, the other half of a door or tall plant) pops or goes
        self.update_neighbors(b, 0);
    }

    /// Use up durability of the held tool (swords take 2 for blocks); breaks it at max damage.
    fn wear_held(&mut self, eid: i32, amount: i32) {
        let Some(p) = self.players.get(&eid) else {
            return;
        };
        let slot = 36 + p.held;
        let s = p.inv[slot];
        if s.count <= 0 {
            return;
        }
        let Some(info) = self.data.item(s.item) else {
            return;
        };
        if info.max_damage <= 0 {
            return;
        }
        let amount = if info.name.ends_with("_sword") {
            amount * 2
        } else {
            amount
        };
        self.wear_held_exact(eid, amount);
    }

    /// Add `amount` damage to the held item (no tool-type scaling); breaks it at max damage.
    fn wear_held_exact(&mut self, eid: i32, amount: i32) {
        let Some(p) = self.players.get(&eid) else {
            return;
        };
        let slot = 36 + p.held;
        let s = p.inv[slot];
        if s.count <= 0 || p.gamemode == 1 {
            return;
        }
        let Some(info) = self.data.item(s.item) else {
            return;
        };
        if info.max_damage <= 0 {
            return;
        }
        let p = self.players.get_mut(&eid).unwrap();
        p.inv[slot].damage += amount;
        if p.inv[slot].damage >= info.max_damage {
            p.inv[slot] = Stack::default();
            // entity event 47: main-hand item broke (sound + particles)
            let mut w = Writer::with_id(cb::ENTITY_EVENT);
            w.i32(eid).u8(47);
            p.send(w);
        }
        Self::send_window(self.players.get_mut(&eid).unwrap());
    }

    fn place(&mut self, eid: i32, b: [i32; 3], face: usize, cursor: [f32; 3]) {
        let Some(p) = self.players.get(&eid) else {
            return;
        };
        if p.dead {
            return;
        }
        let clicked_name = BlockState(self.world.get_block(b[0], b[1], b[2])).name();
        if clicked_name == "crafting_table" && !p.sneaking && p.gamemode != 3 {
            self.open_crafting_table(eid);
            return;
        }
        if let Some(kind) = container::Kind::station_of_block(clicked_name) {
            if p.gamemode != 3 && !(p.sneaking && p.inv[36 + p.held].count > 0) {
                let eye = [p.pos[0], p.pos[1] + 1.62, p.pos[2]];
                let d2: f64 = (0..3)
                    .map(|i| {
                        let lo = b[i] as f64;
                        let c = eye[i].clamp(lo, lo + 1.0);
                        (eye[i] - c).powi(2)
                    })
                    .sum();
                // an ender chest under a solid block stays shut, like a chest
                if d2 <= 5.5 * 5.5 && !(kind == container::Kind::Ender && self.chest_blocked(b)) {
                    self.open_station(eid, b, kind);
                }
                return;
            }
        }
        if container::Kind::of_block(clicked_name).is_some()
            && p.gamemode != 3
            && !(p.sneaking && p.inv[36 + p.held].count > 0)
        {
            // reach: distance from the eye to the block's box <= 4.5 + 1.0 (vanilla's use-on tolerance)
            let eye = [p.pos[0], p.pos[1] + 1.62, p.pos[2]];
            let d2: f64 = (0..3)
                .map(|i| {
                    let lo = b[i] as f64;
                    let c = eye[i].clamp(lo, lo + 1.0);
                    (eye[i] - c).powi(2)
                })
                .sum();
            if d2 <= 5.5 * 5.5 {
                self.open_container(eid, b);
            }
            return;
        }
        let p = &self.players[&eid];
        // doors / trapdoors / fence gates: a plain click toggles them (sneaking with an item in hand places instead)
        if p.gamemode != 3 && !(p.sneaking && p.inv[36 + p.held].count > 0) && self.use_block(b) {
            return;
        }
        let p = &self.players[&eid];
        if p.gamemode == 3 || p.gamemode == 2 {
            return;
        }
        let slot = 36 + p.held;
        let st = p.inv[slot];
        if st.count <= 0 {
            return;
        }
        let Some(item_name) = reg::ITEM.get(st.item as usize) else {
            return;
        };
        if *item_name == "flint_and_steel" {
            self.use_flint_and_steel(eid, b, face);
            return;
        }
        let item_name: &'static str = item_name;
        if self.use_tool_on(eid, b, face, item_name) {
            return;
        }
        let Some(state) = block_default(item_name).or_else(|| block_for_item(item_name)) else {
            return;
        };
        let p = &self.players[&eid];
        let (yaw, pitch, sneaking) = (p.yaw, p.pitch, p.sneaking);
        let clicked = BlockState(self.world.get_block(b[0], b[1], b[2]));
        let replacing = self.replaceable_for(clicked) && clicked.name() != "water";
        let click = place::Click {
            face,
            cursor,
            yaw,
            pitch,
            replacing_clicked: replacing,
        };
        let merging = clicked.get("type").is_some()
            && clicked.block().kind.ends_with("slab")
            && self
                .placement(state, b, &click)
                .is_some_and(|v| BlockState(v[0].1).get("type") == Some("double"));
        let t = if replacing || merging {
            b
        } else {
            [
                b[0] + FACE[face][0],
                b[1] + FACE[face][1],
                b[2] + FACE[face][2],
            ]
        };
        if !merging && !self.replaceable_for(BlockState(self.world.get_block(t[0], t[1], t[2]))) {
            return;
        }
        if !self.world.layout.contains_y(t[1]) {
            return;
        }
        let Some(mut blocks) = self.placement(state, t, &click) else {
            return;
        };
        // don't place a colliding block inside a player
        if basalt_world::props::props(blocks[0].1).blocks_motion() {
            for o in self.players.values() {
                let (lo, hi) = (
                    [o.pos[0] - 0.3, o.pos[1], o.pos[2] - 0.3],
                    [o.pos[0] + 0.3, o.pos[1] + 1.8, o.pos[2] + 0.3],
                );
                if (0..3).all(|i| (t[i] as f64) < hi[i] && (t[i] as f64 + 1.0) > lo[i]) {
                    return;
                }
            }
        }
        if matches!(BlockState(state).name(), "chest" | "trapped_chest") {
            blocks[0].1 = self.chest_placed(t, blocks[0].1, sneaking, face);
        }
        for (q, s) in &blocks {
            self.set_block(q[0], q[1], q[2], *s);
        }
        // beds are placed without neighbour shape updates (measured: a floating snow layer beside a new bed survives)
        if BlockState(state).block().kind != "bed" {
            for (q, _) in &blocks {
                self.update_neighbors(*q, 0);
            }
        }
        let p = self.players.get_mut(&eid).unwrap();
        if p.gamemode != 1 {
            if item_name == "powder_snow_bucket" {
                // the bucket is kept (measured: scenario buckets)
                if let Some(id) = basalt_data::registry_id(reg::ITEM, "bucket") {
                    p.inv[slot] = Stack {
                        item: id,
                        count: 1,
                        ..Stack::default()
                    };
                }
            } else {
                p.inv[slot].count -= 1;
                if p.inv[slot].count <= 0 {
                    p.inv[slot] = Stack::default();
                }
            }
            Self::send_window(p);
        }
    }

    // ------------------------------------------------------------ tick

    fn tick(&mut self) {
        self.tick += 1;
        self.day_time += 1;
        for d in 0..self.parked.len() {
            if !self.dim_active(d) {
                continue;
            }
            self.enter(d);
            self.tick_dim();
        }
        self.enter(dim::OVERWORLD);
        // keepalive (every dimension)
        let mut kick = Vec::new();
        for d in 0..self.parked.len() {
            self.enter(d);
            for p in self.players.values_mut() {
                if let Some((_, t)) = p.keepalive {
                    if t.elapsed() > Duration::from_secs(30) {
                        kick.push(p.eid);
                        continue;
                    }
                }
                if p.keepalive.is_none() && p.last_keepalive.elapsed() > Duration::from_secs(15) {
                    let v = self.tick as i64;
                    p.keepalive = Some((v, Instant::now()));
                    p.last_keepalive = Instant::now();
                    let mut w = Writer::with_id(cb::KEEP_ALIVE);
                    w.i64(v);
                    p.send(w);
                }
            }
        }
        for eid in kick {
            if self.enter_player(eid) {
                if let Some(p) = self.players.get(&eid) {
                    let mut w = Writer::with_id(cb::DISCONNECT);
                    w.nbt(&text::plain("Timed out"));
                    p.send(w);
                    let _ = p.out.send(Vec::new());
                }
            }
            self.leave(eid);
        }
        self.enter(dim::OVERWORLD);
        if self.tick % 20 == 0 {
            let w = self.set_time_packet();
            self.broadcast_all(None, || Writer { buf: w.buf.clone() });
        }
        if self.tick % (20 * 60) == 0 {
            self.save_level();
        }
    }

    /// One tick of the current dimension.
    fn tick_dim(&mut self) {
        for c in self.gen.finished() {
            let (cx, cz) = (c.x, c.z);
            let fresh = self.world.chunk(cx, cz).is_none();
            self.add_generated(c);
            if fresh {
                self.populate_animals(cx, cz);
            }
        }
        self.tick_items();
        self.tick_mobs();
        self.tick_arrows();
        self.tick_projs();
        self.tick_cooldowns();
        self.tick_crossbows();
        self.tick_falling();
        self.tick_fluids();
        self.tick_orbs();
        self.tick_random();
        self.tick_spawning();
        self.broadcast_mob_moves();
        if self.tick % 10 == 5 {
            self.sync_viewers();
        }
        self.tick_survival();
        self.tick_delayed_dig();
        self.tick_environment();
        self.tick_containers();
        if self.cur == dim::OVERWORLD {
            self.tick_weather();
        }
        self.tick_portals();
        let ids: Vec<i32> = self.players.keys().copied().collect();
        for &eid in &ids {
            self.stream_chunks(eid);
        }
        // movement broadcast
        let mut moves = Vec::new();
        for p in self.players.values_mut() {
            if p.pos != p.last_sent_pos.0
                || p.yaw != p.last_sent_pos.1
                || p.pitch != p.last_sent_pos.2
            {
                p.last_sent_pos = (p.pos, p.yaw, p.pitch);
                moves.push((p.eid, Self::sync_entity(p), p.yaw));
            }
        }
        for (eid, w, yaw) in moves {
            for o in self.players.values() {
                if o.eid != eid {
                    o.send(Writer { buf: w.buf.clone() });
                    let mut h = Writer::with_id(cb::ROTATE_HEAD);
                    h.varint(eid).u8(angle(yaw));
                    o.send(h);
                }
            }
        }
        // unload chunks nobody can see
        // (in-memory worlds keep everything: an unloaded chunk would be regenerated from scratch)
        if self.tick % 100 == 0 && self.world.dir.is_some() {
            let seen: HashSet<(i32, i32)> = self
                .players
                .values()
                .flat_map(|p| p.loaded.iter().copied())
                .collect();
            let keep_spawn = self.cur == dim::OVERWORLD;
            for c in self.world.loaded_chunks() {
                if !seen.contains(&c) && (!keep_spawn || c.0.abs() > 2 || c.1.abs() > 2) {
                    let _ = self.world.unload(c.0, c.1);
                    self.be_drop_chunk(c.0, c.1);
                }
            }
        }
    }

    pub fn run(mut self, rx: mpsc::Receiver<ToGame>, stop: Arc<std::sync::atomic::AtomicBool>) {
        let mut next = Instant::now();
        loop {
            if stop.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let deadline = next + Duration::from_millis(50);
            loop {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                match rx.recv_timeout(deadline - now) {
                    Ok(ToGame::Join {
                        eid,
                        uuid,
                        name,
                        out,
                        view,
                    }) => self.join(eid, uuid, name, out, view),
                    Ok(ToGame::Packet { eid, id, data }) => {
                        self.enter_player(eid);
                        if let Err(e) = self.packet(eid, id, &data) {
                            log::warn!("bad packet {} from {eid}: {e:#}", sb::name(id));
                            if let Some(p) = self.players.get(&eid) {
                                let mut w = Writer::with_id(cb::DISCONNECT);
                                w.nbt(&text::plain(&format!("Bad packet: {e}")));
                                p.send(w);
                                let _ = p.out.send(Vec::new());
                            }
                        }
                    }
                    Ok(ToGame::Leave { eid }) => self.leave(eid),
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
            next = deadline;
            if Instant::now() > next + Duration::from_secs(2) {
                log::warn!("can't keep up; skipping ticks");
                next = Instant::now();
            }
            self.tick();
        }
        log::info!("saving world");
        for p in self.all_players() {
            let mut w = Writer::with_id(cb::DISCONNECT);
            w.nbt(&text::plain("Server closed"));
            p.send(w);
        }
        self.save_level();
    }
}

/// Data component type id of `minecraft:damage`.
fn damage_component() -> i32 {
    registry_id(reg::DATA_COMPONENT_TYPE, "damage").unwrap_or(3)
}
fn ench_component() -> i32 {
    registry_id(reg::DATA_COMPONENT_TYPE, "enchantments").unwrap_or(13)
}

fn comp_id(name: &str) -> i32 {
    registry_id(reg::DATA_COMPONENT_TYPE, name).unwrap_or(-1)
}

/// Extra stack components as JSON for player files (None when there are none).
fn stackx_json(e: &stackx::Ench) -> serde_json::Value {
    if !e.has_extras() && !e.stored {
        return serde_json::Value::Null;
    }
    serde_json::json!({ "r": e.repair, "n": e.name_str(), "s": e.stored, "p": e.potion, "tm": e.trim_mat, "tp": e.trim_pat })
}
fn stackx_from_json(e: &mut stackx::Ench, v: &serde_json::Value) {
    if v.is_null() {
        return;
    }
    e.repair = v["r"].as_i64().unwrap_or(0) as i32;
    e.name = v["n"].as_str().map(stackx::intern).unwrap_or(0);
    e.stored = v["s"].as_bool().unwrap_or(false);
    e.potion = v["p"].as_u64().unwrap_or(0) as u8;
    e.trim_mat = v["tm"].as_u64().unwrap_or(0) as u8;
    e.trim_pat = v["tp"].as_u64().unwrap_or(0) as u8;
}

pub(crate) fn write_slot(w: &mut Writer, s: Stack) {
    if s.count <= 0 {
        w.varint(0);
        return;
    }
    w.varint(s.count).varint(s.item);
    let ench: Vec<(u16, u8)> = s.ench.list();
    let x = &s.ench;
    let n = (s.damage > 0) as i32
        + (!ench.is_empty()) as i32
        + (x.repair != 0) as i32
        + (x.name != 0) as i32
        + (x.potion != 0) as i32
        + (x.trim_mat != 0) as i32
        + (x.charged != 0) as i32;
    w.varint(n).varint(0);
    if s.damage > 0 {
        w.varint(damage_component()).varint(s.damage);
    }
    if !ench.is_empty() {
        w.varint(if x.stored {
            comp_id("stored_enchantments")
        } else {
            ench_component()
        })
        .varint(ench.len() as i32);
        for (e, l) in ench {
            w.varint(e as i32).varint(l as i32);
        }
    }
    if x.repair != 0 {
        w.varint(comp_id("repair_cost")).varint(x.repair);
    }
    if let Some(name) = x.name_str() {
        w.varint(comp_id("custom_name")).nbt(&text::plain(&name));
    }
    // potion_contents: Optional<potion holder> (bool + registry id), custom colour (bool), custom effects (varint count), custom name (bool)
    if x.potion != 0 {
        w.varint(comp_id("potion_contents"))
            .bool(true)
            .varint(x.potion as i32 - 1)
            .bool(false)
            .varint(0)
            .bool(false);
    }
    // trim: material holder (id + 1), pattern holder (id + 1)
    if x.trim_mat != 0 {
        w.varint(comp_id("trim"))
            .varint(x.trim_mat as i32)
            .varint(x.trim_pat as i32);
    }
    // charged_projectiles: list of item stacks
    if x.charged != 0 {
        w.varint(comp_id("charged_projectiles"))
            .varint(x.charged_n.max(1) as i32);
        for _ in 0..x.charged_n.max(1) {
            write_slot(
                w,
                Stack {
                    item: x.charged as i32 - 1,
                    count: 1,
                    ..Stack::default()
                },
            );
        }
    }
}

/// Creative / untrusted slot: count, item, then length-prefixed component patches (ignored).
fn read_slot(r: &mut Reader) -> Result<Stack> {
    let count = r.varint()?;
    if count <= 0 {
        return Ok(Stack::default());
    }
    let item = r.varint()?;
    let (add, rem) = (r.varint()?, r.varint()?);
    let mut damage = 0;
    let mut ench = NO_ENCH;
    for _ in 0..add {
        let ty = r.varint()?;
        let len = r.varint()?;
        let body = r.take(len.max(0) as usize)?;
        if ty == damage_component() {
            damage = Reader::new(body).varint().unwrap_or(0).max(0);
        }
        if ty == ench_component() {
            let mut b = Reader::new(body);
            let n = b.varint().unwrap_or(0);
            for i in 0..n.clamp(0, stackx::MAX_ENCH as i32) as usize {
                if let (Ok(e), Ok(l)) = (b.varint(), b.varint()) {
                    ench[i] = (e as u16, l.clamp(0, 255) as u8);
                }
            }
        }
    }
    for _ in 0..rem {
        r.varint()?;
    }
    Ok(Stack {
        item,
        count: count.min(99),
        damage,
        ench,
    })
}

/// `stone`, `water[level=3]`, `oak_log[axis=x]` → block state id.
/// Items whose block has a different name.
fn block_for_item(item: &str) -> Option<u32> {
    let b = match item {
        "redstone" => "redstone_wire",
        "string" => "tripwire",
        "wheat_seeds" => "wheat",
        "carrot" => "carrots",
        "potato" => "potatoes",
        "beetroot_seeds" => "beetroots",
        "melon_seeds" => "melon_stem",
        "pumpkin_seeds" => "pumpkin_stem",
        "sweet_berries" => "sweet_berry_bush",
        "glow_berries" => "cave_vines",
        "cocoa_beans" => "cocoa",
        "water_bucket" => "water",
        "lava_bucket" => "lava",
        "powder_snow_bucket" => "powder_snow",
        _ => return None,
    };
    block_default(b)
}

/// Minimal brigadier tree: each command is a literal with an optional greedy string argument.
fn commands_packet() -> Writer {
    let cmds = [
        "gamemode", "tp", "teleport", "give", "time", "say", "list", "spawn", "save", "save-all",
        "help", "effect", "enchant", "setblock", "fill", "kill", "summon", "mobs",
    ];
    let greedy = registry_id(reg::COMMAND_ARGUMENT_TYPE, "brigadier:string").unwrap_or(5);
    let n = 1 + cmds.len() * 2;
    let mut w = Writer::with_id(cb::COMMANDS);
    w.varint(n as i32);
    // root
    w.u8(0).varint(cmds.len() as i32);
    for i in 0..cmds.len() {
        w.varint(1 + i as i32 * 2);
    }
    for (i, c) in cmds.iter().enumerate() {
        // literal (executable) with one child
        w.u8(0x01 | 0x04).varint(1).varint(2 + i as i32 * 2).str(c);
        // argument "args": greedy string, executable
        w.u8(0x02 | 0x04)
            .varint(0)
            .str("args")
            .varint(greedy)
            .varint(2);
    }
    w.varint(0);
    w
}

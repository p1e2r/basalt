//! Mobs: kinds and their stats, spawn packets and metadata, block-collision physics,
//! movement broadcasting and persistence.
//!
//! Stats are public Minecraft Wiki values. Wire formats were measured against vanilla 26.2 (see
//! NOTES.md "Entities").
use super::Game;
use basalt_data::{registries as reg, registry_id, BlockState};
use basalt_protocol::{ids::play::clientbound as cb, Writer};
use basalt_world::{props, World};
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum MobKind { Zombie, Skeleton, Creeper, Spider, Cow, Pig, Sheep, Chicken }

pub const ALL_KINDS: [MobKind; 8] = [MobKind::Zombie, MobKind::Skeleton, MobKind::Creeper, MobKind::Spider, MobKind::Cow, MobKind::Pig, MobKind::Sheep, MobKind::Chicken];

pub struct Stats {
    pub width: f64,
    pub height: f64,
    pub health: f32,
    /// `movement_speed` attribute.
    pub speed: f64,
    /// Melee damage on Normal difficulty (0 = no melee).
    pub attack: f32,
    pub follow_range: f64,
    pub eye: f64,
}

impl MobKind {
    pub fn name(self) -> &'static str {
        match self { MobKind::Zombie => "zombie", MobKind::Skeleton => "skeleton", MobKind::Creeper => "creeper", MobKind::Spider => "spider",
            MobKind::Cow => "cow", MobKind::Pig => "pig", MobKind::Sheep => "sheep", MobKind::Chicken => "chicken" }
    }
    pub fn from_name(n: &str) -> Option<MobKind> { let n = n.trim_start_matches("minecraft:"); ALL_KINDS.into_iter().find(|k| k.name() == n) }
    pub fn display(self) -> &'static str {
        match self { MobKind::Zombie => "Zombie", MobKind::Skeleton => "Skeleton", MobKind::Creeper => "Creeper", MobKind::Spider => "Spider",
            MobKind::Cow => "Cow", MobKind::Pig => "Pig", MobKind::Sheep => "Sheep", MobKind::Chicken => "Chicken" }
    }
    pub fn stats(self) -> Stats {
        let s = |width, height, health, speed, attack, follow_range, eye| Stats { width, height, health, speed, attack, follow_range, eye };
        match self {
            MobKind::Zombie => s(0.6, 1.95, 20.0, 0.23, 3.0, 35.0, 1.74),
            MobKind::Skeleton => s(0.6, 1.99, 20.0, 0.25, 2.0, 16.0, 1.74),
            MobKind::Creeper => s(0.6, 1.7, 20.0, 0.25, 0.0, 16.0, 1.445),
            MobKind::Spider => s(1.4, 0.9, 16.0, 0.3, 2.0, 16.0, 0.65),
            MobKind::Cow => s(0.9, 1.4, 10.0, 0.2, 0.0, 16.0, 1.3),
            MobKind::Pig => s(0.9, 0.9, 10.0, 0.25, 0.0, 16.0, 0.765),
            MobKind::Sheep => s(0.9, 1.3, 8.0, 0.23, 0.0, 16.0, 1.235),
            MobKind::Chicken => s(0.4, 0.7, 4.0, 0.25, 0.0, 16.0, 0.644),
        }
    }
    pub fn hostile(self) -> bool { matches!(self, MobKind::Zombie | MobKind::Skeleton | MobKind::Creeper | MobKind::Spider) }
    pub fn undead(self) -> bool { matches!(self, MobKind::Zombie | MobKind::Skeleton) }
    pub fn can_be_baby(self) -> bool { matches!(self, MobKind::Zombie | MobKind::Cow | MobKind::Pig | MobKind::Sheep | MobKind::Chicken) }
    /// Items that tempt this animal (and breed it).
    pub fn tempt_items(self) -> &'static [&'static str] {
        match self {
            MobKind::Cow | MobKind::Sheep => &["wheat"],
            MobKind::Pig => &["carrot", "potato", "beetroot", "carrot_on_a_stick"],
            MobKind::Chicken => &["wheat_seeds", "melon_seeds", "pumpkin_seeds", "beetroot_seeds", "torchflower_seeds", "pitcher_pod"],
            _ => &[],
        }
    }
    pub fn entity_type(self) -> i32 { registry_id(reg::ENTITY_TYPE, self.name()).unwrap_or(0) }
    pub fn sound(self, what: &str) -> String { format!("entity.{}.{what}", self.name()) }
    /// Sound category: 5 hostile, 6 neutral (as measured).
    pub fn sound_category(self) -> i32 { if self.hostile() { 5 } else { 6 } }
}

#[derive(Default, Clone)]
pub struct Ai {
    pub target: Option<i32>,
    pub path: Vec<[i32; 3]>,
    pub path_goal: Option<[i32; 3]>,
    pub repath_in: u32,
    pub idle_ticks: u32,
    pub panic_ticks: u32,
    pub attack_cooldown: u32,
    /// Creeper swell (0..=30) and direction (-1 idle, 1 swelling).
    pub fuse: i32,
    pub swell_dir: i32,
    pub ignited: bool,
    /// Skeleton bow draw progress (ticks), None = not drawing.
    pub draw: Option<u32>,
    pub look_at: Option<i32>,
    pub look_ticks: u32,
    pub tempted_by: Option<i32>,
    pub last_pos: [f64; 3],
    pub aggressive: bool,
    pub leap_cooldown: u32,
    pub panic_target: Option<[i32; 3]>,
    pub wander: Option<[i32; 3]>,
}

pub struct Mob {
    pub eid: i32,
    pub uuid: Uuid,
    pub kind: MobKind,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub head_yaw: f32,
    pub health: f32,
    pub on_ground: bool,
    pub h_collision: bool,
    pub in_water: bool,
    pub baby: bool,
    pub sheep_color: u8,
    pub sheared: bool,
    pub no_ai: bool,
    pub persistent: bool,
    pub age: u64,
    pub hurt_cooldown: u32,
    pub fire_ticks: i32,
    pub fall_distance: f64,
    /// Ticks since death (Some while playing the death animation).
    pub dead: Option<u32>,
    /// (player eid, tick) of the last player hit — for killed_by_player loot and XP.
    pub last_player_hit: Option<(i32, u64)>,
    pub ai: Ai,
    pub egg_timer: u32,
    pub ambient_timer: i32,
    pub step_dist: f64,
    // what viewers last saw
    pub sent_pos: [f64; 3],
    pub sent_rot: (u8, u8, u8),
    pub sent_fire: bool,
    pub since_sync: u32,
}

pub const SHEEP_COLORS: [&str; 16] = ["white", "orange", "magenta", "light_blue", "yellow", "lime", "pink", "gray", "light_gray", "cyan", "purple", "blue", "brown", "green", "red", "black"];

pub fn angle(deg: f32) -> u8 { ((deg / 360.0 * 256.0).round() as i32 & 0xFF) as u8 }

impl Mob {
    pub fn new(eid: i32, kind: MobKind, pos: [f64; 3], yaw: f32) -> Mob {
        Mob { eid, uuid: Uuid::new_v4(), kind, pos, vel: [0.0; 3], yaw, pitch: 0.0, head_yaw: yaw, health: kind.stats().health, on_ground: false,
            h_collision: false, in_water: false, baby: false, sheep_color: 0, sheared: false, no_ai: false, persistent: false, age: 0, hurt_cooldown: 0,
            fire_ticks: 0, fall_distance: 0.0, dead: None, last_player_hit: None, ai: Ai { swell_dir: -1, ..Default::default() },
            egg_timer: 6000, ambient_timer: 0, step_dist: 0.0, sent_pos: pos, sent_rot: (angle(yaw), 0, angle(yaw)), sent_fire: false, since_sync: 0 }
    }
    pub fn scale(&self) -> f64 { if self.baby { 0.5 } else { 1.0 } }
    pub fn width(&self) -> f64 { self.kind.stats().width * self.scale() }
    pub fn height(&self) -> f64 { self.kind.stats().height * self.scale() }
    pub fn eye_y(&self) -> f64 { self.pos[1] + self.kind.stats().eye * self.scale() }
    pub fn max_health(&self) -> f32 { self.kind.stats().health }
    /// Effective movement speed attribute (baby zombies +50%).
    pub fn speed(&self) -> f64 { self.kind.stats().speed * if self.baby && self.kind == MobKind::Zombie { 1.5 } else { 1.0 } }
    pub fn chunk(&self) -> (i32, i32) { ((self.pos[0].floor() as i32) >> 4, (self.pos[2].floor() as i32) >> 4) }
    pub fn alive(&self) -> bool { self.dead.is_none() }
    pub fn aabb(&self) -> [f64; 6] { let w = self.width() / 2.0; [self.pos[0] - w, self.pos[1], self.pos[2] - w, self.pos[0] + w, self.pos[1] + self.height(), self.pos[2] + w] }

    /// add_entity: eid, uuid, type, xyz, LpVec3 velocity, pitch, yaw, head yaw, varint data.
    pub fn add_packet(&self) -> Writer {
        let mut w = Writer::with_id(cb::ADD_ENTITY);
        w.varint(self.eid).uuid(self.uuid).varint(self.kind.entity_type()).f64(self.pos[0]).f64(self.pos[1]).f64(self.pos[2]);
        write_lpvec3(&mut w, self.vel);
        w.u8(angle(self.pitch)).u8(angle(self.yaw)).u8(angle(self.head_yaw)).varint(0);
        w
    }

    /// Full metadata as vanilla sends it on spawn (only non-default values).
    pub fn data_packet(&self) -> Writer {
        let mut w = Writer::with_id(cb::SET_ENTITY_DATA); w.varint(self.eid);
        if self.fire_ticks > 0 { w.u8(0).varint(0).u8(0x01); }
        w.u8(9).varint(3).f32(self.health);
        let flags = self.no_ai as u8 | (self.ai.aggressive as u8) << 2;
        if flags != 0 { w.u8(15).varint(0).u8(flags); }
        if self.baby { w.u8(16).varint(8).bool(true); }
        match self.kind {
            MobKind::Sheep => { w.u8(18).varint(0).u8(self.sheep_color & 15 | (self.sheared as u8) << 4); }
            MobKind::Creeper => {
                if self.ai.swell_dir != -1 { w.u8(16).varint(1).varint(self.ai.swell_dir); }
                if self.ai.ignited { w.u8(18).varint(8).bool(true); }
            }
            MobKind::Skeleton => { if self.ai.draw.is_some() { w.u8(8).varint(0).u8(1); } }
            _ => {}
        }
        if self.dead.is_some() { w.u8(6).varint(20).varint(7); }
        w.u8(0xff);
        w
    }

    /// update_attributes: movement_speed (the only attribute vanilla syncs for these mobs);
    /// baby zombies carry the `minecraft:baby` +50% multiplier as in the capture.
    pub fn attributes_packet(&self) -> Writer {
        let mut w = Writer::with_id(cb::UPDATE_ATTRIBUTES);
        w.varint(self.eid).varint(1).varint(registry_id(reg::ATTRIBUTE, "movement_speed").unwrap_or(0)).f64(self.kind.stats().speed);
        if self.baby && self.kind == MobKind::Zombie { w.varint(1).str("minecraft:baby").f64(0.5).u8(1); } else { w.varint(0); }
        w
    }

    /// set_equipment for mobs that hold something (skeleton: bow in main hand).
    pub fn equipment_packet(&self) -> Option<Writer> {
        if self.kind != MobKind::Skeleton { return None; }
        let bow = registry_id(reg::ITEM, "bow")?;
        let mut w = Writer::with_id(cb::SET_EQUIPMENT); w.varint(self.eid).u8(0).varint(1).varint(bow).varint(0).varint(0);
        Some(w)
    }

    pub fn spawn_packets(&self) -> Vec<Writer> {
        let mut v = vec![Writer::with_id(cb::BUNDLE_DELIMITER), self.add_packet(), self.data_packet(), self.attributes_packet()];
        if let Some(e) = self.equipment_packet() { v.push(e); }
        v.push(Writer::with_id(cb::BUNDLE_DELIMITER));
        v
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({ "kind": self.kind.name(), "uuid": self.uuid.to_string(), "pos": self.pos, "yaw": self.yaw, "health": self.health,
            "baby": self.baby, "color": self.sheep_color, "sheared": self.sheared, "persistent": self.persistent, "no_ai": self.no_ai, "age": self.age })
    }
    pub fn from_json(eid: i32, v: &serde_json::Value) -> Option<Mob> {
        let kind = MobKind::from_name(v["kind"].as_str()?)?;
        let a = v["pos"].as_array()?;
        let pos = [a.first()?.as_f64()?, a.get(1)?.as_f64()?, a.get(2)?.as_f64()?];
        let mut m = Mob::new(eid, kind, pos, v["yaw"].as_f64().unwrap_or(0.0) as f32);
        if let Some(u) = v["uuid"].as_str().and_then(|u| Uuid::parse_str(u).ok()) { m.uuid = u; }
        m.health = v["health"].as_f64().map(|h| h as f32).unwrap_or(m.health).clamp(1.0, m.max_health());
        m.baby = v["baby"].as_bool().unwrap_or(false);
        m.sheep_color = v["color"].as_u64().unwrap_or(0) as u8 & 15;
        m.sheared = v["sheared"].as_bool().unwrap_or(false);
        m.persistent = v["persistent"].as_bool().unwrap_or(false);
        m.no_ai = v["no_ai"].as_bool().unwrap_or(false);
        m.age = v["age"].as_u64().unwrap_or(0);
        Some(m)
    }
}

/// LpVec3 velocity (Writer::lpvec3; format in NOTES.md).
pub fn write_lpvec3(w: &mut Writer, v: [f64; 3]) { w.lpvec3(v); }

// ------------------------------------------------------------------ block collision

/// Collision boxes of a block state in block-local coordinates (approximate shapes: full cubes,
/// slabs, 1.5-high fences/walls, a few thin blocks).
pub fn block_boxes(state: u32, out: &mut Vec<[f64; 6]>) {
    out.clear();
    let p = props::props(state);
    if !p.blocks_motion() { return; }
    let st = BlockState(state);
    let b = st.block();
    match b.kind {
        "slab" | "weathering_copper_slab" => match st.get("type") {
            Some("top") => out.push([0.0, 0.5, 0.0, 1.0, 1.0, 1.0]),
            Some("bottom") => out.push([0.0, 0.0, 0.0, 1.0, 0.5, 1.0]),
            _ => out.push([0.0, 0.0, 0.0, 1.0, 1.0, 1.0]),
        },
        "fence" | "wall" => out.push([0.25, 0.0, 0.25, 0.75, 1.5, 0.75]),
        "fence_gate" => { if st.get("open") != Some("true") { out.push([0.0, 0.0, 0.0, 1.0, 1.5, 1.0]); } }
        "carpet" | "wool_carpet" => out.push([0.0, 0.0, 0.0, 1.0, 0.0625, 1.0]),
        "snow_layer" => { let l: f64 = st.get("layers").and_then(|v| v.parse().ok()).unwrap_or(1.0); out.push([0.0, 0.0, 0.0, 1.0, (l - 1.0) * 0.125, 1.0]); }
        "dirt_path" | "farm" | "farmland" => out.push([0.0, 0.0, 0.0, 1.0, 0.9375, 1.0]),
        "bed" => out.push([0.0, 0.0, 0.0, 1.0, 0.5625, 1.0]),
        "door" | "trapdoor" | "pressure_plate" | "button" => {
            if b.kind == "trapdoor" && st.get("open") != Some("true") {
                if st.get("half") == Some("top") { out.push([0.0, 0.8125, 0.0, 1.0, 1.0, 1.0]); } else { out.push([0.0, 0.0, 0.0, 1.0, 0.1875, 1.0]); }
            } else if b.kind == "door" && st.get("open") != Some("true") {
                match st.get("facing") { Some("north") => out.push([0.0, 0.0, 0.8125, 1.0, 1.0, 1.0]), Some("south") => out.push([0.0, 0.0, 0.0, 1.0, 1.0, 0.1875]),
                    Some("west") => out.push([0.8125, 0.0, 0.0, 1.0, 1.0, 1.0]), _ => out.push([0.0, 0.0, 0.0, 0.1875, 1.0, 1.0]) }
            }
        }
        _ => out.push([0.0, 0.0, 0.0, 1.0, 1.0, 1.0]),
    }
}

/// Clip movement `d` along axis `ax` for box `bb` against the world's block boxes.
fn clip_axis(world: &World, bb: [f64; 6], ax: usize, mut d: f64) -> f64 {
    if d == 0.0 { return 0.0; }
    let mut lo = [bb[0], bb[1], bb[2]]; let mut hi = [bb[3], bb[4], bb[5]];
    if d > 0.0 { hi[ax] += d; } else { lo[ax] += d; }
    let mut boxes = Vec::new();
    for x in lo[0].floor() as i32..=(hi[0] - 1e-7).floor() as i32 {
        for y in (lo[1].floor() as i32 - 1)..=(hi[1] - 1e-7).floor() as i32 {
            for z in lo[2].floor() as i32..=(hi[2] - 1e-7).floor() as i32 {
                block_boxes(world.get_block(x, y, z), &mut boxes);
                for b in &boxes {
                    let w = [x as f64 + b[0], y as f64 + b[1], z as f64 + b[2], x as f64 + b[3], y as f64 + b[4], z as f64 + b[5]];
                    // overlap on the other two axes?
                    let others = (0..3).filter(|&a| a != ax).all(|a| w[a] < bb[a + 3] - 1e-7 && w[a + 3] > bb[a] + 1e-7);
                    if !others { continue; }
                    if d > 0.0 && w[ax] >= bb[ax + 3] - 1e-7 { d = d.min(w[ax] - bb[ax + 3]); }
                    else if d < 0.0 && w[ax + 3] <= bb[ax] + 1e-7 { d = d.max(w[ax + 3] - bb[ax]); }
                }
            }
        }
    }
    d
}

fn shift(bb: [f64; 6], ax: usize, d: f64) -> [f64; 6] { let mut b = bb; b[ax] += d; b[ax + 3] += d; b }

pub struct MoveResult { pub moved: [f64; 3], pub h_collision: bool, pub v_collision: bool, pub on_ground: bool }

/// Move a box of `width`×`height` at `pos` (feet centre) by `d`, colliding with blocks, stepping up
/// to `step` blocks when walking into a ledge on the ground. Y first, then X and Z.
pub fn move_box(world: &World, pos: [f64; 3], width: f64, height: f64, d: [f64; 3], step: f64, was_on_ground: bool) -> MoveResult {
    let hw = width / 2.0;
    let bb0 = [pos[0] - hw, pos[1], pos[2] - hw, pos[0] + hw, pos[1] + height, pos[2] + hw];
    let try_move = |bb: [f64; 6], d: [f64; 3]| {
        let mut bb = bb; let mut m = [0.0; 3];
        m[1] = clip_axis(world, bb, 1, d[1]); bb = shift(bb, 1, m[1]);
        let (a, b) = if d[0].abs() >= d[2].abs() { (0, 2) } else { (2, 0) };
        m[a] = clip_axis(world, bb, a, d[a]); bb = shift(bb, a, m[a]);
        m[b] = clip_axis(world, bb, b, d[b]); bb = shift(bb, b, m[b]);
        (m, bb)
    };
    let (mut m, _) = try_move(bb0, d);
    let h_blocked = (m[0] - d[0]).abs() > 1e-7 || (m[2] - d[2]).abs() > 1e-7;
    let landing = d[1] < 0.0 && (m[1] - d[1]).abs() > 1e-7;
    if step > 0.0 && h_blocked && (was_on_ground || landing) {
        // step up: rise, move horizontally, settle back down
        let up = clip_axis(world, bb0, 1, step);
        let bb = shift(bb0, 1, up);
        let (mut m2, bb2) = try_move(bb, [d[0], 0.0, d[2]]);
        let down = clip_axis(world, bb2, 1, -up + d[1].min(0.0));
        m2[1] = up + down;
        if m2[0] * m2[0] + m2[2] * m2[2] > m[0] * m[0] + m[2] * m[2] + 1e-9 { m = m2; }
    }
    let h_collision = (m[0] - d[0]).abs() > 1e-7 || (m[2] - d[2]).abs() > 1e-7;
    let v_collision = (m[1] - d[1]).abs() > 1e-7;
    MoveResult { moved: m, h_collision, v_collision, on_ground: v_collision && d[1] < 0.0 }
}

pub fn is_water(world: &World, x: f64, y: f64, z: f64) -> bool {
    let s = world.get_block(x.floor() as i32, y.floor() as i32, z.floor() as i32);
    let st = BlockState(s);
    matches!(st.name(), "water" | "bubble_column" | "kelp" | "kelp_plant" | "seagrass" | "tall_seagrass") || st.get("waterlogged") == Some("true")
}
pub fn is_lava(world: &World, x: f64, y: f64, z: f64) -> bool { BlockState(world.get_block(x.floor() as i32, y.floor() as i32, z.floor() as i32)).name() == "lava" }

// ------------------------------------------------------------------ game integration

impl Game {
    pub(super) fn sound_id(name: &str) -> i32 { registry_id(reg::SOUND_EVENT, name).map(|i| i + 1).unwrap_or(0) }

    /// `sound` packet: holder (id+1), category, fixed-point xyz (×8), volume, pitch, seed.
    pub(super) fn play_sound(&mut self, name: &str, category: i32, pos: [f64; 3], volume: f32, pitch: f32) {
        let id = Self::sound_id(name);
        if id == 0 { return; }
        let seed: i64 = rand::Rng::gen(&mut self.rng);
        let c = ((pos[0].floor() as i32) >> 4, (pos[2].floor() as i32) >> 4);
        self.broadcast_chunk(c, None, || {
            let mut w = Writer::with_id(cb::SOUND);
            w.varint(id).varint(category).i32((pos[0] * 8.0) as i32).i32((pos[1] * 8.0) as i32).i32((pos[2] * 8.0) as i32).f32(volume).f32(pitch).i64(seed);
            w
        });
    }

    pub(super) fn spawn_mob(&mut self, kind: MobKind, pos: [f64; 3], yaw: f32, setup: impl FnOnce(&mut Mob)) -> i32 {
        let eid = self.sh.next_eid.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut m = Mob::new(eid, kind, pos, yaw);
        m.ambient_timer = -(rand::Rng::gen_range(&mut self.rng, 0..400));
        m.egg_timer = rand::Rng::gen_range(&mut self.rng, 6000..12000);
        setup(&mut m);
        self.mobs.insert(eid, m);
        self.sync_viewers_for(eid);
        eid
    }

    /// Metadata change for viewers of `eid`.
    pub(super) fn send_mob_data(&self, eid: i32, f: impl FnOnce(&mut Writer)) {
        let mut w = Writer::with_id(cb::SET_ENTITY_DATA); w.varint(eid); f(&mut w); w.u8(0xff);
        self.send_to_viewers(eid, &w);
    }

    /// Broadcast position/rotation changes: move_entity_* deltas (1/4096 block) every tick the
    /// mob moved, entity_position_sync when a delta would overflow or every 60 ticks while moving.
    pub(super) fn broadcast_mob_moves(&mut self) {
        let mut out: Vec<(i32, Vec<Writer>)> = Vec::new();
        for m in self.mobs.values_mut() {
            let mut pk = Vec::new();
            let rot = (angle(m.yaw), angle(m.pitch), angle(m.head_yaw));
            let d = [m.pos[0] - m.sent_pos[0], m.pos[1] - m.sent_pos[1], m.pos[2] - m.sent_pos[2]];
            let q = [(d[0] * 4096.0).round() as i64, (d[1] * 4096.0).round() as i64, (d[2] * 4096.0).round() as i64];
            let moved = q.iter().any(|v| *v != 0);
            let rotated = (rot.0, rot.1) != (m.sent_rot.0, m.sent_rot.1);
            m.since_sync += 1;
            if q.iter().any(|v| v.abs() > 32767) || (moved && m.since_sync >= 60) {
                let mut w = Writer::with_id(cb::ENTITY_POSITION_SYNC);
                w.varint(m.eid).f64(m.pos[0]).f64(m.pos[1]).f64(m.pos[2]).f64(m.vel[0]).f64(m.vel[1]).f64(m.vel[2]).f32(m.yaw).f32(m.pitch).bool(m.on_ground);
                pk.push(w); m.sent_pos = m.pos; m.since_sync = 0;
            } else if moved {
                let mut w = Writer::with_id(if rotated { cb::MOVE_ENTITY_POS_ROT } else { cb::MOVE_ENTITY_POS });
                w.varint(m.eid).i16(q[0] as i16).i16(q[1] as i16).i16(q[2] as i16);
                if rotated { w.u8(rot.0).u8(rot.1); }
                w.bool(m.on_ground);
                pk.push(w);
                for i in 0..3 { m.sent_pos[i] += q[i] as f64 / 4096.0; }
            } else if rotated {
                let mut w = Writer::with_id(cb::MOVE_ENTITY_ROT); w.varint(m.eid).u8(rot.0).u8(rot.1).bool(m.on_ground); pk.push(w);
            }
            if rot.2 != m.sent_rot.2 { let mut w = Writer::with_id(cb::ROTATE_HEAD); w.varint(m.eid).u8(rot.2); pk.push(w); }
            m.sent_rot = rot;
            if !pk.is_empty() { out.push((m.eid, pk)); }
        }
        for (eid, pk) in out { for w in &pk { self.send_to_viewers(eid, w); } }
    }

    /// Physics for one mob: input acceleration `accel` (x/z, already scaled), jumping, gravity,
    /// water buoyancy, collisions with step-up, friction. Returns fall damage to apply.
    pub(super) fn mob_physics(&self, m: &mut Mob, accel: [f64; 2], jump: bool) -> f32 {
        let (w, h) = (m.width(), m.height());
        let in_water = is_water(&self.world, m.pos[0], m.pos[1] + h * 0.4, m.pos[2]);
        let in_lava = is_lava(&self.world, m.pos[0], m.pos[1] + 0.1, m.pos[2]);
        m.in_water = in_water;
        let mut v = m.vel;
        if in_water || in_lava {
            v[0] += accel[0] * 0.4; v[2] += accel[1] * 0.4;
            // buoyancy: mobs swim up to the surface (and out of lava, slowly)
            let head_wet = is_water(&self.world, m.pos[0], m.pos[1] + h * 0.8, m.pos[2]);
            if head_wet || in_lava || jump { v[1] += 0.04; } else { v[1] -= 0.02; }
            if m.h_collision { v[1] = v[1].max(0.3); }
        } else {
            v[0] += accel[0]; v[2] += accel[1];
            if jump && m.on_ground { v[1] = 0.42; }
            // spiders climb walls
            if m.kind == MobKind::Spider && m.h_collision { v[1] = 0.2; }
        }
        let r = move_box(&self.world, m.pos, w, h, v, if in_water { 0.0 } else { 0.6 }, m.on_ground);
        for i in 0..3 { m.pos[i] += r.moved[i]; }
        if r.h_collision { if (r.moved[0] - v[0]).abs() > 1e-7 { v[0] = 0.0; } if (r.moved[2] - v[2]).abs() > 1e-7 { v[2] = 0.0; } }
        if r.v_collision { v[1] = 0.0; }
        m.h_collision = r.h_collision;
        let was_ground = m.on_ground;
        m.on_ground = r.on_ground;
        let mut fall_damage = 0.0;
        if in_water || in_lava { m.fall_distance = 0.0; } else if m.on_ground {
            if !was_ground && m.fall_distance > 3.0 && m.kind != MobKind::Chicken { fall_damage = (m.fall_distance - 3.0).ceil() as f32; }
            m.fall_distance = 0.0;
        } else if r.moved[1] < 0.0 { m.fall_distance -= r.moved[1]; }
        // gravity + drag
        if in_water { v[0] *= 0.8; v[2] *= 0.8; v[1] *= 0.8; }
        else if in_lava { v[0] *= 0.5; v[2] *= 0.5; v[1] = v[1] * 0.5 - 0.02; }
        else {
            v[1] = (v[1] - 0.08) * 0.98;
            // chickens flap: slow fall
            if m.kind == MobKind::Chicken && !m.on_ground && v[1] < 0.0 { v[1] *= 0.6; }
            let f = if m.on_ground { 0.6 * 0.91 } else { 0.91 };
            v[0] *= f; v[2] *= f;
        }
        for c in v.iter_mut() { if c.abs() < 0.003 { *c = 0.0; } }
        m.vel = v;
        m.step_dist += (r.moved[0] * r.moved[0] + r.moved[2] * r.moved[2]).sqrt();
        fall_damage
    }

    // ------------------------------------------------------------ persistence

    pub(super) fn save_mobs(&self) {
        let Some(d) = &self.dir else { return };
        let v: Vec<_> = self.mobs.values().filter(|m| m.alive()).map(|m| m.to_json()).collect();
        let _ = std::fs::write(d.join("entities.json"), serde_json::Value::Array(v).to_string());
    }
    pub(super) fn load_mobs(&mut self) {
        let Some(d) = &self.dir else { return };
        let Ok(s) = std::fs::read_to_string(d.join("entities.json")) else { return };
        let Ok(serde_json::Value::Array(a)) = serde_json::from_str::<serde_json::Value>(&s) else { return };
        for v in a {
            let eid = self.sh.next_eid.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if let Some(m) = Mob::from_json(eid, &v) { self.mobs.insert(eid, m); }
        }
        log::info!("loaded {} mobs", self.mobs.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(b: &[u8]) -> [f64; 3] {
        if b[0] == 0 { return [0.0; 3]; }
        let packed = b[0] as u64 | (b[1] as u64) << 8 | (u32::from_be_bytes([b[2], b[3], b[4], b[5]]) as u64) << 16;
        let mut scale = packed & 3;
        if packed & 4 != 0 { let mut r = basalt_protocol::Reader::new(&b[6..]); scale |= (r.varint().unwrap() as u64) << 2; }
        let f = |v: u64| ((v & 0x7fff).min(32766) as f64 * 2.0 / 32766.0 - 1.0) * scale as f64;
        [f(packed >> 3), f(packed >> 18), f(packed >> 33)]
    }

    #[test]
    fn lpvec3_matches_vanilla_capture() {
        // vanilla's resting-mob velocity (0, -0.0784, 0): f9 ff 7f fe eb ed
        let mut w = Writer::new(); write_lpvec3(&mut w, [0.0, -0.0784, 0.0]);
        assert_eq!(w.buf, vec![0xf9, 0xff, 0x7f, 0xfe, 0xeb, 0xed]);
        let mut w = Writer::new(); write_lpvec3(&mut w, [0.0; 3]); assert_eq!(w.buf, vec![0]);
        for v in [[0.3, -1.2, 2.5], [-7.0, 0.01, 3.3], [0.5, 0.5, -0.5]] {
            let mut w = Writer::new(); write_lpvec3(&mut w, v);
            let d = decode(&w.buf);
            for i in 0..3 { assert!((d[i] - v[i]).abs() < 1e-3 * v[i].abs().max(1.0) * 8.0, "{v:?} {d:?}"); }
        }
    }

    #[test]
    fn kinds_roundtrip() {
        for k in ALL_KINDS { assert_eq!(MobKind::from_name(k.name()), Some(k)); assert!(k.entity_type() > 0); }
    }
}

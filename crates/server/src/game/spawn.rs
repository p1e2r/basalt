//! Natural spawning. Monsters: in darkness (block light 0, sky light after night darkening at or
//! below a random 0..=7) on solid blocks 24..128 blocks from players, packs of 1-4, capped per
//! player. Animals: a few per chunk on grass when a chunk is first generated, and rarely
//! afterwards in daylight on grass.
use super::mob::MobKind;
use super::path::{self, Grid};
use super::Game;
use basalt_data::BlockState;
use basalt_world::props;
use rand::Rng;

/// Hostile mobs allowed within 128 blocks of a player (vanilla's global cap is 70 scaled by
/// loaded chunks; this is a per-player simplification).
pub const HOSTILE_CAP: usize = 40;
pub const PASSIVE_CAP: usize = 10;

impl Game {
    fn top_y(&self, x: i32, z: i32) -> Option<i32> {
        let c = self.world.chunk(x >> 4, z >> 4)?;
        let h = c.heightmaps.motion_blocking[((z & 15) * 16 + (x & 15)) as usize] as i32;
        Some(basalt_world::MIN_Y + h)
    }

    fn count_near(&self, pos: [f64; 3], hostile: bool) -> usize {
        self.mobs.values().filter(|m| m.kind.hostile() == hostile && m.alive() && (m.pos[0] - pos[0]).powi(2) + (m.pos[2] - pos[2]).powi(2) < 128.0 * 128.0).count()
    }

    fn spawn_spot_ok(&self, p: [i32; 3], height: i32) -> bool {
        let g = super::ai::WorldGrid(&self.world);
        let pr = path::Params { height, can_swim: false, ..Default::default() };
        if !path::standable(&g, p, &pr) { return false; }
        let below = BlockState(self.world.get_block(p[0], p[1] - 1, p[2])).name();
        if matches!(below, "bedrock" | "barrier" | "glass" | "ice" | "magma_block") || below.ends_with("_leaves") || below.ends_with("_slab") { return false; }
        (0..height).all(|dy| !props::props(self.world.get_block(p[0], p[1] + dy, p[2])).has_fluid()) && g.cell(p[0], p[1] - 1, p[2]) == path::Cell::Solid
    }

    pub(super) fn tick_spawning(&mut self) {
        if self.no_spawning { return; }
        let pls: Vec<([f64; 3], u8)> = self.players.values().filter(|p| !p.dead).map(|p| (p.pos, p.gamemode)).collect();
        for (ppos, gm) in pls {
            if gm == 3 { continue; }
            if self.difficulty > 0 && self.count_near(ppos, true) < HOSTILE_CAP { self.try_spawn_monsters(ppos); }
            if self.tick % 400 == 0 && self.count_near(ppos, false) < PASSIVE_CAP { for _ in 0..4 { self.try_spawn_animals(ppos); } }
        }
    }

    fn random_column(&mut self, around: [f64; 3]) -> Option<(i32, i32, i32)> {
        let ang = self.rng.gen_range(0.0..std::f64::consts::TAU);
        let r = self.rng.gen_range(24.0..96.0);
        let (x, z) = ((around[0] + ang.cos() * r).floor() as i32, (around[2] + ang.sin() * r).floor() as i32);
        let top = self.top_y(x, z)?;
        Some((x, z, top))
    }

    fn far_from_players(&self, p: [f64; 3]) -> bool {
        let near = self.players.values().filter(|q| !q.dead).map(|q| ((q.pos[0] - p[0]).powi(2) + (q.pos[1] - p[1]).powi(2) + (q.pos[2] - p[2]).powi(2)).sqrt()).fold(f64::MAX, f64::min);
        (24.0..=128.0).contains(&near)
    }

    fn try_spawn_monsters(&mut self, around: [f64; 3]) {
        let Some((x, z, top)) = self.random_column(around) else { return };
        let y = self.rng.gen_range(basalt_world::MIN_Y..=top.max(basalt_world::MIN_Y));
        let kind = [MobKind::Zombie, MobKind::Skeleton, MobKind::Creeper, MobKind::Spider][self.rng.gen_range(0..4)];
        let height = kind.stats().height.ceil() as i32;
        let darken = self.sky_darken();
        let n = self.rng.gen_range(1..=4);
        let mut spawned = 0;
        for _ in 0..n * 3 {
            if spawned >= n { break; }
            let p = [x + self.rng.gen_range(-5..=5), y, z + self.rng.gen_range(-5..=5)];
            if !self.world.is_loaded(p[0] >> 4, p[2] >> 4) || !self.spawn_spot_ok(p, height) { continue; }
            if kind == MobKind::Spider && !self.spawn_spot_ok([p[0] + 1, p[1], p[2]], 1) { continue; }
            let (sky, block) = self.world.light(p[0], p[1], p[2]);
            if block > 0 || sky.saturating_sub(darken) > self.rng.gen_range(0..=7) { continue; }
            let pos = [p[0] as f64 + 0.5, p[1] as f64, p[2] as f64 + 0.5];
            if !self.far_from_players(pos) { continue; }
            let baby = kind == MobKind::Zombie && self.rng.gen_range(0..20) == 0;
            let yaw = self.rng.gen_range(0.0..360.0);
            self.spawn_mob(kind, pos, yaw, |m| m.baby = baby);
            spawned += 1;
        }
    }

    fn random_animal(&mut self) -> MobKind {
        let r = self.rng.gen_range(0..40);
        if r < 12 { MobKind::Sheep } else if r < 22 { MobKind::Pig } else if r < 32 { MobKind::Chicken } else { MobKind::Cow }
    }

    /// Natural wool colours: white 81.836%, black/gray/light gray 5% each, brown 3%, pink 0.164%.
    pub(super) fn random_sheep_color(&mut self) -> u8 {
        let r: f64 = self.rng.gen_range(0.0..100.0);
        if r < 5.0 { 15 } else if r < 10.0 { 7 } else if r < 15.0 { 8 } else if r < 18.0 { 12 } else if r < 18.164 { 6 } else { 0 }
    }

    fn spawn_animal_at(&mut self, kind: MobKind, p: [i32; 3]) {
        let pos = [p[0] as f64 + 0.5, p[1] as f64, p[2] as f64 + 0.5];
        let yaw = self.rng.gen_range(0.0..360.0);
        let color = self.random_sheep_color();
        let baby = self.rng.gen_range(0..20) == 0;
        self.spawn_mob(kind, pos, yaw, |m| { m.sheep_color = color; m.baby = baby; });
    }

    fn try_spawn_animals(&mut self, around: [f64; 3]) {
        let Some((x, z, top)) = self.random_column(around) else { return };
        let p = [x, top, z];
        if BlockState(self.world.get_block(x, top - 1, z)).name() != "grass_block" || !self.spawn_spot_ok(p, 2) { return; }
        if self.world.light(x, top, z).0 < 9 || !self.far_from_players([x as f64, top as f64, z as f64]) { return; }
        let kind = self.random_animal();
        for _ in 0..self.rng.gen_range(2..=4) {
            let q = [x + self.rng.gen_range(-3..=3), 0, z + self.rng.gen_range(-3..=3)];
            let Some(t) = self.top_y(q[0], q[2]) else { continue };
            let q = [q[0], t, q[2]];
            if BlockState(self.world.get_block(q[0], t - 1, q[2])).name() == "grass_block" && self.spawn_spot_ok(q, 2) { self.spawn_animal_at(kind, q); }
        }
    }

    /// Chunk-generation animals: ~10% of new chunks get a group of 4 of one kind on grass.
    pub(super) fn populate_animals(&mut self, cx: i32, cz: i32) {
        if self.no_spawning || self.rng.gen::<f32>() >= 0.1 { return; }
        let kind = self.random_animal();
        let mut placed = 0;
        for _ in 0..12 {
            if placed >= 4 { break; }
            let (x, z) = (cx * 16 + self.rng.gen_range(0..16), cz * 16 + self.rng.gen_range(0..16));
            let Some(t) = self.top_y(x, z) else { return };
            if BlockState(self.world.get_block(x, t - 1, z)).name() != "grass_block" || !self.spawn_spot_ok([x, t, z], 2) { continue; }
            self.spawn_animal_at(kind, [x, t, z]);
            placed += 1;
        }
    }
}


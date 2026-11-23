//! Random ticks: every tick each loaded, non-empty 16³ section gets `random_tick_speed` random
//! block positions; crops grow, grass spreads (and dies under solid blocks), leaves far from logs
//! decay, sugar cane / cactus grow up to 3 high, farmland dries or gets wet, saplings grow into a
//! small oak. Outcomes checked against vanilla 26.2 with scenario m6_random_ticks
//! (random_tick_speed raised briefly).
use super::Game;
use basalt_data::BlockState;
use basalt_world::props;
use rand::Rng;

fn name(st: u32) -> &'static str {
    BlockState(st).name()
}
fn age(st: u32) -> i32 {
    BlockState(st)
        .get("age")
        .and_then(|a| a.parse().ok())
        .unwrap_or(0)
}
fn with(st: u32, prop: &str, v: impl ToString) -> u32 {
    BlockState(st)
        .with(prop, &v.to_string())
        .map(|s| s.0)
        .unwrap_or(st)
}
fn st(n: &str) -> u32 {
    BlockState::from_name(n).map(|s| s.0).unwrap_or(0)
}

impl Game {
    pub(super) fn tick_random(&mut self) {
        let speed = self.random_tick_speed;
        if speed <= 0 {
            return;
        }
        let mut picks: Vec<[i32; 3]> = Vec::new();
        // only chunks some player has loaded (vanilla ticks within the simulation distance; in-memory
        // worlds keep every chunk they ever loaded)
        let watched: std::collections::HashSet<(i32, i32)> = self
            .players
            .values()
            .flat_map(|p| p.loaded.iter().copied())
            .collect();
        for (cx, cz) in self
            .world
            .loaded_chunks()
            .into_iter()
            .filter(|c| watched.contains(c))
        {
            let Some(c) = self.world.chunk(cx, cz) else {
                continue;
            };
            for (si, sec) in c.sections.iter().enumerate() {
                if sec.non_air == 0 {
                    continue;
                }
                for _ in 0..speed {
                    let (x, y, z) = (
                        self.rng.gen_range(0..16),
                        self.rng.gen_range(0..16),
                        self.rng.gen_range(0..16),
                    );
                    picks.push([
                        cx * 16 + x,
                        basalt_world::MIN_Y + si as i32 * 16 + y,
                        cz * 16 + z,
                    ]);
                }
            }
        }
        for b in picks {
            self.random_tick_block(b);
        }
    }

    fn sky_or_block_light(&self, b: [i32; 3]) -> u8 {
        let (s, bl) = self.world.light(b[0], b[1], b[2]);
        s.max(bl)
    }

    fn random_tick_block(&mut self, b: [i32; 3]) {
        let s = self.world.get_block(b[0], b[1], b[2]);
        if s == 0 {
            return;
        }
        let n = name(s);
        let up = [b[0], b[1] + 1, b[2]];
        match n {
            "wheat" | "carrots" | "potatoes" | "beetroots" => {
                let max = if n == "beetroots" { 3 } else { 7 };
                let a = age(s);
                if a >= max || self.sky_or_block_light(up) < 9 {
                    return;
                }
                if n == "beetroots" && self.rng.gen_range(0..3) != 0 {
                    return;
                }
                let f = self.growth_speed(b, n);
                if self.rng.gen_range(0..((25.0 / f) as i32 + 1)) == 0 {
                    self.set_block(b[0], b[1], b[2], with(s, "age", a + 1));
                }
            }
            "nether_wart" => {
                let a = age(s);
                if a < 3 && self.rng.gen_range(0..10) == 0 {
                    self.set_block(b[0], b[1], b[2], with(s, "age", a + 1));
                }
            }
            "grass_block" | "mycelium" => {
                let above = self.world.get_block(up[0], up[1], up[2]);
                if (props::props(above).opacity >= 15)
                    || self.sky_or_block_light(up) < 4 && props::props(above).blocks_motion()
                {
                    self.set_block(b[0], b[1], b[2], st("dirt"));
                    return;
                }
                if self.sky_or_block_light(up) < 9 {
                    return;
                }
                for _ in 0..4 {
                    let t = [
                        b[0] + self.rng.gen_range(-1..=1),
                        b[1] + self.rng.gen_range(-3..=1),
                        b[2] + self.rng.gen_range(-1..=1),
                    ];
                    if name(self.world.get_block(t[0], t[1], t[2])) != "dirt" {
                        continue;
                    }
                    let ta = self.world.get_block(t[0], t[1] + 1, t[2]);
                    if (props::props(ta).opacity >= 15)
                        || self.sky_or_block_light([t[0], t[1] + 1, t[2]]) < 4
                    {
                        continue;
                    }
                    self.set_block(t[0], t[1], t[2], st(n));
                }
            }
            "farmland" => {
                let wet = (-4..=4).any(|dx| {
                    (0..=1).any(|dy| {
                        (-4..=4).any(|dz| {
                            matches!(
                                name(self.world.get_block(b[0] + dx, b[1] + dy, b[2] + dz)),
                                "water"
                            )
                        })
                    })
                });
                let m: i32 = BlockState(s)
                    .get("moisture")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                if wet {
                    if m < 7 {
                        self.set_block(b[0], b[1], b[2], with(s, "moisture", 7));
                    }
                } else if m > 0 {
                    self.set_block(b[0], b[1], b[2], with(s, "moisture", m - 1));
                } else if !matches!(
                    name(self.world.get_block(up[0], up[1], up[2])),
                    "wheat"
                        | "carrots"
                        | "potatoes"
                        | "beetroots"
                        | "melon_stem"
                        | "pumpkin_stem"
                        | "attached_melon_stem"
                        | "attached_pumpkin_stem"
                        | "torchflower_crop"
                        | "pitcher_crop"
                ) {
                    self.set_block(b[0], b[1], b[2], st("dirt"));
                }
            }
            "sugar_cane" | "cactus" => {
                if self.world.get_block(up[0], up[1], up[2]) != 0 {
                    return;
                }
                let mut h = 1;
                while name(self.world.get_block(b[0], b[1] - h, b[2])) == n {
                    h += 1;
                }
                if h >= 3 {
                    return;
                }
                let a = age(s);
                if a >= 15 {
                    self.set_block(up[0], up[1], up[2], st(n));
                    self.set_block(b[0], b[1], b[2], with(s, "age", 0));
                } else {
                    self.set_block(b[0], b[1], b[2], with(s, "age", a + 1));
                }
            }
            _ if n.ends_with("_leaves") => {
                let st_ = BlockState(s);
                if st_.get("persistent") == Some("false") && st_.get("distance") == Some("7") {
                    let drops = self.data.block_drops(s, None, false, 0, &mut self.rng);
                    self.set_block(b[0], b[1], b[2], 0);
                    self.drop_at_block(b, drops);
                }
            }
            "oak_sapling" => {
                if self.sky_or_block_light(up) < 9 || self.rng.gen_range(0..7) != 0 {
                    return;
                }
                let stage: i32 = BlockState(s)
                    .get("stage")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                if stage == 0 {
                    self.set_block(b[0], b[1], b[2], with(s, "stage", 1));
                } else {
                    self.grow_oak(b);
                }
            }
            _ => {}
        }
    }

    /// Crop growth factor from the farmland around and under the crop (vanilla-like: moist farmland
    /// under it counts 3, dry 1, neighbours a quarter of that; same crops on both axes or a
    /// diagonal halve it).
    fn growth_speed(&self, b: [i32; 3], crop: &str) -> f32 {
        let mut f = 1.0f32;
        for dx in -1..=1 {
            for dz in -1..=1 {
                let s = self.world.get_block(b[0] + dx, b[1] - 1, b[2] + dz);
                let mut g = if name(s) == "farmland" {
                    if BlockState(s)
                        .get("moisture")
                        .and_then(|m| m.parse::<i32>().ok())
                        .unwrap_or(0)
                        > 0
                    {
                        3.0
                    } else {
                        1.0
                    }
                } else {
                    0.0
                };
                if dx != 0 || dz != 0 {
                    g /= 4.0;
                }
                f += g;
            }
        }
        let is = |dx: i32, dz: i32| name(self.world.get_block(b[0] + dx, b[1], b[2] + dz)) == crop;
        let row_x = is(-1, 0) || is(1, 0);
        let row_z = is(0, -1) || is(0, 1);
        if (row_x && row_z) || is(-1, -1) || is(1, -1) || is(1, 1) || is(-1, 1) {
            f /= 2.0;
        }
        f
    }

    /// A small oak: trunk 4-6 high, blob of leaves around the top (needs room above).
    fn grow_oak(&mut self, b: [i32; 3]) {
        let h: i32 = 4 + self.rng.gen_range(0..3);
        for y in 1..=h + 1 {
            let s = self.world.get_block(b[0], b[1] + y, b[2]);
            if s != 0 && !name(s).ends_with("_leaves") {
                return;
            }
        }
        let (log, leaves) = (
            st("oak_log"),
            BlockState::from_name("oak_leaves")
                .and_then(|s| s.with("distance", "1"))
                .map(|s| s.0)
                .unwrap_or(0),
        );
        for dy in h - 3..=h {
            let r: i32 = if dy >= h - 1 { 1 } else { 2 };
            for dx in -r..=r {
                for dz in -r..=r {
                    if dx.abs() == r && dz.abs() == r && (dy == h || self.rng.gen_range(0..2) == 0)
                    {
                        continue;
                    }
                    let p = [b[0] + dx, b[1] + dy, b[2] + dz];
                    if self.world.get_block(p[0], p[1], p[2]) == 0 {
                        self.set_block(p[0], p[1], p[2], leaves);
                    }
                }
            }
        }
        for y in 0..h {
            self.set_block(b[0], b[1] + y, b[2], log);
        }
        let below = [b[0], b[1] - 1, b[2]];
        if matches!(
            name(self.world.get_block(below[0], below[1], below[2])),
            "grass_block"
        ) {
            self.set_block(below[0], below[1], below[2], st("dirt"));
        }
    }
}

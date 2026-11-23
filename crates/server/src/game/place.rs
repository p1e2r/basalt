//! Block placement rules: placement state per block family, survival (support) checks, connection
//! shapes (fences, walls, panes, bars, stairs) and neighbour updates. Every rule here was measured by
//! diffing against the vanilla 26.2 server with `basalt-check --fuzz place` (see NOTES.md, "Placement").
use super::*;
use basalt_world::props;

pub(super) const DN: [&str; 6] = ["down", "up", "north", "south", "west", "east"];
const VEC: [[i32; 3]; 6] = [
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
    [-1, 0, 0],
    [1, 0, 0],
];

fn di(name: &str) -> usize {
    DN.iter().position(|d| *d == name).unwrap_or(2)
}
fn off(p: [i32; 3], d: usize) -> [i32; 3] {
    [p[0] + VEC[d][0], p[1] + VEC[d][1], p[2] + VEC[d][2]]
}
fn cw(d: usize) -> usize {
    match d {
        2 => 5,
        5 => 3,
        3 => 4,
        4 => 2,
        x => x,
    }
}
fn ccw(d: usize) -> usize {
    match d {
        2 => 4,
        4 => 3,
        3 => 5,
        5 => 2,
        x => x,
    }
}
fn horiz(d: usize) -> bool {
    d >= 2
}

/// Vanilla-style 16-bit sine table: `sin(i * 2pi / 65536)` as f32 (the client/server trig used for view directions).
fn sin_table() -> &'static Vec<f32> {
    static T: std::sync::OnceLock<Vec<f32>> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        (0..65536)
            .map(|i| (i as f64 * std::f64::consts::PI * 2.0 / 65536.0).sin() as f32)
            .collect()
    })
}
fn sin_t(x: f32) -> f32 {
    sin_table()[((x * 10430.378f32) as i32 & 65535) as usize]
}
fn cos_t(x: f32) -> f32 {
    sin_table()[((x * 10430.378f32 + 16384.0f32) as i32 & 65535) as usize]
}

/// The six directions ordered by how closely the player looks along them: the three axes sorted by the magnitude of the view
/// vector (ties: y, then x, then z), each in the direction of its sign, followed by the opposites in reverse. Measured against
/// vanilla: exact vertical looks give [up|down, west, north, south, east, down|up]; a 45 degree pitch tie between x and y picks y.
pub(super) fn look_order(yaw: f32, pitch: f32) -> [usize; 6] {
    let (f, g) = (pitch * 0.017453292f32, -yaw * 0.017453292f32);
    let (h, i, j, k) = (sin_t(f), cos_t(f), sin_t(g), cos_t(g));
    let x_dir = if j > 0.0 { 5 } else { 4 };
    let y_dir = if h < 0.0 { 1 } else { 0 };
    let z_dir = if k > 0.0 { 3 } else { 2 };
    let mut ax = [(h.abs(), y_dir), (j.abs() * i, x_dir), (k.abs() * i, z_dir)];
    for a in 1..3 {
        let mut b = a;
        while b > 0 && ax[b].0 > ax[b - 1].0 {
            ax.swap(b, b - 1);
            b -= 1;
        }
    }
    [
        ax[0].1,
        ax[1].1,
        ax[2].1,
        ax[2].1 ^ 1,
        ax[1].1 ^ 1,
        ax[0].1 ^ 1,
    ]
}

/// Player's horizontal facing as a direction index (north/south/west/east).
fn look_h(yaw: f32) -> usize {
    di(horizontal_facing(yaw))
}

pub(super) struct Click {
    pub face: usize,
    pub cursor: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub replacing_clicked: bool,
}

impl Game {
    fn bs(&self, p: [i32; 3]) -> BlockState {
        BlockState(self.world.get_block(p[0], p[1], p[2]))
    }
    fn tag(&self, tag: &str, s: BlockState) -> bool {
        self.data.tags.block_has(tag, s.name())
    }
    fn is_water_source(&self, p: [i32; 3]) -> bool {
        let s = self.bs(p);
        s.name() == "water" && s.get("level") == Some("0")
    }
    fn is_water(&self, p: [i32; 3]) -> bool {
        let s = self.bs(p);
        s.name() == "water" || s.get("waterlogged") == Some("true")
    }

    /// The block's face pointing along `d` is a full square (supports anything rigid).
    pub(super) fn sturdy(&self, s: BlockState, d: usize) -> bool {
        let k = s.block().kind;
        let name = s.name();
        if matches!(
            name,
            "farmland"
                | "dirt_path"
                | "soul_sand"
                | "mud"
                | "honey_block"
                | "spawner"
                | "barrier"
                | "light"
                | "conduit"
        ) {
            return false;
        }
        if k == "slab" || k.ends_with("_slab") {
            return match s.get("type") {
                Some("double") => true,
                Some("top") => d == 1,
                _ => d == 0,
            };
        }
        if k == "stair" || k.ends_with("_stair") {
            let top = s.get("half") == Some("top");
            if (d == 1) || (d == 0) {
                return (d == 0) != top;
            }
            return s.get("shape") == Some("straight") && s.get("facing").map(di) == Some(d);
        }
        if matches!(k, "trapdoor" | "weathering_copper_trapdoor") && s.get("open") != Some("true") {
            return (s.get("half") == Some("top")) == (d == 1) && d < 2;
        }
        if matches!(k, "door" | "weathering_copper_door") {
            // broad faces of a door: along its facing axis when closed, across it when open
            let Some(f) = s.get("facing").map(di) else {
                return false;
            };
            return d >= 2 && ((d / 2 == f / 2) != (s.get("open") == Some("true")));
        }
        if k == "snow_layer" {
            return s.get("layers") == Some("8") && true;
        }
        if self.tag("leaves", s) {
            return false;
        }
        let name_full = matches!(
            k,
            "transparent"
                | "stained_glass"
                | "tinted_glass"
                | "ice"
                | "frosted_ice"
                | "shulker_box"
                | "waterlogged_transparent"
                | "half_transparent"
        );
        if name_full {
            return true;
        }
        props::props(s.0).opacity == 15
            && !matches!(k, "piston_base" if s.get("extended") == Some("true"))
    }

    /// The block's face along `d` covers the centre of the block (torches, candles, lanterns, pickles sit on it).
    pub(super) fn center_support(&self, s: BlockState, d: usize) -> bool {
        if self.sturdy(s, d) {
            return true;
        }
        let k = s.block().kind;
        if d == 1 {
            return matches!(
                k,
                "fence" | "wall" | "iron_bars" | "stained_glass_pane" | "weathering_copper_bar"
            ) || (k.ends_with("chain") && s.get("axis") == Some("y"));
        }
        if d == 0
            && matches!(
                k,
                "chest"
                    | "trapped_chest"
                    | "ender_chest"
                    | "copper_chest"
                    | "weathering_copper_chest"
            )
        {
            return true;
        }
        if d == 0 {
            return matches!(
                k,
                "fence" | "wall" | "iron_bars" | "stained_glass_pane" | "weathering_copper_bar"
            );
        }
        false
    }

    fn is_solid_material(&self, s: BlockState) -> bool {
        props::props(s.0).blocks_motion()
            && !(s.block().kind == "snow_layer" && s.get("layers") != Some("8"))
    }

    /// Can `state` exist at `p` (support / attachment rules)?
    pub(super) fn survives(&self, s: BlockState, p: [i32; 3]) -> bool {
        let (name, k) = (s.name(), s.block().kind);
        let below = self.bs(off(p, 0));
        let above = self.bs(off(p, 1));
        let attached = |d: usize| -> bool {
            // block at p + d must be sturdy towards p
            let n = self.bs(off(p, d));
            self.sturdy(n, d ^ 1)
        };
        let facing = s.get("facing").map(di);
        let sup_tag = |t: &str| self.data.tags.block_has(t, below.name());
        match k {
            "torch" | "redstone_torch" => self.center_support(below, 1),
            "wall_torch" | "redstone_wall_torch" => facing.map(|f| attached(f ^ 1)).unwrap_or(true),
            "standing_sign" | "banner" => self.is_solid_material(below),
            "wall_sign" | "wall_banner" => facing
                .map(|f| self.is_solid_material(self.bs(off(p, f ^ 1))))
                .unwrap_or(true),
            "ceiling_hanging_sign" => self.center_support(above, 0),
            "wall_hanging_sign" => facing
                .map(|f| {
                    [cw(f), ccw(f)].iter().any(|&d| {
                        let n = self.bs(off(p, d));
                        self.sturdy(n, d ^ 1)
                    })
                })
                .unwrap_or(true),
            "button" | "lever" => {
                let f = facing.unwrap_or(2);
                match s.get("face") {
                    Some("floor") => self.sturdy(below, 1),
                    Some("ceiling") => self.sturdy(above, 0),
                    _ => attached(f ^ 1),
                }
            }
            "grindstone" => {
                let f = facing.unwrap_or(2);
                let n = match s.get("face") {
                    Some("floor") => below,
                    Some("ceiling") => above,
                    _ => self.bs(off(p, f ^ 1)),
                };
                !self.replaceable_for(n)
            }
            "ladder" | "trip_wire_hook" => facing.map(|f| attached(f ^ 1)).unwrap_or(true),
            "pressure_plate" | "weighted_pressure_plate" => self.center_support(below, 1),
            "rail" | "powered_rail" | "detector_rail" | "repeater" | "comparator" => {
                self.sturdy(below, 1)
            }
            "chorus_flower" => sup_tag("supports_chorus_flower") || below.name() == "chorus_plant",
            "chorus_plant" => sup_tag("supports_chorus_plant") || below.name() == "chorus_plant",
            "door" | "weathering_copper_door" => {
                if s.get("half") == Some("upper") {
                    below.block().name == s.block().name && below.get("half") == Some("lower")
                } else {
                    self.sturdy(below, 1)
                }
            }
            "double_plant" | "tall_flower" => {
                if s.get("half") == Some("upper") {
                    below.block().name == s.block().name && below.get("half") == Some("lower")
                } else {
                    sup_tag("supports_vegetation")
                }
            }
            "flower" | "sapling" | "tall_grass" | "bush" | "eyeblossom" | "firefly_bush"
            | "sweet_berry_bush" | "torchflower_crop" => sup_tag("supports_vegetation"),
            "wither_rose" => sup_tag("supports_wither_rose"),
            "small_dripleaf" => {
                if s.get("half") == Some("upper") {
                    below.name() == name && below.get("half") == Some("lower")
                } else {
                    sup_tag("supports_small_dripleaf")
                }
            }
            "big_dripleaf" => {
                sup_tag("supports_big_dripleaf")
                    || matches!(below.name(), "big_dripleaf" | "big_dripleaf_stem")
            }
            "mushroom" | "leaf_litter" => self.sturdy(below, 1),
            "spore_blossom" => self.center_support(above, 0),
            "frogspawn" => self.is_water(off(p, 0)) && above.is_air(),
            "cactus_flower" => {
                sup_tag("supports_cactus")
                    || sup_tag("support_override_cactus_flower")
                    || below.name() == "cactus"
            }
            "mangrove_propagule" => {
                if s.get("hanging") == Some("true") {
                    self.data
                        .tags
                        .block_has("supports_hanging_mangrove_propagule", above.name())
                } else {
                    sup_tag("supports_mangrove_propagule")
                }
            }
            "flower_bed" => sup_tag("supports_vegetation"),
            "lily_pad" => sup_tag("supports_lily_pad"),
            "dry_vegetation" | "short_dry_grass" | "tall_dry_grass" => {
                sup_tag("supports_dry_vegetation")
            }
            "azalea" => sup_tag("supports_azalea"),
            "crop" | "carrot" | "potato" | "beetroot" | "pitcher_crop" => sup_tag("supports_crops"),
            "stem" => sup_tag(&format!("supports_{}", name)),
            "bamboo_sapling" | "bamboo_stalk" => {
                sup_tag("supports_bamboo")
                    || below.name() == "bamboo"
                    || below.name() == "bamboo_sapling"
            }
            "cactus" => {
                (below.name() == "cactus" || sup_tag("supports_cactus"))
                    && (2..6).all(|d| {
                        let n = self.bs(off(p, d));
                        !self.is_solid_material(n) && n.name() != "lava"
                    })
            }
            "sugar_cane" => {
                below.name() == "sugar_cane"
                    || (sup_tag("supports_sugar_cane")
                        && (2..6).any(|d| {
                            let n = self.bs(off(off(p, 0), d));
                            n.name() == "water"
                                || n.get("waterlogged") == Some("true")
                                || n.name() == "frosted_ice"
                        }))
            }
            "nether_wart" => sup_tag("supports_nether_wart"),
            "nether_roots" | "nether_sprouts" | "nether_fungus" => {
                sup_tag(&format!("supports_{}", name)) || sup_tag("supports_vegetation")
            }
            "carpet" | "wool_carpet" | "mossy_carpet" => !below.is_air(),
            "snow_layer" => {
                if self
                    .data
                    .tags
                    .block_has("cannot_support_snow_layer", below.name())
                {
                    return false;
                }
                self.data
                    .tags
                    .block_has("support_override_snow_layer", below.name())
                    || self.sturdy(below, 1)
                    || (below.name() == "snow" && below.get("layers") == Some("8"))
            }
            "candle" | "sea_pickle" => self.center_support(below, 1),
            "cake" => self.is_solid_material(below),
            "coral_plant" | "base_coral_plant" | "coral_fan" | "base_coral_fan" => {
                self.sturdy(below, 1)
            }
            "coral_wall_fan" | "base_coral_wall_fan" => {
                facing.map(|f| attached(f ^ 1)).unwrap_or(true)
            }
            "lantern" | "weathering_lantern" => {
                if s.get("hanging") == Some("true") {
                    self.center_support(above, 0)
                } else {
                    self.center_support(below, 1)
                }
            }
            "scaffolding" => self.scaffold_distance(p) < 7,
            "kelp" => {
                self.is_water(p)
                    && !self
                        .data
                        .tags
                        .block_has("cannot_support_kelp", below.name())
                    && (self.sturdy(below, 1)
                        || below.name() == "kelp"
                        || below.name() == "kelp_plant")
            }
            "seagrass" | "tall_seagrass" => {
                self.is_water(p)
                    && !self
                        .data
                        .tags
                        .block_has("cannot_support_seagrass", below.name())
                    && self.sturdy(below, 1)
            }
            "amethyst_cluster" => facing.map(|f| attached(f ^ 1)).unwrap_or(true),
            "weeping_vines" => {
                self.sturdy(above, 0)
                    || above.name() == "weeping_vines"
                    || above.name() == "weeping_vines_plant"
            }
            "twisting_vines" => {
                self.sturdy(below, 1)
                    || below.name() == "twisting_vines"
                    || below.name() == "twisting_vines_plant"
            }
            "pointed_dripstone" => {
                let v = s.get("vertical_direction").map(di).unwrap_or(1);
                self.sturdy(self.bs(off(p, v ^ 1)), v)
                    || self.bs(off(p, v ^ 1)).name() == "pointed_dripstone"
            }
            "sculk_vein" | "glow_lichen" | "vine" | "multiface" => {
                // at least one face attaches to a sturdy neighbour (vines may also hang from above)
                let flag = |f: usize| s.get(DN[f]) == Some("true");
                (0..6).any(|f| flag(f) && self.sturdy(self.bs(off(p, f)), f ^ 1))
                    || (name == "vine" && false)
            }
            _ => true,
        }
    }

    fn scaffold_distance(&self, p: [i32; 3]) -> i32 {
        let below = self.bs(off(p, 0));
        if below.name() == "scaffolding" {
            return below
                .get("distance")
                .and_then(|d| d.parse().ok())
                .unwrap_or(7);
        }
        if self.sturdy(below, 1) {
            return 0;
        }
        (2..6)
            .filter_map(|d| {
                let n = self.bs(off(p, d));
                (n.name() == "scaffolding").then(|| {
                    n.get("distance")
                        .and_then(|d| d.parse::<i32>().ok())
                        .unwrap_or(7)
                        + 1
                })
            })
            .min()
            .unwrap_or(7)
            .min(7)
    }

    // ------------------------------------------------------------------ connection shapes

    fn fence_like_exception(&self, s: BlockState) -> bool {
        self.tag("leaves", s)
            || matches!(
                s.name(),
                "barrier" | "carved_pumpkin" | "jack_o_lantern" | "melon" | "pumpkin"
            )
            || self.tag("shulker_boxes", s)
    }

    fn gate_connects(s: BlockState, d: usize) -> bool {
        s.get("facing")
            .map(di)
            .map(|f| cw(f) == d || ccw(f) == d)
            .unwrap_or(false)
    }

    /// Recompute the connection-derived properties (fence/pane/bars/wall/stairs) of `s` at `p`.
    #[allow(clippy::needless_range_loop)]
    pub(super) fn reshape(&self, s: BlockState, p: [i32; 3]) -> BlockState {
        let k = s.block().kind;
        let mut st = s.0;
        match k {
            "fence" => {
                for d in 2..6 {
                    let n = self.bs(off(p, d));
                    let c = (self.tag("fences", n)
                        && self.tag("wooden_fences", n) == self.tag("wooden_fences", s))
                        || (self.tag("fence_gates", n) && Self::gate_connects(n, d))
                        || (!self.fence_like_exception(n) && self.sturdy(n, d ^ 1));
                    st = with_prop(st, DN[d], if c { "true" } else { "false" });
                }
            }
            "iron_bars" | "stained_glass_pane" | "weathering_copper_bar" => {
                for d in 2..6 {
                    let n = self.bs(off(p, d));
                    let c = (!self.fence_like_exception(n) && self.sturdy(n, d ^ 1))
                        || self.tag("walls", n)
                        || matches!(
                            n.block().kind,
                            "iron_bars" | "stained_glass_pane" | "weathering_copper_bar"
                        );
                    st = with_prop(st, DN[d], if c { "true" } else { "false" });
                }
            }
            "wall" => st = self.wall_shape(s, p),
            "stair" | "weathering_copper_stair" => {
                st = with_prop(st, "shape", self.stair_shape(s, p))
            }
            "fence_gate" => {
                let f = s.get("facing").map(di).unwrap_or(2);
                let in_wall = self.tag("walls", self.bs(off(p, cw(f))))
                    || self.tag("walls", self.bs(off(p, ccw(f))));
                st = with_prop(st, "in_wall", if in_wall { "true" } else { "false" });
            }
            _ => {}
        }
        BlockState(st)
    }

    fn wall_shape(&self, s: BlockState, p: [i32; 3]) -> u32 {
        let above = self.bs(off(p, 1));
        let mut conn = [false; 4];
        let mut tall = [false; 4];
        for (i, d) in (2..6).enumerate() {
            let n = self.bs(off(p, d));
            conn[i] = self.tag("walls", n)
                || matches!(
                    n.block().kind,
                    "iron_bars" | "stained_glass_pane" | "weathering_copper_bar"
                )
                || (self.tag("fence_gates", n) && Self::gate_connects(n, d))
                || (!self.fence_like_exception(n) && self.sturdy(n, d ^ 1));
            // "tall" when the block above reaches down to this side
            tall[i] = conn[i]
                && (self.sturdy(above, 0)
                    || (self.tag("walls", above)
                        && above.get(DN[d]).map(|v| v != "none").unwrap_or(false))
                    || (matches!(
                        above.block().kind,
                        "iron_bars" | "stained_glass_pane" | "weathering_copper_bar"
                    ) && above.get(DN[d]) == Some("true")));
        }
        let straight = (conn[0] && conn[1] && !conn[2] && !conn[3])
            || (conn[2] && conn[3] && !conn[0] && !conn[1]);
        let above_post = self.tag("wall_post_override", above)
            || (self.tag("walls", above) && above.get("up") == Some("true"))
            || self.sturdy(above, 0);
        let up = !straight
            || above_post
            || conn.iter().filter(|c| **c).count() == 0
            || above.get("up") == Some("true") && self.tag("walls", above);
        let mut st = s.0;
        for (i, d) in (2..6).enumerate() {
            st = with_prop(
                st,
                DN[d],
                if !conn[i] {
                    "none"
                } else if tall[i] {
                    "tall"
                } else {
                    "low"
                },
            );
        }
        with_prop(st, "up", if up { "true" } else { "false" })
    }

    fn stair_shape(&self, s: BlockState, p: [i32; 3]) -> &'static str {
        let f = s.get("facing").map(di).unwrap_or(2);
        let half = s.get("half");
        let stair = |n: BlockState| -> Option<usize> {
            (matches!(n.block().kind, "stair" | "weathering_copper_stair") && n.get("half") == half)
                .then(|| n.get("facing").map(di))
                .flatten()
        };
        let can_take = |d: usize| -> bool {
            let n = self.bs(off(p, d));
            !(matches!(n.block().kind, "stair" | "weathering_copper_stair"))
                || n.get("facing") != s.get("facing")
                || n.get("half") != s.get("half")
        };
        if let Some(c) = stair(self.bs(off(p, f))) {
            if c / 2 != f / 2 && can_take(c ^ 1) {
                return if c == ccw(f) {
                    "outer_left"
                } else {
                    "outer_right"
                };
            }
        }
        if let Some(c) = stair(self.bs(off(p, f ^ 1))) {
            if c / 2 != f / 2 && can_take(c) {
                return if c == ccw(f) {
                    "inner_left"
                } else {
                    "inner_right"
                };
            }
        }
        "straight"
    }

    // ------------------------------------------------------------------ placement

    fn double_slab_ok(&self, clicked: BlockState, item: BlockState, c: &Click) -> bool {
        if clicked.block().name != item.block().name {
            return false;
        }
        match clicked.get("type") {
            Some("bottom") => c.face == 1 || (horiz(c.face) && c.cursor[1] <= 0.5),
            Some("top") => c.face == 0 || (horiz(c.face) && c.cursor[1] > 0.5),
            _ => false,
        }
    }

    /// Blocks to set for placing `item` at `t` (primary first), or None when vanilla refuses.
    pub(super) fn placement(
        &self,
        item: u32,
        t: [i32; 3],
        c: &Click,
    ) -> Option<Vec<([i32; 3], u32)>> {
        let it = BlockState(item);
        let (name, k) = (it.name(), it.block().kind);
        let old = self.bs(t);
        let has = |p: &str| it.get(p).is_some();
        let order = look_order(c.yaw, c.pitch);
        let lh = look_h(c.yaw);
        let mut st = item;
        let top_half = c.face == 0 || (horiz(c.face) && c.cursor[1] > 0.5);
        let mut extra: Vec<([i32; 3], u32)> = Vec::new();
        let first_vertical = order.iter().copied().find(|d| *d < 2).unwrap_or(0);

        match k {
            "slab" | "weathering_copper_slab" => {
                if old.block().name == it.block().name
                    && old.get("type") != Some("double")
                    && self.double_slab_ok(old, it, c)
                {
                    st = with_prop(old.0, "type", "double");
                    if old.get("waterlogged") == Some("true") {
                        st = with_prop(st, "waterlogged", "false");
                    }
                } else {
                    st = with_prop(st, "type", if top_half { "top" } else { "bottom" });
                }
            }
            "stair" | "weathering_copper_stair" => {
                st = with_prop(st, "facing", DN[lh]);
                st = with_prop(st, "half", if top_half { "top" } else { "bottom" });
            }
            "trapdoor" | "weathering_copper_trapdoor" => {
                if !c.replacing_clicked && horiz(c.face) {
                    st = with_prop(st, "facing", DN[c.face]);
                    st = with_prop(st, "half", if c.cursor[1] > 0.5 { "top" } else { "bottom" });
                } else {
                    st = with_prop(st, "facing", DN[lh ^ 1]);
                    st = with_prop(st, "half", if c.face == 1 { "bottom" } else { "top" });
                }
            }
            "door" | "weathering_copper_door" => {
                let up = off(t, 1);
                if !self.replaceable_for(self.bs(up)) {
                    return None;
                }
                let hinge = self.door_hinge(t, lh, c.cursor, it);
                st = with_prop(with_prop(st, "facing", DN[lh]), "hinge", hinge);
                extra.push((up, with_prop(st, "half", "upper")));
            }
            "bed" => {
                let head = off(t, lh);
                if !self.replaceable_for(self.bs(head)) {
                    return None;
                }
                st = with_prop(st, "facing", DN[lh]);
                extra.push((head, with_prop(st, "part", "head")));
                st = with_prop(st, "part", "foot");
            }
            "small_dripleaf" => {
                let up = off(t, 1);
                if !self.replaceable_for(self.bs(up)) {
                    return None;
                }
                st = with_prop(st, "facing", DN[lh ^ 1]);
                extra.push((up, with_prop(st, "half", "upper")));
            }
            "double_plant" | "tall_flower" => {
                let up = off(t, 1);
                if !self.replaceable_for(self.bs(up)) {
                    return None;
                }
                extra.push((up, with_prop(st, "half", "upper")));
            }
            "fence_gate" | "decorated_pot" | "calibrated_sculk_sensor" => {
                st = with_prop(st, "facing", DN[lh]);
            }
            "anvil" => {
                st = with_prop(st, "facing", DN[cw(lh)]);
            }
            "dispenser" | "dropper" | "piston_base" => {
                st = with_prop(st, "facing", DN[order[0] ^ 1]);
            }
            "observer" => {
                st = with_prop(st, "facing", DN[order[0]]);
            }
            "barrel" => {
                st = with_prop(st, "facing", DN[order[0] ^ 1]);
            }
            "shulker_box"
            | "end_rod"
            | "lightning_rod"
            | "weathering_lightning_rod"
            | "amethyst_cluster" => {
                st = with_prop(st, "facing", DN[c.face]);
            }
            "hopper" => {
                st = with_prop(st, "facing", DN[if horiz(c.face) { c.face ^ 1 } else { 0 }]);
            }
            "rotated_pillar"
            | "chain"
            | "weathering_copper_chain"
            | "infested_rotated_pillar"
            | "hay"
            | "huge_mushroom"
                if has("axis") =>
            {
                st = with_prop(
                    st,
                    "axis",
                    match c.face {
                        0 | 1 => "y",
                        2 | 3 => "z",
                        _ => "x",
                    },
                );
            }
            "button" | "lever" | "grindstone" => {
                let mut found = None;
                // the clicked face's candidate is tried first, then the look directions
                let first = if c.face == 1 {
                    with_prop(with_prop(st, "face", "floor"), "facing", DN[lh])
                } else if c.face == 0 {
                    with_prop(with_prop(st, "face", "ceiling"), "facing", DN[lh])
                } else {
                    with_prop(with_prop(st, "face", "wall"), "facing", DN[c.face])
                };
                if self.survives(BlockState(first), t) {
                    found = Some(first);
                }
                for &d in order.iter().filter(|_| found.is_none()) {
                    let cand = if d < 2 {
                        with_prop(
                            with_prop(st, "face", if d == 1 { "ceiling" } else { "floor" }),
                            "facing",
                            DN[lh],
                        )
                    } else {
                        with_prop(with_prop(st, "face", "wall"), "facing", DN[d ^ 1])
                    };
                    if self.survives(BlockState(cand), t) {
                        found = Some(cand);
                        break;
                    }
                }
                st = found?;
            }
            "ladder" | "trip_wire_hook" => {
                let mut found = None;
                if horiz(c.face) {
                    let first = with_prop(st, "facing", DN[c.face]);
                    if self.survives(BlockState(first), t) {
                        found = Some(first);
                    }
                }
                for &d in order.iter().filter(|d| horiz(**d) && found.is_none()) {
                    let cand = with_prop(st, "facing", DN[d ^ 1]);
                    if self.survives(BlockState(cand), t) {
                        found = Some(cand);
                        break;
                    }
                }
                st = found?;
            }
            "torch" | "redstone_torch" | "standing_sign" | "banner" | "skull" | "wither_skull"
            | "player_head" | "coral_fan" | "base_coral_fan" => {
                // StandingAndWall items: a side click attaches to the clicked block (wall variant); otherwise the standing variant
                let wall_default = wall_variant(name).and_then(|n| block_default(&n));
                let skull = k.contains("skull") || k == "player_head";
                let mut standing = item;
                if matches!(k, "standing_sign" | "banner") {
                    standing = with_prop(standing, "rotation", &segment(c.yaw + 180.0).to_string());
                }
                if skull {
                    standing = with_prop(standing, "rotation", &segment(c.yaw).to_string());
                }
                let mut found = None;
                if c.face == 1 && self.survives(BlockState(standing), t) {
                    found = Some(standing);
                }
                let wall_ok = |cs: u32, d_attach: usize| {
                    if skull {
                        !self.replaceable_for(self.bs(off(t, d_attach)))
                    } else {
                        self.survives(BlockState(cs), t)
                    }
                };
                if found.is_none() && horiz(c.face) {
                    // a side click attaches to the clicked block
                    if let Some(wd) = wall_default {
                        let cs = with_prop(wd, "facing", DN[c.face]);
                        if wall_ok(cs, c.face ^ 1) {
                            found = Some(cs);
                        }
                    }
                }
                if found.is_none() && !skull {
                    // otherwise walk the look directions: down tries the standing variant, the others the wall variant
                    'outer: for &d in &order {
                        if d == 0 {
                            if self.survives(BlockState(standing), t) {
                                found = Some(standing);
                                break;
                            }
                            continue;
                        }
                        if d == 1 {
                            continue;
                        }
                        if let Some(wd) = wall_default {
                            for &d2 in order.iter().filter(|d| horiz(**d)) {
                                let cs = with_prop(wd, "facing", DN[d2 ^ 1]);
                                if wall_ok(cs, d2) {
                                    found = Some(cs);
                                    break 'outer;
                                }
                            }
                        }
                    }
                }
                st = found.or(if skull { Some(standing) } else { None })?;
            }
            "ceiling_hanging_sign" => {
                // look directions: UP tries the ceiling variant, horizontals the wall variant (facing away from the look direction,
                // never along the clicked face's axis), DOWN is skipped
                let wall = block_default(&name.replace("_hanging_sign", "_wall_hanging_sign"));
                let mut found = None;
                'o: for &d in &order {
                    if d == 0 {
                        continue;
                    }
                    if d == 1 {
                        let cs = with_prop(
                            with_prop(st, "rotation", &segment(c.yaw + 180.0).to_string()),
                            "attached",
                            "true",
                        );
                        if self.survives(BlockState(cs), t) {
                            found = Some(cs);
                            break;
                        }
                    } else if let Some(w) = wall {
                        for &d2 in order
                            .iter()
                            .filter(|d2| horiz(**d2) && !(horiz(c.face) && **d2 / 2 == c.face / 2))
                        {
                            let cs = with_prop(w, "facing", DN[d2 ^ 1]);
                            if self.survives(BlockState(cs), t) {
                                found = Some(cs);
                                break 'o;
                            }
                        }
                    }
                }
                st = found?;
            }
            "lantern" | "weathering_lantern" => {
                let mut found = None;
                for &d in order.iter().filter(|d| **d < 2) {
                    let cand = with_prop(st, "hanging", if d == 1 { "true" } else { "false" });
                    if self.survives(BlockState(cand), t) {
                        found = Some(cand);
                        break;
                    }
                }
                st = found?;
            }
            "pointed_dripstone" => {
                let mut found = None;
                for &d in &[first_vertical, first_vertical ^ 1] {
                    let cand = with_prop(st, "vertical_direction", DN[d ^ 1]);
                    if self.survives(BlockState(cand), t) {
                        found = Some(cand);
                        break;
                    }
                }
                st = found?;
            }
            "iron_bars" | "stained_glass_pane" | "weathering_copper_bar" | "fence" | "wall" => {}
            "scaffolding" => {
                let dist = self.scaffold_distance(t);
                let below = self.bs(off(t, 0));
                st = with_prop(
                    with_prop(st, "distance", &dist.to_string()),
                    "bottom",
                    if dist > 0 && below.name() != "scaffolding" {
                        "true"
                    } else {
                        "false"
                    },
                );
            }
            "farmland" => {
                if self.is_solid_material(self.bs(off(t, 1))) {
                    st = block_default("dirt")?;
                }
            }
            // an isolated rail runs along the player's look axis
            "rail" | "powered_rail" | "detector_rail" => {
                if lh >= 4 {
                    st = with_prop(st, "shape", "east_west");
                }
            }
            "bamboo_stalk" => {
                let below = self.bs(off(t, 0));
                st = if below.name() == "bamboo" || below.name() == "bamboo_sapling" {
                    st
                } else {
                    block_default("bamboo_sapling")?
                };
            }
            "leaves"
            | "tinted_particle_leaves"
            | "untinted_particle_leaves"
            | "mangrove_leaves" => {
                st = with_prop(st, "persistent", "true");
            }
            _ if k.ends_with("leaves") => {
                st = with_prop(st, "persistent", "true");
            }
            "concrete_powder" => {}
            "multiface" | "glow_lichen" | "sculk_vein" | "vine" => {
                // attach to the neighbour opposite the clicked face (side/top/bottom), merging with an existing one
                let base = if old.name() == name { old.0 } else { st };
                let mut found = None;
                for f in std::iter::once(c.face ^ 1).chain(order.iter().copied()) {
                    if name == "vine" && f == 0 {
                        continue;
                    }
                    let cand = with_prop(base, DN[f], "true");
                    if self.survives(BlockState(cand), t) {
                        found = Some(cand);
                        break;
                    }
                }
                st = found?;
            }
            _ => {
                if has("axis") {
                    st = with_prop(
                        st,
                        "axis",
                        match c.face {
                            0 | 1 => "y",
                            2 | 3 => "z",
                            _ => "x",
                        },
                    );
                }
                if has("facing") {
                    let vertical = it
                        .block()
                        .props
                        .iter()
                        .find(|(n, _)| *n == "facing")
                        .map(|(_, v)| v.contains(&"up"))
                        .unwrap_or(false);
                    let f = if vertical {
                        order[0] ^ 1
                    } else if matches!(k, "chest" | "trapped_chest" | "ender_chest") || true {
                        lh ^ 1
                    } else {
                        lh
                    };
                    st = with_prop(st, "facing", DN[f]);
                }
            }
        }
        // waterlogging: only a water source cell
        if it.block().props.iter().any(|(n, _)| *n == "waterlogged")
            && BlockState(st).get("type") != Some("double")
        {
            st = with_prop(
                st,
                "waterlogged",
                if self.is_water_source(t) {
                    "true"
                } else {
                    "false"
                },
            );
        }
        // concrete powder solidifies in / next to water
        if k == "concrete_powder"
            && (self.is_water(t) || (1..6).any(|d| self.bs(off(t, d)).name() == "water"))
        {
            st = block_default(&name.replace("_powder", ""))?;
        }
        let fin = self.reshape(BlockState(st), t);
        if !self.survives(fin, t) {
            return None;
        }
        let mut out = vec![(t, fin.0)];
        // gravity blocks over open space are placed as usual and fall as falling_block entities (falling.rs)
        for (p, s2) in extra {
            out.push((
                p,
                with_prop(
                    s2,
                    "waterlogged",
                    if self.is_water_source(p) && BlockState(s2).get("waterlogged").is_some() {
                        "true"
                    } else {
                        "false"
                    },
                ),
            ));
        }
        Some(out)
    }

    fn door_hinge(&self, p: [i32; 3], f: usize, cur: [f32; 3], door: BlockState) -> &'static str {
        let (l, r) = (ccw(f), cw(f));
        let full = |q: [i32; 3]| self.sturdy(self.bs(q), 0) && self.sturdy(self.bs(q), 1);
        let (lp, rp) = (off(p, l), off(p, r));
        let i = (if full(lp) { -1 } else { 0 })
            + (if full(off(lp, 1)) { -1 } else { 0 })
            + (if full(rp) { 1 } else { 0 })
            + (if full(off(rp, 1)) { 1 } else { 0 });
        let is_door = |q: [i32; 3]| {
            let n = self.bs(q);
            n.block().name == door.block().name && n.get("half") == Some("lower")
        };
        let (ld, rd) = (is_door(lp), is_door(rp));
        if (ld && !rd) || i > 0 {
            "right"
        } else if (rd && !ld) || i < 0 {
            "left"
        } else {
            let (dx, dz) = (VEC[f][0], VEC[f][2]);
            let right = (dx < 0 && cur[2] < 0.5)
                || (dx > 0 && cur[2] > 0.5)
                || (dz < 0 && cur[0] > 0.5)
                || (dz > 0 && cur[0] < 0.5);
            if right {
                "right"
            } else {
                "left"
            }
        }
    }

    /// Vanilla's "replaceable" for placing a block into the cell (air, water, grass, ...); snow layers only when 1 layer.
    pub(super) fn replaceable_for(&self, s: BlockState) -> bool {
        self.tag("replaceable", s) && !(s.name() == "snow" && s.get("layers") != Some("1"))
    }

    // ------------------------------------------------------------------ neighbour updates

    /// Directions (from the block) of the supports whose change can break it; None = any neighbour update re-checks it.
    fn support_dirs(&self, s: BlockState) -> Option<Vec<usize>> {
        let k = s.block().kind;
        let f = s.get("facing").map(di);
        Some(match k {
            "snow_layer" | "flower" | "sapling" | "tall_grass" | "double_plant" | "tall_flower"
            | "bush" | "mushroom" | "crop" | "carrot" | "potato" | "beetroot" | "carpet"
            | "wool_carpet" | "mossy_carpet" | "dry_vegetation" | "short_dry_grass"
            | "tall_dry_grass" | "scaffolding" | "sculk_vein" | "glow_lichen" | "vine"
            | "multiface" | "sweet_berry_bush" | "eyeblossom" | "wither_rose" | "azalea"
            | "small_dripleaf" | "big_dripleaf" | "cactus" | "sugar_cane" | "bamboo_sapling"
            | "bamboo_stalk" | "kelp" | "seagrass" | "tall_seagrass" | "nether_wart"
            | "nether_roots" | "nether_sprouts" | "nether_fungus" => return None,
            "wall_torch"
            | "redstone_wall_torch"
            | "wall_sign"
            | "wall_banner"
            | "coral_wall_fan"
            | "base_coral_wall_fan"
            | "amethyst_cluster"
            | "ladder"
            | "trip_wire_hook" => vec![f.map(|f| f ^ 1).unwrap_or(0)],
            "wall_hanging_sign" => f.map(|f| vec![cw(f), ccw(f)]).unwrap_or_default(),
            "ceiling_hanging_sign" | "weeping_vines" => vec![1],
            "button" | "lever" | "grindstone" => match s.get("face") {
                Some("floor") => vec![0],
                Some("ceiling") => vec![1],
                _ => vec![f.map(|f| f ^ 1).unwrap_or(0)],
            },
            "lantern" | "weathering_lantern" => vec![if s.get("hanging") == Some("true") {
                1
            } else {
                0
            }],
            "pointed_dripstone" => vec![s.get("vertical_direction").map(di).unwrap_or(1) ^ 1],
            _ => vec![0],
        })
    }

    /// After the block at `p` changed: re-shape / break the six neighbours (cascading a few levels).
    pub(super) fn update_neighbors(&mut self, p: [i32; 3], depth: u32) {
        if depth > 6 {
            return;
        }
        for d in [4usize, 5, 2, 3, 0, 1] {
            let q = off(p, d);
            let s = self.bs(q);
            if s.is_air() {
                continue;
            }
            let mut n = self.reshape(s, q);
            if !self.survives(n, q) {
                let hit = match self.support_dirs(s) {
                    None => true,
                    Some(v) => v.contains(&(d ^ 1)),
                };
                if hit {
                    n = BlockState(if n.get("waterlogged") == Some("true") {
                        block_default("water").unwrap_or(0)
                    } else {
                        0
                    });
                }
            }
            // farmland turns to dirt when something solid sits on it
            if n.name() == "farmland" && d == 0 && self.is_solid_material(self.bs(p)) {
                n = BlockState(block_default("dirt").unwrap_or(0));
            }
            if n.0 != s.0 {
                self.set_block(q[0], q[1], q[2], n.0);
                self.update_neighbors(q, depth + 1);
            }
        }
    }

    /// `/setblock`: the block takes the shape its neighbours give it (fence/wall/pane connections) and its neighbours update.
    pub(super) fn set_block_shaped(&mut self, x: i32, y: i32, z: i32, state: u32) {
        let st = self.reshape(BlockState(state), [x, y, z]).0;
        self.set_block(x, y, z, st);
        self.update_neighbors([x, y, z], 0);
    }

    /// Right-click on a door / trapdoor / fence gate toggles it. Returns true if the click was consumed.
    pub(super) fn use_block(&mut self, b: [i32; 3]) -> bool {
        let s = self.bs(b);
        let k = s.block().kind;
        let toggles = match k {
            "trapdoor" => s.name() != "iron_trapdoor",
            "weathering_copper_trapdoor" => true,
            "door" => s.name() != "iron_door",
            "weathering_copper_door" => true,
            "fence_gate" => true,
            _ => false,
        };
        if !toggles {
            return false;
        }
        let open = if s.get("open") == Some("true") {
            "false"
        } else {
            "true"
        };
        let ns = with_prop(s.0, "open", open);
        self.set_block(b[0], b[1], b[2], ns);
        self.update_neighbors(b, 0);
        if matches!(k, "door" | "weathering_copper_door") {
            let o = off(b, if s.get("half") == Some("upper") { 0 } else { 1 });
            let os = self.bs(o);
            if os.block().name == s.block().name {
                self.set_block(o[0], o[1], o[2], with_prop(os.0, "open", open));
            }
        }
        true
    }
}

fn segment(deg: f32) -> i32 {
    ((deg * 16.0 / 360.0 + 0.5).floor() as i32) & 15
}

/// Wall-mounted variant of a standing block ("torch" -> "wall_torch", "oak_sign" -> "oak_wall_sign", "creeper_head" -> "creeper_wall_head").
fn wall_variant(name: &str) -> Option<String> {
    Some(match name {
        n if n.ends_with("torch") => n.replace("torch", "wall_torch"),
        n if n.ends_with("_sign") => n.replace("_sign", "_wall_sign"),
        n if n.ends_with("_banner") => n.replace("_banner", "_wall_banner"),
        n if n.ends_with("_head") => n.replace("_head", "_wall_head"),
        n if n.ends_with("_skull") => n.replace("_skull", "_wall_skull"),
        n if n.ends_with("coral_fan") => n.replace("coral_fan", "coral_wall_fan"),
        _ => return None,
    })
}

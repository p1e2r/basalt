//! Per-block-state physical properties needed for lighting and heightmaps.
//!
//! The data generator reports don't include light emission, light opacity or collision,
//! so these are **approximations** derived from block names / definition kinds and state
//! properties:
//! - opacity 15 for full opaque cubes (plain "block" kinds, logs, ores, dirt, ...), double
//!   slabs and full snow layers; 1 for water, lava, waterlogged blocks, leaves, ice, cobweb,
//!   slime/honey; 0 for everything else (air, glass, plants, slabs, stairs, fences, ...).
//!   Vanilla's directional (face-shape) occlusion for slabs/stairs is not modelled.
//! - emission from a hand-written table (torch 14, glowstone 15, lava 15, magma 3, ...),
//!   honouring `lit`, `level`, `candles`, `pickles`, `charges` and `berries` properties.
//! - "blocks motion" (for MOTION_BLOCKING / OCEAN_FLOOR heightmaps) is true for
//!   everything except air, fluids and a list of collision-less kinds (plants, torches,
//!   rails, signs, ...).

use basalt_data::BlockState;
use std::sync::OnceLock;

pub const FLAG_AIR: u8 = 1;
/// Contains a fluid (water/lava source or flowing, waterlogged, seagrass/kelp/bubble column).
pub const FLAG_FLUID: u8 = 2;
pub const FLAG_MOTION_BLOCKING: u8 = 4;
pub const FLAG_LEAVES: u8 = 8;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlockProps {
    /// Light emitted (0..=15).
    pub emission: u8,
    /// Light lost when passing into this block (0..=15; 15 = fully opaque).
    pub opacity: u8,
    pub flags: u8,
}

impl BlockProps {
    #[inline]
    pub fn is_air(&self) -> bool {
        self.flags & FLAG_AIR != 0
    }
    #[inline]
    pub fn has_fluid(&self) -> bool {
        self.flags & FLAG_FLUID != 0
    }
    #[inline]
    pub fn blocks_motion(&self) -> bool {
        self.flags & FLAG_MOTION_BLOCKING != 0
    }
    #[inline]
    pub fn is_leaves(&self) -> bool {
        self.flags & FLAG_LEAVES != 0
    }
}

fn table() -> &'static [BlockProps] {
    static T: OnceLock<Vec<BlockProps>> = OnceLock::new();
    T.get_or_init(|| {
        (0..basalt_data::blocks::STATE_COUNT as u32)
            .map(|s| classify(BlockState(s)))
            .collect()
    })
}

/// Properties of a block state id (unknown ids are treated as air).
#[inline]
pub fn props(state: u32) -> BlockProps {
    table().get(state as usize).copied().unwrap_or(BlockProps {
        emission: 0,
        opacity: 0,
        flags: FLAG_AIR,
    })
}
#[inline]
pub fn emission(state: u32) -> u8 {
    props(state).emission
}
#[inline]
pub fn opacity(state: u32) -> u8 {
    props(state).opacity
}
#[inline]
pub fn is_air(state: u32) -> bool {
    props(state).is_air()
}

/// Kinds (data-generator definition types) that are full opaque cubes.
const OPAQUE_KINDS: &[&str] = &[
    "block",
    "rotated_pillar",
    "drop_experience",
    "grass",
    "snowy_dirt",
    "mycelium",
    "nylium",
    "sand",
    "colored_falling",
    "concrete_powder",
    "glazed_terracotta",
    "infested",
    "infested_rotated_pillar",
    "coral",
    "huge_mushroom",
    "hay",
    "sponge",
    "wet_sponge",
    "netherrack",
    "magma",
    "mud",
    "soul_sand",
    "redstone_ore",
    "redstone_lamp",
    "note",
    "jukebox",
    "furnace",
    "blast_furnace",
    "smoker",
    "dispenser",
    "dropper",
    "crafting_table",
    "cartography_table",
    "smithing_table",
    "loom",
    "barrel",
    "beehive",
    "target",
    "tnt",
    "pumpkin",
    "jack_o_lantern",
    "chiseled_book_shelf",
    "crafter",
    "observer",
    "piston_base",
    "command",
    "structure",
    "jigsaw",
    "budding_amethyst",
    "amethyst",
    "crying_obsidian",
    "powered",
    "respawn_anchor",
    "sculk",
    "sculk_catalyst",
    "bonemealable_feature_placer",
    "rooted_dirt",
    "dirt_path",
    "farmland",
    "weathering_copper_full",
    "copper_bulb_block",
    "weathering_copper_bulb",
    "brushable",
    "test",
    "test_instance",
    "half_transparent",
    "potent_sulfur",
];

/// Kinds with no collision box (don't count for MOTION_BLOCKING / OCEAN_FLOOR).
const NO_COLLISION_KINDS: &[&str] = &[
    "air",
    "liquid",
    "flower",
    "tall_grass",
    "double_plant",
    "tall_flower",
    "sapling",
    "mushroom",
    "torch",
    "wall_torch",
    "redstone_torch",
    "redstone_wall_torch",
    "redstone_wire",
    "rail",
    "powered_rail",
    "detector_rail",
    "button",
    "lever",
    "pressure_plate",
    "weighted_pressure_plate",
    "tripwire",
    "trip_wire_hook",
    "fire",
    "soul_fire",
    "seagrass",
    "tall_seagrass",
    "kelp",
    "kelp_plant",
    "vine",
    "glow_lichen",
    "sculk_vein",
    "multiface",
    "crop",
    "carrot",
    "potato",
    "beetroot",
    "stem",
    "attached_stem",
    "sugar_cane",
    "nether_wart",
    "nether_roots",
    "nether_sprouts",
    "nether_fungus",
    "coral_plant",
    "base_coral_plant",
    "coral_fan",
    "base_coral_fan",
    "coral_wall_fan",
    "base_coral_wall_fan",
    "cave_vines",
    "cave_vines_plant",
    "weeping_vines",
    "weeping_vines_plant",
    "twisting_vines",
    "twisting_vines_plant",
    "hanging_roots",
    "spore_blossom",
    "sweet_berry_bush",
    "bush",
    "dry_vegetation",
    "short_dry_grass",
    "tall_dry_grass",
    "firefly_bush",
    "leaf_litter",
    "flower_bed",
    "eyeblossom",
    "wither_rose",
    "torchflower_crop",
    "pitcher_crop",
    "standing_sign",
    "wall_sign",
    "wall_banner",
    "banner",
    "ceiling_hanging_sign",
    "wall_hanging_sign",
    "nether_portal",
    "end_portal",
    "end_gateway",
    "light",
    "structure_void",
    "web",
    "bubble_column",
    "hanging_moss",
    "mangrove_propagule",
    "big_dripleaf_stem",
    "small_dripleaf",
    "sulfur_spike",
    "moving_piston",
];

fn prop_num(s: BlockState, p: &str) -> u8 {
    s.get(p).and_then(|v| v.parse().ok()).unwrap_or(0)
}
fn prop_true(s: BlockState, p: &str) -> bool {
    s.get(p) == Some("true")
}

fn emission_of(s: BlockState, name: &str, kind: &str) -> u8 {
    let lit = || prop_true(s, "lit");
    match name {
        "glowstone"
        | "sea_lantern"
        | "jack_o_lantern"
        | "shroomlight"
        | "beacon"
        | "lava"
        | "fire"
        | "conduit"
        | "end_gateway"
        | "end_portal"
        | "lava_cauldron"
        | "ochre_froglight"
        | "verdant_froglight"
        | "pearlescent_froglight" => 15,
        "torch" | "wall_torch" | "end_rod" | "copper_torch" | "copper_wall_torch" => 14,
        "soul_torch" | "soul_wall_torch" | "soul_lantern" | "soul_fire" | "crying_obsidian" => 10,
        "nether_portal" => 11,
        "magma_block" => 3,
        "enchanting_table" | "ender_chest" => 7,
        // glow lichen only glows with at least one face (measured with the client-mesh oracle)
        "glow_lichen" => {
            if ["down", "up", "north", "south", "west", "east"]
                .iter()
                .any(|f| prop_true(s, f))
            {
                7
            } else {
                0
            }
        }
        // Light levels below were observed from the vanilla 26.2 client (client-mesh oracle, 2026-11-23); state -> level.
        "trial_spawner" => match s.get("trial_spawner_state") {
            Some("waiting_for_players") => 4,
            Some("active" | "waiting_for_reward_ejection" | "ejecting_reward") => 8,
            _ => 0,
        },
        "vault" => {
            if s.get("vault_state") == Some("inactive") {
                6
            } else {
                12
            }
        }
        "sculk_catalyst" => 6,
        "amethyst_cluster" => 5,
        "large_amethyst_bud" => 4,
        "medium_amethyst_bud" | "firefly_bush" => 2,
        "small_amethyst_bud"
        | "brewing_stand"
        | "brown_mushroom"
        | "dragon_egg"
        | "end_portal_frame"
        | "sculk_sensor"
        | "calibrated_sculk_sensor" => 1,
        "redstone_torch" | "redstone_wall_torch" => {
            if lit() {
                7
            } else {
                0
            }
        }
        "redstone_ore" | "deepslate_redstone_ore" => {
            if lit() {
                9
            } else {
                0
            }
        }
        "redstone_lamp" => {
            if lit() {
                15
            } else {
                0
            }
        }
        "furnace" | "blast_furnace" | "smoker" => {
            if lit() {
                13
            } else {
                0
            }
        }
        "campfire" => {
            if lit() {
                15
            } else {
                0
            }
        }
        "soul_campfire" => {
            if lit() {
                10
            } else {
                0
            }
        }
        "light" => prop_num(s, "level"),
        "respawn_anchor" => [0, 3, 7, 11, 15][prop_num(s, "charges").min(4) as usize],
        "sea_pickle" => {
            if prop_true(s, "waterlogged") {
                3 + 3 * prop_num(s, "pickles")
            } else {
                0
            }
        }
        "cave_vines" | "cave_vines_plant" => {
            if prop_true(s, "berries") {
                14
            } else {
                0
            }
        }
        _ if kind == "candle" => {
            if lit() {
                3 * prop_num(s, "candles")
            } else {
                0
            }
        }
        _ if kind == "candle_cake" => {
            if lit() {
                3
            } else {
                0
            }
        }
        _ if kind == "lantern" || kind == "weathering_lantern" => 15,
        _ if name.ends_with("copper_bulb") => {
            if !lit() {
                0
            } else if name.contains("oxidized") {
                4
            } else if name.contains("weathered") {
                8
            } else if name.contains("exposed") {
                12
            } else {
                15
            }
        }
        _ => 0,
    }
}

fn classify(s: BlockState) -> BlockProps {
    let b = s.block();
    let name = b.name;
    let kind = b.kind;
    let air = kind == "air";
    let waterlogged = prop_true(s, "waterlogged");
    let fluid = kind == "liquid"
        || waterlogged
        || matches!(
            name,
            "bubble_column" | "kelp" | "kelp_plant" | "seagrass" | "tall_seagrass"
        );
    let leaves = kind.ends_with("leaves");
    let mut opacity = if air {
        0
    } else if OPAQUE_KINDS.contains(&kind) {
        15
    } else if kind == "slab" {
        if s.get("type") == Some("double") {
            15
        } else {
            0
        }
    } else if kind == "snow_layer" {
        if prop_num(s, "layers") >= 8 {
            15
        } else {
            0
        }
    } else if name == "tinted_glass" {
        15
    } else if fluid || leaves || matches!(kind, "ice" | "frosted_ice" | "web" | "slime" | "honey") {
        1
    } else {
        0
    };
    // The few transparent-ish blocks that are of the plain "block" kind.
    if matches!(name, "spawner" | "barrier") {
        opacity = 0;
    }
    let mut flags = 0;
    if air {
        flags |= FLAG_AIR;
    }
    if fluid {
        flags |= FLAG_FLUID;
    }
    if leaves {
        flags |= FLAG_LEAVES;
    }
    let no_collision =
        NO_COLLISION_KINDS.contains(&kind) || (kind == "snow_layer" && prop_num(s, "layers") <= 1);
    if !no_collision {
        flags |= FLAG_MOTION_BLOCKING;
    }
    BlockProps {
        emission: emission_of(s, name, kind),
        opacity,
        flags,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn p(n: &str) -> BlockProps {
        props(BlockState::parse(n).unwrap().0)
    }
    #[test]
    fn table_sanity() {
        assert!(p("air").is_air() && p("cave_air").is_air());
        assert_eq!(p("stone").opacity, 15);
        assert_eq!(p("glass").opacity, 0);
        assert_eq!(p("oak_leaves").opacity, 1);
        assert!(p("oak_leaves").is_leaves());
        assert_eq!(p("water").opacity, 1);
        assert!(p("water").has_fluid() && !p("water").blocks_motion());
        assert_eq!(p("torch").emission, 14);
        assert_eq!(p("glowstone").emission, 15);
        assert_eq!(p("lava").emission, 15);
        assert_eq!(p("magma_block").emission, 3);
        assert_eq!(p("redstone_lamp[lit=true]").emission, 15);
        assert_eq!(p("redstone_lamp[lit=false]").emission, 0);
        assert!(!p("short_grass").blocks_motion());
        assert!(p("oak_fence").blocks_motion());
        assert_eq!(p("oak_slab[type=double]").opacity, 15);
        assert_eq!(p("oak_slab[type=bottom]").opacity, 0);
    }
}

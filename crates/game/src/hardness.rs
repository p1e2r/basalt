//! Block hardness and "requires correct tool for drops" — neither is in the data reports, so this
//! table is built from public knowledge (Minecraft Wiki "Breaking" / per-block infobox values),
//! keyed by block name and the block's definition kind from `blocks.json`.

fn kind_of(name: &str) -> &'static str {
    basalt_data::BlockState::from_name(name).map(|s| s.block().kind).unwrap_or("")
}

/// Hardness of a block by name (with or without `minecraft:`). -1 = unbreakable.
/// Unknown names get 1.0.
pub fn hardness(block_name: &str) -> f32 {
    let n = block_name.trim_start_matches("minecraft:");
    lookup(n, kind_of(n)).unwrap_or(1.0)
}

/// Base block for a slab/stairs/wall name ("stone_brick_slab" → "stone_bricks").
fn base_of(n: &str) -> Option<String> {
    let stem = n.strip_suffix("_slab").or_else(|| n.strip_suffix("_stairs")).or_else(|| n.strip_suffix("_wall"))?;
    let stem = stem.strip_prefix("waxed_").unwrap_or(stem);
    [format!("{stem}_planks"), stem.to_string(), format!("{stem}s"), format!("{stem}_block")]
        .into_iter()
        .find(|c| basalt_data::BlockState::from_name(c).is_some())
}

pub(crate) fn lookup(n: &str, kind: &str) -> Option<f32> {
    // Unbreakable technical blocks.
    let h = match n {
        "bedrock" | "barrier" | "light" | "command_block" | "chain_command_block" | "repeating_command_block" | "structure_block"
        | "jigsaw" | "end_portal" | "end_portal_frame" | "end_gateway" | "nether_portal" | "moving_piston" | "test_block"
        | "test_instance_block" => return Some(-1.0),
        "water" | "lava" => return Some(100.0),
        "air" | "cave_air" | "void_air" | "structure_void" | "fire" | "soul_fire" | "bubble_column" => return Some(0.0),
        _ => match kind {
            // ---- shapes ----
            "slab" | "weathering_copper_slab" => {
                if matches!(
                    n,
                    "stone_slab" | "smooth_stone_slab" | "sandstone_slab" | "cut_sandstone_slab" | "red_sandstone_slab"
                        | "cut_red_sandstone_slab" | "cobblestone_slab" | "brick_slab" | "stone_brick_slab"
                        | "nether_brick_slab" | "quartz_slab" | "purpur_slab" | "petrified_oak_slab" | "smooth_quartz_slab"
                        | "smooth_sandstone_slab" | "smooth_red_sandstone_slab" | "blackstone_slab"
                        | "polished_blackstone_slab" | "polished_blackstone_brick_slab"
                ) {
                    2.0
                } else {
                    lookup_base(n)?
                }
            }
            "stair" | "weathering_copper_stair" | "wall" => lookup_base(n)?,
            "door" | "weathering_copper_door" => {
                if n == "iron_door" { 5.0 } else { 3.0 }
            }
            "trapdoor" | "weathering_copper_trapdoor" => {
                if n == "iron_trapdoor" { 5.0 } else { 3.0 }
            }
            "fence" | "fence_gate" => 2.0,
            "button" | "pressure_plate" | "weighted_pressure_plate" | "lever" => 0.5,
            "standing_sign" | "wall_sign" | "ceiling_hanging_sign" | "wall_hanging_sign" | "banner" | "wall_banner" => 1.0,
            "shelf" => 2.0,
            "candle" => 0.1,
            "candle_cake" | "cake" => 0.5,
            "shulker_box" => 2.0,
            "bed" => 0.2,
            "wool_carpet" | "carpet" | "mossy_carpet" | "snow_layer" => 0.1,
            "concrete_powder" => 0.5,
            "glazed_terracotta" => 1.4,
            "stained_glass" | "stained_glass_pane" | "transparent" | "tinted_glass" => 0.3,
            "iron_bars" | "weathering_copper_bar" => {
                if n == "glass_pane" { 0.3 } else { 5.0 }
            }
            "chain" | "weathering_copper_chain" => 5.0,
            "lantern" | "weathering_lantern" => 3.5,
            "weathering_copper_full" | "weathering_copper_bulb" | "copper_bulb_block" | "weathering_copper_grate"
            | "waterlogged_transparent" | "weathering_copper_chest" | "copper_chest" | "weathering_copper_golem_statue"
            | "copper_golem_statue" | "weathering_lightning_rod" | "lightning_rod" => 3.0,
            "drop_experience" | "redstone_ore" => {
                if n.starts_with("deepslate_") { 4.5 } else { 3.0 }
            }
            "infested" | "infested_rotated_pillar" => match n {
                "infested_cobblestone" => 1.0,
                "infested_deepslate" => 1.5,
                _ => 0.75,
            },
            "flower" | "tall_flower" | "sapling" | "flower_pot" | "torch" | "wall_torch" | "redstone_torch"
            | "redstone_wall_torch" | "redstone_wire" | "repeater" | "comparator" | "tripwire" | "trip_wire_hook"
            | "double_plant" | "tall_grass" | "attached_stem" | "stem" | "azalea" | "mushroom" | "eyeblossom" | "nether_fungus"
            | "nether_roots" | "flower_bed" | "coral_plant" | "coral_fan" | "coral_wall_fan" | "base_coral_plant"
            | "base_coral_fan" | "base_coral_wall_fan" | "crop" | "beetroot" | "carrot" | "potato" | "nether_wart"
            | "sugar_cane" | "kelp" | "kelp_plant" | "seagrass" | "tall_seagrass" | "lily_pad" | "sea_pickle" | "dry_vegetation"
            | "bush" | "firefly_bush" | "cactus_flower" | "short_dry_grass" | "tall_dry_grass" | "leaf_litter"
            | "hanging_moss" | "hanging_roots" | "spore_blossom" | "cave_vines" | "cave_vines_plant" | "twisting_vines"
            | "twisting_vines_plant" | "weeping_vines" | "weeping_vines_plant" | "small_dripleaf" | "sweet_berry_bush"
            | "pitcher_crop" | "torchflower_crop" | "mangrove_propagule" | "nether_sprouts" | "frogspawn" | "end_rod"
            | "scaffolding" | "tnt" | "slime" | "honey" | "decorated_pot" | "multiface" | "dried_ghast" | "wither_rose" => 0.0,
            "tinted_particle_leaves" | "untinted_particle_leaves" | "mangrove_leaves" | "huge_mushroom" | "vine"
            | "glow_lichen" | "sculk_vein" | "sculk" | "cocoa" | "daylight_detector" => 0.2,
            "skull" | "wall_skull" | "piglinwallskull" | "player_head" | "player_wall_head" | "wither_skull"
            | "wither_wall_skull" => 1.0,
            "amethyst_cluster" | "amethyst" | "budding_amethyst" => 1.5,
            "coral" => 1.5,
            "anvil" => 5.0,
            "powered_rail" | "rail" | "detector_rail" => 0.7,
            "beehive" => {
                if n == "bee_nest" { 0.3 } else { 0.6 }
            }
            "campfire" => 2.0,
            "jack_o_lantern" | "pumpkin" => 1.0,
            "nylium" => 0.4,
            "liquid" => 100.0,
            "bonemealable_feature_placer" => 0.1,
            "piston_base" | "piston_head" => 1.5,
            "layered_cauldron" | "lava_cauldron" | "cauldron" => 2.0,
            "sand" => 0.5,
            "brushable" => 0.25,
            "bamboo_stalk" | "bamboo_sapling" => 1.0,
            "big_dripleaf" | "big_dripleaf_stem" => 0.1,
            "chorus_flower" | "chorus_plant" | "cactus" => 0.4,
            "rotated_pillar" => match n {
                "basalt" | "polished_basalt" => 1.25,
                "deepslate" => 3.0,
                "muddy_mangrove_roots" => 0.7,
                "ochre_froglight" | "pearlescent_froglight" | "verdant_froglight" => 0.3,
                "purpur_pillar" => 1.5,
                "quartz_pillar" => 0.8,
                _ => 2.0, // logs, wood, stems, hyphae, bamboo blocks, bone block
            },
            _ => return lookup_name(n),
        },
    };
    Some(h)
}

fn lookup_base(n: &str) -> Option<f32> {
    let b = base_of(n)?;
    lookup(&b, kind_of(&b))
}

/// Blocks of kind "block" and the one-off kinds.
fn lookup_name(n: &str) -> Option<f32> {
    let h = match n {
        "stone" | "granite" | "diorite" | "andesite" | "polished_granite" | "polished_diorite" | "polished_andesite"
        | "stone_bricks" | "mossy_stone_bricks" | "cracked_stone_bricks" | "chiseled_stone_bricks" | "tuff" | "polished_tuff"
        | "tuff_bricks" | "chiseled_tuff" | "chiseled_tuff_bricks" | "dripstone_block" | "pointed_dripstone" | "blackstone"
        | "polished_blackstone_bricks" | "cracked_polished_blackstone_bricks" | "chiseled_polished_blackstone"
        | "gilded_blackstone" | "prismarine" | "prismarine_bricks" | "dark_prismarine" | "purpur_block" | "mud_bricks"
        | "resin_bricks" | "chiseled_resin_bricks" | "bookshelf" | "chiseled_bookshelf" | "crafter" | "sculk_sensor"
        | "calibrated_sculk_sensor" => 1.5,
        // 26.x additions (cinnabar / sulfur families): no published values seen; stone-like guess.
        "cinnabar" | "polished_cinnabar" | "cinnabar_bricks" | "chiseled_cinnabar" | "sulfur" | "polished_sulfur"
        | "sulfur_bricks" | "chiseled_sulfur" | "potent_sulfur" | "sulfur_spike" => 1.5,
        "cobblestone" | "mossy_cobblestone" | "smooth_stone" | "bricks" | "nether_bricks" | "red_nether_bricks"
        | "cracked_nether_bricks" | "chiseled_nether_bricks" | "polished_blackstone" | "smooth_quartz" | "smooth_sandstone"
        | "smooth_red_sandstone" | "bone_block" | "grindstone" | "jukebox" => 2.0,
        "cobbled_deepslate" | "polished_deepslate" | "deepslate_bricks" | "deepslate_tiles" | "cracked_deepslate_bricks"
        | "cracked_deepslate_tiles" | "chiseled_deepslate" | "lodestone" | "furnace" | "blast_furnace" | "smoker"
        | "dispenser" | "dropper" | "stonecutter" => 3.5,
        "reinforced_deepslate" => 55.0,
        "calcite" => 0.75,
        "smooth_basalt" => 1.25,
        "end_stone" | "end_stone_bricks" | "gold_block" | "lapis_block" | "beacon" | "conduit" | "observer" | "hopper"
        | "dragon_egg" | "sculk_catalyst" | "sculk_shrieker" => 3.0,
        "netherrack" => 0.4,
        "quartz_block" | "quartz_bricks" | "chiseled_quartz_block" | "sandstone" | "cut_sandstone" | "chiseled_sandstone"
        | "red_sandstone" | "cut_red_sandstone" | "chiseled_red_sandstone" | "note_block" => 0.8,
        "obsidian" | "crying_obsidian" | "netherite_block" | "respawn_anchor" | "trial_spawner" | "vault" => 50.0,
        "ancient_debris" => 30.0,
        "magma_block" | "dirt" | "coarse_dirt" | "rooted_dirt" | "podzol" | "mud" | "soul_sand" | "soul_soil"
        | "dried_kelp_block" | "hay_block" | "ice" | "packed_ice" | "frosted_ice" | "turtle_egg" | "sniffer_egg"
        | "target" | "brewing_stand" => 0.5,
        "glowstone" | "sea_lantern" | "redstone_lamp" | "bee_nest" => 0.3,
        "shroomlight" | "melon" | "nether_wart_block" | "warped_wart_block" | "packed_mud" => 1.0,
        "terracotta" => 1.25,
        "clay" | "grass_block" | "mycelium" | "farmland" | "gravel" | "honeycomb_block" | "sponge" | "wet_sponge"
        | "composter" => 0.6,
        "dirt_path" => 0.65,
        "mangrove_roots" => 0.7,
        "snow_block" => 0.2,
        "powder_snow" => 0.25,
        "blue_ice" => 2.8,
        "coal_block" | "iron_block" | "diamond_block" | "emerald_block" | "redstone_block" | "raw_iron_block"
        | "raw_copper_block" | "raw_gold_block" | "enchanting_table" | "spawner" | "bell" => 5.0,
        "crafting_table" | "cartography_table" | "fletching_table" | "smithing_table" | "loom" | "chest"
        | "trapped_chest" | "barrel" | "lectern" => 2.5,
        "ender_chest" => 22.5,
        "cobweb" => 4.0,
        "ladder" => 0.4,
        "creaking_heart" | "heavy_core" => 10.0,
        "resin_block" => 0.0,
        _ => {
            if n.ends_with("_planks") || n == "bamboo_mosaic" {
                2.0
            } else if n.ends_with("_concrete") {
                1.8
            } else if n.ends_with("_terracotta") {
                1.25
            } else if n.ends_with("_wool") {
                0.8
            } else if n.ends_with("_coral_block") {
                1.5
            } else if n.contains("copper") {
                3.0
            } else {
                return None;
            }
        }
    };
    Some(h)
}

/// Whether the block drops nothing (and mines at 1/100 instead of 1/30 speed) unless broken with
/// a tool whose rule has `correct_for_drops`. Public-knowledge list: nearly all pickaxe blocks
/// (stone, ores, metal, bricks, …) plus cobweb and snow.
pub fn requires_correct_tool(block_name: &str) -> bool {
    let n = block_name.trim_start_matches("minecraft:");
    let kind = kind_of(n);
    // Pickaxe-mineable blocks that still drop when mined by hand.
    if matches!(
        n,
        "ice" | "packed_ice" | "blue_ice" | "frosted_ice" | "piston" | "sticky_piston" | "piston_head" | "moving_piston"
            | "rail" | "powered_rail" | "detector_rail" | "activator_rail" | "conduit" | "bell" | "pointed_dripstone"
            | "amethyst_cluster" | "large_amethyst_bud" | "medium_amethyst_bud" | "small_amethyst_bud" | "heavy_core"
            | "glass" | "glass_pane" | "tinted_glass" | "copper_torch" | "copper_wall_torch" | "decorated_pot"
    ) {
        return false;
    }
    if matches!(n, "cobweb" | "snow" | "snow_block") {
        return true;
    }
    if !matches!(kind, "slab" | "stair" | "wall") && is_wood_or_soft(n) {
        return false;
    }
    match kind {
        "slab" | "stair" | "wall" => {
            if n == "petrified_oak_slab" {
                return true;
            }
            base_of(n).is_some_and(|b| requires_correct_tool(&b))
        }
        "door" | "trapdoor" => n.contains("iron") || n.contains("copper"),
        "button" | "pressure_plate" => n.contains("stone") || n.contains("blackstone"),
        "weighted_pressure_plate" | "drop_experience" | "redstone_ore" | "infested" | "infested_rotated_pillar"
        | "glazed_terracotta" | "iron_bars" | "chain" | "lantern" | "anvil" | "amethyst" | "budding_amethyst" | "coral"
        | "weathering_copper_full" | "weathering_copper_bar" | "weathering_copper_bulb" | "weathering_copper_chain"
        | "weathering_copper_chest" | "weathering_copper_door" | "weathering_copper_golem_statue"
        | "weathering_copper_grate" | "weathering_lantern" | "weathering_copper_trapdoor" | "weathering_copper_slab"
        | "weathering_copper_stair" | "weathering_lightning_rod" | "copper_bulb_block" | "copper_chest"
        | "copper_golem_statue" | "waterlogged_transparent" | "lightning_rod" | "layered_cauldron" | "lava_cauldron"
        | "cauldron" | "nylium" => n != "glass_pane",
        "fence" => n == "nether_brick_fence",
        "rotated_pillar" => matches!(n, "basalt" | "polished_basalt" | "deepslate" | "purpur_pillar" | "quartz_pillar" | "bone_block"),
        "block" => !matches!(
            n,
            "bookshelf" | "clay" | "coarse_dirt" | "dirt" | "dried_kelp_block" | "fletching_table" | "glowstone" | "honeycomb_block"
                | "melon" | "nether_wart_block" | "warped_wart_block" | "packed_ice" | "packed_mud" | "sea_lantern" | "shroomlight"
                | "soul_soil" | "snow_block" | "resin_block" | "bedrock" | "reinforced_deepslate"
        ) && !n.ends_with("_planks")
            && !n.ends_with("_wool")
            && n != "bamboo_mosaic",
        _ => matches!(
            n,
            "netherrack" | "magma_block" | "obsidian" | "crying_obsidian" | "respawn_anchor" | "ender_chest" | "enchanting_table"
                | "spawner" | "dispenser" | "dropper" | "furnace" | "blast_furnace" | "smoker"
                | "stonecutter" | "grindstone" | "observer" | "hopper" | "brewing_stand"
                | "crafter" | "lodestone" | "smooth_basalt" | "redstone_block" | "pointed_dripstone"
                | "sulfur_spike" | "potent_sulfur"
        ),
    }
}

/// Names that are never tool-gated regardless of kind (wood, plants, soft blocks).
fn is_wood_or_soft(n: &str) -> bool {
    n.ends_with("_planks") || n.ends_with("_log") || n.ends_with("_wood") || n.ends_with("_stem") || n.ends_with("_hyphae")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_block_has_hardness() {
        let missing: Vec<&str> =
            basalt_data::blocks::BLOCKS.iter().filter(|b| lookup(b.name, b.kind).is_none()).map(|b| b.name).collect();
        assert!(missing.is_empty(), "no hardness for {} blocks: {:?}", missing.len(), missing);
    }

    #[test]
    fn well_known_hardness() {
        for (n, h) in [
            ("stone", 1.5),
            ("cobblestone", 2.0),
            ("dirt", 0.5),
            ("sand", 0.5),
            ("oak_log", 2.0),
            ("oak_planks", 2.0),
            ("oak_leaves", 0.2),
            ("obsidian", 50.0),
            ("bedrock", -1.0),
            ("iron_ore", 3.0),
            ("deepslate_iron_ore", 4.5),
            ("glass", 0.3),
            ("white_wool", 0.8),
            ("poppy", 0.0),
            ("short_grass", 0.0),
            ("grass_block", 0.6),
            ("gravel", 0.6),
            ("deepslate", 3.0),
            ("crafting_table", 2.5),
            ("furnace", 3.5),
            ("stone_brick_stairs", 1.5),
            ("cobblestone_wall", 2.0),
            ("oak_slab", 2.0),
            ("ender_chest", 22.5),
            ("netherrack", 0.4),
            ("ancient_debris", 30.0),
            ("torch", 0.0),
        ] {
            assert_eq!(hardness(n), h, "{n}");
        }
    }

    #[test]
    fn correct_tool_flags() {
        for n in ["stone", "cobblestone", "iron_ore", "obsidian", "cobblestone_slab", "iron_block", "cobweb", "snow", "andesite_wall", "iron_door"] {
            assert!(requires_correct_tool(n), "{n}");
        }
        for n in ["dirt", "oak_log", "oak_planks", "oak_slab", "glass", "white_wool", "poppy", "oak_door", "ice", "sand", "crafting_table"] {
            assert!(!requires_correct_tool(n), "{n}");
        }
    }
}

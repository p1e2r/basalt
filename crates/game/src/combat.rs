//! Combat math from public documentation (Minecraft Wiki "Armor", "Explosion", "Difficulty").

/// Damage left after armor: `damage * (1 - min(20, max(armor / 5, armor - 4 * damage / (toughness + 8))) / 25)`.
pub fn armor_reduced(damage: f32, armor: f32, toughness: f32) -> f32 {
    if damage <= 0.0 {
        return 0.0;
    }
    let armor = armor.clamp(0.0, 30.0);
    let eff = (armor / 5.0).max(armor - 4.0 * damage / (toughness.max(0.0) + 8.0)).min(20.0);
    damage * (1.0 - eff / 25.0)
}

/// Mob-vs-player damage scaled by difficulty (0 peaceful, 1 easy, 2 normal, 3 hard) from the
/// Normal value: Easy = min(d / 2 + 1, d), Hard = d * 1.5, Peaceful = 0.
pub fn difficulty_scaled(normal: f32, difficulty: u8) -> f32 {
    match difficulty {
        0 => 0.0,
        1 => (normal / 2.0 + 1.0).min(normal),
        2 => normal,
        _ => normal * 1.5,
    }
}

/// Blast resistance by block name. An approximation of the Minecraft Wiki per-block values,
/// keyed by name patterns; unknown blocks fall back to their hardness.
pub fn blast_resistance(block_name: &str) -> f32 {
    let n = block_name.trim_start_matches("minecraft:");
    match n {
        "air" | "cave_air" | "void_air" => return 0.0,
        "bedrock" | "barrier" | "command_block" | "chain_command_block" | "repeating_command_block" | "structure_block" | "jigsaw"
        | "end_portal" | "end_portal_frame" | "end_gateway" | "light" | "reinforced_deepslate" | "test_block" | "test_instance_block" => {
            return 3_600_000.0
        }
        "obsidian" | "crying_obsidian" | "ancient_debris" | "netherite_block" | "respawn_anchor" | "enchanting_table" | "anvil"
        | "chipped_anvil" | "damaged_anvil" => return 1200.0,
        "water" | "lava" => return 100.0,
        "end_stone" | "end_stone_bricks" => return 9.0,
        "ender_chest" => return 600.0,
        "glass" | "glass_pane" | "glowstone" | "sea_lantern" | "redstone_lamp" | "beacon" => return 0.3,
        "netherrack" | "nether_gold_ore" | "nether_quartz_ore" => return 0.4,
        "dirt" | "coarse_dirt" | "rooted_dirt" | "farmland" | "sand" | "red_sand" | "gravel" | "clay" | "soul_sand" | "soul_soil"
        | "mud" | "ice" | "packed_ice" | "magma_block" | "dirt_path" | "suspicious_sand" | "suspicious_gravel" => return 0.5,
        "grass_block" | "podzol" | "mycelium" | "sponge" | "wet_sponge" => return 0.6,
        "sandstone" | "red_sandstone" | "chiseled_sandstone" | "cut_sandstone" | "chiseled_red_sandstone" | "cut_red_sandstone" => return 0.8,
        "tnt" | "short_grass" | "tall_grass" | "fern" | "large_fern" | "dead_bush" | "torch" | "wall_torch" | "redstone_wire"
        | "fire" | "soul_fire" | "snow" | "seagrass" | "tall_seagrass" | "vine" => return 0.0,
        "iron_block" | "iron_door" | "iron_trapdoor" | "iron_bars" | "copper_block" => return 6.0,
        "gold_block" | "emerald_block" | "diamond_block" => return 6.0,
        _ => {}
    }
    if n.ends_with("_leaves") || n.ends_with("_wool") || n.ends_with("_carpet") || n.contains("flower") || n.ends_with("_sapling") {
        return 0.2;
    }
    if n.ends_with("_glass") || n.ends_with("_glass_pane") {
        return 0.3;
    }
    if n.ends_with("_ore") {
        return 3.0;
    }
    if n.ends_with("_terracotta") || n == "terracotta" {
        return 4.2;
    }
    if n.ends_with("_concrete") {
        return 1.8;
    }
    if n.ends_with("_concrete_powder") {
        return 0.5;
    }
    let stony = ["stone", "cobble", "brick", "deepslate", "andesite", "diorite", "granite", "blackstone", "basalt", "tuff",
        "calcite", "prismarine", "purpur", "quartz", "dripstone", "copper", "mossy"];
    if stony.iter().any(|s| n.contains(s)) {
        return 6.0;
    }
    if n.ends_with("_planks") || n.ends_with("_fence") || n.ends_with("_fence_gate") || n.ends_with("_slab") && n.contains("oak") {
        return 3.0;
    }
    if n.ends_with("_log") || n.ends_with("_wood") || n.ends_with("_stem") || n.ends_with("_hyphae") {
        return 2.0;
    }
    if n.ends_with("_door") || n.ends_with("_trapdoor") {
        return 3.0;
    }
    crate::hardness(n).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armor_formula() {
        // no armor: unchanged
        assert_eq!(armor_reduced(10.0, 0.0, 0.0), 10.0);
        // full diamond (20 armor, 8 toughness) vs 10 damage: max(4, 20 - 40/16 = 17.5) = 17.5 → 30% left
        assert!((armor_reduced(10.0, 20.0, 8.0) - 3.0).abs() < 1e-4);
        // full iron (15 armor, 0 toughness) vs 4 damage: max(3, 15 - 2) = 13 → 4 * 12/25
        assert!((armor_reduced(4.0, 15.0, 0.0) - 1.92).abs() < 1e-4);
        // huge hits: armor/5 floor applies (20 armor → at least 4/25 = 16% reduction)
        assert!((armor_reduced(100.0, 20.0, 0.0) - 84.0).abs() < 1e-3);
        // cap at 80% reduction
        assert!((armor_reduced(1.0, 30.0, 20.0) - 0.2).abs() < 1e-4);
    }

    #[test]
    fn difficulty() {
        assert_eq!(difficulty_scaled(3.0, 1), 2.5); // zombie on easy
        assert_eq!(difficulty_scaled(3.0, 3), 4.5);
        assert_eq!(difficulty_scaled(1.0, 1), 1.0);
        assert_eq!(difficulty_scaled(3.0, 0), 0.0);
    }

    #[test]
    fn resistance() {
        assert_eq!(blast_resistance("stone"), 6.0);
        assert_eq!(blast_resistance("dirt"), 0.5);
        assert_eq!(blast_resistance("obsidian"), 1200.0);
        assert!(blast_resistance("bedrock") > 1e6);
        assert_eq!(blast_resistance("oak_leaves"), 0.2);
        assert_eq!(blast_resistance("oak_planks"), 3.0);
    }
}

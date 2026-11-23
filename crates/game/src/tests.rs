use super::*;
use rand::{rngs::StdRng, SeedableRng};
use std::sync::OnceLock;

fn gd() -> &'static GameData {
    static G: OnceLock<GameData> = OnceLock::new();
    G.get_or_init(|| {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        GameData::load(&dir).expect("load ./data (run tools/extract-data.sh)")
    })
}

fn it(n: &str) -> i32 {
    item_id(n).unwrap_or_else(|| panic!("no item {n}"))
}

fn st(n: &str) -> u32 {
    BlockState::from_name(n).unwrap_or_else(|| panic!("no block {n}")).0
}

fn rng() -> StdRng {
    StdRng::seed_from_u64(12345)
}

#[test]
fn loads() {
    let g = gd();
    assert_eq!(g.items.len(), registries::ITEM.len());
    assert!(g.recipes.crafting.len() > 1000);
    assert!(g.loot_tables.len() > 1000);
    let dp = g.item(it("diamond_pickaxe")).unwrap();
    assert_eq!(dp.max_stack, 1);
    assert_eq!(dp.max_damage, 1561);
    assert!((dp.attack_damage - 5.0).abs() < 1e-4);
    assert!((dp.attack_speed - 1.2).abs() < 1e-4);
    let bread = g.item(it("bread")).unwrap();
    assert_eq!(bread.food.as_ref().unwrap().nutrition, 5);
    assert_eq!(bread.consume_ticks, 32);
    let chest = g.item(it("diamond_chestplate")).unwrap();
    assert_eq!((chest.armor, chest.toughness, chest.equip_slot.as_deref()), (8.0, 2.0, Some("chest")));
    assert_eq!(g.item(it("stone")).unwrap().block, Some(st("stone")));
    assert_eq!(g.item(it("stick")).unwrap().block, None);
    assert!(g.tags.block_has("#minecraft:mineable/pickaxe", "stone"));
    assert!(g.tags.block_has("logs", "oak_log"));
    assert!(g.tags.item_has("minecraft:planks", it("oak_planks")));
}

#[test]
fn dig_times() {
    let g = gd();
    assert_eq!(g.dig_ticks(st("stone"), None, true, false), Some(150));
    assert_eq!(g.dig_ticks(st("stone"), Some(it("wooden_pickaxe")), true, false), Some(23));
    assert_eq!(g.dig_ticks(st("dirt"), None, true, false), Some(15));
    assert_eq!(g.dig_ticks(st("obsidian"), Some(it("diamond_pickaxe")), true, false), Some(188));
    assert_eq!(g.dig_ticks(st("oak_leaves"), Some(it("shears")), true, false), Some(0));
    assert_eq!(g.dig_ticks(st("oak_leaves"), None, true, false), Some(6));
    assert_eq!(g.dig_ticks(st("bedrock"), Some(it("netherite_pickaxe")), true, false), None);
    assert_eq!(g.dig_ticks(st("short_grass"), None, true, false), Some(0));
    // Iron ore with a wooden pickaxe: pickaxe speed but wrong tier → /100 (7.5 s).
    assert_eq!(g.dig_ticks(st("iron_ore"), Some(it("wooden_pickaxe")), true, false), Some(150));
    // Penalties.
    assert_eq!(g.dig_ticks(st("dirt"), None, false, false), Some(75));
    assert_eq!(g.dig_ticks(st("dirt"), None, false, true), Some(375));
    // Wrong tool type: axe on stone behaves like a hand.
    assert_eq!(g.dig_ticks(st("stone"), Some(it("wooden_axe")), true, false), Some(150));
    assert_eq!(g.dig_ticks(st("oak_log"), Some(it("wooden_axe")), true, false), Some(30));
    assert_eq!(g.dig_ticks(st("cobweb"), Some(it("iron_sword")), true, false), Some(8));
}

#[test]
fn harvest() {
    let g = gd();
    assert!(!g.can_harvest(st("iron_ore"), Some(it("wooden_pickaxe"))));
    assert!(g.can_harvest(st("iron_ore"), Some(it("stone_pickaxe"))));
    assert!(!g.can_harvest(st("diamond_ore"), Some(it("stone_pickaxe"))));
    assert!(g.can_harvest(st("diamond_ore"), Some(it("iron_pickaxe"))));
    assert!(!g.can_harvest(st("obsidian"), Some(it("iron_pickaxe"))));
    assert!(g.can_harvest(st("obsidian"), Some(it("diamond_pickaxe"))));
    assert!(!g.can_harvest(st("stone"), None));
    assert!(g.can_harvest(st("dirt"), None));
    assert!(g.can_harvest(st("oak_log"), None));
}

fn count_of(d: &[(i32, i32)], item: &str) -> i32 {
    let id = it(item);
    d.iter().filter(|s| s.0 == id).map(|s| s.1).sum()
}

#[test]
fn drops() {
    let g = gd();
    let mut r = rng();
    let wp = Some(it("wooden_pickaxe"));
    assert_eq!(g.block_drops(st("stone"), wp, false, 0, &mut r), vec![(it("cobblestone"), 1)]);
    assert_eq!(g.block_drops(st("stone"), wp, true, 0, &mut r), vec![(it("stone"), 1)]);
    assert!(g.block_drops(st("stone"), None, false, 0, &mut r).is_empty());
    assert_eq!(g.block_drops(st("grass_block"), None, false, 0, &mut r), vec![(it("dirt"), 1)]);
    assert_eq!(g.block_drops(st("coal_ore"), wp, false, 0, &mut r), vec![(it("coal"), 1)]);

    let avg = |f: u8, r: &mut StdRng| -> f64 {
        (0..2000).map(|_| count_of(&g.block_drops(st("coal_ore"), wp, false, f, r), "coal") as f64).sum::<f64>() / 2000.0
    };
    let (a0, a3) = (avg(0, &mut r), avg(3, &mut r));
    assert_eq!(a0, 1.0);
    assert!(a3 > 2.0 && a3 < 2.4, "fortune III coal avg {a3}"); // expected 2.2

    let mut flint = 0;
    let mut gravel = 0;
    for _ in 0..1000 {
        let d = g.block_drops(st("gravel"), None, false, 0, &mut r);
        assert_eq!(d.len(), 1);
        flint += count_of(&d, "flint");
        gravel += count_of(&d, "gravel");
    }
    assert!(flint > 50 && flint < 160, "flint {flint}");
    assert_eq!(flint + gravel, 1000);

    let (mut sap, mut stick, mut apple) = (0, 0, 0);
    for _ in 0..4000 {
        let d = g.block_drops(st("oak_leaves"), None, false, 0, &mut r);
        sap += count_of(&d, "oak_sapling");
        stick += count_of(&d, "stick");
        apple += count_of(&d, "apple");
        assert_eq!(count_of(&d, "oak_leaves"), 0);
    }
    assert!(sap > 100 && stick > 40 && apple > 5, "sapling {sap} stick {stick} apple {apple}");
    assert_eq!(g.block_drops(st("oak_leaves"), Some(it("shears")), false, 0, &mut r), vec![(it("oak_leaves"), 1)]);

    // Block-state conditions: a lower door half drops the door, the upper half nothing.
    let door = BlockState(st("oak_door"));
    assert_eq!(g.block_drops(door.with("half", "lower").unwrap().0, None, false, 0, &mut r), vec![(it("oak_door"), 1)]);
    assert!(g.block_drops(door.with("half", "upper").unwrap().0, None, false, 0, &mut r).is_empty());
    let slab = BlockState(st("oak_slab")).with("type", "double").unwrap();
    assert_eq!(g.block_drops(slab.0, None, false, 0, &mut r), vec![(it("oak_slab"), 2)]);
    // Ripe wheat drops wheat + seeds; young wheat only seeds.
    let wheat = BlockState(st("wheat"));
    let ripe = g.block_drops(wheat.with("age", "7").unwrap().0, None, false, 0, &mut r);
    assert_eq!(count_of(&ripe, "wheat"), 1);
    assert!(count_of(&ripe, "wheat_seeds") >= 1);
    let young = g.block_drops(wheat.0, None, false, 0, &mut r);
    assert_eq!(young, vec![(it("wheat_seeds"), 1)]);
}

#[test]
fn entity_loot() {
    let g = gd();
    let mut r = rng();
    let ctx = LootContext { killed_by_player: true, ..Default::default() };
    let mut flesh = 0;
    for _ in 0..500 {
        let d = g.loot("minecraft:entities/zombie", &ctx, &mut r);
        flesh += count_of(&d, "rotten_flesh");
    }
    assert!(flesh > 300 && flesh < 700, "flesh {flesh}"); // uniform 0..2 → avg 1
    let looting = LootContext { looting: 3, ..ctx.clone() };
    let f3: i32 = (0..500).map(|_| count_of(&g.loot("entities/zombie", &looting, &mut r), "rotten_flesh")).sum();
    assert!(f3 > flesh + 300, "looting flesh {f3} vs {flesh}");
    // Explosions destroy some drops.
    let boom = LootContext { tool: Some(it("wooden_pickaxe")), explosion_radius: Some(4.0), ..Default::default() };
    let n: i32 = (0..1000).map(|_| count_of(&g.loot("blocks/dirt", &boom, &mut r), "dirt")).sum();
    assert!(n > 150 && n < 350, "explosion dirt {n}");
}

fn grid(names: &[&str]) -> Vec<Option<i32>> {
    names.iter().map(|n| if n.is_empty() { None } else { Some(it(n)) }).collect()
}

#[test]
fn crafting() {
    let g = gd();
    let p = "oak_planks";
    let r = g.craft(&grid(&[p, p, p, p]), 2, 2).unwrap();
    assert_eq!((r.item, r.count), (it("crafting_table"), 1));
    for slot in 0..9 {
        let mut gr = vec![None; 9];
        gr[slot] = Some(it("oak_log"));
        let r = g.craft(&gr, 3, 3).unwrap();
        assert_eq!((r.item, r.count), (it("oak_planks"), 4));
    }
    let r = g.craft(&grid(&["", "", "", "oak_log"]), 2, 2).unwrap();
    assert_eq!((r.item, r.count), (it("oak_planks"), 4));
    let s = "stick";
    let r = g.craft(&grid(&[p, p, p, "", s, "", "", s, ""]), 3, 3).unwrap();
    assert_eq!((r.item, r.count), (it("wooden_pickaxe"), 1));
    // Mixed planks via #planks tag.
    let r = g.craft(&grid(&["birch_planks", p, "spruce_planks", "", s, "", "", s, ""]), 3, 3).unwrap();
    assert_eq!(r.item, it("wooden_pickaxe"));
    // Wooden axe, both orientations, shifted in the grid.
    let a = g.craft(&grid(&[p, p, "", p, s, "", "", s, ""]), 3, 3).unwrap();
    let b = g.craft(&grid(&["", p, p, "", s, p, "", s, ""]), 3, 3).unwrap();
    assert_eq!(a.item, it("wooden_axe"));
    assert_eq!(b.item, it("wooden_axe"));
    // Sticks: 2 planks vertical, anywhere.
    let r = g.craft(&grid(&["", "", p, "", "", p, "", "", ""]), 3, 3).unwrap();
    assert_eq!((r.item, r.count), (it("stick"), 4));
    // Pickaxe pattern doesn't fit a 2x2.
    assert!(g.craft(&grid(&[p, s, "", ""]), 2, 2).is_none());
    assert!(g.craft(&grid(&["", "", "", ""]), 2, 2).is_none());
    // Shapeless dyes, any order/position.
    let r = g.craft(&grid(&["blue_dye", "", "", "red_dye"]), 2, 2).unwrap();
    assert_eq!((r.item, r.count), (it("purple_dye"), 2));
    let r = g.craft(&grid(&["red_dye", "blue_dye", "", ""]), 2, 2).unwrap();
    assert_eq!(r.item, it("purple_dye"));
    let r = g.craft(&grid(&["poppy", "", "", ""]), 2, 2).unwrap();
    assert_eq!(r.item, it("red_dye"));
    // Transmute: dye a shulker box.
    let r = g.craft(&grid(&["shulker_box", "black_dye", "", ""]), 2, 2).unwrap();
    assert_eq!(r.item, it("black_shulker_box"));
    // Remainders: cake leaves 3 empty buckets.
    let m = "milk_bucket";
    let r = g.craft(&grid(&[m, m, m, "sugar", "egg", "sugar", "wheat", "wheat", "wheat"]), 3, 3).unwrap();
    assert_eq!(r.item, it("cake"));
    assert_eq!(r.remainders.iter().filter(|x| **x == Some(it("bucket"))).count(), 3);
    assert_eq!(r.remainders.len(), 9);
}

#[test]
fn smelting_and_fuel() {
    let g = gd();
    let (res, xp, t) = g.smelt(it("iron_ore")).unwrap();
    assert_eq!((res, t), (it("iron_ingot"), 200));
    assert!((xp - 0.7).abs() < 1e-6);
    assert_eq!(g.smelt(it("raw_iron")).unwrap().0, it("iron_ingot"));
    assert_eq!(g.smelt(it("cobblestone")).unwrap().0, it("stone"));
    assert_eq!(g.cook(CookKind::Blasting, it("iron_ore")).unwrap().2, 100);
    assert!(g.smelt(it("stick")).is_none());
    assert_eq!(g.fuel_ticks(it("coal")), Some(1600));
    assert_eq!(g.fuel_ticks(it("oak_planks")), Some(300));
    assert_eq!(g.fuel_ticks(it("oak_log")), Some(300));
    assert_eq!(g.fuel_ticks(it("lava_bucket")), Some(20000));
    assert_eq!(g.fuel_ticks(it("stick")), Some(100));
    assert_eq!(g.fuel_ticks(it("oak_slab")), Some(150));
    assert_eq!(g.fuel_ticks(it("crimson_planks")), None);
    assert_eq!(g.fuel_ticks(it("stone")), None);
    assert!(g.stonecut(it("stone")).contains(&(it("stone_slab"), 2)));
}

/// Sanity check of the public-knowledge `requires_correct_tool` list against the data's
/// mineable tags: every block that needs a tool must be mineable by some tool (or be cobweb).
#[test]
fn correct_tool_blocks_are_mineable() {
    let g = gd();
    let mut bad = Vec::new();
    for b in basalt_data::blocks::BLOCKS {
        if requires_correct_tool(b.name)
            && !["pickaxe", "shovel", "axe", "hoe"].iter().any(|t| g.tags.block_has(&format!("mineable/{t}"), b.name))
            && b.name != "cobweb"
        {
            bad.push(b.name);
        }
    }
    assert!(bad.is_empty(), "requires tool but not mineable: {bad:?}");
}

/// Pickaxe-mineable blocks that drop without a pickaxe should be the documented exceptions only.
#[test]
fn pickaxe_blocks_without_tool_requirement() {
    let g = gd();
    let allowed = |n: &str, kind: &str| {
        matches!(kind, "shulker_box" | "base_coral_plant" | "base_coral_fan" | "base_coral_wall_fan" | "amethyst_cluster" | "powered_rail")
            || matches!(
                n,
                "bell" | "blue_ice" | "conduit" | "detector_rail" | "heavy_core" | "ice" | "packed_ice" | "packed_mud" | "piston"
                    | "sticky_piston" | "piston_head" | "pointed_dripstone" | "rail"
            )
    };
    let bad: Vec<&str> = basalt_data::blocks::BLOCKS
        .iter()
        .filter(|b| g.tags.block_has("mineable/pickaxe", b.name) && !requires_correct_tool(b.name) && !allowed(b.name, b.kind))
        .map(|b| b.name)
        .collect();
    assert!(bad.is_empty(), "{bad:?}");
}

//! Furnace fuel burn times (ticks). Not in the data reports; values are the publicly documented
//! burn times (Minecraft Wiki "Smelting § Fuel"), expressed over item tags where possible.
use crate::item_id;
use crate::tags::Tags;
use std::collections::HashMap;

const TAGS: &[(&str, u32)] = &[
    ("boats", 1200),
    ("chest_boats", 1200),
    ("logs_that_burn", 300),
    ("bamboo_blocks", 300),
    ("planks", 300),
    ("wooden_stairs", 300),
    ("wooden_slabs", 150),
    ("wooden_trapdoors", 300),
    ("wooden_pressure_plates", 300),
    ("wooden_fences", 300),
    ("fence_gates", 300),
    ("wooden_doors", 200),
    ("wooden_buttons", 100),
    ("wooden_shelves", 300),
    ("signs", 200),
    ("hanging_signs", 800),
    ("banners", 300),
    ("wool", 100),
    ("wool_carpets", 67),
    ("saplings", 100),
];

const ITEMS: &[(&str, u32)] = &[
    ("lava_bucket", 20000),
    ("coal_block", 16000),
    ("dried_kelp_block", 4001),
    ("blaze_rod", 2400),
    ("coal", 1600),
    ("charcoal", 1600),
    ("bamboo_mosaic", 300),
    ("bamboo_mosaic_stairs", 300),
    ("bamboo_mosaic_slab", 150),
    ("note_block", 300),
    ("bookshelf", 300),
    ("chiseled_bookshelf", 300),
    ("lectern", 300),
    ("jukebox", 300),
    ("chest", 300),
    ("trapped_chest", 300),
    ("crafting_table", 300),
    ("cartography_table", 300),
    ("fletching_table", 300),
    ("smithing_table", 300),
    ("loom", 300),
    ("barrel", 300),
    ("composter", 300),
    ("daylight_detector", 300),
    ("ladder", 300),
    ("bow", 300),
    ("crossbow", 300),
    ("fishing_rod", 300),
    ("mangrove_roots", 300),
    ("wooden_sword", 200),
    ("wooden_pickaxe", 200),
    ("wooden_axe", 200),
    ("wooden_shovel", 200),
    ("wooden_hoe", 200),
    ("wooden_spear", 200),
    ("stick", 100),
    ("bowl", 100),
    ("azalea", 100),
    ("flowering_azalea", 100),
    ("dead_bush", 100),
    ("bamboo", 50),
    ("scaffolding", 50),
];

pub(crate) fn build(tags: &Tags) -> HashMap<i32, u32> {
    let mut m = HashMap::new();
    for (t, ticks) in TAGS {
        for id in tags.item_tag(t) {
            // Nether wood (crimson / warped) doesn't burn.
            if !tags.item_has("non_flammable_wood", id) {
                m.insert(id, *ticks);
            }
        }
    }
    for (n, ticks) in ITEMS {
        if let Some(id) = item_id(n) {
            m.insert(id, *ticks);
        }
    }
    m
}

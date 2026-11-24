//! Block sound groups (`block.<group>.break|place|step|hit|fall`), shared by the client's
//! local sounds and the server's sounds for other players.

/// Block sound group (public-knowledge table by block name; the data generator doesn't export it).
pub fn block_group(name: &str) -> &'static str {
    let has = |s: &str| name.contains(s);
    if name.ends_with("_wool") || name.ends_with("_carpet") {
        "wool"
    } else if has("glass")
        || has("ice") && !has("packed_ice")
        || name == "glowstone"
        || name == "sea_lantern"
        || name == "beacon"
    {
        "glass"
    } else if name == "sand"
        || name == "red_sand"
        || name.ends_with("concrete_powder")
        || name == "soul_sand"
    {
        "sand"
    } else if name == "gravel"
        || name == "clay"
        || name == "farmland"
        || name == "dirt_path"
        || name == "coarse_dirt"
        || name == "dirt"
        || name == "rooted_dirt"
        || name == "mud"
    {
        "gravel"
    } else if name == "grass_block"
        || name == "mycelium"
        || name == "podzol"
        || has("leaves")
        || has("sapling")
        || name == "short_grass"
        || name == "tall_grass"
        || name == "fern"
        || has("flower")
        || name == "hay_block"
        || has("tulip")
        || name == "dandelion"
        || name == "poppy"
        || name == "vine"
        || name == "sugar_cane"
        || name == "sponge"
        || name == "kelp"
    {
        "grass"
    } else if name == "snow" || name == "snow_block" || name == "powder_snow" {
        "snow"
    } else if has("planks")
        || has("_log")
        || has("_wood")
        || has("stripped")
        || name.starts_with("oak")
        || name.starts_with("spruce")
        || name.starts_with("birch")
        || name.starts_with("jungle")
        || name.starts_with("acacia")
        || name.starts_with("dark_oak")
        || name.starts_with("mangrove")
        || name.starts_with("pale_oak")
        || name == "crafting_table"
        || name == "bookshelf"
        || name == "chest"
        || name == "barrel"
        || name == "note_block"
        || name == "jukebox"
        || has("fence") && !has("nether_brick")
        || name == "ladder"
        || name == "composter"
        || name == "lectern"
        || name == "cartography_table"
        || name == "fletching_table"
        || name == "smithing_table"
        || name == "loom"
    {
        "wood"
    } else if name.ends_with("_block")
        && (has("iron")
            || has("gold")
            || has("diamond")
            || has("emerald")
            || has("netherite")
            || has("lapis")
            || has("redstone"))
        || name == "anvil"
        || name == "hopper"
        || name == "cauldron"
        || name == "iron_bars"
        || name == "iron_door"
        || name == "iron_trapdoor"
    {
        "metal"
    } else {
        "stone"
    }
}

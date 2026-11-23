//! M3 worldgen: terrain the bot receives from a generated Basalt world (seed 12345) vs the vanilla
//! test server's saved region files (vanilla/srv/world, same seed).
use crate::runner::Ctx;
use anyhow::{bail, Result};
use basalt_data::BlockState;
use std::time::Duration;

/// Natural terrain blocks (noise + surface rules); features and player edits are excluded.
fn terrain(name: &str) -> bool {
    matches!(name, "stone" | "deepslate" | "bedrock" | "dirt" | "grass_block" | "sand" | "red_sand" | "sandstone" | "gravel" | "water" | "lava"
        | "air" | "cave_air" | "granite" | "tuff" | "diorite" | "andesite" | "calcite" | "snow_block" | "podzol" | "coarse_dirt" | "mud" | "clay" | "terracotta")
}

pub fn worldgen_columns(ctx: &mut Ctx) -> Result<()> {
    let region = std::env::var("VANILLA_WORLD").unwrap_or_else(|_| "vanilla/srv/world".into());
    let region = std::path::Path::new(&region).join("dimensions/minecraft/overworld/region");
    if !region.exists() { bail!("no vanilla world at {} (set VANILLA_WORLD)", region.display()); }
    let mut vw = basalt_world::World::new(Some(region));
    let bot = ctx.bot()?;
    ctx.gamemode(&bot, "creative")?; bot.fly(true);
    // chunks near the vanilla spawn that vanilla has generated (arenas sit at y >= 200, skipped)
    let sites = [(2, 2), (-3, 1), (1, -3), (-2, -2), (3, -1)];
    let (mut same, mut total, mut chunks) = (0u64, 0u64, 0);
    for (cx, cz) in sites {
        if !vw.load(cx, cz).unwrap_or(false) { ctx.note(format!("vanilla chunk {cx},{cz} not generated; skipped")); continue; }
        bot.tp([cx as f64 * 16.0 + 8.0, 190.0, cz as f64 * 16.0 + 8.0])?;
        bot.set_frozen(true);
        if !bot.wait_until(Duration::from_secs(30), |s| s.world.chunks.contains_key(&(cx, cz))) { bail!("chunk {cx},{cz} not received"); }
        chunks += 1;
        let s = bot.st();
        for x in 0..16 { for z in 0..16 { for y in -64..180 {
            let (wx, wz) = (cx * 16 + x, cz * 16 + z);
            let ours = BlockState(s.world.get(wx, y, wz)).name();
            let theirs = BlockState(vw.get_block(wx, y, wz)).name();
            if !(terrain(ours) && terrain(theirs)) { continue; }
            total += 1;
            if ours == theirs || (matches!(ours, "air" | "cave_air") && matches!(theirs, "air" | "cave_air")) { same += 1; }
        } } }
        drop(s);
        bot.set_frozen(false);
    }
    if chunks == 0 { bail!("no comparable vanilla chunks"); }
    let pct = 100.0 * same as f64 / total.max(1) as f64;
    ctx.note(format!("{chunks} chunks, {total} terrain blocks, {pct:.3}% identical"));
    ctx.check(pct >= 98.0, format!("only {pct:.2}% of terrain blocks match vanilla"));
    ctx.metric("terrain_match_pct", (pct * 10.0).round() / 10.0, 1.0);
    Ok(())
}

/// `/locate structure` from a fixed spot near spawn (seed 12345): found position and distance per
/// structure (and one tag), plus the reply key. Differential: the vanilla test server runs the same
/// read-only command. Structure types whose placement Basalt does not model yet (mansion,
/// desert pyramid's extra checks) are left out.
pub fn locate(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    ctx.gamemode(&bot, "creative")?;
    bot.fly(true);
    bot.tp([8.5, 150.0, 8.5])?;
    let ids = ["minecraft:village_plains", "#minecraft:village", "minecraft:pillager_outpost", "minecraft:shipwreck", "minecraft:ruined_portal",
        "minecraft:trial_chambers", "minecraft:ancient_city", "minecraft:ocean_ruin_cold", "minecraft:buried_treasure", "minecraft:mineshaft",
        "minecraft:igloo", "minecraft:swamp_hut", "minecraft:jungle_pyramid", "minecraft:stronghold"];
    for id in ids {
        let reply = bot.cmd_reply(&format!("locate structure {id}"))?;
        let Some(t) = reply.iter().find(|t| t.key.as_deref().map(|k| k.starts_with("commands.locate")).unwrap_or(false)) else {
            ctx.check(false, format!("/locate structure {id}: no locate reply ({:?})", reply.iter().map(|t| t.plain.clone()).collect::<Vec<_>>()));
            continue;
        };
        let key = t.key.clone().unwrap_or_default();
        let name = id.trim_start_matches('#').trim_start_matches("minecraft:");
        ctx.check(key == "commands.locate.structure.success", format!("/locate structure {id}: {key} ({})", t.plain));
        // "The nearest <id> is at [x, ~, z] (d blocks away)"
        // (rendered either as text or as `key(args)`): the last three integers are x, z, distance
        let nums: Vec<f64> = t.plain.split(|c: char| !(c.is_ascii_digit() || c == '-')).filter_map(|w| w.parse::<f64>().ok()).collect();
        ctx.note(format!("{id}: {}", t.plain));
        if nums.len() >= 3 {
            ctx.metric(&format!("{name}_x"), nums[nums.len() - 3], 0.0);
            ctx.metric(&format!("{name}_z"), nums[nums.len() - 2], 0.0);
            ctx.metric(&format!("{name}_dist"), nums[nums.len() - 1], 0.0);
        } else {
            ctx.check(false, format!("/locate structure {id}: unparsed reply {}", t.plain));
        }
    }
    let bad = bot.cmd_reply("locate structure minecraft:not_a_structure")?;
    let k = bad.iter().filter_map(|t| t.key.clone()).next().unwrap_or_default();
    ctx.check(k.starts_with("commands.locate.structure.invalid") || k.starts_with("argument"), format!("unknown structure: {k}"));
    Ok(())
}

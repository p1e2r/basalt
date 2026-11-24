//! Integration tests (skipped when ./data is missing).
use basalt_data::BlockState;
use basalt_worldgen::{default_data_dir, GeneratedChunk, Generator, Stage};
use std::sync::Arc;

fn gen() -> Option<Generator> {
    let d = default_data_dir();
    if !d.join("data/minecraft/worldgen").exists() {
        return None;
    }
    Some(Generator::with_data_dir(12345, d))
}

fn count(c: &GeneratedChunk, pred: impl Fn(&str) -> bool) -> usize {
    c.blocks
        .iter()
        .flat_map(|s| s.iter())
        .filter(|b| pred(BlockState(**b as u32).name()))
        .count()
}

#[test]
fn deterministic_across_threads() {
    let Some(g) = gen() else { return };
    let g = Arc::new(g);
    let hs: Vec<_> = (0..2)
        .map(|_| {
            let g = g.clone();
            std::thread::spawn(move || g.generate_chunk(3, -2).blocks)
        })
        .collect();
    let r: Vec<_> = hs.into_iter().map(|h| h.join().unwrap()).collect();
    assert!(r[0] == r[1]);
    // A fresh generator (empty neighbour cache) gives the same result.
    let g2 = gen().unwrap();
    assert!(g2.generate_chunk(3, -2).blocks == r[0]);
}

#[test]
fn stages_add_content() {
    let Some(g) = gen() else { return };
    // Snowy taiga at seed 12345: spruce trees, snow, ores.
    let noise = g.generate_stages(100, -37, Stage::Noise);
    let full = g.generate_chunk(100, -37);
    assert_eq!(count(&noise, |n| n == "spruce_log"), 0);
    assert!(count(&full, |n| n == "spruce_log") > 10, "trees");
    assert!(count(&full, |n| n == "spruce_leaves") > 50, "foliage");
    assert!(count(&full, |n| n == "snow") > 50, "freeze_top_layer");
    assert!(count(&full, |n| n.ends_with("_ore")) > 20, "ores");
    assert!(
        count(&full, |n| n == "bedrock") > 0 && count(&full, |n| n == "deepslate") > 1000,
        "surface rules"
    );
}

#[test]
fn geode_matches_vanilla_location() {
    let Some(g) = gen() else { return };
    // Vanilla seed 12345 has an amethyst geode around (1612, -44, -508), chunk (100, -32).
    let c = g.generate_chunk(100, -32);
    assert!(count(&c, |n| n == "amethyst_block") > 50);
    assert!(count(&c, |n| n == "calcite") > 50 && count(&c, |n| n == "smooth_basalt") > 50);
}

#[test]
fn features_registry_mostly_supported() {
    let Some(g) = gen() else { return };
    assert!(
        g.unsupported_features().len() < 40,
        "{:?}",
        g.unsupported_features()
    );
}

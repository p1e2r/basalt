//! Generates the biome registry order (sorted file names of the vanilla data pack's
//! worldgen/biome directory, which is the order the server sends in registry_data).
//! Reads ./data produced by tools/extract-data.sh; nothing from Mojang is committed.
use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../data");
    let root = std::env::var("BASALT_DATA")
        .map(PathBuf::from)
        .unwrap_or(root);
    let dir = root.join("data/minecraft/worldgen/biome");
    println!("cargo:rerun-if-changed={}", dir.display());
    println!("cargo:rerun-if-env-changed=BASALT_DATA");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|_| panic!("missing {} — run `cargo xtask extract-data`", dir.display()))
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            e.file_name()
                .to_str()
                .and_then(|n| n.strip_suffix(".json"))
                .map(str::to_string)
        })
        .collect();
    names.sort();
    let mut o =
        String::from("/// Biome registry, in network id order.\npub const BIOMES: &[&str] = &[");
    for n in &names {
        o.push_str(&format!("{n:?},"));
    }
    o.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("biomes.rs");
    std::fs::write(out, o).unwrap();
}

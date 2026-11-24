//! Synced registries and tags for the configuration phase, built at startup from the
//! data generator output in ./data (same files the vanilla server ships).
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Registries vanilla 26.2 sends in configuration, in the order observed on the wire.
pub const SYNCED: &[&str] = &[
    "worldgen/biome",
    "chat_type",
    "trim_pattern",
    "trim_material",
    "wolf_variant",
    "wolf_sound_variant",
    "pig_variant",
    "pig_sound_variant",
    "frog_variant",
    "cat_variant",
    "cat_sound_variant",
    "cow_sound_variant",
    "cow_variant",
    "chicken_sound_variant",
    "chicken_variant",
    "zombie_nautilus_variant",
    "painting_variant",
    "sulfur_cube_archetype",
    "dimension_type",
    "damage_type",
    "banner_pattern",
    "enchantment",
    "jukebox_song",
    "instrument",
    "test_environment",
    "test_instance",
    "dialog",
    "world_clock",
    "timeline",
];

/// Registries vanilla sends tags for (update_tags).
pub const TAGGED: &[&str] = &[
    "point_of_interest_type",
    "item",
    "game_event",
    "painting_variant",
    "block",
    "worldgen/biome",
    "fluid",
    "timeline",
    "potion",
    "entity_type",
    "instrument",
    "enchantment",
    "damage_type",
    "dialog",
    "banner_pattern",
];

pub struct Registries {
    /// (registry, entry names) in send order.
    pub synced: Vec<(String, Vec<String>)>,
    /// (registry, [(tag, ids)]).
    pub tags: Vec<(String, Vec<(String, Vec<i32>)>)>,
}

fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, base, out);
        } else if let Some(rel) = p
            .strip_prefix(base)
            .ok()
            .and_then(|r| r.to_str())
            .and_then(|r| r.strip_suffix(".json"))
        {
            out.push(format!("minecraft:{}", rel.replace('\\', "/")));
        }
    }
}

fn entries(root: &Path, reg: &str) -> Vec<String> {
    let base = root.join(reg);
    let mut v = Vec::new();
    walk(&base, &base, &mut v);
    v.sort();
    v
}

fn static_registry(reg: &str) -> Option<&'static [&'static str]> {
    use basalt_data::registries as r;
    Some(match reg {
        "block" => r::BLOCK,
        "item" => r::ITEM,
        "fluid" => r::FLUID,
        "entity_type" => r::ENTITY_TYPE,
        "game_event" => r::GAME_EVENT,
        "point_of_interest_type" => r::POINT_OF_INTEREST_TYPE,
        "potion" => r::POTION,
        _ => return None,
    })
}

impl Registries {
    /// `data_dir` is the repo's ./data (contains data/minecraft/...).
    pub fn load(data_dir: &Path) -> Result<Registries> {
        let root: PathBuf = data_dir.join("data/minecraft");
        anyhow::ensure!(
            root.is_dir(),
            "{} not found — run `cargo xtask extract-data`",
            root.display()
        );
        let mut synced = Vec::new();
        let mut ids: HashMap<&str, HashMap<String, i32>> = HashMap::new();
        for reg in SYNCED {
            let e = entries(&root, reg);
            ids.insert(
                reg,
                e.iter()
                    .enumerate()
                    .map(|(i, n)| (n.clone(), i as i32))
                    .collect(),
            );
            synced.push((format!("minecraft:{reg}"), e));
        }
        let mut tags = Vec::new();
        for reg in TAGGED {
            let lookup = |name: &str| -> Option<i32> {
                match static_registry(reg) {
                    Some(r) => basalt_data::registry_id(r, name),
                    None => ids.get(reg)?.get(name).copied(),
                }
            };
            let tag_root = root.join("tags").join(reg);
            let names = {
                let mut v = Vec::new();
                walk(&tag_root, &tag_root, &mut v);
                v.sort();
                v
            };
            let mut raw: HashMap<String, Vec<Value>> = HashMap::new();
            for n in &names {
                let path = tag_root.join(format!("{}.json", n.trim_start_matches("minecraft:")));
                let v: Value = serde_json::from_slice(&std::fs::read(&path)?)
                    .with_context(|| path.display().to_string())?;
                raw.insert(
                    n.clone(),
                    v["values"].as_array().cloned().unwrap_or_default(),
                );
            }
            fn resolve(
                n: &str,
                raw: &HashMap<String, Vec<Value>>,
                seen: &mut HashSet<String>,
                out: &mut Vec<String>,
            ) {
                if !seen.insert(n.to_string()) {
                    return;
                }
                for v in raw.get(n).map(|v| v.as_slice()).unwrap_or(&[]) {
                    let s = v.as_str().or_else(|| v["id"].as_str()).unwrap_or("");
                    if let Some(t) = s.strip_prefix('#') {
                        resolve(t, raw, seen, out);
                    } else if !s.is_empty() {
                        out.push(s.to_string());
                    }
                }
            }
            let mut list = Vec::new();
            for n in &names {
                let mut out = Vec::new();
                resolve(n, &raw, &mut HashSet::new(), &mut out);
                let mut v: Vec<i32> = Vec::new();
                for e in out {
                    if let Some(id) = lookup(&e) {
                        if !v.contains(&id) {
                            v.push(id);
                        }
                    }
                }
                list.push((n.clone(), v));
            }
            tags.push((format!("minecraft:{reg}"), list));
        }
        Ok(Registries { synced, tags })
    }

    pub fn entries(&self, reg: &str) -> &[String] {
        self.synced
            .iter()
            .find(|(r, _)| r == reg)
            .map(|(_, e)| e.as_slice())
            .unwrap_or(&[])
    }
    pub fn id(&self, reg: &str, name: &str) -> Option<i32> {
        self.entries(reg)
            .iter()
            .position(|e| e == name)
            .map(|i| i as i32)
    }
}

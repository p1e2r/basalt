//! Block / item tags from `data/minecraft/tags/{block,item}`, fully expanded at load time.
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Strip a `minecraft:` namespace (and a leading `#`, which callers may or may not include).
pub(crate) fn short(s: &str) -> &str {
    s.trim_start_matches('#').trim_start_matches("minecraft:")
}

/// Expanded tags: tag name (no namespace, e.g. `mineable/pickaxe`) → member names (no namespace).
#[derive(Debug, Default)]
pub struct Tags {
    pub blocks: HashMap<String, HashSet<String>>,
    pub items: HashMap<String, HashSet<String>>,
    /// Item tags resolved to numeric item ids.
    item_ids: HashMap<String, HashSet<i32>>,
}

impl Tags {
    pub(crate) fn load(tags_dir: &Path) -> anyhow::Result<Tags> {
        let blocks = load_kind(&tags_dir.join("block"))?;
        let items = load_kind(&tags_dir.join("item"))?;
        let item_ids = items
            .iter()
            .map(|(k, v)| (k.clone(), v.iter().filter_map(|n| basalt_data::registry_id(basalt_data::registries::ITEM, n)).collect()))
            .collect();
        Ok(Tags { blocks, items, item_ids })
    }

    /// `tag` may be written `#minecraft:logs`, `minecraft:logs` or `logs`.
    pub fn block_has(&self, tag: &str, block_name: &str) -> bool {
        self.blocks.get(short(tag)).is_some_and(|s| s.contains(short(block_name)))
    }

    pub fn item_has(&self, tag: &str, item: i32) -> bool {
        self.item_ids.get(short(tag)).is_some_and(|s| s.contains(&item))
    }

    pub fn item_has_name(&self, tag: &str, item_name: &str) -> bool {
        self.items.get(short(tag)).is_some_and(|s| s.contains(short(item_name)))
    }

    /// Item ids in an item tag (empty if the tag doesn't exist).
    pub fn item_tag(&self, tag: &str) -> impl Iterator<Item = i32> + '_ {
        self.item_ids.get(short(tag)).into_iter().flat_map(|s| s.iter().copied())
    }

    /// A block "selector" as used by tool rules: `#tag`, a single block, or handled by the caller for lists.
    pub(crate) fn block_matches(&self, selector: &str, block_name: &str) -> bool {
        if selector.starts_with('#') { self.block_has(selector, block_name) } else { short(selector) == short(block_name) }
    }
}

pub(crate) fn load_kind(dir: &Path) -> anyhow::Result<HashMap<String, HashSet<String>>> {
    let mut raw: HashMap<String, Vec<String>> = HashMap::new();
    if dir.is_dir() {
        walk(dir, dir, &mut raw)?;
    }
    let mut out = HashMap::new();
    for name in raw.keys() {
        let mut set = HashSet::new();
        let mut visiting = HashSet::new();
        expand(name, &raw, &mut set, &mut visiting);
        out.insert(name.clone(), set);
    }
    Ok(out)
}

fn walk(root: &Path, dir: &Path, raw: &mut HashMap<String, Vec<String>>) -> anyhow::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_dir() {
            walk(root, &p, raw)?;
        } else if p.extension().is_some_and(|x| x == "json") {
            let rel = p.strip_prefix(root)?.with_extension("");
            let name = rel.to_string_lossy().replace('\\', "/");
            let v: Value = serde_json::from_str(&std::fs::read_to_string(&p)?)?;
            let vals = v["values"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| match x {
                            Value::String(s) => Some(s.clone()),
                            Value::Object(o) => o.get("id").and_then(|i| i.as_str()).map(str::to_string),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();
            raw.insert(name, vals);
        }
    }
    Ok(())
}

fn expand(name: &str, raw: &HashMap<String, Vec<String>>, out: &mut HashSet<String>, visiting: &mut HashSet<String>) {
    if !visiting.insert(name.to_string()) {
        return;
    }
    if let Some(vals) = raw.get(name) {
        for v in vals {
            if v.starts_with('#') {
                expand(short(v), raw, out, visiting);
            } else {
                out.insert(short(v).to_string());
            }
        }
    }
}

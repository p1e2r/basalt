//! Crafting, cooking and stonecutting recipes from `data/minecraft/recipe`.
use crate::tags::{short, Tags};
use crate::{item_id, read_json};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Set of item ids an ingredient slot accepts (tags expanded at load).
#[derive(Debug, Clone, Default)]
pub struct Ingredient(pub HashSet<i32>);

impl Ingredient {
    pub fn matches(&self, item: i32) -> bool {
        self.0.contains(&item)
    }

    fn parse(v: &Value, tags: &Tags) -> Option<Ingredient> {
        let mut set = HashSet::new();
        fn add(v: &Value, tags: &Tags, set: &mut HashSet<i32>) {
            match v {
                Value::String(s) if s.starts_with('#') => set.extend(tags.item_tag(s)),
                Value::String(s) => set.extend(item_id(s)),
                Value::Array(a) => a.iter().for_each(|x| add(x, tags, set)),
                Value::Object(o) => {
                    if let Some(i) = o.get("item").and_then(|x| x.as_str()) {
                        set.extend(item_id(i));
                    }
                    if let Some(t) = o.get("tag").and_then(|x| x.as_str()) {
                        set.extend(tags.item_tag(t));
                    }
                }
                _ => {}
            }
        }
        add(v, tags, &mut set);
        (!set.is_empty()).then_some(Ingredient(set))
    }
}

#[derive(Debug, Clone)]
pub enum Recipe {
    Shaped { id: String, width: usize, height: usize, cells: Vec<Option<Ingredient>>, result: (i32, i32) },
    Shapeless { id: String, ingredients: Vec<Ingredient>, result: (i32, i32) },
    /// `input` + `material` → `result` (components of the input carry over in vanilla).
    Transmute { id: String, input: Ingredient, material: Ingredient, result: (i32, i32) },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CookKind {
    Smelting,
    Blasting,
    Smoking,
    Campfire,
}

#[derive(Debug, Clone)]
pub struct CraftResult {
    pub item: i32,
    pub count: i32,
    /// Per grid slot (same indexing as the input grid): item left behind after crafting.
    pub remainders: Vec<Option<i32>>,
}

#[derive(Debug, Default)]
pub struct Recipes {
    pub crafting: Vec<Recipe>,
    cooking: HashMap<(CookKind, i32), (i32, f32, u32)>,
    stonecutting: HashMap<i32, Vec<(i32, i32)>>,
}

fn result_of(v: &Value) -> Option<(i32, i32)> {
    let r = &v["result"];
    let (id, count) = match r {
        Value::String(s) => (s.as_str(), 1),
        _ => (r["id"].as_str().or_else(|| r["item"].as_str())?, r["count"].as_i64().unwrap_or(1) as i32),
    };
    Some((item_id(id)?, count))
}

impl Recipes {
    pub(crate) fn load(dir: &Path, tags: &Tags) -> anyhow::Result<Recipes> {
        let mut files: Vec<_> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        files.sort();
        let mut out = Recipes::default();
        for p in files {
            let id = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
            let v = read_json(&p)?;
            let ty = short(v["type"].as_str().unwrap_or(""));
            let Some(result) = result_of(&v) else { continue };
            match ty {
                "crafting_shaped" => {
                    let key: HashMap<char, Ingredient> = v["key"]
                        .as_object()
                        .into_iter()
                        .flatten()
                        .filter_map(|(k, x)| Some((k.chars().next()?, Ingredient::parse(x, tags)?)))
                        .collect();
                    let rows: Vec<Vec<char>> =
                        v["pattern"].as_array().into_iter().flatten().filter_map(|r| r.as_str()).map(|r| r.chars().collect()).collect();
                    let w = rows.iter().map(|r| r.len()).max().unwrap_or(0);
                    let h = rows.len();
                    let mut cells = Vec::with_capacity(w * h);
                    let mut bad = false;
                    for r in &rows {
                        for x in 0..w {
                            let c = r.get(x).copied().unwrap_or(' ');
                            if c == ' ' {
                                cells.push(None);
                            } else if let Some(i) = key.get(&c) {
                                cells.push(Some(i.clone()));
                            } else {
                                bad = true;
                            }
                        }
                    }
                    if bad || w == 0 {
                        continue;
                    }
                    let (width, height, cells) = trim(w, h, &cells);
                    out.crafting.push(Recipe::Shaped { id, width, height, cells, result });
                }
                "crafting_shapeless" => {
                    let ingredients: Option<Vec<Ingredient>> =
                        v["ingredients"].as_array().into_iter().flatten().map(|x| Ingredient::parse(x, tags)).collect();
                    if let Some(ingredients) = ingredients.filter(|i| !i.is_empty()) {
                        out.crafting.push(Recipe::Shapeless { id, ingredients, result });
                    }
                }
                "crafting_transmute" => {
                    if let (Some(input), Some(material)) = (Ingredient::parse(&v["input"], tags), Ingredient::parse(&v["material"], tags)) {
                        out.crafting.push(Recipe::Transmute { id, input, material, result });
                    }
                }
                "smelting" | "blasting" | "smoking" | "campfire_cooking" => {
                    let (kind, default_time) = match ty {
                        "smelting" => (CookKind::Smelting, 200),
                        "blasting" => (CookKind::Blasting, 100),
                        "smoking" => (CookKind::Smoking, 100),
                        _ => (CookKind::Campfire, 100),
                    };
                    let xp = v["experience"].as_f64().unwrap_or(0.0) as f32;
                    let time = v["cookingtime"].as_u64().unwrap_or(default_time) as u32;
                    if let Some(ing) = Ingredient::parse(&v["ingredient"], tags) {
                        let mut ids: Vec<i32> = ing.0.into_iter().collect();
                        ids.sort();
                        for i in ids {
                            out.cooking.entry((kind, i)).or_insert((result.0, xp, time));
                        }
                    }
                }
                "stonecutting" => {
                    if let Some(ing) = Ingredient::parse(&v["ingredient"], tags) {
                        for i in ing.0 {
                            out.stonecutting.entry(i).or_default().push(result);
                        }
                    }
                }
                _ => {} // crafting_special_*, crafting_dye, decorated pot, imbue, smithing: not handled
            }
        }
        for v in out.stonecutting.values_mut() {
            v.sort();
            v.dedup();
        }
        Ok(out)
    }

    pub fn cook(&self, kind: CookKind, item: i32) -> Option<(i32, f32, u32)> {
        self.cooking.get(&(kind, item)).copied()
    }

    pub fn stonecut(&self, item: i32) -> Vec<(i32, i32)> {
        self.stonecutting.get(&item).cloned().unwrap_or_default()
    }

    pub fn craft(&self, grid: &[Option<i32>], width: usize, height: usize) -> Option<CraftResult> {
        if width == 0 || height == 0 || grid.len() != width * height {
            return None;
        }
        let (bw, bh, cells) = trim(width, height, grid);
        if bw == 0 {
            return None;
        }
        let items: Vec<i32> = grid.iter().flatten().copied().collect();
        let found = self.crafting.iter().find_map(|r| match r {
            Recipe::Shaped { width: w, height: h, cells: pat, result, .. } => {
                (*w == bw && *h == bh && (shaped_match(pat, &cells, bw, false) || shaped_match(pat, &cells, bw, true))).then_some(*result)
            }
            Recipe::Shapeless { ingredients, result, .. } => shapeless_match(ingredients, &items).then_some(*result),
            Recipe::Transmute { input, material, result, .. } => (items.len() == 2
                && ((input.matches(items[0]) && material.matches(items[1])) || (input.matches(items[1]) && material.matches(items[0])))
                && !(items.contains(&result.0) && input.matches(result.0)))
            .then_some(*result),
        })?;
        Some(CraftResult { item: found.0, count: found.1, remainders: grid.iter().map(|s| s.and_then(remainder)).collect() })
    }
}

/// Crafting remainder of an ingredient (left in the grid after crafting).
fn remainder(item: i32) -> Option<i32> {
    let name = basalt_data::registries::ITEM.get(item as usize)?;
    let r = match *name {
        "milk_bucket" | "water_bucket" | "lava_bucket" | "powder_snow_bucket" => "bucket",
        "honey_bottle" | "dragon_breath" => "glass_bottle",
        _ => return None,
    };
    item_id(r)
}

/// Crop a row-major grid to the bounding box of its non-empty cells.
fn trim<T: Clone>(w: usize, h: usize, cells: &[Option<T>]) -> (usize, usize, Vec<Option<T>>) {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if cells[y * w + x].is_some() {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x0 == usize::MAX {
        return (0, 0, Vec::new());
    }
    let (nw, nh) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut out = Vec::with_capacity(nw * nh);
    for y in y0..=y1 {
        for x in x0..=x1 {
            out.push(cells[y * w + x].clone());
        }
    }
    (nw, nh, out)
}

fn shaped_match(pat: &[Option<Ingredient>], grid: &[Option<i32>], w: usize, mirror: bool) -> bool {
    pat.iter().enumerate().all(|(i, p)| {
        let (x, y) = (i % w, i / w);
        let gx = if mirror { w - 1 - x } else { x };
        match (p, grid[y * w + gx]) {
            (None, None) => true,
            (Some(ing), Some(it)) => ing.matches(it),
            _ => false,
        }
    })
}

fn shapeless_match(ings: &[Ingredient], items: &[i32]) -> bool {
    if ings.len() != items.len() {
        return false;
    }
    fn go(ings: &[Ingredient], items: &[i32], used: &mut [bool]) -> bool {
        let Some((first, rest)) = ings.split_first() else { return true };
        for i in 0..items.len() {
            if !used[i] && first.matches(items[i]) {
                used[i] = true;
                if go(rest, items, used) {
                    return true;
                }
                used[i] = false;
            }
        }
        false
    }
    go(ings, items, &mut vec![false; items.len()])
}

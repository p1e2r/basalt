//! Asset coverage: `--asset-grid out.png [--blocks|--items] [--allowlist FILE]` renders every
//! distinct block model (or every item icon) into paged grid images and writes `out.json`
//! listing everything that fell back to a missing model/texture. Exit status is non-zero if a
//! problem is not covered by the allowlist (lines: `pattern reason...`, `*` wildcards,
//! optional `block:` / `item:` prefix).
use crate::assets::Assets;
use crate::mesh::MeshData;
use crate::render::{Frame, Gpu, Tex, Uniforms};
use crate::Args;
use anyhow::Result;
use basalt_data::BlockState;
use glam::{Mat4, Vec3};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

const PAGE: u32 = 2048;
const CELL: f32 = 34.0;

fn glob(p: &str, s: &str) -> bool {
    match p.as_bytes().first() {
        None => s.is_empty(),
        Some(b'*') => (0..=s.len()).any(|i| s.is_char_boundary(i) && glob(&p[1..], &s[i..])),
        Some(&c) => s.as_bytes().first() == Some(&c) && glob(&p[1..], &s[1..]),
    }
}

struct Allow { entries: Vec<(String, String, String)> }
impl Allow {
    fn load(path: &str) -> Allow {
        let mut entries = Vec::new();
        if let Ok(s) = std::fs::read_to_string(path) {
            for l in s.lines() {
                let l = l.trim();
                if l.is_empty() || l.starts_with('#') { continue; }
                let (p, reason) = l.split_once(char::is_whitespace).unwrap_or((l, ""));
                let (kind, p) = match p.split_once(':') { Some((k, p)) if k == "block" || k == "item" => (k.to_string(), p.to_string()), _ => (String::new(), p.to_string()) };
                entries.push((kind, p, reason.trim().to_string()));
            }
        }
        Allow { entries }
    }
    fn reason(&self, kind: &str, name: &str) -> Option<&str> {
        self.entries.iter().find(|(k, p, _)| (k.is_empty() || k == kind) && glob(p, name)).map(|e| e.2.as_str())
    }
}

pub fn run(args: &Args, assets: Arc<Assets>, out: &str) -> Result<bool> {
    let allow = Allow::load(&args.allowlist);
    if let Some(d) = std::path::Path::new(out).parent() { if !d.as_os_str().is_empty() { std::fs::create_dir_all(d)?; } }
    let kind = if args.grid_items { "item" } else { "block" };
    // (name, cell drawer) and problems
    let mut cells: Vec<(String, Option<u32>, i32)> = Vec::new(); // (label, model, item)
    let mut problems: Vec<(String, String, usize)> = Vec::new();
    if !args.grid_items {
        let mut seen: HashMap<u32, ()> = HashMap::new();
        let mut per_block: HashMap<(String, String), usize> = HashMap::new();
        for id in 0..basalt_data::blocks::STATE_COUNT as u32 {
            let st = BlockState(id); let name = st.name();
            let info = assets.info(id as u16);
            let m = &assets.models[info.model as usize];
            let nm = assets.no_model[id as usize];
            let reason = if nm == 2 { None } else if nm == 1 { Some("blockstate applies no model") }
                else if m.missing { Some("missing texture or model file") }
                else if m.quads.is_empty() && info.fluid == 0 { Some("empty model (needs a block-entity renderer)") }
                else { None };
            if let Some(r) = reason { *per_block.entry((name.to_string(), r.to_string())).or_default() += 1; }
            if seen.insert(info.model, ()).is_none() && !m.quads.is_empty() { cells.push((st.to_string(), Some(info.model), -1)); }
        }
        let mut v: Vec<_> = per_block.into_iter().map(|((n, r), c)| (n, r, c)).collect();
        v.sort();
        problems = v;
    } else {
        for (i, n) in basalt_data::registries::ITEM.iter().enumerate() {
            let has = assets.item_models.get(i).copied().flatten().is_some() || assets.item_icons.get(i).map(|x| x.is_some()).unwrap_or(false);
            if !has { problems.push((n.to_string(), "no item model or icon texture".into(), 1)); }
            cells.push((n.to_string(), None, i as i32));
        }
    }
    // render pages
    let per_row = (PAGE as f32 / CELL) as usize;
    let per_page = per_row * per_row;
    let ui = crate::ui::UiAtlas::build(&assets);
    let ea = crate::entity::EntityAtlas::build(&assets);
    let mut gpu = Gpu::new(None, (PAGE, PAGE), &assets, (&ui.rgba, ui.w, ui.h), (&ea.rgba, ea.w, ea.h))?;
    let pages = cells.len().div_ceil(per_page).max(1);
    let mut files = Vec::new();
    for p in 0..pages {
        let (mut bg, mut icons) = (MeshData::default(), MeshData::default());
        ui.rect(&mut bg, 0.0, 0.0, PAGE as f32, PAGE as f32, [139, 139, 139, 255]);
        for (k, (_, model, item)) in cells.iter().enumerate().skip(p * per_page).take(per_page) {
            let j = k - p * per_page;
            let (x, y) = ((j % per_row) as f32 * CELL + 1.0, (j / per_row) as f32 * CELL + 1.0);
            match model {
                Some(mi) => crate::ui::iso_model(&mut icons, &assets.models[*mi as usize], x + 16.0, y + 16.0, 20.0, None),
                None => {
                    let ok = assets.item_models.get(*item as usize).copied().flatten().is_some() || assets.item_icons.get(*item as usize).map(|x| x.is_some()).unwrap_or(false);
                    if !ok { ui.rect(&mut bg, x, y, 32.0, 32.0, [248, 0, 248, 255]); }
                    crate::ui::item_icon(&assets, &mut icons, *item, x, y, 2.0);
                }
            }
        }
        let mut u: Uniforms = bytemuck::Zeroable::zeroed();
        u.vp = Mat4::IDENTITY.to_cols_array_2d(); u.inv_vp = Mat4::IDENTITY.to_cols_array_2d(); u.screen = [PAGE as f32, PAGE as f32, 0.0, 0.0]; u.params = [1e6, 1e6 + 1.0, 1.0, 6000.0];
        let empty = MeshData::default();
        let layers = vec![(bg, Tex::Ui), (icons, Tex::Blocks)];
        let f = Frame { uniforms: u, cam: Vec3::ZERO, vp: Mat4::IDENTITY, render_dist: 0.0, min_y: 0.0, extra: &empty, skinned: &empty, items3d: &empty, overlay: &empty, ui_layers: &layers };
        let rgb = gpu.capture(&f)?;
        let path = if p == 0 { out.to_string() } else { out.replace(".png", &format!("_{}.png", p + 1)) };
        crate::render::save_png(&path, PAGE, PAGE, &rgb)?;
        files.push(path);
    }
    let mut unexplained = Vec::new(); let mut allowed = Vec::new();
    for (n, r, c) in &problems {
        match allow.reason(kind, n) { Some(why) => allowed.push(json!({"name": n, "problem": r, "states": c, "allowlisted": why})), None => unexplained.push(json!({"name": n, "problem": r, "states": c})) }
    }
    let report = json!({
        "kind": kind, "cells": cells.len(), "pages": files, "total": if args.grid_items { basalt_data::registries::ITEM.len() } else { basalt_data::blocks::STATE_COUNT },
        "problems": unexplained, "allowlisted": allowed,
    });
    let jp = out.replace(".png", ".json");
    std::fs::write(&jp, serde_json::to_vec_pretty(&report)?)?;
    println!("asset-grid {kind}: {} cells on {} page(s), {} problems ({} allowlisted) → {jp}", cells.len(), files.len(), unexplained.len(), allowed.len());
    for u in &unexplained { println!("  missing: {}", u); }
    Ok(unexplained.is_empty())
}

#[cfg(test)]
mod tests {
    #[test]
    fn globbing() {
        assert!(super::glob("*_sign", "oak_wall_sign"));
        assert!(super::glob("air", "air"));
        assert!(!super::glob("air", "cave_air"));
        assert!(super::glob("*_banner*", "white_wall_banner"));
        assert!(super::glob("potted_*", "potted_oak_sapling"));
    }
}

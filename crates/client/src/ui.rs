//! HUD: bitmap font text, hotbar, crosshair, hearts, chat.
use crate::assets::Assets;
use crate::mesh::{MeshData, Vertex};
use std::collections::HashMap;

pub struct UiAtlas { pub w: u32, pub h: u32, pub rgba: Vec<u8>, pub rects: HashMap<String, [u32; 4]>, pub glyph_w: [u8; 256] }

impl UiAtlas {
    pub fn build(a: &Assets) -> UiAtlas {
        let (w, h) = (512u32, 512u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        let mut rects = HashMap::new();
        let mut glyph_w = [6u8; 256];
        let blit = |rects: &mut HashMap<String, [u32; 4]>, name: &str, img: &crate::assets::Image, x: u32, y: u32, rgba: &mut Vec<u8>| {
            for py in 0..img.h.min(h - y) { for px in 0..img.w.min(w - x) {
                let s = ((py * img.w + px) * 4) as usize; let d = (((y + py) * w + x + px) * 4) as usize;
                rgba[d..d + 4].copy_from_slice(&img.rgba[s..s + 4]);
            } }
            rects.insert(name.to_string(), [x, y, img.w, img.h]);
        };
        if let Some(f) = &a.font {
            blit(&mut rects, "font", f, 0, 0, &mut rgba);
            let cell = f.w / 16;
            for c in 0..256u32 {
                let (cx, cy) = (c % 16 * cell, c / 16 * cell);
                let mut maxx = 0;
                for py in 0..cell { for px in 0..cell { if cy + py < f.h && f.rgba[(((cy + py) * f.w + cx + px) * 4 + 3) as usize] > 0 { maxx = maxx.max(px + 1); } } }
                glyph_w[c as usize] = if c == 32 { 4 } else { (maxx * 8 / cell + 1) as u8 };
            }
        }
        // white block
        for py in 0..8 { for px in 0..8 { let d = (((200 + py) * w + 300 + px) * 4) as usize; rgba[d..d + 4].copy_from_slice(&[255; 4]); } }
        rects.insert("white".into(), [302, 202, 4, 4]);
        let (mut x, mut y, mut row) = (0u32, 260u32, 0u32);
        let mut names: Vec<&String> = a.hud.keys().filter(|n| matches!(n.as_str(), "hotbar" | "hotbar_selection" | "crosshair" | "heart/container" | "heart/full" | "heart/half" | "food_empty" | "food_full" | "food_half")).collect();
        names.sort();
        for n in names {
            let img = &a.hud[n];
            if x + img.w + 2 > w { x = 0; y += row + 2; row = 0; }
            blit(&mut rects, n, img, x, y, &mut rgba); x += img.w + 2; row = row.max(img.h);
        }
        UiAtlas { w, h, rgba, rects, glyph_w }
    }

    pub fn quad(&self, m: &mut MeshData, x: f32, y: f32, w: f32, h: f32, uv: [f32; 4], color: [u8; 4]) {
        let b = m.verts.len() as u32;
        for (px, py, u, v) in [(x, y, uv[0], uv[1]), (x, y + h, uv[0], uv[3]), (x + w, y + h, uv[2], uv[3]), (x + w, y, uv[2], uv[1])] {
            m.verts.push(Vertex { pos: [px, py, 0.0], uv: [u, v], color, light: [255; 4] });
        }
        m.idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
    }
    fn uv(&self, r: [u32; 4]) -> [f32; 4] { [r[0] as f32 / self.w as f32, r[1] as f32 / self.h as f32, (r[0] + r[2]) as f32 / self.w as f32, (r[1] + r[3]) as f32 / self.h as f32] }

    pub fn sprite(&self, m: &mut MeshData, name: &str, x: f32, y: f32, scale: f32) {
        if let Some(r) = self.rects.get(name) { self.quad(m, x, y, r[2] as f32 * scale, r[3] as f32 * scale, self.uv(*r), [255; 4]); }
    }
    pub fn rect(&self, m: &mut MeshData, x: f32, y: f32, w: f32, h: f32, color: [u8; 4]) {
        if let Some(r) = self.rects.get("white") { self.quad(m, x, y, w, h, self.uv(*r), color); }
    }
    pub fn text_width(&self, s: &str, scale: f32) -> f32 { s.chars().map(|c| self.glyph_w[(c as u32).min(255) as usize] as f32 * scale).sum() }
    pub fn text(&self, m: &mut MeshData, s: &str, x: f32, y: f32, scale: f32, color: [u8; 4]) {
        let Some(f) = self.rects.get("font") else { return };
        let cell = f[2] as f32 / 16.0;
        for (pass, col) in [(1.0, [color[0] / 4, color[1] / 4, color[2] / 4, color[3]]), (0.0, color)] {
            let mut cx = x + pass * scale;
            for ch in s.chars() {
                let c = if (ch as u32) < 256 { ch as u32 } else { '?' as u32 };
                if c != 32 {
                    let (gx, gy) = ((c % 16) as f32 * cell, (c / 16) as f32 * cell);
                    let uv = [(f[0] as f32 + gx) / self.w as f32, (f[1] as f32 + gy) / self.h as f32, (f[0] as f32 + gx + cell) / self.w as f32, (f[1] as f32 + gy + cell) / self.h as f32];
                    self.quad(m, cx, y + pass * scale, 8.0 * scale, 8.0 * scale, uv, col);
                }
                cx += self.glyph_w[c as usize] as f32 * scale;
            }
        }
    }
}

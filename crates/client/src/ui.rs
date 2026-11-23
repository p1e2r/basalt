//! 2D UI drawing: an atlas of the jar's GUI textures + bitmap font, and helpers for
//! sprites, nine-slice widgets, text and item icons.
use crate::assets::Assets;
use crate::mesh::{MeshData, Vertex};
use std::collections::HashMap;

pub struct UiAtlas { pub w: u32, pub h: u32, pub rgba: Vec<u8>, pub rects: HashMap<String, [u32; 4]>, pub glyph_w: [u8; 256], glyphs: HashMap<char, Glyph> }

/// A bitmap glyph in the atlas: uv rect, drawn size and vertical offset (GUI px at scale 1), advance.
#[derive(Clone, Copy)]
struct Glyph { uv: [f32; 4], w: f32, h: f32, yoff: f32, adv: f32 }

/// GUI texture folders / files packed into the UI atlas.
fn wanted(name: &str, img: &crate::assets::Image) -> bool {
    (name.starts_with("sprites/") && img.w <= 256 && img.h <= 256 && !name.starts_with("sprites/realm") && !name.starts_with("sprites/advancements") && !name.starts_with("sprites/recipe_book")
        && !name.starts_with("sprites/social_interactions") && !name.starts_with("sprites/spectator") && !name.starts_with("sprites/statistics"))
        || matches!(name, "container/inventory" | "container/crafting_table" | "container/generic_54" | "container/furnace" | "container/blast_furnace" | "container/smoker" | "title/minecraft" | "title/edition" | "menu_background" | "inworld_menu_background"
            | "header_separator" | "footer_separator" | "menu_list_background" | "inworld_header_separator" | "inworld_footer_separator" | "inworld_menu_list_background")
}

impl UiAtlas {
    pub fn build(a: &Assets) -> UiAtlas {
        let (w, h) = (2048u32, 2048u32);
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
        // shelf-pack the GUI textures, tallest first
        let mut names: Vec<&String> = a.gui.iter().filter(|(n, i)| wanted(n, i)).map(|(n, _)| n).collect();
        names.sort_by(|x, y| a.gui[*y].h.cmp(&a.gui[*x].h).then(x.cmp(y)));
        let (mut x, mut y, mut row) = (0u32, 260u32, 0u32);
        for n in names {
            let img = &a.gui[n];
            if x + img.w + 2 > w { x = 0; y += row + 2; row = 0; }
            if y + img.h > h { log::warn!("ui atlas full at {n}"); break; }
            blit(&mut rects, n, img, x, y, &mut rgba); x += img.w + 2; row = row.max(img.h);
            // short aliases for HUD sprites ("hotbar", "heart/full", ...)
            if let Some(s) = n.strip_prefix("sprites/hud/") { let r = rects[n.as_str()]; rects.insert(s.to_string(), r); }
        }
        // font pages (vanilla's providers in order; the first page with a character wins)
        let mut glyphs: HashMap<char, Glyph> = HashMap::new();
        for (pi, page) in a.font_pages.iter().enumerate() {
            let img = &page.img;
            // continue the shelf after the GUI textures
            if x + img.w + 2 > w { x = 0; y += row + 2; row = 0; }
            if y + img.h > h { log::warn!("ui atlas full at font page {pi}"); break; }
            let (fx, py0, rows) = (x, y, page.chars.len() as u32);
            blit(&mut rects, &format!("fontpage{pi}"), img, fx, py0, &mut rgba);
            x += img.w + 2; row = row.max(img.h);
            let cols = page.chars.iter().map(|r| r.len()).max().unwrap_or(16) as u32;
            let (cw, ch) = (img.w / cols.max(1), img.h / rows.max(1));
            let k = page.height as f32 / ch as f32;
            for (ry, row) in page.chars.iter().enumerate() {
                for (rx, c) in row.iter().enumerate() {
                    if *c == '\0' || *c == ' ' || glyphs.contains_key(c) { continue; }
                    let (cx, cy) = (rx as u32 * cw, ry as u32 * ch);
                    let mut maxx = 0;
                    for yy in 0..ch { for xx in 0..cw { if img.rgba[(((cy + yy) * img.w + cx + xx) * 4 + 3) as usize] > 0 { maxx = maxx.max(xx + 1); } } }
                    if maxx == 0 { continue; }
                    let (ax, ay) = (fx + cx, py0 + cy);
                    glyphs.insert(*c, Glyph {
                        uv: [ax as f32 / w as f32, ay as f32 / h as f32, (ax + cw) as f32 / w as f32, (ay + ch) as f32 / h as f32],
                        w: cw as f32 * k, h: ch as f32 * k, yoff: (7 - page.ascent) as f32, adv: (maxx as f32 * k + 0.5).floor() + 1.0,
                    });
                }
            }
        }
        UiAtlas { w, h, rgba, rects, glyph_w, glyphs }
    }

    /// True when a bitmap font page has this character.
    pub fn has_glyph(&self, c: char) -> bool { self.glyphs.contains_key(&c) }

    /// Advance of one character (GUI px at scale 1): bitmap glyphs, space = 4, unknown = '?'.
    fn advance(&self, c: char) -> f32 {
        if c == ' ' { return 4.0; }
        if let Some(g) = self.glyphs.get(&c) { return g.adv; }
        if self.glyphs.is_empty() { return self.glyph_w[(c as u32).min(255) as usize] as f32; }
        self.glyphs.get(&'?').map(|g| g.adv).unwrap_or(6.0)
    }

    pub fn quad(&self, m: &mut MeshData, x: f32, y: f32, w: f32, h: f32, uv: [f32; 4], color: [u8; 4]) {
        let b = m.verts.len() as u32;
        for (px, py, u, v) in [(x, y, uv[0], uv[1]), (x, y + h, uv[0], uv[3]), (x + w, y + h, uv[2], uv[3]), (x + w, y, uv[2], uv[1])] {
            m.verts.push(Vertex { pos: [px, py, 0.0], uv: [u, v], color, light: [255; 4] });
        }
        m.idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
    }
    fn uv(&self, r: [u32; 4]) -> [f32; 4] { [r[0] as f32 / self.w as f32, r[1] as f32 / self.h as f32, (r[0] + r[2]) as f32 / self.w as f32, (r[1] + r[3]) as f32 / self.h as f32] }
    fn sub_uv(&self, r: [u32; 4], sx: f32, sy: f32, sw: f32, sh: f32) -> [f32; 4] {
        [(r[0] as f32 + sx) / self.w as f32, (r[1] as f32 + sy) / self.h as f32, (r[0] as f32 + sx + sw) / self.w as f32, (r[1] as f32 + sy + sh) / self.h as f32]
    }
    pub fn size(&self, name: &str) -> Option<(f32, f32)> { self.rects.get(name).map(|r| (r[2] as f32, r[3] as f32)) }

    pub fn sprite(&self, m: &mut MeshData, name: &str, x: f32, y: f32, scale: f32) {
        if let Some(r) = self.rects.get(name) { self.quad(m, x, y, r[2] as f32 * scale, r[3] as f32 * scale, self.uv(*r), [255; 4]); }
    }
    /// Draw a sprite stretched to (w, h) GUI units.
    pub fn sprite_sized(&self, m: &mut MeshData, name: &str, x: f32, y: f32, w: f32, h: f32, scale: f32, color: [u8; 4]) {
        if let Some(r) = self.rects.get(name) { self.quad(m, x, y, w * scale, h * scale, self.uv(*r), color); }
    }
    /// Draw part (sx, sy, sw, sh in texture pixels) of a texture at (x, y), `scale` screen px per texel.
    pub fn region(&self, m: &mut MeshData, name: &str, x: f32, y: f32, sx: f32, sy: f32, sw: f32, sh: f32, scale: f32) {
        if let Some(r) = self.rects.get(name) { self.quad(m, x, y, sw * scale, sh * scale, self.sub_uv(*r, sx, sy, sw, sh), [255; 4]); }
    }
    /// Nine-slice a sprite into a (w × h) GUI-unit box with the given border (texels).
    pub fn nine(&self, m: &mut MeshData, name: &str, x: f32, y: f32, w: f32, h: f32, border: f32, scale: f32) {
        let Some(r) = self.rects.get(name).copied() else { return };
        let (tw, th) = (r[2] as f32, r[3] as f32);
        let b = border.min(w / 2.0).min(h / 2.0);
        let xs = [(0.0, 0.0, b, b), (b, b, w - 2.0 * b, tw - 2.0 * b), (w - b, tw - b, b, b)];
        let ys = [(0.0, 0.0, b, b), (b, b, h - 2.0 * b, th - 2.0 * b), (h - b, th - b, b, b)];
        for (dy, sy, dh, sh) in ys { for (dx, sx, dw, sw) in xs {
            if dw <= 0.0 || dh <= 0.0 { continue; }
            self.quad(m, x + dx * scale, y + dy * scale, dw * scale, dh * scale, self.sub_uv(r, sx, sy, sw, sh), [255; 4]);
        } }
    }
    pub fn rect(&self, m: &mut MeshData, x: f32, y: f32, w: f32, h: f32, color: [u8; 4]) {
        if let Some(r) = self.rects.get("white") { self.quad(m, x, y, w, h, self.uv(*r), color); }
    }
    /// Vertical gradient rectangle.
    pub fn gradient(&self, m: &mut MeshData, x: f32, y: f32, w: f32, h: f32, top: [u8; 4], bottom: [u8; 4]) {
        let Some(r) = self.rects.get("white") else { return };
        let uv = self.uv(*r);
        let b = m.verts.len() as u32;
        for (px, py, c) in [(x, y, top), (x, y + h, bottom), (x + w, y + h, bottom), (x + w, y, top)] {
            m.verts.push(Vertex { pos: [px, py, 0.0], uv: [uv[0], uv[1]], color: c, light: [255; 4] });
        }
        m.idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
    }
    pub fn text_width(&self, s: &str, scale: f32) -> f32 { s.chars().map(|c| self.advance(c) * scale).sum() }
    /// Text with drop shadow.
    pub fn text(&self, m: &mut MeshData, s: &str, x: f32, y: f32, scale: f32, color: [u8; 4]) { self.text_ex(m, s, x, y, scale, color, true) }
    pub fn text_ex(&self, m: &mut MeshData, s: &str, x: f32, y: f32, scale: f32, color: [u8; 4], shadow: bool) {
        let passes: &[(f32, [u8; 4])] = if shadow { &[(1.0, [color[0] / 4, color[1] / 4, color[2] / 4, color[3]]), (0.0, color)] } else { &[(0.0, color)] };
        for &(pass, col) in passes {
            let mut cx = x + pass * scale;
            for ch in s.chars() {
                if ch != ' ' {
                    let g = self.glyphs.get(&ch).or_else(|| self.glyphs.get(&'?'));
                    if let Some(g) = g {
                        self.quad(m, cx, y + (g.yoff + pass) * scale, g.w * scale, g.h * scale, g.uv, col);
                    } else if let Some(f) = self.rects.get("font") {
                        // no font pages (old jars): the ascii page by Latin-1 code
                        let cell = f[2] as f32 / 16.0;
                        let c = if (ch as u32) < 256 { ch as u32 } else { '?' as u32 };
                        let (gx, gy) = ((c % 16) as f32 * cell, (c / 16) as f32 * cell);
                        let uv = [(f[0] as f32 + gx) / self.w as f32, (f[1] as f32 + gy) / self.h as f32, (f[0] as f32 + gx + cell) / self.w as f32, (f[1] as f32 + gy + cell) / self.h as f32];
                        self.quad(m, cx, y + pass * scale, 8.0 * scale, 8.0 * scale, uv, col);
                    }
                }
                cx += self.advance(ch) * scale;
            }
        }
    }
    pub fn text_centered(&self, m: &mut MeshData, s: &str, cx: f32, y: f32, scale: f32, color: [u8; 4]) {
        let w = self.text_width(s, scale);
        self.text(m, s, (cx - w / 2.0).floor(), y, scale, color);
    }
}

// ---------------------------------------------------------------- item icons

/// Draw an item icon (16×16 GUI units at `scale`) into `items` (block-atlas mesh).
/// Block items are drawn as a small isometric block (top, north and west faces visible, like
/// the vanilla GUI view); everything else as its flat sprite.
pub fn item_icon(a: &Assets, items: &mut MeshData, item: i32, x: f32, y: f32, scale: f32) {
    if let Some(m) = a.item_model(item) {
        let tint = a.item_tint.get(item as usize).copied().flatten();
        iso_model(items, m, x + 8.0 * scale, y + 8.0 * scale, 10.0 * scale, tint);
        return;
    }
    if let Some(uv) = a.icon_uv(item) {
        let b = items.verts.len() as u32;
        let s = 16.0 * scale;
        for (px, py, u, v) in [(x, y, uv[0], uv[1]), (x, y + s, uv[0], uv[3]), (x + s, y + s, uv[2], uv[3]), (x + s, y, uv[2], uv[1])] {
            items.verts.push(Vertex { pos: [px, py, 0.0], uv: [u, v], color: [255; 4], light: [255; 4] });
        }
        items.idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
    }
}

/// Project a block model (0..1 cube space) isometrically, viewed from north-west and above.
pub fn iso_model(out: &mut MeshData, m: &crate::assets::Model, cx: f32, cy: f32, size: f32, tint: Option<u32>) {
    use glam::Vec3;
    // viewer direction (towards the eye): north-west, 30° up
    let (e, az) = (30f32.to_radians(), 45f32.to_radians());
    let d = Vec3::new(-az.sin() * e.cos(), e.sin(), -az.cos() * e.cos());
    let right = Vec3::new(-1.0, 0.0, 1.0).normalize();
    let up = d.cross(right).normalize() * -1.0;
    let up = if up.y < 0.0 { -up } else { up };
    let tc = tint.map(|c| [(c >> 16 & 255) as f32 / 255.0, (c >> 8 & 255) as f32 / 255.0, (c & 255) as f32 / 255.0]).unwrap_or([1.0; 3]);
    let mut faces: Vec<(f32, [Vertex; 4])> = Vec::new();
    for q in &m.quads {
        let p: Vec<Vec3> = q.pos.iter().map(|p| Vec3::new(p[0] - 0.5, p[1] - 0.5, p[2] - 0.5)).collect();
        let n = (p[1] - p[0]).cross(p[2] - p[0]);
        if n.dot(d) <= 1e-6 { continue; }
        let n = n.normalize();
        let shade = if n.y > 0.5 { 1.0 } else if n.y < -0.5 { 0.5 } else if n.z.abs() > n.x.abs() { 0.8 } else { 0.6 };
        let t = if q.tinted { tc } else { [1.0; 3] };
        let col = [(t[0] * shade * 255.0) as u8, (t[1] * shade * 255.0) as u8, (t[2] * shade * 255.0) as u8, 255];
        let mut vs = [Vertex::default(); 4];
        let mut depth = 0.0;
        for i in 0..4 {
            let sx = cx + p[i].dot(right) * size;
            let sy = cy - p[i].dot(up) * size;
            depth += p[i].dot(d);
            vs[i] = Vertex { pos: [sx, sy, 0.0], uv: q.uv[i], color: col, light: [255; 4] };
        }
        faces.push((depth, vs));
    }
    faces.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    for (_, vs) in faces {
        let b = out.verts.len() as u32;
        out.verts.extend_from_slice(&vs);
        out.idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
    }
}

/// Stack count + durability bar for an item drawn at (x, y).
pub fn item_decorations(ui: &UiAtlas, m: &mut MeshData, it: &crate::net::ItemStack, max_damage: i32, x: f32, y: f32, s: f32) {
    if max_damage > 0 && it.damage > 0 {
        let f = (1.0 - it.damage as f32 / max_damage as f32).clamp(0.0, 1.0);
        let w = (13.0 * f).round();
        let (r, g, b) = hsv(f / 3.0, 1.0, 1.0);
        ui.rect(m, x + 2.0 * s, y + 13.0 * s, 13.0 * s, 2.0 * s, [0, 0, 0, 255]);
        ui.rect(m, x + 2.0 * s, y + 13.0 * s, w * s, s, [r, g, b, 255]);
    }
    if it.count > 1 {
        let t = it.count.to_string();
        ui.text(m, &t, x + 17.0 * s - ui.text_width(&t, s), y + 9.0 * s, s, [255; 4]);
    }
}

fn hsv(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let i = (h * 6.0).floor(); let f = h * 6.0 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
    let (r, g, b) = match i as i32 % 6 { 0 => (v, t, p), 1 => (q, v, p), 2 => (p, v, t), 3 => (p, q, v), 4 => (t, p, v), _ => (v, p, q) };
    ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

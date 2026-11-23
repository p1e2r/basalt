//! Loads block states, models, textures and biome data from the user's own
//! client.jar at runtime and bakes them into renderable quads + a texture atlas.
use anyhow::{anyhow, Context, Result};
use basalt_data::{blocks::BLOCKS, BlockState};
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;

pub const DOWN: u8 = 0; pub const UP: u8 = 1; pub const NORTH: u8 = 2; pub const SOUTH: u8 = 3; pub const WEST: u8 = 4; pub const EAST: u8 = 5;
pub const DIR_VEC: [[i32; 3]; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];
pub const NO_CULL: u8 = 6;

pub const LAYER_OPAQUE: u8 = 0; pub const LAYER_CUTOUT: u8 = 1; pub const LAYER_TRANSLUCENT: u8 = 2;

#[derive(Clone, Debug)]
pub struct Quad {
    pub pos: [[f32; 3]; 4],
    pub uv: [[f32; 2]; 4],
    pub cull: u8,
    pub dir: u8,
    pub tinted: bool,
    pub layer: u8,
    pub shade: bool,
    /// the model allows smooth lighting / ambient occlusion ("ambientocclusion", default true)
    pub ao: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tint { None, Grass, Foliage, Water, Fixed(u32), DryFoliage }

#[derive(Default)]
pub struct Model {
    pub quads: Vec<Quad>,
    /// Element boxes in block units (0..1).
    pub boxes: Vec<[f32; 6]>,
    /// boxes of the elements that are not rotated (rotated ones - tendrils, cross plants - do not shape the cell)
    pub shape_boxes: Vec<[f32; 6]>,
    pub occludes: bool,
    pub full_cube: bool,
    /// a face referenced a texture that could not be resolved / is not in the atlas
    pub missing: bool,
    /// For models drawn entirely in the solid layer: per direction, a 16x16 mask of the block
    /// boundary face covered by its element boxes (row = first in-plane axis, bit = second).
    /// A neighbour's face is hidden when this mask covers it (shape-based face culling).
    pub occl: Option<Box<[[u16; 16]; 6]>>,
    /// Same masks from the element boxes regardless of render layer: the block's own shape, used to cull
    /// fluid faces that the block itself covers (waterlogged stairs, slabs, sculk sensors).
    pub shape: Option<Box<[[u16; 16]; 6]>>,
}

/// In-plane axes of a face direction's axis (shared by culling masks and smooth lighting).
pub fn plane_axes(dir: u8) -> (usize, usize, usize) {
    match dir { DOWN | UP => (1, 0, 2), NORTH | SOUTH => (2, 0, 1), _ => (0, 1, 2) }
}

impl Model {
    fn compute_shape(&mut self) { self.shape = Self::masks(&self.shape_boxes); }

    fn compute_occlusion(&mut self) {
        if self.boxes.is_empty() || self.quads.is_empty() || self.quads.iter().any(|q| q.layer != LAYER_OPAQUE) { return; }
        self.occl = Self::masks(&self.boxes);
    }

    fn masks(boxes: &[[f32; 6]]) -> Option<Box<[[u16; 16]; 6]>> {
        if boxes.is_empty() { return None; }
        let mut m = [[0u16; 16]; 6];
        for b in boxes {
            for d in 0..6u8 {
                let (ax, t1, t2) = plane_axes(d);
                let touches = if DIR_VEC[d as usize][ax] > 0 { b[3 + ax] >= 0.999 } else { b[ax] <= 0.001 };
                if !touches { continue; }
                let r = |lo: f32, hi: f32| ((lo * 16.0 + 0.01).floor().max(0.0) as usize, ((hi * 16.0 - 0.01).ceil() as usize).min(16));
                let (a0, a1) = r(b[t1], b[3 + t1]); let (b0, b1) = r(b[t2], b[3 + t2]);
                for i in a0..a1 { for j in b0..b1 { m[d as usize][i] |= 1 << j; } }
            }
        }
        Some(Box::new(m))
    }
}

#[derive(Clone, Copy)]
pub struct StateInfo {
    pub model: u32,
    pub tint: Tint,
    pub occludes: bool,
    /// Full non-opaque cube (glass, ice): cull faces against the same block.
    pub self_cull: bool,
    /// full non-opaque cube (glass, leaves, ice): a fluid side face against it is drawn with the overlay sprite
    pub fluid_overlay: bool,
    /// position-dependent model offset: (max horizontal, vertical range); (0, 0) = none. See `mesh::model_offset`.
    pub offset: (f32, f32),
    /// the block's own shape hides the fluid faces it covers (stairs, slabs, sculk sensors; not
    /// trapdoors, roots, leaves, glass, which never occlude)
    pub hides_fluid: bool,
    pub collides: bool,
    /// 0 = none, 1 = water, 2 = lava
    pub fluid: u8,
    /// fluid level 0..8 (0 = source), 8+ = falling
    pub fluid_level: u8,
    pub emits_light: bool,
    /// block light emitted (0..15)
    pub emission: u8,
    /// drawn at full block light regardless of surroundings (magma, active sculk sensors)
    pub full_bright: bool,
    /// casts ambient occlusion on neighbouring faces (full-cube collidable models that are not glass-like)
    pub ao_occluder: bool,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Rect { pub x: u32, pub y: u32, pub w: u32, pub h: u32 }

pub struct Image { pub w: u32, pub h: u32, pub rgba: Vec<u8> }

pub struct Atlas { pub w: u32, pub h: u32, pub rgba: Vec<u8>, pub rects: HashMap<String, (Rect, u8)> }

#[derive(Clone, Debug)]
pub struct Biome { pub name: String, pub grass: u32, pub foliage: u32, pub dry_foliage: u32, pub water: u32, pub sky: u32, pub fog: u32 }

#[derive(Clone, Debug)]
pub struct DimensionType { pub name: String, pub min_y: i32, pub height: i32, pub has_skylight: bool }

pub struct Jar { pub files: HashMap<String, Vec<u8>> }

impl Jar {
    pub fn open(path: &str) -> Result<Jar> {
        let f = std::fs::File::open(path).with_context(|| format!("cannot open client jar {path} (pass --jar path/to/26.2 client.jar)"))?;
        let mut z = zip::ZipArchive::new(std::io::BufReader::new(f))?;
        let wanted = ["assets/minecraft/blockstates/", "assets/minecraft/models/block/", "assets/minecraft/textures/block/", "assets/minecraft/textures/colormap/",
            "assets/minecraft/textures/font/ascii.png", "assets/minecraft/textures/gui/", "assets/minecraft/textures/item/", "assets/minecraft/models/item/", "assets/minecraft/items/",
            "assets/minecraft/textures/entity/", "assets/minecraft/textures/environment/", "assets/minecraft/lang/en_us.json",
            "data/minecraft/dimension_type/", "data/minecraft/worldgen/biome/"];
        let mut files = HashMap::new();
        for i in 0..z.len() {
            let mut e = z.by_index(i)?;
            let name = e.name().to_string();
            if e.is_file() && wanted.iter().any(|w| name.starts_with(w)) {
                let mut b = Vec::with_capacity(e.size() as usize); e.read_to_end(&mut b)?; files.insert(name, b);
            }
        }
        Ok(Jar { files })
    }
    pub fn json(&self, path: &str) -> Option<Value> { serde_json::from_slice(self.files.get(path)?).ok() }
    pub fn png(&self, path: &str) -> Option<Image> { decode_png(self.files.get(path)?).ok() }
}

pub fn decode_png(data: &[u8]) -> Result<Image> {
    let mut dec = png::Decoder::new(data);
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut rd = dec.read_info()?;
    let mut buf = vec![0; rd.output_buffer_size()];
    let info = rd.next_frame(&mut buf)?;
    let buf = &buf[..info.buffer_size()];
    let n = (info.width * info.height) as usize;
    let mut rgba = Vec::with_capacity(n * 4);
    match info.color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(buf),
        png::ColorType::Rgb => for p in buf.chunks(3) { rgba.extend_from_slice(&[p[0], p[1], p[2], 255]); },
        png::ColorType::Grayscale => for p in buf { rgba.extend_from_slice(&[*p, *p, *p, 255]); },
        png::ColorType::GrayscaleAlpha => for p in buf.chunks(2) { rgba.extend_from_slice(&[p[0], p[0], p[0], p[1]]); },
        png::ColorType::Indexed => return Err(anyhow!("indexed png not expanded")),
    }
    Ok(Image { w: info.width, h: info.height, rgba })
}

fn strip(s: &str) -> &str { s.strip_prefix("minecraft:").unwrap_or(s) }

fn parse_color(v: Option<&Value>) -> Option<u32> {
    match v? {
        Value::String(s) => u32::from_str_radix(s.trim_start_matches('#'), 16).ok(),
        Value::Number(n) => n.as_i64().map(|x| x as u32 & 0xFFFFFF),
        _ => None,
    }
}

// ------------------------------------------------------------------ models

/// `forced`: texture variables declared as {"sprite": .., "force_translucent": true}
struct RawModel { textures: HashMap<String, String>, forced: std::collections::HashSet<String>, elements: Option<Vec<Value>>, ao: bool }

struct Baker<'a> {
    jar: &'a Jar,
    raw: HashMap<String, std::rc::Rc<RawModel>>,
    atlas: &'a Atlas,
}

fn rot_point(p: [f32; 3], axis: usize, deg: f32, origin: [f32; 3]) -> [f32; 3] {
    let (s, c) = deg.to_radians().sin_cos();
    let v = [p[0] - origin[0], p[1] - origin[1], p[2] - origin[2]];
    let r = match axis {
        0 => [v[0], v[1] * c - v[2] * s, v[1] * s + v[2] * c],
        1 => [v[0] * c + v[2] * s, v[1], -v[0] * s + v[2] * c],
        _ => [v[0] * c - v[1] * s, v[0] * s + v[1] * c, v[2]],
    };
    [r[0] + origin[0], r[1] + origin[1], r[2] + origin[2]]
}

fn rot_dir(d: u8, axis: usize, quarter_turns: i32) -> u8 {
    if d >= 6 { return d; }
    let v = DIR_VEC[d as usize];
    let p = rot_point([v[0] as f32, v[1] as f32, v[2] as f32], axis, quarter_turns as f32 * 90.0, [0.0; 3]);
    let q = [p[0].round() as i32, p[1].round() as i32, p[2].round() as i32];
    DIR_VEC.iter().position(|x| *x == q).unwrap_or(d as usize) as u8
}

fn dir_from_name(s: &str) -> u8 { match s { "down" | "bottom" => DOWN, "up" => UP, "north" => NORTH, "south" => SOUTH, "west" => WEST, "east" => EAST, _ => NO_CULL } }

fn face_normal_dir(pos: &[[f32; 3]; 4]) -> u8 {
    let a = glam::Vec3::from(pos[1]) - glam::Vec3::from(pos[0]);
    let b = glam::Vec3::from(pos[2]) - glam::Vec3::from(pos[0]);
    let n = a.cross(b).normalize_or_zero();
    let mut best = UP; let mut bd = -2.0;
    for (i, d) in DIR_VEC.iter().enumerate() { let dot = n.dot(glam::Vec3::new(d[0] as f32, d[1] as f32, d[2] as f32)); if dot > bd { bd = dot; best = i as u8; } }
    best
}

impl<'a> Baker<'a> {
    fn raw(&mut self, name: &str) -> Option<std::rc::Rc<RawModel>> {
        let name = strip(name).to_string();
        if let Some(m) = self.raw.get(&name) { return Some(m.clone()); }
        let v = self.jar.json(&format!("assets/minecraft/models/{name}.json"))?;
        let mut textures = HashMap::new(); let mut forced = std::collections::HashSet::new(); let mut elements = None; let mut ao = true;
        if let Some(p) = v.get("parent").and_then(|p| p.as_str()) {
            if let Some(pm) = self.raw(p) { textures = pm.textures.clone(); forced = pm.forced.clone(); elements = pm.elements.clone(); ao = pm.ao; }
        }
        if let Some(t) = v.get("textures").and_then(|t| t.as_object()) {
            for (k, val) in t {
                let s = match val { Value::String(s) => s.clone(), Value::Object(o) => o.get("sprite").and_then(|s| s.as_str()).unwrap_or("").to_string(), _ => continue };
                if val.get("force_translucent").and_then(|f| f.as_bool()).unwrap_or(false) { forced.insert(k.clone()); } else { forced.remove(k); }
                textures.insert(k.clone(), s);
            }
        }
        if let Some(e) = v.get("elements").and_then(|e| e.as_array()) { elements = Some(e.clone()); }
        if let Some(a) = v.get("ambientocclusion").and_then(|a| a.as_bool()) { ao = a; }
        let m = std::rc::Rc::new(RawModel { textures, forced, elements, ao });
        self.raw.insert(name, m.clone());
        Some(m)
    }

    /// Resolve a face texture reference; also reports whether a variable on the way was force_translucent.
    fn resolve_tex(m: &RawModel, t: &str) -> Option<(String, bool)> {
        let mut t = t.to_string();
        let mut forced = false;
        // a bare key ("all") is a texture variable reference too
        if !t.starts_with('#') && !t.contains('/') && !t.contains(':') && m.textures.contains_key(&t) { t = format!("#{t}"); }
        for _ in 0..16 {
            match t.strip_prefix('#') {
                Some(k) => { forced |= m.forced.contains(k); t = m.textures.get(k)?.clone() }
                None => return Some((strip(&t).to_string(), forced)),
            }
        }
        None
    }

    /// Bake one model with a variant rotation into quads appended to `out`.
    fn bake(&mut self, model: &str, rx: i32, ry: i32, uvlock: bool, out: &mut Model) {
        let Some(m) = self.raw(model) else { out.missing = true; return };
        let Some(elements) = &m.elements else { return };
        for el in elements {
            let f = |k: &str| -> [f32; 3] { let a = el[k].as_array(); let g = |i: usize| a.and_then(|a| a.get(i)).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32; [g(0), g(1), g(2)] };
            let (from, to) = (f("from"), f("to"));
            let shade = el.get("shade").and_then(|s| s.as_bool()).unwrap_or(true);
            let rot = el.get("rotation");
            let Some(faces) = el.get("faces").and_then(|f| f.as_object()) else { continue };
            let (x1, y1, z1, x2, y2, z2) = (from[0], from[1], from[2], to[0], to[1], to[2]);
            let mut el_min = [f32::MAX; 3]; let mut el_max = [f32::MIN; 3];
            let mut opaque_faces = 0;
            for (fname, face) in faces {
                let d = dir_from_name(fname); if d == NO_CULL { continue; }
                let mut pos: [[f32; 3]; 4] = match d {
                    DOWN => [[x1, y1, z2], [x1, y1, z1], [x2, y1, z1], [x2, y1, z2]],
                    UP => [[x1, y2, z1], [x1, y2, z2], [x2, y2, z2], [x2, y2, z1]],
                    NORTH => [[x2, y2, z1], [x2, y1, z1], [x1, y1, z1], [x1, y2, z1]],
                    SOUTH => [[x1, y2, z2], [x1, y1, z2], [x2, y1, z2], [x2, y2, z2]],
                    WEST => [[x1, y2, z1], [x1, y1, z1], [x1, y1, z2], [x1, y2, z2]],
                    _ => [[x2, y2, z2], [x2, y1, z2], [x2, y1, z1], [x2, y2, z1]],
                };
                let uv: [f32; 4] = match face.get("uv").and_then(|u| u.as_array()) {
                    Some(a) if a.len() == 4 => [a[0].as_f64().unwrap_or(0.0) as f32, a[1].as_f64().unwrap_or(0.0) as f32, a[2].as_f64().unwrap_or(16.0) as f32, a[3].as_f64().unwrap_or(16.0) as f32],
                    _ => match d {
                        DOWN => [x1, 16.0 - z2, x2, 16.0 - z1], UP => [x1, z1, x2, z2],
                        NORTH => [16.0 - x2, 16.0 - y2, 16.0 - x1, 16.0 - y1], SOUTH => [x1, 16.0 - y2, x2, 16.0 - y1],
                        WEST => [z1, 16.0 - y2, z2, 16.0 - y1], _ => [16.0 - z2, 16.0 - y2, 16.0 - z1, 16.0 - y1],
                    },
                };
                let corners = [[uv[0], uv[1]], [uv[0], uv[3]], [uv[2], uv[3]], [uv[2], uv[1]]];
                let r = (face.get("rotation").and_then(|r| r.as_i64()).unwrap_or(0) / 90).rem_euclid(4) as usize;
                let mut uvs = [[0f32; 2]; 4];
                for i in 0..4 { uvs[i] = corners[(i + r) % 4]; }
                // element rotation
                if let Some(rot) = rot {
                    let origin = rot.get("origin").and_then(|o| o.as_array()).map(|a| [a[0].as_f64().unwrap_or(8.0) as f32, a[1].as_f64().unwrap_or(8.0) as f32, a[2].as_f64().unwrap_or(8.0) as f32]).unwrap_or([8.0; 3]);
                    let rescale = rot.get("rescale").and_then(|r| r.as_bool()).unwrap_or(false);
                    let mut apply = |axis: usize, angle: f32| {
                        if angle == 0.0 { return; }
                        for p in pos.iter_mut() {
                            *p = rot_point(*p, axis, angle, origin);
                        }
                        if rescale {
                            let s = 1.0 / angle.to_radians().cos().abs().max(0.01);
                            for p in pos.iter_mut() { for a in 0..3 { if a != axis { p[a] = (p[a] - origin[a]) * s + origin[a]; } } }
                        }
                    };
                    if let Some(axis) = rot.get("axis").and_then(|a| a.as_str()) {
                        let angle = rot.get("angle").and_then(|a| a.as_f64()).unwrap_or(0.0) as f32;
                        apply(match axis { "x" => 0, "y" => 1, _ => 2 }, angle);
                    } else {
                        for (i, k) in ["x", "y", "z"].iter().enumerate() { if let Some(a) = rot.get(*k).and_then(|a| a.as_f64()) { apply(i, a as f32); } }
                    }
                }
                // faces of a zero-thickness element (flat planes, rotated or not) have no area: not emitted
                let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
                let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
                let len = |a: [f32; 3]| (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
                if len(cross(sub(pos[1], pos[0]), sub(pos[2], pos[0]))) + len(cross(sub(pos[2], pos[0]), sub(pos[3], pos[0]))) < 1e-3 { continue; }
                let mut cull = face.get("cullface").and_then(|c| c.as_str()).map(dir_from_name).unwrap_or(NO_CULL);
                // variant rotation (x then y), negative angles about block centre
                if rx != 0 { for p in pos.iter_mut() { *p = rot_point(*p, 0, -(rx as f32), [8.0; 3]); } cull = rot_dir(cull, 0, -rx / 90); }
                if ry != 0 { for p in pos.iter_mut() { *p = rot_point(*p, 1, -(ry as f32), [8.0; 3]); } cull = rot_dir(cull, 1, -ry / 90); }
                let dir = face_normal_dir(&pos);
                // uvlock (matches the vanilla client): under a pure y rotation side faces keep their authored
                // UVs; otherwise the UVs follow the face position in world space
                if uvlock && (rx != 0 || ry != 0) && !(rx == 0 && dir != UP && dir != DOWN) {
                    for i in 0..4 {
                        let p = pos[i];
                        uvs[i] = match dir { DOWN => [p[0], 16.0 - p[2]], UP => [p[0], p[2]], NORTH => [16.0 - p[0], 16.0 - p[1]], SOUTH => [p[0], 16.0 - p[1]], WEST => [p[2], 16.0 - p[1]], _ => [16.0 - p[2], 16.0 - p[1]] };
                    }
                }
                let Some((tex, forced)) = face.get("texture").and_then(|t| t.as_str()).and_then(|t| Self::resolve_tex(&m, t)) else { out.missing = true; continue };
                if !self.atlas.rects.contains_key(&tex) { out.missing = true; }
                let (rect, _) = self.atlas.rects.get(&tex).or_else(|| self.atlas.rects.get("missing")).copied().unwrap();
                // render layer from the alpha of the texture region this face samples (matches the
                // vanilla client per quad, see basalt-check client-mesh): any partially transparent
                // pixel -> translucent, any fully transparent pixel -> cutout, else solid
                let layer = if forced || matches!(tex.as_str(), "block/glass" | "block/glass_pane_top") { LAYER_TRANSLUCENT } else {
                    let (mut u0, mut u1, mut v0, mut v1) = (16f32, 0f32, 16f32, 0f32);
                    for u in &uvs { u0 = u0.min(u[0]); u1 = u1.max(u[0]); v0 = v0.min(u[1]); v1 = v1.max(u[1]); }
                    self.atlas.region_layer(rect, [u0 / 16.0, v0 / 16.0, u1 / 16.0, v1 / 16.0])
                };
                for u in uvs.iter_mut() {
                    let cu = u[0].clamp(0.0, 16.0) / 16.0; let cv = u[1].clamp(0.0, 16.0) / 16.0;
                    u[0] = (rect.x as f32 + cu * rect.w as f32) / self.atlas.w as f32;
                    u[1] = (rect.y as f32 + cv * rect.h as f32) / self.atlas.h as f32;
                }
                // no UV inset: the vanilla client's quads use the exact face UVs (client-mesh harness); the
                // atlas pads every sprite with its edge pixels instead
                for p in pos.iter_mut() { for a in 0..3 { p[a] /= 16.0; el_min[a] = el_min[a].min(p[a]); el_max[a] = el_max[a].max(p[a]); } }
                if layer == LAYER_OPAQUE { opaque_faces += 1; }
                out.quads.push(Quad { pos, uv: uvs, cull, dir, tinted: face.get("tintindex").and_then(|t| t.as_i64()).map(|t| t >= 0).unwrap_or(false), layer, shade, ao: m.ao });
            }
            if el_min[0] <= el_max[0] {
                let b = [el_min[0], el_min[1], el_min[2], el_max[0], el_max[1], el_max[2]];
                let full = b[0] <= 0.001 && b[1] <= 0.001 && b[2] <= 0.001 && b[3] >= 0.999 && b[4] >= 0.999 && b[5] >= 0.999;
                if full { out.full_cube = true; if opaque_faces == 6 { out.occludes = true; } }
                out.boxes.push(b);
                if rot.and_then(|r| r.get("angle")).and_then(|a| a.as_f64()).unwrap_or(0.0) == 0.0 && rot.map(|r| r.get("x").is_none() && r.get("y").is_none() && r.get("z").is_none()).unwrap_or(true) { out.shape_boxes.push(b); }
            }
        }
    }
}

impl Atlas {
    /// Layer for the sub-rectangle [u0, v0, u1, v1] (0..1 within the sprite) of a sprite.
    pub fn region_layer(&self, r: Rect, uv: [f32; 4]) -> u8 {
        let px = |f: f32, n: u32| f.clamp(0.0, 1.0) * n as f32;
        let x0 = (px(uv[0], r.w) + 1e-3).floor() as u32; let x1 = ((px(uv[2], r.w) - 1e-3).ceil() as u32).max(x0 + 1).min(r.w);
        let y0 = (px(uv[1], r.h) + 1e-3).floor() as u32; let y1 = ((px(uv[3], r.h) - 1e-3).ceil() as u32).max(y0 + 1).min(r.h);
        let (mut zero, mut part) = (false, false);
        for y in y0.min(r.h - 1)..y1 { for x in x0.min(r.w - 1)..x1 {
            let a = self.rgba[(((r.y + y) * self.w + r.x + x) * 4 + 3) as usize];
            if a == 0 { zero = true } else if a < 255 { part = true }
        } }
        if part { LAYER_TRANSLUCENT } else if zero { LAYER_CUTOUT } else { LAYER_OPAQUE }
    }
}

// ------------------------------------------------------------------ atlas

fn build_atlas(jar: &Jar) -> Atlas {
    let mut imgs: Vec<(String, Image)> = Vec::new();
    for (path, data) in &jar.files {
        let Some(rest) = path.strip_prefix("assets/minecraft/textures/") else { continue };
        if !(rest.starts_with("block/") || rest.starts_with("item/")) || !rest.ends_with(".png") { continue; }
        if let Ok(mut img) = decode_png(data) {
            if img.h > img.w && img.h % img.w == 0 { img.rgba.truncate((img.w * img.w * 4) as usize); img.h = img.w; } // animation strip: first frame
            imgs.push((rest.trim_end_matches(".png").to_string(), img));
        }
    }
    // magenta/black missing texture
    let mut miss = vec![0u8; 16 * 16 * 4];
    for y in 0..16 { for x in 0..16 { let c: [u8; 4] = if (x < 8) ^ (y < 8) { [248, 0, 248, 255] } else { [0, 0, 0, 255] }; miss[(y * 16 + x) * 4..][..4].copy_from_slice(&c); } }
    imgs.push(("missing".into(), Image { w: 16, h: 16, rgba: miss }));
    imgs.push(("white".into(), Image { w: 16, h: 16, rgba: vec![255; 16 * 16 * 4] }));
    imgs.sort_by(|a, b| b.1.h.cmp(&a.1.h).then(a.0.cmp(&b.0)));
    let w = 2048u32; let pad = 2u32;
    let (mut x, mut y, mut row_h) = (0u32, 0u32, 0u32);
    let mut placed = Vec::new();
    for (name, img) in &imgs {
        if x + img.w + pad > w { x = 0; y += row_h + pad; row_h = 0; }
        placed.push((name.clone(), Rect { x: x + pad / 2, y: y + pad / 2, w: img.w, h: img.h }));
        x += img.w + pad; row_h = row_h.max(img.h);
    }
    let h = (y + row_h + pad).next_power_of_two();
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let mut rects = HashMap::new();
    for ((name, img), (_, r)) in imgs.iter().zip(&placed) {
        let (mut zero, mut partial) = (0, 0);
        for py in 0..img.h {
            let src = &img.rgba[(py * img.w * 4) as usize..((py + 1) * img.w * 4) as usize];
            let dst = ((r.y + py) * w + r.x) as usize * 4;
            rgba[dst..dst + src.len()].copy_from_slice(src);
            for p in src.chunks(4) { if p[3] < 26 { zero += 1 } else if p[3] < 230 { partial += 1 } }
        }
        // extend edge pixels into padding to avoid seams
        for py in 0..img.h {
            let row = ((r.y + py) * w) as usize * 4;
            let l = row + r.x as usize * 4; let rr = row + (r.x + r.w - 1) as usize * 4;
            let (lp, rp): ([u8; 4], [u8; 4]) = (rgba[l..l + 4].try_into().unwrap(), rgba[rr..rr + 4].try_into().unwrap());
            rgba[l - 4..l].copy_from_slice(&lp); rgba[rr + 4..rr + 8].copy_from_slice(&rp);
        }
        for px in 0..img.w + 2 {
            let cx = (r.x + px - 1) as usize;
            let top = (r.y * w) as usize * 4 + cx * 4; let bot = ((r.y + r.h - 1) * w) as usize * 4 + cx * 4;
            let (tp, bp): ([u8; 4], [u8; 4]) = (rgba[top..top + 4].try_into().unwrap(), rgba[bot..bot + 4].try_into().unwrap());
            let up = top - (w as usize * 4); let dn = bot + (w as usize * 4);
            rgba[up..up + 4].copy_from_slice(&tp); rgba[dn..dn + 4].copy_from_slice(&bp);
        }
        let layer = if partial * 20 > (img.w * img.h) as usize { LAYER_TRANSLUCENT } else if zero > 0 || partial > 0 { LAYER_CUTOUT } else { LAYER_OPAQUE };
        rects.insert(name.clone(), (*r, layer));
    }
    Atlas { w, h, rgba, rects }
}

// ------------------------------------------------------------------ block states

fn cond_matches(when: &Value, st: BlockState) -> bool {
    let Some(o) = when.as_object() else { return true };
    if let Some(or) = o.get("OR").and_then(|v| v.as_array()) { return or.iter().any(|w| cond_matches(w, st)); }
    if let Some(and) = o.get("AND").and_then(|v| v.as_array()) { return and.iter().all(|w| cond_matches(w, st)); }
    o.iter().all(|(k, v)| {
        let want = match v { Value::String(s) => s.clone(), other => other.to_string() };
        let (neg, want) = match want.strip_prefix('!') { Some(w) => (true, w.to_string()), None => (false, want) };
        match st.get(k) { Some(have) => want.split('|').any(|w| w == have) != neg, None => false }
    })
}

fn variant_matches(key: &str, st: BlockState) -> bool {
    key.split(',').filter(|s| !s.is_empty()).all(|kv| match kv.split_once('=') { Some((k, v)) => st.get(k) == Some(v), None => true })
}

const NO_COLLIDE_KINDS: &[&str] = &["air", "attached_stem", "bamboo_sapling", "banner", "base_coral_fan", "base_coral_plant", "base_coral_wall_fan", "beetroot", "bubble_column", "bush", "button",
    "carrot", "cave_vines", "cave_vines_plant", "coral_fan", "coral_plant", "coral_wall_fan", "crop", "detector_rail", "double_plant", "dry_vegetation", "end_gateway", "end_portal", "eyeblossom",
    "fire", "firefly_bush", "flower", "flower_bed", "frogspawn", "glow_lichen", "hanging_moss", "hanging_roots", "kelp", "kelp_plant", "leaf_litter", "lever", "light", "liquid", "mangrove_propagule",
    "multiface", "mushroom", "nether_fungus", "nether_portal", "nether_roots", "nether_sprouts", "nether_wart", "pitcher_crop", "potato", "powder_snow", "powered_rail", "pressure_plate", "rail",
    "redstone_torch", "redstone_wall_torch", "redstone_wire", "sapling", "sculk_vein", "seagrass", "short_dry_grass", "small_dripleaf", "soul_fire", "spore_blossom", "standing_sign", "wall_sign",
    "ceiling_hanging_sign", "wall_hanging_sign", "stem", "structure_void", "sugar_cane", "sweet_berry_bush", "tall_dry_grass", "tall_flower", "tall_grass", "tall_seagrass", "torch", "torchflower_crop",
    "trip_wire_hook", "tripwire", "twisting_vines", "twisting_vines_plant", "vine", "wall_banner", "wall_torch", "web", "weeping_vines", "weeping_vines_plant", "weighted_pressure_plate", "wither_rose",
    "big_dripleaf_stem", "cactus_flower", "nether_sprouts", "moving_piston"];

/// Redstone dust colour per power level (measured from the vanilla client with the client-mesh oracle).
const REDSTONE_TINT: [u32; 16] = [0x4C0000, 0x700000, 0x7A0000, 0x840000, 0x8E0000, 0x990000, 0xA30000, 0xAD0000,
    0xB70000, 0xC10000, 0xCC0000, 0xD60000, 0xE00000, 0xEA0600, 0xF41B00, 0xFF3200];

/// Per-state tint: state-dependent colours first, then the per-block table.
fn tint_for_state(name: &str, st: BlockState) -> Tint {
    let num = |p: &str| st.get(p).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    match name {
        "redstone_wire" => Tint::Fixed(REDSTONE_TINT[num("power").min(15) as usize]),
        // stem colour moves linearly from green to yellow with age (measured)
        "melon_stem" | "pumpkin_stem" => { let a = num("age").min(7); Tint::Fixed((a * 32) << 16 | (255 - a * 8) << 8 | a * 4) }
        _ => tint_for(name),
    }
}

fn tint_for(name: &str) -> Tint {
    match name {
        "grass_block" | "short_grass" | "tall_grass" | "fern" | "large_fern" | "potted_fern" | "sugar_cane" | "bush" => Tint::Grass,
        "oak_leaves" | "jungle_leaves" | "acacia_leaves" | "dark_oak_leaves" | "mangrove_leaves" | "vine" => Tint::Foliage,
        "birch_leaves" => Tint::Fixed(0x80A755), "spruce_leaves" => Tint::Fixed(0x619961), "lily_pad" => Tint::Fixed(0x208030),
        "water" | "bubble_column" | "water_cauldron" => Tint::Water,
        "leaf_litter" => Tint::DryFoliage,
        "attached_melon_stem" | "attached_pumpkin_stem" => Tint::Fixed(0xE0C71C), "melon_stem" | "pumpkin_stem" => Tint::Fixed(0x60C020),
        "redstone_wire" => Tint::Fixed(0xC01000),
        "pink_petals" | "wildflowers" => Tint::Grass,
        _ => Tint::None,
    }
}

/// Blocks whose model is shifted by a position-dependent offset in the vanilla client (measured with
/// `basalt-check client-mesh` + probes at 100 positions per block): (max horizontal, vertical range).
pub fn model_offset_params(name: &str) -> (f32, f32) {
    match name {
        "pointed_dripstone" | "sulfur_spike" => (0.125, 0.0),
        "fern" | "short_grass" | "short_dry_grass" | "tall_dry_grass" => (0.25, 0.2),
        "small_dripleaf" => (0.25, 0.1),
        "bamboo" | "bamboo_sapling" | "mangrove_propagule" | "hanging_roots" | "crimson_roots" | "warped_roots" | "nether_sprouts"
        | "large_fern" | "tall_grass" | "tall_seagrass" | "lilac" | "peony" | "rose_bush" | "sunflower" | "pitcher_plant"
        | "allium" | "azure_bluet" | "blue_orchid" | "cornflower" | "dandelion" | "golden_dandelion" | "lily_of_the_valley"
        | "oxeye_daisy" | "poppy" | "orange_tulip" | "pink_tulip" | "red_tulip" | "white_tulip" | "torchflower" | "wither_rose"
        | "closed_eyeblossom" | "open_eyeblossom" => (0.25, 0.0),
        _ => (0.0, 0.0),
    }
}

pub struct Assets {
    pub atlas: Atlas,
    pub models: Vec<Model>,
    pub states: Vec<StateInfo>,
    pub water_still: [f32; 4], pub water_flow: [f32; 4], pub lava_still: [f32; 4], pub water_overlay: [f32; 4], pub lava_flow: [f32; 4],
    pub grass_map: Option<Image>, pub foliage_map: Option<Image>,
    pub biome_json: HashMap<String, Value>,
    pub dimension_types: HashMap<String, DimensionType>,
    pub font: Option<Image>,
    /// textures/gui/** keyed by path below textures/gui/ without ".png" (e.g. "sprites/hud/hotbar")
    pub gui: HashMap<String, Image>,
    /// textures/entity/** keyed by path below textures/entity/ without ".png" (e.g. "zombie/zombie")
    pub entity_tex: HashMap<String, Image>,
    /// item id -> texture name in atlas (flat icons)
    pub item_icons: Vec<Option<String>>,
    /// item id -> baked block model (3D icon / dropped mini-block), tinted with `item_tint`
    pub item_models: Vec<Option<u32>>,
    pub item_tint: Vec<Option<u32>>,
    /// per block state: the blockstate file applied no model at all
    /// 0 = has a model, 1 = the blockstate file applied no model, 2 = multipart with no matching part (renders nothing, like vanilla)
    pub no_model: Vec<u8>,
    /// per block state: the blockstate file offers several weighted models (vanilla picks one by position)
    pub random_variant: Vec<bool>,
    /// per block state with weighted variants: for each part (multipart apply / the variants entry) its (model, weight) list;
    /// vanilla picks one per part from a position-seeded random generator (see `mesh::pick_variants`)
    pub variant_parts: Vec<Option<Box<Vec<Vec<(u32, u32)>>>>>,
}

impl Assets {
    pub fn load(jar_path: &str) -> Result<Assets> {
        let jar = Jar::open(jar_path)?;
        let atlas = build_atlas(&jar);
        let mut models: Vec<Model> = vec![Model::default()];
        let mut states = Vec::with_capacity(basalt_data::blocks::STATE_COUNT);
        crate::lang::init(jar.files.get("assets/minecraft/lang/en_us.json").map(|v| v.as_slice()));
        let mut no_model = Vec::with_capacity(basalt_data::blocks::STATE_COUNT);
        let mut random_variant = Vec::with_capacity(basalt_data::blocks::STATE_COUNT);
        let mut variant_parts: Vec<Option<Box<Vec<Vec<(u32, u32)>>>>> = vec![None; basalt_data::blocks::STATE_COUNT];
        let (mut item_models, mut item_tint, mut item_icons): (Vec<Option<u32>>, Vec<Option<u32>>, Vec<Option<String>>) = (Vec::new(), Vec::new(), Vec::new());
        {
            let mut baker = Baker { jar: &jar, raw: HashMap::new(), atlas: &atlas };
            let mut cache: HashMap<String, u32> = HashMap::new();
            let mut bs_cache: HashMap<&str, Option<Value>> = HashMap::new();
            for id in 0..basalt_data::blocks::STATE_COUNT as u32 {
                let st = BlockState(id); let b = st.block();
                let bs = bs_cache.entry(b.name).or_insert_with(|| jar.json(&format!("assets/minecraft/blockstates/{}.json", b.name)));
                // collect applied (model, x, y, uvlock) = the first variant of every part, plus every part's weighted variants
                let mut applied: Vec<(String, i32, i32, bool)> = Vec::new();
                let mut parts: Vec<Vec<(String, i32, i32, bool, u32)>> = Vec::new();
                let mut random = false;
                let mut push = |v: &Value| {
                    let list: Vec<&Value> = match v.as_array() { Some(a) => a.iter().collect(), None => vec![v] };
                    if list.len() > 1 { random = true; }
                    let mut part = Vec::new();
                    for v in list {
                        if let Some(m) = v.get("model").and_then(|m| m.as_str()) {
                            part.push((m.to_string(), v.get("x").and_then(|x| x.as_i64()).unwrap_or(0) as i32, v.get("y").and_then(|x| x.as_i64()).unwrap_or(0) as i32,
                                v.get("uvlock").and_then(|x| x.as_bool()).unwrap_or(false), v.get("weight").and_then(|w| w.as_u64()).unwrap_or(1) as u32));
                        }
                    }
                    if let Some(f) = part.first() { applied.push((f.0.clone(), f.1, f.2, f.3)); parts.push(part); }
                };
                if let Some(bs) = bs {
                    if let Some(vars) = bs.get("variants").and_then(|v| v.as_object()) {
                        if let Some((_, v)) = vars.iter().find(|(k, _)| variant_matches(k, st)) { push(v); }
                    } else if let Some(mp) = bs.get("multipart").and_then(|v| v.as_array()) {
                        for p in mp { if p.get("when").map(|w| cond_matches(w, st)).unwrap_or(true) { if let Some(a) = p.get("apply") { push(a); } } }
                    }
                }
                // states with weighted variants: bake every variant of every part on its own (picked per position at mesh time)
                if random {
                    let mut out_parts = Vec::new();
                    for part in &parts {
                        let mut vs = Vec::new();
                        for (name, x, y, uvl, w) in part {
                            let key = format!("single:{name}:{x}:{y}:{uvl}");
                            let mi = match cache.get(&key) { Some(i) => *i, None => {
                                let mut m = Model::default(); baker.bake(name, *x, *y, *uvl, &mut m);
                                models.push(m); let i = models.len() as u32 - 1; cache.insert(key, i); i
                            } };
                            vs.push((mi, (*w).max(1)));
                        }
                        out_parts.push(vs);
                    }
                    variant_parts[id as usize] = Some(Box::new(out_parts));
                }
                // a multipart state matching no part renders nothing in vanilla either (e.g. an isolated wall without post)
                random_variant.push(random);
                no_model.push(if !applied.is_empty() { 0 } else if bs.as_ref().map(|b| b.get("multipart").is_some()).unwrap_or(false) { 2 } else { 1 });
                let key = format!("{applied:?}");
                let mi = match cache.get(&key) { Some(i) => *i, None => {
                    let mut m = Model::default();
                    for (name, x, y, uvl) in &applied { baker.bake(name, *x, *y, *uvl, &mut m); }
                    // blocks drawn like entities (signs, beds, banners, heads) never hide neighbour faces
                    if !matches!(b.kind, "bed" | "standing_sign" | "wall_sign" | "ceiling_hanging_sign" | "wall_hanging_sign" | "banner" | "wall_banner" | "skull" | "wall_skull" | "piglinwallskull" | "player_head" | "player_wall_head" | "wither_skull" | "wither_wall_skull") { m.compute_occlusion(); }
                    m.compute_shape();
                    models.push(m); let i = models.len() as u32 - 1; cache.insert(key, i); i
                } };
                let m = &models[mi as usize];
                let name = b.name;
                let waterlogged = st.get("waterlogged") == Some("true") || matches!(name, "kelp" | "kelp_plant" | "seagrass" | "tall_seagrass" | "bubble_column");
                let (fluid, level) = if name == "water" || name == "lava" {
                    (if name == "water" { 1 } else { 2 }, st.get("level").and_then(|l| l.parse::<u8>().ok()).unwrap_or(0))
                } else if waterlogged { (1, 0) } else { (0, 0) };
                let mut collides = !NO_COLLIDE_KINDS.contains(&b.kind) && !matches!(name, "short_grass" | "fern" | "dead_bush" | "snow" | "structure_void" | "light" | "cobweb") ;
                if name == "snow" { collides = st.get("layers") != Some("1"); }
                if b.kind == "fence_gate" && st.get("open") == Some("true") { collides = false; }
                states.push(StateInfo {
                    model: mi, tint: tint_for_state(name, st), occludes: m.occludes, self_cull: m.full_cube && !m.occludes && !name.ends_with("leaves"), fluid_overlay: m.full_cube && !m.occludes, offset: model_offset_params(name), hides_fluid: !(name.contains("trapdoor") || name == "mangrove_roots" || name.ends_with("leaves") || name.contains("glass") || name.contains("grate")), collides, fluid, fluid_level: level,
                    emits_light: basalt_world::props::emission(id) > 0, emission: basalt_world::props::emission(id),
                    full_bright: name == "magma_block" || (name.ends_with("sculk_sensor") && st.get("sculk_sensor_phase") == Some("active")),
                    ao_occluder: m.full_cube && collides && !(m.full_cube && !m.occludes && !name.ends_with("leaves")),
                });
            }
            // items: items/<name>.json → a block model (3D icon) or an item model's layer0 texture
            for n in basalt_data::registries::ITEM.iter() {
                let mut model_idx = None; let mut icon = None; let mut tint = None;
                let def = jar.json(&format!("assets/minecraft/items/{n}.json"));
                let model = def.as_ref().and_then(|d| d.get("model"));
                // follow the first plain "minecraft:model" reachable through select/condition/range cases
                fn find_model(v: &Value, depth: u32) -> Option<&Value> {
                    if depth > 6 { return None; }
                    if v.get("type").and_then(|t| t.as_str()) == Some("minecraft:model") { return Some(v); }
                    for k in ["fallback", "on_false", "on_true"] { if let Some(m) = v.get(k).and_then(|m| find_model(m, depth + 1)) { return Some(m); } }
                    if let Some(cs) = v.get("cases").or(v.get("entries")).and_then(|c| c.as_array()) { for c in cs { if let Some(m) = c.get("model").and_then(|m| find_model(m, depth + 1)) { return Some(m); } } }
                    None
                }
                if let Some(m) = model.and_then(|m| find_model(m, 0)) {
                    let mname = m.get("model").and_then(|x| x.as_str()).unwrap_or("");
                    if let Some(t) = m.get("tints").and_then(|t| t.as_array()).and_then(|t| t.first()) {
                        tint = Some(match t.get("type").and_then(|x| x.as_str()) {
                            Some("minecraft:grass") => 0x7CBD6B, Some("minecraft:constant") => t.get("value").and_then(|v| v.as_i64()).map(|v| v as u32 & 0xFFFFFF).unwrap_or(0x48B518),
                            _ => t.get("default").and_then(|v| v.as_i64()).map(|v| v as u32 & 0xFFFFFF).unwrap_or(0x48B518),
                        });
                    }
                    if strip(mname).starts_with("block/") {
                        let mut bm = Model::default();
                        baker.bake(mname, 0, 0, false, &mut bm);
                        if !bm.quads.is_empty() { models.push(bm); model_idx = Some(models.len() as u32 - 1); }
                    }
                    // flat icon: layer0 of the model (follow parents)
                    if let Some(raw) = baker.raw(mname) {
                        if let Some(t) = raw.textures.get("layer0").or(raw.textures.get("particle")) {
                            let t = strip(t).to_string();
                            if !t.starts_with('#') && atlas.rects.contains_key(&t) { icon = Some(t); }
                        }
                    }
                }
                item_models.push(model_idx); item_tint.push(tint); item_icons.push(icon);
            }
        }
        // icons not found through the item definition: guess from texture names
        for (i, n) in basalt_data::registries::ITEM.iter().enumerate() {
            if item_icons[i].is_some() { continue; }
            if atlas.rects.contains_key(&format!("item/{n}")) { item_icons[i] = Some(format!("item/{n}")); continue; }
            for cand in [format!("block/{n}"), format!("block/{n}_top"), format!("block/{n}_side"), format!("block/{n}_front"), format!("block/{}", n.replace("_slab", "").replace("_stairs", "")), format!("block/{}s", n.replace("_slab", "").replace("_stairs", "")), format!("block/{}_planks", n.replace("_slab", "").replace("_stairs", "").replace("_fence", ""))] {
                if atlas.rects.contains_key(&cand) { item_icons[i] = Some(cand); break; }
            }
        }
        let uvrect = |n: &str| -> [f32; 4] {
            let (r, _) = atlas.rects.get(n).or(atlas.rects.get("missing")).copied().unwrap();
            [r.x as f32 / atlas.w as f32, r.y as f32 / atlas.h as f32, (r.x + r.w) as f32 / atlas.w as f32, (r.y + r.h) as f32 / atlas.h as f32]
        };
        let mut biome_json = HashMap::new(); let mut dimension_types = HashMap::new();
        for (p, d) in &jar.files {
            if let Some(n) = p.strip_prefix("data/minecraft/worldgen/biome/").and_then(|n| n.strip_suffix(".json")) {
                if let Ok(v) = serde_json::from_slice::<Value>(d) { biome_json.insert(format!("minecraft:{n}"), v); }
            }
            if let Some(n) = p.strip_prefix("data/minecraft/dimension_type/").and_then(|n| n.strip_suffix(".json")) {
                if let Ok(v) = serde_json::from_slice::<Value>(d) {
                    dimension_types.insert(format!("minecraft:{n}"), DimensionType { name: n.to_string(), min_y: v["min_y"].as_i64().unwrap_or(-64) as i32, height: v["height"].as_i64().unwrap_or(384) as i32, has_skylight: v["has_skylight"].as_bool().unwrap_or(true) });
                }
            }
        }
        let (mut gui, mut entity_tex) = (HashMap::new(), HashMap::new());
        for (p, d) in &jar.files {
            if let Some(n) = p.strip_prefix("assets/minecraft/textures/gui/").and_then(|n| n.strip_suffix(".png")) { if let Ok(i) = decode_png(d) { gui.insert(n.to_string(), i); } }
            if let Some(n) = p.strip_prefix("assets/minecraft/textures/entity/").and_then(|n| n.strip_suffix(".png")) { if let Ok(i) = decode_png(d) { entity_tex.insert(n.to_string(), i); } }
        }
        let _ = BLOCKS;
        Ok(Assets {
            water_still: uvrect("block/water_still"), water_flow: uvrect("block/water_flow"), lava_still: uvrect("block/lava_still"), water_overlay: uvrect("block/water_overlay"), lava_flow: uvrect("block/lava_flow"),
            grass_map: jar.png("assets/minecraft/textures/colormap/grass.png"), foliage_map: jar.png("assets/minecraft/textures/colormap/foliage.png"),
            font: jar.png("assets/minecraft/textures/font/ascii.png"),
            atlas, models, states, biome_json, dimension_types, gui, entity_tex, item_icons, item_models, item_tint, no_model, random_variant, variant_parts,
        })
    }

    /// Atlas UV rect [u0, v0, u1, v1] of a named block/item texture.
    pub fn tex_uv(&self, name: &str) -> Option<[f32; 4]> {
        let (r, _) = self.atlas.rects.get(name)?;
        Some([r.x as f32 / self.atlas.w as f32, r.y as f32 / self.atlas.h as f32, (r.x + r.w) as f32 / self.atlas.w as f32, (r.y + r.h) as f32 / self.atlas.h as f32])
    }
    pub fn item_model(&self, item: i32) -> Option<&Model> { self.item_models.get(item as usize).copied().flatten().map(|i| &self.models[i as usize]) }

    #[inline] pub fn info(&self, state: u16) -> &StateInfo { self.states.get(state as usize).unwrap_or(&self.states[0]) }
    #[inline] pub fn model(&self, state: u16) -> &Model { &self.models[self.info(state).model as usize] }

    pub fn icon_uv(&self, item: i32) -> Option<[f32; 4]> {
        let n = self.item_icons.get(item as usize)?.as_ref()?;
        let (r, _) = self.atlas.rects.get(n)?;
        Some([r.x as f32 / self.atlas.w as f32, r.y as f32 / self.atlas.h as f32, (r.x + r.w) as f32 / self.atlas.w as f32, (r.y + r.h) as f32 / self.atlas.h as f32])
    }

    fn colormap(img: &Option<Image>, temp: f32, downfall: f32, default: u32) -> u32 {
        let Some(img) = img else { return default };
        let t = temp.clamp(0.0, 1.0); let d = downfall.clamp(0.0, 1.0) * t;
        let x = ((1.0 - t) * (img.w - 1) as f32) as u32; let y = ((1.0 - d) * (img.h - 1) as f32) as u32;
        let i = ((y * img.w + x) * 4) as usize;
        (img.rgba[i] as u32) << 16 | (img.rgba[i + 1] as u32) << 8 | img.rgba[i + 2] as u32
    }

    /// Build biome colour table for the registry order sent by the server.
    pub fn biomes(&self, names: &[String]) -> Vec<Biome> {
        names.iter().map(|n| {
            let v = self.biome_json.get(n).cloned().unwrap_or(Value::Null);
            let temp = v["temperature"].as_f64().unwrap_or(0.8) as f32; let down = v["downfall"].as_f64().unwrap_or(0.4) as f32;
            let fx = &v["effects"]; let at = &v["attributes"];
            let mut grass = parse_color(fx.get("grass_color")).unwrap_or_else(|| Self::colormap(&self.grass_map, temp, down, 0x79C05A));
            match fx.get("grass_color_modifier").and_then(|m| m.as_str()) {
                Some("swamp") => grass = 0x6A7039,
                Some("dark_forest") => grass = ((grass & 0xFEFEFE) + 0x28340A) >> 1,
                _ => {}
            }
            Biome {
                name: n.clone(), grass,
                foliage: parse_color(fx.get("foliage_color")).unwrap_or_else(|| Self::colormap(&self.foliage_map, temp, down, 0x59AE30)),
                dry_foliage: parse_color(fx.get("dry_foliage_color")).unwrap_or(0x7B5334),
                water: parse_color(fx.get("water_color")).unwrap_or(0x3F76E4),
                sky: parse_color(at.get("minecraft:visual/sky_color")).or(parse_color(fx.get("sky_color"))).unwrap_or(0x78A7FF),
                fog: parse_color(at.get("minecraft:visual/fog_color")).or(parse_color(fx.get("fog_color"))).unwrap_or(0xC0D8FF),
            }
        }).collect()
    }
}

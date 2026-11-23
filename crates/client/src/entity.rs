//! Entities: client-side state, vanilla-style position interpolation, animation and models.
//!
//! Model geometry is written from public knowledge of the classic entity models (box sizes,
//! pivots and texture offsets as documented by the community, e.g. Blockbench templates and the
//! Minecraft Wiki) in the usual Java model space: y down, front = -Z, units of 1/16 block, origin
//! 24 px above the feet. Textures come from the user's client jar at runtime.
use crate::assets::{Assets, Image};
use crate::mesh::{MeshData, Vertex};
use crate::net::{ItemStack, Meta};
use glam::{Mat4, Vec3, Vec4};
use std::collections::HashMap;
use std::f32::consts::PI;
use uuid::Uuid;

pub fn type_name(kind: i32) -> &'static str { basalt_data::registries::ENTITY_TYPE.get(kind as usize).copied().unwrap_or("") }

/// Hitbox width, height (vanilla sizes, from the wiki) and the fallback box colour.
pub fn dims(kind: i32) -> (f32, f32, [u8; 4]) {
    match type_name(kind) {
        "player" => (0.6, 1.8, [200, 170, 140, 255]),
        "zombie" | "husk" | "drowned" => (0.6, 1.95, [60, 140, 70, 255]),
        "skeleton" | "stray" | "bogged" => (0.6, 1.99, [220, 220, 220, 255]),
        "creeper" => (0.6, 1.7, [40, 200, 40, 255]),
        "spider" => (1.4, 0.9, [70, 50, 50, 255]),
        "cave_spider" => (0.7, 0.5, [40, 60, 70, 255]),
        "cow" | "mooshroom" => (0.9, 1.4, [90, 60, 40, 255]),
        "pig" => (0.9, 0.9, [240, 160, 170, 255]),
        "sheep" => (0.9, 1.3, [245, 245, 245, 255]),
        "chicken" => (0.4, 0.7, [255, 255, 230, 255]),
        "item" | "experience_orb" => (0.25, 0.25, [230, 200, 80, 255]),
        "arrow" | "spectral_arrow" => (0.5, 0.5, [120, 90, 60, 255]),
        _ => (0.6, 0.9, [200, 170, 140, 255]),
    }
}

pub fn is_living(kind: i32) -> bool { !matches!(type_name(kind), "item" | "arrow" | "spectral_arrow" | "experience_orb" | "") }

pub struct Entity {
    pub id: i32,
    pub uuid: Uuid,
    pub kind: i32,
    /// authoritative position (sum of server deltas)
    pub server_pos: [f64; 3],
    pub pos: [f64; 3],
    pub prev: [f64; 3],
    target: [f64; 3],
    steps: u32,
    pub yaw: f32, pub pitch: f32, target_yaw: f32, target_pitch: f32,
    pub head_yaw: f32, prev_head_yaw: f32, target_head_yaw: f32,
    pub body_yaw: f32, prev_body_yaw: f32,
    prev_pitch: f32,
    pub vel: [f64; 3],
    pub on_ground: bool,
    pub flags: u8,
    pub pose: i32,
    pub item: Option<ItemStack>,
    pub health: Option<f32>,
    pub baby: bool,
    pub wool: u8,
    pub mob_flags: u8,
    pub hurt: u32,
    pub death: u32,
    pub swing: i32,
    limb_pos: f32, limb_amt: f32, prev_limb_amt: f32, prev_limb_pos: f32,
    pub age: u32,
    bob: f32,
}

impl Entity {
    pub fn new(id: i32, uuid: Uuid, kind: i32, pos: [f64; 3], vel: [f64; 3], yaw: f32, pitch: f32, head_yaw: f32) -> Entity {
        Entity {
            id, uuid, kind, server_pos: pos, pos, prev: pos, target: pos, steps: 0,
            yaw, pitch, target_yaw: yaw, target_pitch: pitch, head_yaw, prev_head_yaw: head_yaw, target_head_yaw: head_yaw, body_yaw: yaw, prev_body_yaw: yaw, prev_pitch: pitch,
            vel, on_ground: false, flags: 0, pose: 0, item: None, health: None, baby: false, wool: 0, mob_flags: 0, hurt: 0, death: 0, swing: -1,
            limb_pos: 0.0, limb_amt: 0.0, prev_limb_amt: 0.0, prev_limb_pos: 0.0, age: 0, bob: (id as u32).wrapping_mul(2654435761) as f32 / u32::MAX as f32 * PI * 2.0,
        }
    }
    fn lerp_steps(&self) -> u32 { match type_name(self.kind) { "item" | "experience_orb" => 1, "arrow" | "spectral_arrow" => 1, _ => 3 } }

    /// Server moved the entity to `p` (absolute): interpolate over a few ticks like vanilla clients.
    pub fn move_to(&mut self, p: [f64; 3], rot: Option<(f32, f32)>) {
        self.server_pos = p;
        // big jumps (teleports) snap
        let d2 = (0..3).map(|i| (p[i] - self.pos[i]).powi(2)).sum::<f64>();
        if d2 > 64.0 * 64.0 { self.pos = p; self.prev = p; self.steps = 0; } else { self.target = p; self.steps = self.lerp_steps(); }
        if let Some((y, pi)) = rot { self.target_yaw = y; self.target_pitch = pi; if !is_living(self.kind) || type_name(self.kind) != "player" { if self.steps == 0 { self.yaw = y; self.pitch = pi; } } }
    }
    pub fn set_head(&mut self, y: f32) { self.target_head_yaw = y; }

    pub fn apply_meta(&mut self, entries: &[(u8, Meta)]) {
        let name = type_name(self.kind);
        for (idx, v) in entries {
            match (*idx, v) {
                (0, Meta::Byte(b)) => self.flags = *b,
                (6, Meta::Int(p)) => self.pose = *p,
                (8, Meta::Item(it)) if name == "item" => self.item = Some(it.clone()),
                (9, Meta::Float(h)) => { self.health = Some(*h); if *h <= 0.0 && self.death == 0 { self.death = 1; } }
                (15, Meta::Byte(b)) => self.mob_flags = *b,
                (16, Meta::Bool(b)) => self.baby = *b,
                (18, Meta::Byte(b)) if name == "sheep" => self.wool = *b,
                _ => {}
            }
        }
    }
    pub fn sneaking(&self) -> bool { self.flags & 0x02 != 0 || self.pose == 5 }

    pub fn tick(&mut self) {
        self.prev = self.pos; self.prev_head_yaw = self.head_yaw; self.prev_body_yaw = self.body_yaw; self.prev_pitch = self.pitch;
        self.prev_limb_amt = self.limb_amt; self.prev_limb_pos = self.limb_pos;
        if self.steps > 0 {
            let n = self.steps as f64;
            for i in 0..3 { self.pos[i] += (self.target[i] - self.pos[i]) / n; }
            self.yaw += wrap(self.target_yaw - self.yaw) / n as f32;
            self.pitch += (self.target_pitch - self.pitch) / n as f32;
            self.steps -= 1;
        } else { self.yaw = self.target_yaw; self.pitch = self.target_pitch; }
        self.head_yaw += wrap(self.target_head_yaw - self.head_yaw) * 0.5;
        // limb swing from horizontal movement
        let (dx, dz) = (self.pos[0] - self.prev[0], self.pos[2] - self.prev[2]);
        let moved = ((dx * dx + dz * dz).sqrt() as f32 * 4.0).min(1.0);
        self.limb_amt += (moved - self.limb_amt) * 0.4;
        self.limb_pos += self.limb_amt;
        // body yaw: follows movement, stays within 50° of the head
        if type_name(self.kind) != "player" {
            // mobs: the server's yaw is the body rotation, the head turns separately
            self.body_yaw = self.yaw;
        } else {
        let head = self.yaw;
        let mut body = self.body_yaw;
        if dx * dx + dz * dz > 0.0025 * 0.0025 {
            let dir = (-dx as f32).atan2(dz as f32).to_degrees();
            body += wrap(dir - body) * 0.3;
            if wrap(head - body).abs() > 95.0 { body += wrap(head - body) * 0.3; }
        } else if !is_living(self.kind) { body = self.yaw; }
        else if type_name(self.kind) != "player" { body += wrap(self.yaw - body) * 0.3; }
        let d = wrap(head - body);
        if d > 50.0 { body = head - 50.0; } else if d < -50.0 { body = head + 50.0; }
        self.body_yaw = body;
        }
        if self.hurt > 0 { self.hurt -= 1; }
        if self.death > 0 && self.death < 20 { self.death += 1; }
        if self.swing >= 0 { self.swing += 1; if self.swing >= 6 { self.swing = -1; } }
        self.age += 1;
    }

    pub fn render_pos(&self, a: f32) -> [f64; 3] { let a = a as f64; [self.prev[0] + (self.pos[0] - self.prev[0]) * a, self.prev[1] + (self.pos[1] - self.prev[1]) * a, self.prev[2] + (self.pos[2] - self.prev[2]) * a] }
}

pub fn wrap(d: f32) -> f32 { let mut d = d % 360.0; if d >= 180.0 { d -= 360.0; } if d < -180.0 { d += 360.0; } d }
fn lerp_angle(a: f32, b: f32, t: f32) -> f32 { a + wrap(b - a) * t }

// ---------------------------------------------------------------- texture atlas

pub struct EntityAtlas { pub w: u32, pub h: u32, pub rgba: Vec<u8>, rects: HashMap<String, [u32; 4]>, legacy: std::collections::HashSet<String> }

pub const SKINS: [&str; 9] = ["alex", "ari", "efe", "kai", "makena", "noor", "steve", "sunny", "zuri"];

impl EntityAtlas {
    pub fn build(a: &Assets) -> EntityAtlas {
        let mut names: Vec<String> = vec!["zombie/zombie", "zombie/husk", "zombie/drowned", "skeleton/skeleton", "skeleton/stray", "creeper/creeper", "spider/spider", "spider/cave_spider",
            "cow/cow_temperate", "cow/mooshroom_red", "pig/pig_temperate", "sheep/sheep", "sheep/sheep_wool", "chicken/chicken_temperate", "projectiles/arrow"].into_iter().map(String::from).collect();
        for s in SKINS { names.push(format!("player/wide/{s}")); names.push(format!("player/slim/{s}")); }
        let (w, h) = (1024u32, 512u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        let mut rects = HashMap::new();
        let mut legacy = std::collections::HashSet::new();
        // white texel block for untextured boxes
        for y in 0..4 { for x in 0..4 { let d = ((y * w + x) * 4) as usize; rgba[d..d + 4].copy_from_slice(&[255; 4]); } }
        rects.insert("white".to_string(), [0, 0, 4, 4]);
        let (mut x, mut y, mut row) = (8u32, 0u32, 4u32);
        for n in names {
            let Some(img): Option<&Image> = a.entity_tex.get(&n) else { continue };
            if x + img.w > w { x = 0; y += row; row = 0; }
            if y + img.h > h { break; }
            for py in 0..img.h { for px in 0..img.w {
                let s = ((py * img.w + px) * 4) as usize; let d = (((y + py) * w + x + px) * 4) as usize;
                rgba[d..d + 4].copy_from_slice(&img.rgba[s..s + 4]);
            } }
            // 64×32 layout, or a 64×64 texture whose lower half is empty: left limbs mirror the right ones
            if img.h * 2 <= img.w || (img.h == img.w && img.rgba[(img.w * img.h * 2) as usize..].chunks(4).all(|p| p[3] == 0)) { legacy.insert(n.clone()); }
            rects.insert(n, [x, y, img.w, img.h]);
            x += img.w; row = row.max(img.h);
        }
        EntityAtlas { w, h, rgba, rects, legacy }
    }
    pub fn has(&self, n: &str) -> bool { self.rects.contains_key(n) }
}

// ---------------------------------------------------------------- model building

#[derive(Clone, Copy)]
struct Cube { from: [f32; 3], size: [f32; 3], uv: [f32; 2], inflate: f32, mirror: bool }
fn cube(from: [f32; 3], size: [f32; 3], uv: [f32; 2]) -> Cube { Cube { from, size, uv, inflate: 0.0, mirror: false } }
fn cube_i(from: [f32; 3], size: [f32; 3], uv: [f32; 2], inflate: f32) -> Cube { Cube { from, size, uv, inflate, mirror: false } }
fn cube_m(from: [f32; 3], size: [f32; 3], uv: [f32; 2]) -> Cube { Cube { from, size, uv, inflate: 0.0, mirror: true } }

struct Emitter<'a> { out: &'a mut MeshData, base: Mat4, tex: [u32; 4], aw: f32, ah: f32, color: [u8; 4], light: [u8; 2] }

impl Emitter<'_> {
    /// Emit cubes of a part with pivot + rotation (radians, applied Z·Y·X like Java models).
    fn part(&mut self, pivot: [f32; 3], rot: [f32; 3], cubes: &[Cube]) {
        let m = self.base * Mat4::from_translation(Vec3::from(pivot)) * Mat4::from_rotation_z(rot[2]) * Mat4::from_rotation_y(rot[1]) * Mat4::from_rotation_x(rot[0]);
        for c in cubes { self.cube(&m, c); }
    }
    fn cube(&mut self, m: &Mat4, c: &Cube) {
        let (w, h, d) = (c.size[0], c.size[1], c.size[2]);
        let (u, v) = (c.uv[0], c.uv[1]);
        let (x0, y0, z0) = (c.from[0] - c.inflate, c.from[1] - c.inflate, c.from[2] - c.inflate);
        let (x1, y1, z1) = (c.from[0] + w + c.inflate, c.from[1] + h + c.inflate, c.from[2] + d + c.inflate);
        // (corners TL, BL, BR, TR as seen from outside, texture rect u0 v0 u1 v1)
        let (mut west, mut east) = ([u, v + d, u + d, v + d + h], [u + d + w, v + d, u + 2.0 * d + w, v + d + h]);
        if c.mirror { std::mem::swap(&mut west, &mut east); }
        let faces: [([[f32; 3]; 4], [f32; 4]); 6] = [
            ([[x0, y0, z0], [x0, y1, z0], [x1, y1, z0], [x1, y0, z0]], [u + d, v + d, u + d + w, v + d + h]),               // front (-Z)
            ([[x1, y0, z1], [x1, y1, z1], [x0, y1, z1], [x0, y0, z1]], [u + 2.0 * d + w, v + d, u + 2.0 * d + 2.0 * w, v + d + h]), // back (+Z)
            ([[x0, y0, z1], [x0, y1, z1], [x0, y1, z0], [x0, y0, z0]], west),                                                 // -X (right side)
            ([[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]], east),                                                 // +X (left side)
            ([[x0, y0, z1], [x0, y0, z0], [x1, y0, z0], [x1, y0, z1]], [u + d, v, u + d + w, v + d]),                         // top (-Y)
            ([[x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0]], [u + d + w, v, u + d + 2.0 * w, v + d]),               // bottom (+Y)
        ];
        for (fi, (p, r)) in faces.iter().enumerate() {
            let (mut u0, mut u1) = (r[0], r[2]);
            if c.mirror { std::mem::swap(&mut u0, &mut u1); }
            let uvs = [[u0, r[1]], [u0, r[3]], [u1, r[3]], [u1, r[1]]];
            let shade = [0.8, 0.8, 0.6, 0.6, 1.0, 0.5][fi];
            let col = [(self.color[0] as f32 * shade) as u8, (self.color[1] as f32 * shade) as u8, (self.color[2] as f32 * shade) as u8, self.color[3]];
            let b = self.out.verts.len() as u32;
            for i in 0..4 {
                let wp = *m * Vec4::new(p[i][0], p[i][1], p[i][2], 1.0);
                let uv = [(self.tex[0] as f32 + uvs[i][0]) / self.aw, (self.tex[1] as f32 + uvs[i][1]) / self.ah];
                self.out.verts.push(Vertex { pos: [wp.x, wp.y, wp.z], uv, color: col, light: [self.light[0], self.light[1], 0, 0] });
            }
            self.out.idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
        }
    }
    /// A flat quad in part space (for arrows): corners in model px, uv in texture px.
    fn quad(&mut self, m: &Mat4, p: [[f32; 3]; 4], uv: [f32; 4]) {
        let uvs = [[uv[0], uv[1]], [uv[0], uv[3]], [uv[2], uv[3]], [uv[2], uv[1]]];
        let b = self.out.verts.len() as u32;
        for i in 0..4 {
            let wp = *m * Vec4::new(p[i][0], p[i][1], p[i][2], 1.0);
            self.out.verts.push(Vertex { pos: [wp.x, wp.y, wp.z], uv: [(self.tex[0] as f32 + uvs[i][0]) / self.aw, (self.tex[1] as f32 + uvs[i][1]) / self.ah], color: self.color, light: [self.light[0], self.light[1], 0, 0] });
        }
        self.out.idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
    }
}

/// Java's UUID.hashCode, used to pick a default skin.
fn uuid_hash(u: &Uuid) -> i32 {
    let (hi, lo) = u.as_u64_pair();
    let x = hi ^ lo;
    ((x >> 32) as i32) ^ (x as i32)
}
/// Default skin for a player without a profile skin: (texture name, slim arms).
pub fn default_skin(u: &Uuid) -> (String, bool) {
    let i = uuid_hash(u).rem_euclid(18) as usize;
    let slim = i < 9;
    (format!("player/{}/{}", if slim { "slim" } else { "wide" }, SKINS[i % 9]), slim)
}

const WOOL: [u32; 16] = [0xF9FFFE, 0xF9801D, 0xC74EBD, 0x3AB3DA, 0xFED83D, 0x80C71F, 0xF38BAA, 0x474F52, 0x9D9D97, 0x169C9C, 0x8932B8, 0x3C44AA, 0x835432, 0x5E7C16, 0xB02E26, 0x1D1D21];

pub struct Pose { pub limb: f32, pub amt: f32, pub head_yaw: f32, pub pitch: f32, pub age: f32, pub swing: f32, pub sneak: bool }

/// Append the model of `e` to `out` (entity atlas UVs). `light` = (sky, block) 0..255.
/// Returns false if there is no model for this type (caller draws a box).
pub fn build_model(atlas: &EntityAtlas, e: &Entity, alpha: f32, light: [u8; 2], out: &mut MeshData) -> bool {
    let name = type_name(e.kind);
    let p = e.render_pos(alpha);
    let body = lerp_angle(e.prev_body_yaw, e.body_yaw, alpha);
    let head = if name == "player" { lerp_angle(e.prev_body_yaw, e.yaw, 1.0) } else { lerp_angle(e.prev_head_yaw, e.head_yaw, alpha) };
    let pitch = e.prev_pitch + (e.pitch - e.prev_pitch) * alpha;
    let pose = Pose {
        limb: e.prev_limb_pos + (e.limb_pos - e.prev_limb_pos) * alpha, amt: (e.prev_limb_amt + (e.limb_amt - e.prev_limb_amt) * alpha).min(1.0),
        head_yaw: wrap(head - body).clamp(-75.0, 75.0).to_radians(), pitch: pitch.to_radians(), age: e.age as f32 + alpha,
        swing: if e.swing >= 0 { (e.swing as f32 + alpha) / 6.0 } else { 0.0 }, sneak: e.sneaking(),
    };
    let tint = if e.hurt > 0 || e.death > 0 { [255, 110, 110, 255] } else { [255; 4] };
    let scale = if e.baby { 0.5 } else { 1.0 };
    // world ← model: translate, body yaw, death roll, scale to blocks, model space (y down, -Z front, origin 24px up)
    let mut base = Mat4::from_translation(Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32)) * Mat4::from_rotation_y(-body.to_radians());
    if e.death > 0 {
        let f = (((e.death as f32 + alpha - 1.0) / 20.0 * 1.6).sqrt()).min(1.0);
        base *= Mat4::from_rotation_z(f * PI / 2.0);
    }
    base *= Mat4::from_scale(Vec3::splat(scale / 16.0)) * Mat4::from_translation(Vec3::new(0.0, 24.0, 0.0)) * Mat4::from_scale(Vec3::new(1.0, -1.0, -1.0));
    let tex_for = |n: &str| atlas.rects.get(n).copied();
    macro_rules! em { ($tex:expr, $color:expr, $out:expr) => { Emitter { out: $out, base, tex: $tex, aw: atlas.w as f32, ah: atlas.h as f32, color: $color, light } } }
    let swing_l = |a: f32| (a * 0.6662).cos();
    let (limb, amt) = (pose.limb, pose.amt);
    match name {
        "player" | "zombie" | "husk" | "drowned" | "skeleton" | "stray" => {
            let (texn, slim) = match name {
                "player" => default_skin(&e.uuid),
                "zombie" => ("zombie/zombie".into(), false), "husk" => ("zombie/husk".into(), false), "drowned" => ("zombie/drowned".into(), false),
                "stray" => ("skeleton/stray".into(), false), _ => ("skeleton/skeleton".into(), false),
            };
            let Some(t) = tex_for(&texn).or(tex_for("zombie/zombie")) else { return false };
            let mut m = em!(t, tint, out);
            let thin = name == "skeleton" || name == "stray";
            let old = atlas.legacy.contains(&texn);
            let sn = pose.sneak;
            let (hy, by, ay, ly, lz) = if sn { (4.2, 3.2, 5.2, 12.2, 4.0) } else { (0.0, 0.0, 2.0, 12.0, 0.1) };
            m.part([0.0, hy, 0.0], [pose.pitch, pose.head_yaw, 0.0], &[cube([-4.0, -8.0, -4.0], [8.0, 8.0, 8.0], [0.0, 0.0]), cube_i([-4.0, -8.0, -4.0], [8.0, 8.0, 8.0], [32.0, 0.0], 0.5)]);
            let body_rot = if sn { 0.5 } else { 0.0 };
            let mut body_c = vec![cube([-4.0, 0.0, -2.0], [8.0, 12.0, 4.0], [16.0, 16.0])];
            if !old && !thin { body_c.push(cube_i([-4.0, 0.0, -2.0], [8.0, 12.0, 4.0], [16.0, 32.0], 0.25)); }
            m.part([0.0, by, 0.0], [body_rot, 0.0, 0.0], &body_c);
            // arms
            let zombie_arms = matches!(name, "zombie" | "husk" | "drowned");
            let aiming = thin && e.mob_flags & 4 != 0;
            let mut ra = swing_l(limb + PI) * 2.0 * amt * 0.5;
            let mut la = swing_l(limb) * 2.0 * amt * 0.5;
            if zombie_arms || aiming { ra = -PI / 2.0 + (pose.age * 0.067).sin() * 0.05; la = ra; }
            if sn { ra += 0.4; la += 0.4; }
            if pose.swing > 0.0 { ra -= (pose.swing * PI).sin() * 1.2; }
            let aw = if slim { 3.0 } else if thin { 2.0 } else { 4.0 };
            let (rx, lx) = if thin { (-1.0, -1.0) } else { (-aw + 1.0, -1.0) };
            let arm_d = if thin { 2.0 } else { 4.0 };
            let ra_c = if thin { vec![cube([rx, -2.0, -1.0], [2.0, 12.0, 2.0], [40.0, 16.0])] } else {
                vec![cube([rx, -2.0, -2.0], [aw, 12.0, arm_d], [40.0, 16.0]), cube_i([rx, -2.0, -2.0], [aw, 12.0, arm_d], [40.0, 32.0], 0.25)] };
            let la_c = if thin || old { vec![cube_m([lx, -2.0, if thin { -1.0 } else { -2.0 }], [aw, 12.0, arm_d], [40.0, 16.0])] } else {
                vec![cube([lx, -2.0, -2.0], [aw, 12.0, arm_d], [32.0, 48.0]), cube_i([lx, -2.0, -2.0], [aw, 12.0, arm_d], [48.0, 48.0], 0.25)] };
            let ra_c: Vec<Cube> = if old { ra_c.into_iter().take(1).collect() } else { ra_c };
            m.part([-5.0, ay, 0.0], [ra, 0.0, 0.0], &ra_c);
            m.part([5.0, ay, 0.0], [la, 0.0, 0.0], &la_c);
            // legs
            let (rl, ll) = (swing_l(limb) * 1.4 * amt, swing_l(limb + PI) * 1.4 * amt);
            let lw = if thin { 2.0 } else { 4.0 };
            let lo = -lw / 2.0;
            let rl_c = if thin || old { vec![cube([lo, 0.0, lo], [lw, 12.0, lw], [0.0, 16.0])] } else { vec![cube([-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], [0.0, 16.0]), cube_i([-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], [0.0, 32.0], 0.25)] };
            let ll_c = if thin || old { vec![cube_m([lo, 0.0, lo], [lw, 12.0, lw], [0.0, 16.0])] } else { vec![cube([-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], [16.0, 48.0]), cube_i([-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], [0.0, 48.0], 0.25)] };
            m.part([-1.9, ly, lz], [rl, 0.0, 0.0], &rl_c);
            m.part([1.9, ly, lz], [ll, 0.0, 0.0], &ll_c);
        }
        "creeper" => {
            let Some(t) = tex_for("creeper/creeper") else { return false };
            let mut m = em!(t, tint, out);
            m.part([0.0, 6.0, 0.0], [pose.pitch, pose.head_yaw, 0.0], &[cube([-4.0, -8.0, -4.0], [8.0, 8.0, 8.0], [0.0, 0.0])]);
            m.part([0.0, 6.0, 0.0], [0.0; 3], &[cube([-4.0, 0.0, -2.0], [8.0, 12.0, 4.0], [16.0, 16.0])]);
            let (a, b) = (swing_l(limb) * 1.4 * amt, swing_l(limb + PI) * 1.4 * amt);
            for (x, z, r) in [(-2.0, 4.0, a), (2.0, 4.0, b), (-2.0, -4.0, b), (2.0, -4.0, a)] { m.part([x, 18.0, z], [r, 0.0, 0.0], &[cube([-2.0, 0.0, -2.0], [4.0, 6.0, 4.0], [0.0, 16.0])]); }
        }
        "spider" | "cave_spider" => {
            let Some(t) = tex_for(if name == "spider" { "spider/spider" } else { "spider/cave_spider" }) else { return false };
            if name == "cave_spider" { base *= Mat4::from_scale(Vec3::splat(0.7)); }
            let mut m = Emitter { out, base, tex: t, aw: atlas.w as f32, ah: atlas.h as f32, color: tint, light };
            m.part([0.0, 15.0, -3.0], [pose.pitch, pose.head_yaw, 0.0], &[cube([-4.0, -4.0, -8.0], [8.0, 8.0, 8.0], [32.0, 4.0])]);
            m.part([0.0, 15.0, 0.0], [0.0; 3], &[cube([-3.0, -3.0, -3.0], [6.0, 6.0, 6.0], [0.0, 0.0])]);
            m.part([0.0, 15.0, 9.0], [0.0; 3], &[cube([-5.0, -4.0, -6.0], [10.0, 8.0, 12.0], [0.0, 12.0])]);
            let q = PI / 4.0;
            for (i, (z, ya)) in [(2.0, q), (1.0, q / 2.0), (0.0, -q / 2.0), (-1.0, -q)].iter().enumerate() {
                let ph = i as f32 * PI / 2.0;
                let sy = -((limb * 0.6662 * 2.0 + ph).cos() * 0.4) * amt;
                let sz = ((limb * 0.6662 + ph).sin() * 0.4).abs() * amt;
                let zr = if i == 0 || i == 3 { q } else { q * 0.74 };
                m.part([-4.0, 15.0, *z], [0.0, *ya + sy, -zr + sz], &[cube([-15.0, -1.0, -1.0], [16.0, 2.0, 2.0], [18.0, 0.0])]);
                m.part([4.0, 15.0, *z], [0.0, -*ya - sy, zr - sz], &[cube([-1.0, -1.0, -1.0], [16.0, 2.0, 2.0], [18.0, 0.0])]);
            }
        }
        "cow" | "mooshroom" => {
            let Some(t) = tex_for(if name == "cow" { "cow/cow_temperate" } else { "cow/mooshroom_red" }) else { return false };
            let mut m = em!(t, tint, out);
            m.part([0.0, 4.0, -8.0], [pose.pitch, pose.head_yaw, 0.0], &[cube([-4.0, -4.0, -6.0], [8.0, 8.0, 6.0], [0.0, 0.0]), cube([-5.0, -5.0, -4.0], [1.0, 3.0, 1.0], [22.0, 0.0]), cube_m([4.0, -5.0, -4.0], [1.0, 3.0, 1.0], [22.0, 0.0])]);
            m.part([0.0, 5.0, 2.0], [PI / 2.0, 0.0, 0.0], &[cube([-6.0, -10.0, -7.0], [12.0, 18.0, 10.0], [18.0, 4.0]), cube([-2.0, 2.0, -8.0], [4.0, 6.0, 1.0], [52.0, 0.0])]);
            quad_legs(&mut m, limb, amt, 12.0, [(-4.0, 7.0), (4.0, 7.0), (-4.0, -6.0), (4.0, -6.0)], [4.0, 12.0, 4.0]);
        }
        "pig" => {
            let Some(t) = tex_for("pig/pig_temperate") else { return false };
            let mut m = em!(t, tint, out);
            m.part([0.0, 12.0, -6.0], [pose.pitch, pose.head_yaw, 0.0], &[cube([-4.0, -4.0, -8.0], [8.0, 8.0, 8.0], [0.0, 0.0]), cube([-2.0, 0.0, -9.0], [4.0, 3.0, 1.0], [16.0, 16.0])]);
            m.part([0.0, 11.0, 2.0], [PI / 2.0, 0.0, 0.0], &[cube([-5.0, -10.0, -7.0], [10.0, 16.0, 8.0], [28.0, 8.0])]);
            quad_legs(&mut m, limb, amt, 18.0, [(-3.0, 7.0), (3.0, 7.0), (-3.0, -5.0), (3.0, -5.0)], [4.0, 6.0, 4.0]);
        }
        "sheep" => {
            let Some(t) = tex_for("sheep/sheep") else { return false };
            let mut m = em!(t, tint, out);
            let head = |m: &mut Emitter, c: &[Cube]| m.part([0.0, 6.0, -8.0], [pose.pitch, pose.head_yaw, 0.0], c);
            head(&mut m, &[cube([-3.0, -4.0, -6.0], [6.0, 6.0, 8.0], [0.0, 0.0])]);
            m.part([0.0, 5.0, 2.0], [PI / 2.0, 0.0, 0.0], &[cube([-4.0, -10.0, -7.0], [8.0, 16.0, 6.0], [28.0, 8.0])]);
            quad_legs(&mut m, limb, amt, 12.0, [(-3.0, 7.0), (3.0, 7.0), (-3.0, -5.0), (3.0, -5.0)], [4.0, 12.0, 4.0]);
            if e.wool & 0x10 == 0 {
                if let Some(wt) = tex_for("sheep/sheep_wool") {
                    let c = WOOL[(e.wool & 15) as usize];
                    let col = [((c >> 16 & 255) * tint[0] as u32 / 255) as u8, ((c >> 8 & 255) * tint[1] as u32 / 255) as u8, ((c & 255) * tint[2] as u32 / 255) as u8, 255];
                    let mut w = em!(wt, col, out);
                    w.part([0.0, 6.0, -8.0], [pose.pitch, pose.head_yaw, 0.0], &[cube_i([-3.0, -4.0, -4.0], [6.0, 6.0, 6.0], [0.0, 0.0], 0.6)]);
                    w.part([0.0, 5.0, 2.0], [PI / 2.0, 0.0, 0.0], &[cube_i([-4.0, -10.0, -7.0], [8.0, 16.0, 6.0], [28.0, 8.0], 1.75)]);
                    let (a, b) = (swing_l(limb) * 1.4 * amt, swing_l(limb + PI) * 1.4 * amt);
                    for (x, z, r) in [(-3.0, 7.0, a), (3.0, 7.0, b), (-3.0, -5.0, b), (3.0, -5.0, a)] { w.part([x, 12.0, z], [r, 0.0, 0.0], &[cube_i([-2.0, 0.0, -2.0], [4.0, 6.0, 4.0], [0.0, 16.0], 0.5)]); }
                }
            }
        }
        "chicken" => {
            let Some(t) = tex_for("chicken/chicken_temperate") else { return false };
            let mut m = em!(t, tint, out);
            let hr = [pose.pitch, pose.head_yaw, 0.0];
            m.part([0.0, 15.0, -4.0], hr, &[cube([-2.0, -6.0, -2.0], [4.0, 6.0, 3.0], [0.0, 0.0]), cube([-2.0, -4.0, -4.0], [4.0, 2.0, 2.0], [14.0, 0.0]), cube([-1.0, -2.0, -3.0], [2.0, 2.0, 2.0], [14.0, 4.0])]);
            m.part([0.0, 16.0, 0.0], [PI / 2.0, 0.0, 0.0], &[cube([-3.0, -4.0, -3.0], [6.0, 8.0, 6.0], [0.0, 9.0])]);
            let (a, b) = (swing_l(limb) * 1.4 * amt, swing_l(limb + PI) * 1.4 * amt);
            m.part([-2.0, 19.0, 1.0], [a, 0.0, 0.0], &[cube([-1.0, 0.0, -3.0], [3.0, 5.0, 3.0], [26.0, 0.0])]);
            m.part([1.0, 19.0, 1.0], [b, 0.0, 0.0], &[cube([-1.0, 0.0, -3.0], [3.0, 5.0, 3.0], [26.0, 0.0])]);
            let flap = if e.on_ground || (e.pos[1] - e.prev[1]).abs() < 0.01 { 0.0 } else { (pose.age * 1.5).sin().abs() * 1.2 };
            m.part([-4.0, 13.0, 0.0], [0.0, 0.0, flap], &[cube([0.0, 0.0, -3.0], [1.0, 4.0, 6.0], [24.0, 13.0])]);
            m.part([4.0, 13.0, 0.0], [0.0, 0.0, -flap], &[cube([-1.0, 0.0, -3.0], [1.0, 4.0, 6.0], [24.0, 13.0])]);
        }
        "arrow" | "spectral_arrow" => {
            let Some(t) = tex_for("projectiles/arrow") else { return false };
            // arrows face their flight direction: yaw/pitch, model along -Z
            let base = Mat4::from_translation(Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32)) * Mat4::from_rotation_y(-e.yaw.to_radians()) * Mat4::from_rotation_x(e.pitch.to_radians()) * Mat4::from_scale(Vec3::splat(0.05 / 1.0));
            let mut m = Emitter { out, base, tex: t, aw: atlas.w as f32, ah: atlas.h as f32, color: tint, light };
            let id = Mat4::IDENTITY;
            for r in 0..2 {
                let rm = id * Mat4::from_rotation_z(r as f32 * PI / 2.0);
                m.quad(&rm, [[0.0, 2.5, -8.0], [0.0, -2.5, -8.0], [0.0, -2.5, 8.0], [0.0, 2.5, 8.0]], [0.0, 0.0, 16.0, 5.0]);
            }
            m.quad(&id, [[-2.5, 2.5, -7.0], [-2.5, -2.5, -7.0], [2.5, -2.5, -7.0], [2.5, 2.5, -7.0]], [0.0, 5.0, 5.0, 10.0]);
        }
        _ => return false,
    }
    true
}

fn quad_legs(m: &mut Emitter, limb: f32, amt: f32, y: f32, at: [(f32, f32); 4], size: [f32; 3]) {
    let (a, b) = ((limb * 0.6662).cos() * 1.4 * amt, (limb * 0.6662 + PI).cos() * 1.4 * amt);
    for (i, (x, z)) in at.iter().enumerate() {
        let r = if i == 0 || i == 3 { a } else { b };
        let c = if i % 2 == 1 { cube_m([-2.0, 0.0, -2.0], size, [0.0, 16.0]) } else { cube([-2.0, 0.0, -2.0], size, [0.0, 16.0]) };
        m.part([*x, y, *z], [r, 0.0, 0.0], &[c]);
    }
}

/// Dropped item: bobbing, spinning mini block or flat sprite, extra copies for bigger stacks.
/// Writes block-atlas geometry to `out`.
pub fn build_item(a: &Assets, e: &Entity, alpha: f32, light: [u8; 2], out: &mut MeshData) {
    let Some(it) = &e.item else { return };
    if it.count <= 0 { return; }
    let p = e.render_pos(alpha);
    let t = e.age as f32 + alpha;
    let bob = ((t / 10.0 + e.bob).sin() * 0.1 + 0.1) as f32;
    let spin = t / 20.0 + e.bob;
    let copies = match it.count { 1 => 1, 2..=16 => 2, 17..=32 => 3, 33..=48 => 4, _ => 5 };
    let tint = a.item_tint.get(it.item as usize).copied().flatten();
    let tc = tint.map(|c| [(c >> 16 & 255) as f32 / 255.0, (c >> 8 & 255) as f32 / 255.0, (c & 255) as f32 / 255.0]).unwrap_or([1.0; 3]);
    for k in 0..copies {
        // deterministic small offsets per copy
        let h = |s: u32| (((e.id as u32).wrapping_mul(747796405).wrapping_add(k * 2891336453 + s)) >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
        if let Some(m) = a.item_model(it.item) {
            let off = if k == 0 { Vec3::ZERO } else { Vec3::new(h(1), h(2), h(3)) * 0.15 };
            let tr = Mat4::from_translation(Vec3::new(p[0] as f32, p[1] as f32 + bob + 0.125, p[2] as f32) + off) * Mat4::from_rotation_y(spin) * Mat4::from_scale(Vec3::splat(0.25)) * Mat4::from_translation(Vec3::splat(-0.5));
            for q in &m.quads {
                let b = out.verts.len() as u32;
                let sh = [0.5, 1.0, 0.8, 0.8, 0.6, 0.6][q.dir as usize];
                let tt = if q.tinted { tc } else { [1.0; 3] };
                for i in 0..4 {
                    let wp = tr * Vec4::new(q.pos[i][0], q.pos[i][1], q.pos[i][2], 1.0);
                    out.verts.push(Vertex { pos: [wp.x, wp.y, wp.z], uv: q.uv[i], color: [(tt[0] * sh * 255.0) as u8, (tt[1] * sh * 255.0) as u8, (tt[2] * sh * 255.0) as u8, 255], light: [light[0], light[1], 0, 0] });
                }
                out.idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
            }
        } else if let Some(uv) = a.icon_uv(it.item) {
            let off = if k == 0 { Vec3::ZERO } else { Vec3::new(h(1) * 0.3, h(2) * 0.3, -(k as f32) * 0.06) };
            let tr = Mat4::from_translation(Vec3::new(p[0] as f32, p[1] as f32 + bob, p[2] as f32)) * Mat4::from_rotation_y(spin) * Mat4::from_translation(off);
            let s = 0.25;
            let pts = [[-s, 2.0 * s, 0.0], [-s, 0.0, 0.0], [s, 0.0, 0.0], [s, 2.0 * s, 0.0]];
            let uvs = [[uv[0], uv[1]], [uv[0], uv[3]], [uv[2], uv[3]], [uv[2], uv[1]]];
            let b = out.verts.len() as u32;
            for i in 0..4 {
                let wp = tr * Vec4::new(pts[i][0], pts[i][1], pts[i][2], 1.0);
                out.verts.push(Vertex { pos: [wp.x, wp.y, wp.z], uv: uvs[i], color: [(tc[0] * 255.0) as u8, (tc[1] * 255.0) as u8, (tc[2] * 255.0) as u8, 255], light: [light[0], light[1], 0, 0] });
            }
            out.idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
        }
    }
}

/// First-person right arm (skin texture) at the camera, with the swing animation.
pub fn first_person_arm(atlas: &EntityAtlas, uuid: &Uuid, eye: Vec3, yaw: f32, pitch: f32, swing: f32, bob: f32, light: [u8; 2], out: &mut MeshData) {
    let (texn, slim) = default_skin(uuid);
    let Some(t) = atlas.rects.get(&texn).copied() else { return };
    let s = (swing * PI).sin();
    let s2 = (swing.sqrt() * PI).sin();
    let base = Mat4::from_translation(eye) * Mat4::from_rotation_y(-yaw.to_radians()) * Mat4::from_rotation_x(pitch.to_radians())
        // view space (x right = -X world-local since we face +Z): place the arm low on the right
        * Mat4::from_translation(Vec3::new(-0.56 - s2 * 0.1, -0.62 + bob + s * 0.12, 0.62 - s * 0.15))
        * Mat4::from_rotation_y(-0.6 + s2 * 0.5) * Mat4::from_rotation_x(-1.3 - s * 0.9) * Mat4::from_rotation_z(0.2)
        * Mat4::from_scale(Vec3::splat(1.0 / 16.0)) * Mat4::from_scale(Vec3::new(1.0, -1.0, -1.0));
    let mut m = Emitter { out, base, tex: t, aw: atlas.w as f32, ah: atlas.h as f32, color: [255; 4], light };
    let w = if slim { 3.0 } else { 4.0 };
    m.part([0.0; 3], [0.0; 3], &[cube([-w / 2.0, -2.0, -2.0], [w, 12.0, 4.0], [40.0, 16.0]), cube_i([-w / 2.0, -2.0, -2.0], [w, 12.0, 4.0], [40.0, 32.0], 0.25)]);
}

#[cfg(test)]
mod tests {
    #[test]
    fn wraps() {
        assert_eq!(super::wrap(190.0), -170.0);
        assert_eq!(super::wrap(-190.0), 170.0);
        assert_eq!(super::wrap(30.0), 30.0);
    }
}

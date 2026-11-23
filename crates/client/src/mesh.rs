//! Chunk section meshing: turns an 18³ snapshot of blocks + light into vertex buffers.
use crate::assets::*;
use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub color: [u8; 4],
    /// sky light, block light (0..255), unused, unused
    pub light: [u8; 4],
}

pub struct Snapshot {
    pub origin: [i32; 3],
    pub blocks: Box<[u16; 5832]>,
    /// sky << 4 | block
    pub light: Box<[u8; 5832]>,
    pub biomes: [u8; 64],
}

#[inline] pub fn sidx(x: i32, y: i32, z: i32) -> usize { ((x + 1) + (z + 1) * 18 + (y + 1) * 324) as usize }

#[derive(Default)]
pub struct MeshData { pub verts: Vec<Vertex>, pub idx: Vec<u32> }

#[derive(Default)]
pub struct SectionMesh { pub layers: [MeshData; 3] }

const FACE_SHADE: [f32; 6] = [0.5, 1.0, 0.8, 0.8, 0.6, 0.6];

fn rgb(c: u32) -> [f32; 3] { [(c >> 16 & 255) as f32 / 255.0, (c >> 8 & 255) as f32 / 255.0, (c & 255) as f32 / 255.0] }

fn fluid_height(a: &Assets, s: &Snapshot, x: i32, y: i32, z: i32, kind: u8) -> f32 {
    let i = a.info(s.blocks[sidx(x, y, z)]);
    if i.fluid != kind { return -1.0; }
    if a.info(s.blocks[sidx(x, (y + 1).min(16), z)]).fluid == kind && y < 16 { return 1.0; }
    if i.fluid_level >= 8 { 1.0 } else { (8 - i.fluid_level) as f32 / 9.0 }
}

/// Per-quad metadata recorded by `mesh_block` when tracing (used by `--mesh-dump`).
#[derive(Clone, Copy)]
pub struct QuadTrace {
    /// 0 = block model, 1 = fluid
    pub src: u8,
    pub layer: u8,
    pub dir: u8,
    /// cull face (NO_CULL = none)
    pub cull: u8,
    pub tinted: bool,
    pub tint: [f32; 3],
    pub shade: bool,
    pub verts: [Vertex; 4],
}

pub fn mesh_section(a: &Assets, biomes: &[Biome], s: &Snapshot) -> SectionMesh {
    let mut out = SectionMesh::default();
    for y in 0..16i32 { for z in 0..16i32 { for x in 0..16i32 {
        mesh_block(a, biomes, s, x, y, z, &mut out, None);
    } } }
    out
}

/// Mesh the block at section-local (x, y, z) into `out`; with `trace`, also record every quad.
pub fn mesh_block(a: &Assets, biomes: &[Biome], s: &Snapshot, x: i32, y: i32, z: i32, out: &mut SectionMesh, mut trace: Option<&mut Vec<QuadTrace>>) {
    let ox = s.origin[0] as f32; let oy = s.origin[1] as f32; let oz = s.origin[2] as f32;
    {
        let state = s.blocks[sidx(x, y, z)];
        if state == 0 { return; }
        let info = a.info(state);
        let biome = biomes.get(s.biomes[((y >> 2) << 4 | (z >> 2) << 2 | (x >> 2)) as usize] as usize);
        let tint = match info.tint {
            Tint::None => [1.0; 3], Tint::Grass => rgb(biome.map(|b| b.grass).unwrap_or(0x79C05A)), Tint::Foliage => rgb(biome.map(|b| b.foliage).unwrap_or(0x59AE30)),
            Tint::Water => rgb(biome.map(|b| b.water).unwrap_or(0x3F76E4)), Tint::Fixed(c) => rgb(c), Tint::DryFoliage => rgb(0x7B5334),
        };
        let base = [ox + x as f32, oy + y as f32, oz + z as f32];
        let model = &a.models[info.model as usize];
        let own_light = s.light[sidx(x, y, z)];
        for q in &model.quads {
            if q.cull < 6 {
                let d = DIR_VEC[q.cull as usize];
                let n = s.blocks[sidx(x + d[0], y + d[1], z + d[2])];
                let ni = a.info(n);
                if ni.occludes || (info.self_cull && n != 0 && basalt_data::BlockState(n as u32).block_index() == basalt_data::BlockState(state as u32).block_index()) { continue; }
            }
            let d = DIR_VEC[q.dir as usize];
            let axis = if d[0] != 0 { 0 } else if d[1] != 0 { 1 } else { 2 };
            let shade = if q.shade { FACE_SHADE[q.dir as usize] } else { 1.0 };
            let mut vs = [Vertex::default(); 4]; let mut aos = [0u8; 4];
            // Smooth lighting (verified against the vanilla client with `basalt-check client-mesh`):
            // the face plane is the neighbour layer when the face lies on the block boundary, else
            // the block's own layer. Each corner of that unit square averages 4 cells (centre, the
            // two side cells, the diagonal): AO factor = mean of 0.2 (occluder) / 1.0, sky light =
            // mean with unlit (0) cells taking the centre's sky value, block light = mean where only
            // completely dark cells (sky 0 and block 0) take the centre's value.
            // Vertices interpolate the four corners bilinearly by their position on the face.
            let smooth = q.ao && info.emission == 0;
            let (t1, t2) = match axis { 0 => (1, 2), 1 => (0, 2), _ => (0, 1) };
            let mut corner = [[0f32; 3]; 4]; // [ao factor, sky, block] for (-,-), (+,-), (-,+), (+,+)
            if smooth {
                let plane = q.pos[0][axis];
                let flush = if d[axis] > 0 { plane >= 0.9999 } else { plane <= 0.0001 };
                let f = if flush { [x + d[0], y + d[1], z + d[2]] } else { [x, y, z] };
                let fl = s.light[sidx(f[0], f[1], f[2])];
                for (ci, (c1, c2)) in [(-1, -1), (1, -1), (-1, 1), (1, 1)].into_iter().enumerate() {
                    let cell = |a1: i32, a2: i32| { let mut c = f; c[t1] += a1; c[t2] += a2; sidx(c[0], c[1], c[2]) };
                    let (mut ao, mut sky, mut blk) = (0.0f32, 0.0f32, 0.0f32);
                    for ix in [cell(0, 0), cell(c1, 0), cell(0, c2), cell(c1, c2)] {
                        ao += if a.info(s.blocks[ix]).ao_occluder { 0.2 } else { 1.0 };
                        let l = s.light[ix];
                        sky += if l >> 4 == 0 { (fl >> 4) as f32 } else { (l >> 4) as f32 };
                        blk += if l == 0 { (fl & 15) as f32 } else { (l & 15) as f32 };
                    }
                    corner[ci] = [ao / 4.0, sky / 4.0, blk / 4.0];
                }
            }
            for i in 0..4 {
                let p = q.pos[i];
                let (sky, mut blk, aof);
                if smooth {
                    let u = p[t1].clamp(0.0, 1.0); let v = p[t2].clamp(0.0, 1.0);
                    let w = [(1.0 - u) * (1.0 - v), u * (1.0 - v), (1.0 - u) * v, u * v];
                    let mix = |k: usize| corner.iter().zip(w).map(|(c, w)| c[k] * w).sum::<f32>();
                    aof = mix(0); sky = mix(1); blk = mix(2);
                } else {
                    let l = if q.cull < 6 { s.light[sidx(x + d[0], y + d[1], z + d[2])] } else { own_light };
                    sky = (l >> 4) as f32; blk = (l & 15) as f32; aof = 1.0;
                }
                blk = if info.full_bright { 15.0 } else { blk.max(info.emission as f32) };
                let br = shade * aof;
                let t = if q.tinted { tint } else { [1.0; 3] };
                aos[i] = ((1.0 - aof) * 10.0) as u8;
                vs[i] = Vertex {
                    pos: [base[0] + p[0], base[1] + p[1], base[2] + p[2]], uv: q.uv[i],
                    color: [(t[0] * br * 255.0) as u8, (t[1] * br * 255.0) as u8, (t[2] * br * 255.0) as u8, 255],
                    light: [(sky * 17.0) as u8, (blk * 17.0) as u8, 0, 0],
                };
            }
            if let Some(t) = trace.as_deref_mut() { t.push(QuadTrace { src: 0, layer: q.layer, dir: q.dir, cull: q.cull, tinted: q.tinted, tint: if q.tinted { tint } else { [1.0; 3] }, shade: q.shade, verts: vs }); }
            let m = &mut out.layers[q.layer as usize];
            let b = m.verts.len() as u32;
            m.verts.extend_from_slice(&vs);
            if aos[0] + aos[2] > aos[1] + aos[3] { m.idx.extend_from_slice(&[b + 1, b + 2, b + 3, b + 1, b + 3, b]); } else { m.idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]); }
        }
        if info.fluid != 0 {
            let k = info.fluid;
            let uvr = if k == 1 { a.water_still } else { a.lava_still };
            let col = if k == 1 { rgb(biome.map(|b| b.water).unwrap_or(0x3F76E4)) } else { [1.0; 3] };
            let layer = if k == 1 { LAYER_TRANSLUCENT } else { LAYER_OPAQUE };
            let above_same = a.info(s.blocks[sidx(x, y + 1, z)]).fluid == k;
            // corner heights (nw, sw, se, ne)
            let corner = |cx: i32, cz: i32| -> f32 {
                if above_same { return 1.0; }
                let (mut sum, mut n) = (0.0, 0.0);
                for (dx, dz) in [(0, 0), (cx, 0), (0, cz), (cx, cz)] {
                    let h = fluid_height(a, s, x + dx, y, z + dz, k);
                    if h >= 1.0 { return 1.0; }
                    if h >= 0.0 { sum += h; n += 1.0; }
                }
                if n == 0.0 { 8.0 / 9.0 } else { sum / n }
            };
            let (h_nw, h_sw, h_se, h_ne) = (corner(-1, -1), corner(-1, 1), corner(1, 1), corner(1, -1));
            let l = { let up = s.light[sidx(x, y + 1, z)]; if up == 0 { own_light } else { up.max(own_light & 0xF0) } };
            let light = [if k == 2 { 255 } else { (l >> 4) * 17 }, if k == 2 { 255 } else { (l & 15) * 17 }, 0, 0];
            let mut quad = |p: [[f32; 3]; 4], sh: f32, uv: [[f32; 2]; 4], dir: u8| {
                let m = &mut out.layers[layer as usize]; let b = m.verts.len() as u32;
                let mut vs = [Vertex::default(); 4];
                for i in 0..4 {
                    vs[i] = Vertex { pos: [base[0] + p[i][0], base[1] + p[i][1], base[2] + p[i][2]], uv: [uvr[0] + (uvr[2] - uvr[0]) * uv[i][0], uvr[1] + (uvr[3] - uvr[1]) * uv[i][1]],
                        color: [(col[0] * sh * 255.0) as u8, (col[1] * sh * 255.0) as u8, (col[2] * sh * 255.0) as u8, if k == 1 { 200 } else { 255 }], light };
                }
                m.verts.extend_from_slice(&vs);
                if let Some(t) = trace.as_deref_mut() { t.push(QuadTrace { src: 1, layer, dir, cull: NO_CULL, tinted: k == 1, tint: col, shade: true, verts: vs }); }
                m.idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
            };
            let full_uv = [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]];
            if !above_same { quad([[0.0, h_nw, 0.0], [0.0, h_sw, 1.0], [1.0, h_se, 1.0], [1.0, h_ne, 0.0]], 1.0, full_uv, UP); }
            let nb = |dx: i32, dy: i32, dz: i32| { let ni = a.info(s.blocks[sidx(x + dx, y + dy, z + dz)]); ni.fluid == k || ni.occludes };
            if !nb(0, -1, 0) { quad([[0.0, 0.0, 1.0], [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0]], 0.5, full_uv, DOWN); }
            let side_uv = |h1: f32, h2: f32| [[0.0, 1.0 - h1], [0.0, 1.0], [1.0, 1.0], [1.0, 1.0 - h2]];
            if !nb(0, 0, -1) { quad([[1.0, h_ne, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, h_nw, 0.0]], 0.8, side_uv(h_ne, h_nw), NORTH); }
            if !nb(0, 0, 1) { quad([[0.0, h_sw, 1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [1.0, h_se, 1.0]], 0.8, side_uv(h_sw, h_se), SOUTH); }
            if !nb(-1, 0, 0) { quad([[0.0, h_nw, 0.0], [0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, h_sw, 1.0]], 0.6, side_uv(h_nw, h_sw), WEST); }
            if !nb(1, 0, 0) { quad([[1.0, h_se, 1.0], [1.0, 0.0, 1.0], [1.0, 0.0, 0.0], [1.0, h_ne, 0.0]], 0.6, side_uv(h_se, h_ne), EAST); }
        }
    }
}

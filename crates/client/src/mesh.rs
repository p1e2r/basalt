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

/// Logical height of a fluid cell: sources and falling fluid 8/9, flowing level l (1..7) (8-l)/9.
pub fn fluid_level_height(level: u8) -> f32 { if level >= 8 { 8.0 / 9.0 } else { (8 - level) as f32 / 9.0 } }

/// One cell around a fluid corner, as far as the corner average cares.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CornerCell { /** same fluid, with its level */ Fluid(u8), /** non-solid, no fluid (air, plants) */ Open, /** solid */ Solid }

/// Logical height of a fluid corner (before the 0.001 inset), measured against the vanilla client
/// (basalt-check client-mesh): the average over the cells touching the corner - `cells` = own cell,
/// the two side cells, the diagonal. A fluid cell weighs 10 at height >= 0.8 (sources, falling) and 1
/// below that, an open non-fluid cell weighs 1 at height 0, a solid cell does not count. The diagonal
/// counts only when one of the two side cells holds the fluid. `above_same` = a cell above any of the
/// four holds the same fluid, which makes the corner full (1.0).
pub fn fluid_corner_height(cells: [CornerCell; 4], above_same: bool) -> f32 {
    if above_same { return 1.0; }
    let (mut sum, mut wt) = (0.0f32, 0.0f32);
    let side_fluid = matches!(cells[1], CornerCell::Fluid(_)) || matches!(cells[2], CornerCell::Fluid(_));
    for (n, c) in cells.iter().enumerate() {
        if n == 3 && !side_fluid { continue; }
        match c {
            CornerCell::Fluid(l) => { let h = fluid_level_height(*l); if h >= 0.8 { sum += h * 10.0; wt += 10.0; } else { sum += h; wt += 1.0; } }
            CornerCell::Open => wt += 1.0,
            CornerCell::Solid => {}
        }
    }
    if wt == 0.0 { 0.0 } else { sum / wt }
}

/// Horizontal flow direction (x, z) of the fluid at a cell: every horizontal neighbour pushes away from
/// a higher fluid level; an open neighbour with the fluid below it pulls towards it. (0, 0) = still.
pub fn fluid_flow(a: &Assets, s: &Snapshot, x: i32, y: i32, z: i32, kind: u8, own_level: u8) -> [f32; 2] {
    let own = fluid_level_height(own_level);
    let mut v = [0.0f32; 2];
    for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
        let n = a.info(s.blocks[sidx(x + dx, y, z + dz)]);
        let diff = if n.fluid == kind { own - fluid_level_height(n.fluid_level) } else if !n.collides {
            let b = a.info(s.blocks[sidx(x + dx, y - 1, z + dz)]);
            if b.fluid == kind { own - (fluid_level_height(b.fluid_level) - 8.0 / 9.0) } else { 0.0 }
        } else { 0.0 };
        v[0] += dx as f32 * diff; v[1] += dz as f32 * diff;
    }
    v
}

fn fluid_corner(a: &Assets, s: &Snapshot, x: i32, y: i32, z: i32, kind: u8, cx: i32, cz: i32) -> f32 {
    let mut cells = [CornerCell::Solid; 4];
    let mut above_same = false;
    for (n, (dx, dz)) in [(0, 0), (cx, 0), (0, cz), (cx, cz)].into_iter().enumerate() {
        let i = a.info(s.blocks[sidx(x + dx, y, z + dz)]);
        if a.info(s.blocks[sidx(x + dx, y + 1, z + dz)]).fluid == kind { above_same = true; }
        cells[n] = if i.fluid == kind { CornerCell::Fluid(i.fluid_level) } else if !i.collides { CornerCell::Open } else { CornerCell::Solid };
    }
    fluid_corner_height(cells, above_same)
}

/// Whether a neighbour/own 16x16 face mask covers a fluid side face (the whole width, from the floor up to `h`).
fn mask_covers_side(m: &[u16; 16], dir: u8, h: f32) -> bool {
    let (_, t1, _) = plane_axes(dir);
    let rows = ((h * 16.0 - 0.01).ceil().max(0.0) as usize).min(16);
    let ymask: u16 = if rows >= 16 { 0xFFFF } else { (1u16 << rows) - 1 };
    if t1 == 1 { m[..rows].iter().all(|r| *r == 0xFFFF) } else { m.iter().all(|r| r & ymask == ymask) }
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
            Tint::Water => rgb(biome.map(|b| b.water).unwrap_or(0x3F76E4)), Tint::Fixed(c) => rgb(c), Tint::DryFoliage => rgb(biome.map(|b| b.dry_foliage).unwrap_or(0x7B5334)),
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
                // shape-based culling (matches the vanilla client per basalt-check client-mesh): this
                // block's whole boundary face on that side is covered by the neighbour's face
                if let (Some(own), Some(nm)) = (&model.occl, &a.models[ni.model as usize].occl) {
                    let (mine, theirs) = (&own[q.cull as usize], &nm[(q.cull ^ 1) as usize]);
                    if mine.iter().any(|r| *r != 0) && mine.iter().zip(theirs.iter()).all(|(m, t)| m & t == *m) { continue; }
                }
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
            let plane = q.pos[0][axis];
            let flush = if d[axis] > 0 { plane >= 0.9999 } else { plane <= 0.0001 };
            if smooth {
                // the centre cell is always the one in front of the face; the side and diagonal cells sit in
                // that layer for a flush face, in the block's own layer for an inset one (measured with
                // controlled light probes against the vanilla client)
                let nbc = [x + d[0], y + d[1], z + d[2]];
                let lb = if flush { nbc } else { [x, y, z] };
                // ... unless that cell is an opaque block (an inset face covered by a neighbour): then the block's own cell
                let nb_ix = sidx(nbc[0], nbc[1], nbc[2]);
                let centre_ix = if a.info(s.blocks[nb_ix]).occludes { sidx(x, y, z) } else { nb_ix };
                let fl = s.light[centre_ix];
                for (ci, (c1, c2)) in [(-1, -1), (1, -1), (-1, 1), (1, 1)].into_iter().enumerate() {
                    let cell = |a1: i32, a2: i32| { let mut c = lb; c[t1] += a1; c[t2] += a2; sidx(c[0], c[1], c[2]) };
                    let (mut ao, mut sky, mut blk) = (0.0f32, 0.0f32, 0.0f32);
                    let aoc = { let c = lb; sidx(c[0], c[1], c[2]) };
                    for (r, ix) in [centre_ix, cell(c1, 0), cell(0, c2), cell(c1, c2)].into_iter().enumerate() {
                        // ambient occlusion samples the same layer for all four cells (the centre only differs for light)
                        ao += if a.info(s.blocks[if r == 0 { aoc } else { ix }]).ao_occluder { 0.2 } else { 1.0 };
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
                    // flat lighting: the cell in front of a face on the block boundary, the block's own cell for an inset face
                    let l = if flush { s.light[sidx(x + d[0], y + d[1], z + d[2])] } else { own_light };
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
            let (still, flow, overlay) = if k == 1 { (a.water_still, a.water_flow, a.water_overlay) } else { (a.lava_still, a.lava_flow, a.lava_flow) }; // lava has no overlay
            let col = if k == 1 { rgb(biome.map(|b| b.water).unwrap_or(0x3F76E4)) } else { [1.0; 3] };
            let layer = if k == 1 { LAYER_TRANSLUCENT } else { LAYER_OPAQUE };
            let above_same = a.info(s.blocks[sidx(x, y + 1, z)]).fluid == k;
            let (h_nw, h_sw, h_se, h_ne) = if above_same { (1.0, 1.0, 1.0, 1.0) } else {
                (fluid_corner(a, s, x, y, z, k, -1, -1), fluid_corner(a, s, x, y, z, k, -1, 1), fluid_corner(a, s, x, y, z, k, 1, 1), fluid_corner(a, s, x, y, z, k, 1, -1))
            };
            // drawn heights sit 0.001 below the logical height unless the corner is full
            let inset = |h: f32| if h < 1.0 { h - 0.001 } else { h };
            let (h_nw, h_sw, h_se, h_ne) = (inset(h_nw), inset(h_sw), inset(h_se), inset(h_ne));
            // light: own cell combined (per channel max) with the cell above (top, sides) / below (bottom); lava is full block light
            let mxl = |p: u8, q: u8| (((p >> 4).max(q >> 4)) << 4) | ((p & 15).max(q & 15));
            let lit = |l: u8| [(l >> 4) * 17, if k == 2 { 255 } else { (l & 15) * 17 }, 0, 0];
            // the block's own light emission counts as block light of its cell
            let own_l = if info.full_bright { 0xFF } else { (own_light & 0xF0) | (own_light & 15).max(info.emission) };
            let light_side = lit(mxl(own_l, s.light[sidx(x, y + 1, z)]));
            let light_bottom = lit(mxl(own_l, s.light[sidx(x, y - 1, z)]));
            let mut quad = |p: [[f32; 3]; 4], sh: f32, uv: [[f32; 2]; 4], r: [f32; 4], light: [u8; 4], dir: u8| {
                let m = &mut out.layers[layer as usize]; let b = m.verts.len() as u32;
                let mut vs = [Vertex::default(); 4];
                for i in 0..4 {
                    vs[i] = Vertex { pos: [base[0] + p[i][0], base[1] + p[i][1], base[2] + p[i][2]], uv: [r[0] + (r[2] - r[0]) * uv[i][0], r[1] + (r[3] - r[1]) * uv[i][1]],
                        color: [(col[0] * sh * 255.0) as u8, (col[1] * sh * 255.0) as u8, (col[2] * sh * 255.0) as u8, if k == 1 { 200 } else { 255 }], light };
                }
                m.verts.extend_from_slice(&vs);
                if let Some(t) = trace.as_deref_mut() { t.push(QuadTrace { src: 1, layer, dir, cull: NO_CULL, tinted: k == 1, tint: col, shade: true, verts: vs }); }
                m.idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
            };
            let rev = |p: [[f32; 3]; 4]| [p[0], p[3], p[2], p[1]];
            let rev_uv = |u: [[f32; 2]; 4]| [u[0], u[3], u[2], u[1]];
            let full_uv = [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]];
            // top: drawn double-sided unless the cell above holds the same fluid
            if !above_same {
                let p = [[0.0, h_nw, 0.0], [0.0, h_sw, 1.0], [1.0, h_se, 1.0], [1.0, h_ne, 0.0]];
                let flow_v = fluid_flow(a, s, x, y, z, k, info.fluid_level);
                if flow_v[0].abs() < 1e-6 && flow_v[1].abs() < 1e-6 {
                    quad(p, 1.0, full_uv, still, light_side, UP);
                    quad(rev(p), 1.0, rev_uv(full_uv), still, light_side, UP);
                } else {
                    // flowing: the flow sprite's central half is rotated so it runs along the flow direction
                    // (flow +z = unrotated, measured against the vanilla client)
                    let phi = flow_v[1].atan2(flow_v[0]) - std::f32::consts::FRAC_PI_2;
                    let (sn, cs) = phi.sin_cos();
                    let uvf = |cx: f32, cz: f32| { let (ax, bz) = ((cx - 0.5) * 0.5, (cz - 0.5) * 0.5); [0.5 + ax * cs + bz * sn, 0.5 - ax * sn + bz * cs] };
                    let uv = [uvf(0.0, 0.0), uvf(0.0, 1.0), uvf(1.0, 1.0), uvf(1.0, 0.0)];
                    quad(p, 1.0, uv, flow, light_side, UP);
                    quad(rev(p), 1.0, rev_uv(uv), flow, light_side, UP);
                }
            }
            // bottom: sits 0.001 above the cell floor; the side faces start there too when it is drawn
            let below = a.info(s.blocks[sidx(x, y - 1, z)]);
            let full = |o: &Option<Box<[[u16; 16]; 6]>>, d: u8| o.as_ref().map(|o| o[d as usize].iter().all(|r| *r == 0xFFFF)).unwrap_or(false);
            let own_model = &a.models[if info.hides_fluid { info.model } else { 0 } as usize];
            let bottom_hidden = below.fluid == k || below.occludes || full(&a.models[below.model as usize].occl, UP) || full(&own_model.shape, DOWN);
            let yo = if bottom_hidden { 0.0 } else { 0.001 };
            if !bottom_hidden {
                quad([[0.0, yo, 0.0], [1.0, yo, 0.0], [1.0, yo, 1.0], [0.0, yo, 1.0]], 0.5, [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]], still, light_bottom, DOWN);
            }
            // sides: culled by the same fluid or a neighbour face covering 0..h; full non-opaque cubes
            // (glass, leaves) get one overlay quad instead; otherwise the flow sprite at half scale, double-sided
            let vt = |h: f32| (1.0 - h) * 0.5;
            let sides: [(u8, [i32; 3], f32, [[f32; 3]; 4], [[f32; 2]; 4], f32); 4] = [
                (NORTH, [0, 0, -1], 0.8, [[0.0, h_nw, 0.001], [1.0, h_ne, 0.001], [1.0, yo, 0.001], [0.0, yo, 0.001]], [[0.0, vt(h_nw)], [0.5, vt(h_ne)], [0.5, 0.5], [0.0, 0.5]], h_nw.max(h_ne)),
                (SOUTH, [0, 0, 1], 0.8, [[1.0, h_se, 0.999], [0.0, h_sw, 0.999], [0.0, yo, 0.999], [1.0, yo, 0.999]], [[0.0, vt(h_se)], [0.5, vt(h_sw)], [0.5, 0.5], [0.0, 0.5]], h_se.max(h_sw)),
                (WEST, [-1, 0, 0], 0.6, [[0.001, h_sw, 1.0], [0.001, h_nw, 0.0], [0.001, yo, 0.0], [0.001, yo, 1.0]], [[0.0, vt(h_sw)], [0.5, vt(h_nw)], [0.5, 0.5], [0.0, 0.5]], h_sw.max(h_nw)),
                (EAST, [1, 0, 0], 0.6, [[0.999, h_ne, 0.0], [0.999, h_se, 1.0], [0.999, yo, 1.0], [0.999, yo, 0.0]], [[0.0, vt(h_ne)], [0.5, vt(h_se)], [0.5, 0.5], [0.0, 0.5]], h_ne.max(h_se)),
            ];
            for (dir, d, sh, p, uv, hmax) in sides {
                let ni = a.info(s.blocks[sidx(x + d[0], y + d[1], z + d[2])]);
                if ni.fluid == k || ni.occludes { continue; }
                if let Some(o) = &a.models[ni.model as usize].occl { if mask_covers_side(&o[(dir ^ 1) as usize], dir, hmax) { continue; } }
                if let Some(o) = &own_model.shape { if mask_covers_side(&o[dir as usize], dir, hmax) { continue; } }
                if ni.fluid_overlay && k == 1 { quad(p, sh, uv, overlay, light_side, dir); } else {
                    quad(p, sh, uv, flow, light_side, dir);
                    quad(rev(p), sh, rev_uv(uv), flow, light_side, dir);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use CornerCell::*;

    /// Drawn corner heights (logical height - 0.001) measured from the vanilla client by `basalt-check client-mesh`.
    #[test]
    fn fluid_corner_heights_match_vanilla_measurements() {
        let drawn = |cells: [CornerCell; 4]| fluid_corner_height(cells, false) - 0.001;
        let close = |a: f32, b: f32| (a - b).abs() < 0.0006;
        // [own cell, side A, side B, diagonal]
        assert!(close(drawn([Fluid(0), Open, Open, Open]), 0.7397), "isolated source");
        assert!(close(drawn([Fluid(0), Open, Open, Solid]), 0.7397), "diagonal ignored without a fluid side");
        assert!(close(drawn([Fluid(0), Open, Solid, Open]), 0.8071));
        assert!(close(drawn([Fluid(0), Fluid(0), Open, Open]), 0.8071));
        assert!(close(drawn([Fluid(0), Fluid(0), Open, Solid]), 0.8456));
        assert!(close(drawn([Fluid(0), Fluid(0), Solid, Open]), 0.8456), "open diagonal counts next to a fluid side");
        assert!(close(drawn([Fluid(0), Fluid(0), Fluid(0), Open]), 0.8592));
        assert!(close(drawn([Fluid(0), Fluid(0), Fluid(0), Solid]), 0.8879));
        assert!(close(drawn([Fluid(0), Solid, Solid, Open]), 0.8879));
        assert!(close(drawn([Fluid(1), Open, Open, Open]), 0.2583), "isolated level 1");
        assert!(close(drawn([Fluid(3), Open, Open, Open]), 0.1842));
        assert!(close(drawn([Fluid(7), Open, Open, Open]), 0.0360));
        assert!(close(drawn([Fluid(8), Open, Open, Open]), 0.7397), "falling fluid weighs like a source");
        assert_eq!(fluid_corner_height([Fluid(0), Open, Open, Open], true), 1.0);
    }

    #[test]
    fn fluid_level_heights() {
        assert!((fluid_level_height(0) - 8.0 / 9.0).abs() < 1e-6);
        assert!((fluid_level_height(7) - 1.0 / 9.0).abs() < 1e-6);
        assert!((fluid_level_height(12) - 8.0 / 9.0).abs() < 1e-6);
    }
}

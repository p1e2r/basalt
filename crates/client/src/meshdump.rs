//! `--mesh-dump <requests.json|dir> --out <dir>`: mesh the oracle's block-meshing scenes
//! (schema: tools/oracle-mod/SCHEMA.md, kind "mesh") with Basalt's own mesher and write the
//! same response schema, so crates/check can diff Basalt against the vanilla client's dump.
//! No window, no GPU.
use crate::assets::*;
use crate::mesh::{self, QuadTrace, SectionMesh, Snapshot};
use anyhow::{anyhow, Context, Result};
use basalt_data::BlockState;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

const DIRS: [&str; 6] = ["down", "up", "north", "south", "west", "east"];
const LAYERS: [&str; 3] = ["solid", "cutout", "translucent"];

/// Reverse lookup atlas pixel -> texture name (index into names).
struct TexIndex { w: u32, h: u32, cells: Vec<u32>, names: Vec<(String, Rect)> }
impl TexIndex {
    fn new(a: &Assets) -> TexIndex {
        let (w, h) = (a.atlas.w, a.atlas.h);
        let mut cells = vec![u32::MAX; (w * h) as usize];
        let mut names = Vec::new();
        for (n, (r, _)) in &a.atlas.rects {
            let i = names.len() as u32;
            names.push((n.clone(), *r));
            for y in r.y..r.y + r.h { for x in r.x..r.x + r.w { cells[(y * w + x) as usize] = i; } }
        }
        TexIndex { w, h, cells, names }
    }
    fn find(&self, u: f32, v: f32) -> Option<&(String, Rect)> {
        let x = (u * self.w as f32) as i64; let y = (v * self.h as f32) as i64;
        if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 { return None; }
        self.names.get(*self.cells.get((y as u32 * self.w + x as u32) as usize)? as usize)
    }
}

fn r5(f: f32) -> f64 { (f as f64 * 100000.0).round() / 100000.0 }

fn ivec(v: &Value) -> Option<[i32; 3]> { let a = v.as_array()?; Some([a.first()?.as_i64()? as i32, a.get(1)?.as_i64()? as i32, a.get(2)?.as_i64()? as i32]) }

/// Mesh one scene; returns the quads in the response schema.
fn mesh_scene(a: &Assets, tex: &TexIndex, s: &Value) -> Result<Vec<Value>> {
    let origin = s.get("origin").and_then(ivec).unwrap_or([0, 64, 0]);
    // the target sits at section-local (8, 8, 8)
    let so = [origin[0] - 8, origin[1] - 8, origin[2] - 8];
    let mut blocks = Box::new([0u16; 5832]);
    let mut light = Box::new([0u8; 5832]);
    let (mut dsky, mut dblk) = (15u8, 0u8);
    if let Some(d) = s.pointer("/light/default").and_then(|d| d.as_array()) { dsky = d[0].as_u64().unwrap_or(15) as u8; dblk = d[1].as_u64().unwrap_or(0) as u8; }
    light.fill(dsky << 4 | dblk);
    let inside = |d: [i32; 3]| d.iter().all(|c| (-9..=8).contains(c));
    for b in s["blocks"].as_array().ok_or_else(|| anyhow!("no blocks"))? {
        let d = [b[0].as_i64().unwrap_or(0) as i32, b[1].as_i64().unwrap_or(0) as i32, b[2].as_i64().unwrap_or(0) as i32];
        let name = b[3].as_str().unwrap_or("");
        let st = BlockState::parse(name).ok_or_else(|| anyhow!("unknown block state {name}"))?;
        if inside(d) { blocks[mesh::sidx(8 + d[0], 8 + d[1], 8 + d[2])] = st.0 as u16; }
    }
    if let Some(cells) = s.pointer("/light/cells").and_then(|c| c.as_array()) {
        for c in cells {
            let d = [c[0].as_i64().unwrap_or(0) as i32, c[1].as_i64().unwrap_or(0) as i32, c[2].as_i64().unwrap_or(0) as i32];
            if inside(d) { light[mesh::sidx(8 + d[0], 8 + d[1], 8 + d[2])] = (c[3].as_u64().unwrap_or(0) as u8) << 4 | c[4].as_u64().unwrap_or(0) as u8; }
        }
    }
    let t = &s["tint"];
    let col = |k: &str, d: u32| t.get(k).and_then(|v| v.as_u64()).map(|v| v as u32).unwrap_or(d);
    let biome = Biome { name: "scene".into(), grass: col("grass", 0x91BD59), foliage: col("foliage", 0x77AB2F), water: col("water", 0x3F76E4), sky: 0x78A7FF, fog: 0xC0D8FF };
    let snap = Snapshot { origin: so, blocks, light, biomes: [0; 64] };
    let mut out = SectionMesh::default();
    let mut trace: Vec<QuadTrace> = Vec::new();
    mesh::mesh_block(a, std::slice::from_ref(&biome), &snap, 8, 8, 8, &mut out, Some(&mut trace));
    let mut quads = Vec::with_capacity(trace.len());
    for q in &trace {
        let cu = q.verts.iter().map(|v| v.uv[0]).sum::<f32>() / 4.0; let cv = q.verts.iter().map(|v| v.uv[1]).sum::<f32>() / 4.0;
        let found = tex.find(cu, cv);
        let name = found.map(|(n, _)| if n.contains(':') { n.clone() } else { format!("minecraft:{n}") }).unwrap_or_else(|| "?".into());
        let verts: Vec<Value> = q.verts.iter().map(|v| {
            let (lu, lv) = match found {
                Some((_, r)) => ((v.uv[0] * tex.w as f32 - r.x as f32) / r.w as f32, (v.uv[1] * tex.h as f32 - r.y as f32) / r.h as f32),
                None => (v.uv[0], v.uv[1]),
            };
            let argb = 0xFF00_0000u32 | (v.color[0] as u32) << 16 | (v.color[1] as u32) << 8 | v.color[2] as u32;
            // Basalt keeps light as level*17 in a u8; report it in vanilla's packed light-coords form
            let sky = (v.light[0] as f32 / 17.0 * 16.0).round() as u32; let blk = (v.light[1] as f32 / 17.0 * 16.0).round() as u32;
            json!([r5(v.pos[0] - origin[0] as f32), r5(v.pos[1] - origin[1] as f32), r5(v.pos[2] - origin[2] as f32), r5(lu), r5(lv), argb, sky << 16 | blk])
        }).collect();
        let mut o = json!({
            "src": if q.src == 0 { "block" } else { "fluid" }, "layer": LAYERS[q.layer as usize], "dir": DIRS[q.dir as usize],
            "tex": name, "ti": if q.tinted { 0 } else { -1 }, "shade": q.shade, "v": verts,
        });
        if q.src == 0 && q.cull < 6 { o["cull"] = json!(DIRS[q.cull as usize]); }
        if q.tinted {
            let c = ((q.tint[0] * 255.0).round() as u32) << 16 | ((q.tint[1] * 255.0).round() as u32) << 8 | (q.tint[2] * 255.0).round() as u32;
            o["tint"] = json!(c);
        }
        quads.push(o);
    }
    Ok(quads)
}

fn run_one(a: &Assets, tex: &TexIndex, req_path: &Path, out_dir: &Path) -> Result<usize> {
    let req: Value = serde_json::from_slice(&std::fs::read(req_path).with_context(|| format!("reading {}", req_path.display()))?)?;
    let id = req["id"].as_str().map(String::from).unwrap_or_else(|| req_path.file_stem().unwrap().to_string_lossy().into_owned());
    let t0 = std::time::Instant::now();
    let mut scenes = Vec::new();
    let mut total = 0usize;
    for s in req["scenes"].as_array().ok_or_else(|| anyhow!("{}: no scenes", req_path.display()))? {
        let sid = s["id"].clone();
        match mesh_scene(a, tex, s) {
            Ok(q) => {
                total += q.len();
                // the vanilla client picks one of several weighted models by position; Basalt uses the first
                let target = s.pointer("/blocks/0/3").and_then(|v| v.as_str()).and_then(BlockState::parse);
                let random = target.map(|t| a.random_variant.get(t.0 as usize).copied().unwrap_or(false)).unwrap_or(false);
                scenes.push(json!({"id": sid, "quads": q, "random_variant": random}));
            }
            Err(e) => scenes.push(json!({"id": sid, "error": e.to_string(), "quads": []})),
        }
    }
    let resp = json!({"id": id, "kind": "mesh", "oracle": "basalt", "options": {"ao": true, "cull": true}, "scenes": scenes,
        "total_quads": total, "millis": t0.elapsed().as_millis() as u64});
    std::fs::create_dir_all(out_dir)?;
    let tmp = out_dir.join(format!("{id}.json.tmp"));
    let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
    serde_json::to_writer(&mut f, &resp)?; f.flush()?; drop(f);
    std::fs::rename(&tmp, out_dir.join(format!("{id}.json")))?;
    Ok(total)
}

/// `input` is a request file or a directory of request files (*.json).
pub fn run(a: &Assets, input: &str, out: &str) -> Result<()> {
    let tex = TexIndex::new(a);
    let p = PathBuf::from(input);
    let mut files: Vec<PathBuf> = if p.is_dir() {
        std::fs::read_dir(&p)?.filter_map(|e| e.ok().map(|e| e.path())).filter(|f| f.extension().map(|e| e == "json").unwrap_or(false)).collect()
    } else { vec![p] };
    files.sort();
    let mut total = 0;
    for f in &files { total += run_one(a, &tex, f, Path::new(out))?; }
    eprintln!("mesh-dump: {} request file(s), {} quads -> {}", files.len(), total, out);
    Ok(())
}

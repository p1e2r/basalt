//! Density-function graphs built from the worldgen JSON, and their evaluator.

use crate::noise::{BlendedNoise, NoiseParams, NormalNoise};
use crate::random::PosFactory;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub type NodeId = u32;

#[derive(Clone, Debug)]
pub enum SplineValue {
    Const(f32),
    Spline(Box<Spline>),
}

#[derive(Clone, Debug)]
pub struct Spline {
    pub coord: NodeId,
    pub locs: Vec<f32>,
    pub vals: Vec<SplineValue>,
    pub ders: Vec<f32>,
}

#[derive(Clone, Debug)]
pub enum Node {
    Const(f64),
    Add(NodeId, NodeId),
    Mul(NodeId, NodeId),
    Min(NodeId, NodeId),
    Max(NodeId, NodeId),
    Abs(NodeId),
    Square(NodeId),
    Cube(NodeId),
    HalfNegative(NodeId),
    QuarterNegative(NodeId),
    Squeeze(NodeId),
    Invert(NodeId),
    Clamp(NodeId, f64, f64),
    YGradient { from_y: i32, to_y: i32, from_v: f64, to_v: f64 },
    Noise { noise: u32, xz: f64, y: f64 },
    ShiftedNoise { noise: u32, xz: f64, y: f64, sx: NodeId, sy: NodeId, sz: NodeId },
    ShiftA(u32),
    ShiftB(u32),
    Shift(u32),
    RangeChoice { input: NodeId, min: f64, max: f64, inside: NodeId, outside: NodeId },
    IntervalSelect { input: NodeId, thresholds: Vec<f64>, fns: Vec<NodeId> },
    Interpolated(NodeId, u32),
    FlatCache(NodeId),
    Cache2d(NodeId),
    CacheOnce(NodeId, u32),
    BlendAlpha,
    BlendOffset,
    BlendDensity(NodeId),
    Spline(Box<Spline>),
    OldBlendedNoise(u32),
    FindTopSurface { density: NodeId, upper: NodeId, lower: i32, cell: i32 },
}

/// A compiled set of density functions sharing nodes and noises.
pub struct Graph {
    pub nodes: Vec<Node>,
    pub noises: Vec<NormalNoise>,
    pub noise_names: Vec<String>,
    pub blended: Vec<BlendedNoise>,
    pub n_interp: u32,
    pub n_once: u32,
}

pub struct GraphBuilder<'a> {
    root: PathBuf,
    rand: PosFactory,
    nodes: Vec<Node>,
    refs: HashMap<String, NodeId>,
    noise_idx: HashMap<String, u32>,
    noises: Vec<NormalNoise>,
    noise_names: Vec<String>,
    blended: Vec<BlendedNoise>,
    n_interp: u32,
    n_once: u32,
    consts: HashMap<u64, NodeId>,
    _p: std::marker::PhantomData<&'a ()>,
}

pub fn load_noise_params(root: &Path, name: &str) -> Result<NoiseParams> {
    let n = name.trim_start_matches("minecraft:");
    let p = root.join("noise").join(format!("{n}.json"));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?)?;
    Ok(NoiseParams {
        first_octave: v["firstOctave"].as_i64().ok_or_else(|| anyhow!("firstOctave"))? as i32,
        amplitudes: v["amplitudes"].as_array().ok_or_else(|| anyhow!("amplitudes"))?.iter().map(|a| a.as_f64().unwrap_or(0.0)).collect(),
    })
}

fn full_name(s: &str) -> String { if s.contains(':') { s.to_string() } else { format!("minecraft:{s}") } }

impl<'a> GraphBuilder<'a> {
    /// `root` is data/minecraft/worldgen; `rand` is the world's positional factory.
    pub fn new(root: &Path, rand: PosFactory) -> Self {
        Self {
            root: root.to_path_buf(), rand, nodes: Vec::new(), refs: HashMap::new(), noise_idx: HashMap::new(),
            noises: Vec::new(), noise_names: Vec::new(), blended: Vec::new(), n_interp: 0, n_once: 0, consts: HashMap::new(),
            _p: std::marker::PhantomData,
        }
    }

    fn push(&mut self, n: Node) -> NodeId { self.nodes.push(n); (self.nodes.len() - 1) as NodeId }

    pub fn constant(&mut self, v: f64) -> NodeId {
        if let Some(i) = self.consts.get(&v.to_bits()) { return *i; }
        let i = self.push(Node::Const(v));
        self.consts.insert(v.to_bits(), i);
        i
    }

    pub fn noise(&mut self, name: &str) -> Result<u32> {
        let name = full_name(name);
        if let Some(i) = self.noise_idx.get(&name) { return Ok(*i); }
        let params = load_noise_params(&self.root, &name)?;
        let mut r = self.rand.from_hash(&name);
        let nn = NormalNoise::new(&mut r, &params);
        self.noises.push(nn);
        self.noise_names.push(name.clone());
        let i = (self.noises.len() - 1) as u32;
        self.noise_idx.insert(name, i);
        Ok(i)
    }

    pub fn parse(&mut self, v: &Value) -> Result<NodeId> {
        match v {
            Value::Number(n) => Ok(self.constant(n.as_f64().unwrap())),
            Value::String(s) => self.reference(s),
            Value::Object(o) => {
                let ty = o.get("type").and_then(|t| t.as_str()).ok_or_else(|| anyhow!("density function without type"))?;
                let ty = ty.trim_start_matches("minecraft:");
                let arg = |b: &mut Self, k: &str| -> Result<NodeId> { b.parse(o.get(k).ok_or_else(|| anyhow!("{ty}: missing {k}"))?) };
                let num = |k: &str| -> Result<f64> { o.get(k).and_then(|x| x.as_f64()).ok_or_else(|| anyhow!("{ty}: missing {k}")) };
                let node = match ty {
                    "constant" => return Ok(self.constant(num("argument")?)),
                    "add" | "mul" | "min" | "max" => {
                        let a = arg(self, "argument1")?;
                        let b = arg(self, "argument2")?;
                        match ty { "add" => Node::Add(a, b), "mul" => Node::Mul(a, b), "min" => Node::Min(a, b), _ => Node::Max(a, b) }
                    }
                    "abs" => Node::Abs(arg(self, "argument")?),
                    "square" => Node::Square(arg(self, "argument")?),
                    "cube" => Node::Cube(arg(self, "argument")?),
                    "half_negative" => Node::HalfNegative(arg(self, "argument")?),
                    "quarter_negative" => Node::QuarterNegative(arg(self, "argument")?),
                    "squeeze" => Node::Squeeze(arg(self, "argument")?),
                    "invert" => Node::Invert(arg(self, "argument")?),
                    "clamp" => Node::Clamp(arg(self, "input")?, num("min")?, num("max")?),
                    "y_clamped_gradient" => Node::YGradient {
                        from_y: num("from_y")? as i32, to_y: num("to_y")? as i32, from_v: num("from_value")?, to_v: num("to_value")?,
                    },
                    "noise" => {
                        let n = self.noise(o["noise"].as_str().ok_or_else(|| anyhow!("noise name"))?)?;
                        Node::Noise { noise: n, xz: num("xz_scale")?, y: num("y_scale")? }
                    }
                    "shifted_noise" => {
                        let n = self.noise(o["noise"].as_str().ok_or_else(|| anyhow!("noise name"))?)?;
                        Node::ShiftedNoise {
                            noise: n, xz: num("xz_scale")?, y: num("y_scale")?,
                            sx: arg(self, "shift_x")?, sy: arg(self, "shift_y")?, sz: arg(self, "shift_z")?,
                        }
                    }
                    "shift_a" | "shift_b" | "shift" => {
                        let n = self.noise(o["argument"].as_str().ok_or_else(|| anyhow!("shift noise"))?)?;
                        match ty { "shift_a" => Node::ShiftA(n), "shift_b" => Node::ShiftB(n), _ => Node::Shift(n) }
                    }
                    "range_choice" => Node::RangeChoice {
                        input: arg(self, "input")?, min: num("min_inclusive")?, max: num("max_exclusive")?,
                        inside: arg(self, "when_in_range")?, outside: arg(self, "when_out_of_range")?,
                    },
                    "interval_select" => {
                        let input = arg(self, "input")?;
                        let thresholds: Vec<f64> = o["thresholds"].as_array().ok_or_else(|| anyhow!("thresholds"))?.iter().map(|t| t.as_f64().unwrap()).collect();
                        let mut fns = Vec::new();
                        for f in o["functions"].as_array().ok_or_else(|| anyhow!("functions"))? { fns.push(self.parse(f)?); }
                        if fns.len() != thresholds.len() + 1 { bail!("interval_select: {} functions for {} thresholds", fns.len(), thresholds.len()); }
                        Node::IntervalSelect { input, thresholds, fns }
                    }
                    "interpolated" => { let a = arg(self, "argument")?; self.n_interp += 1; Node::Interpolated(a, self.n_interp - 1) }
                    "flat_cache" => Node::FlatCache(arg(self, "argument")?),
                    "cache_2d" => Node::Cache2d(arg(self, "argument")?),
                    "cache_once" => { let a = arg(self, "argument")?; self.n_once += 1; Node::CacheOnce(a, self.n_once - 1) }
                    "cache_all_in_cell" => return arg(self, "argument"),
                    "blend_alpha" => Node::BlendAlpha,
                    "blend_offset" => Node::BlendOffset,
                    "blend_density" => Node::BlendDensity(arg(self, "argument")?),
                    "spline" => Node::Spline(Box::new(self.spline(&o["spline"])?)),
                    "old_blended_noise" => {
                        let mut r = self.rand.from_hash("minecraft:terrain");
                        let b = BlendedNoise::new(&mut r, num("xz_scale")?, num("y_scale")?, num("xz_factor")?, num("y_factor")?, num("smear_scale_multiplier")?);
                        self.blended.push(b);
                        Node::OldBlendedNoise((self.blended.len() - 1) as u32)
                    }
                    "find_top_surface" => Node::FindTopSurface {
                        density: arg(self, "density")?, upper: arg(self, "upper_bound")?,
                        lower: num("lower_bound")? as i32, cell: num("cell_height")? as i32,
                    },
                    other => bail!("unsupported density function type {other}"),
                };
                Ok(self.push(node))
            }
            _ => bail!("bad density function {v}"),
        }
    }

    fn spline(&mut self, v: &Value) -> Result<Spline> {
        let coord = self.parse(&v["coordinate"])?;
        let mut locs = Vec::new();
        let mut vals = Vec::new();
        let mut ders = Vec::new();
        for p in v["points"].as_array().ok_or_else(|| anyhow!("spline points"))? {
            locs.push(p["location"].as_f64().unwrap() as f32);
            ders.push(p["derivative"].as_f64().unwrap() as f32);
            let val = &p["value"];
            vals.push(if let Some(f) = val.as_f64() { SplineValue::Const(f as f32) } else { SplineValue::Spline(Box::new(self.spline(val)?)) });
        }
        Ok(Spline { coord, locs, vals, ders })
    }

    pub fn reference(&mut self, name: &str) -> Result<NodeId> {
        let name = full_name(name);
        if let Some(i) = self.refs.get(&name) { return Ok(*i); }
        let path = self.root.join("density_function").join(format!("{}.json", name.trim_start_matches("minecraft:")));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?)?;
        let id = self.parse(&v).with_context(|| format!("in {name}"))?;
        self.refs.insert(name, id);
        Ok(id)
    }

    pub fn finish(self) -> Graph {
        Graph { nodes: self.nodes, noises: self.noises, noise_names: self.noise_names, blended: self.blended, n_interp: self.n_interp, n_once: self.n_once }
    }
}

#[inline]
pub fn clamped_map(v: f64, a: f64, b: f64, c: f64, d: f64) -> f64 {
    let t = (v - a) / (b - a);
    if t < 0.0 { c } else if t > 1.0 { d } else { c + t * (d - c) }
}

/// Evaluation mode for `interpolated` nodes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Evaluate the arguments of interpolated nodes directly.
    Direct,
    /// Interpolated nodes return the values stored in `Eval::interp`.
    Interpolated,
}

/// Per-thread evaluation state (caches).
pub struct Eval<'g> {
    pub g: &'g Graph,
    pub mode: Mode,
    pub interp: Vec<f64>,
    once: Vec<(i32, i32, i32, f64)>,
    flat: HashMap<(NodeId, i32, i32), f64>,
}

impl<'g> Eval<'g> {
    pub fn new(g: &'g Graph) -> Self {
        Self {
            g, mode: Mode::Direct, interp: vec![0.0; g.n_interp as usize],
            once: vec![(i32::MIN, 0, 0, 0.0); g.n_once as usize], flat: HashMap::new(),
        }
    }

    pub fn set_mode(&mut self, m: Mode) {
        if m != self.mode {
            self.mode = m;
            for o in &mut self.once { o.0 = i32::MIN; }
        }
    }

    pub fn eval(&mut self, id: NodeId, x: i32, y: i32, z: i32) -> f64 {
        let g = self.g;
        match &g.nodes[id as usize] {
            Node::Const(v) => *v,
            Node::Add(a, b) => self.eval(*a, x, y, z) + self.eval(*b, x, y, z),
            Node::Mul(a, b) => {
                let d = self.eval(*a, x, y, z);
                if d == 0.0 { 0.0 } else { d * self.eval(*b, x, y, z) }
            }
            Node::Min(a, b) => { let d = self.eval(*a, x, y, z); d.min(self.eval(*b, x, y, z)) }
            Node::Max(a, b) => { let d = self.eval(*a, x, y, z); d.max(self.eval(*b, x, y, z)) }
            Node::Abs(a) => self.eval(*a, x, y, z).abs(),
            Node::Square(a) => { let d = self.eval(*a, x, y, z); d * d }
            Node::Cube(a) => { let d = self.eval(*a, x, y, z); d * d * d }
            Node::HalfNegative(a) => { let d = self.eval(*a, x, y, z); if d > 0.0 { d } else { d * 0.5 } }
            Node::QuarterNegative(a) => { let d = self.eval(*a, x, y, z); if d > 0.0 { d } else { d * 0.25 } }
            Node::Squeeze(a) => {
                let d = self.eval(*a, x, y, z).clamp(-1.0, 1.0);
                d / 2.0 - d * d * d / 24.0
            }
            Node::Invert(a) => 1.0 / self.eval(*a, x, y, z),
            Node::Clamp(a, lo, hi) => self.eval(*a, x, y, z).clamp(*lo, *hi),
            Node::YGradient { from_y, to_y, from_v, to_v } => clamped_map(y as f64, *from_y as f64, *to_y as f64, *from_v, *to_v),
            Node::Noise { noise, xz, y: ys } => g.noises[*noise as usize].get(x as f64 * xz, y as f64 * ys, z as f64 * xz),
            Node::ShiftedNoise { noise, xz, y: ys, sx, sy, sz } => {
                let dx = x as f64 * xz + self.eval(*sx, x, y, z);
                let dy = y as f64 * ys + self.eval(*sy, x, y, z);
                let dz = z as f64 * xz + self.eval(*sz, x, y, z);
                g.noises[*noise as usize].get(dx, dy, dz)
            }
            Node::ShiftA(n) => g.noises[*n as usize].get(x as f64 * 0.25, 0.0, z as f64 * 0.25) * 4.0,
            Node::ShiftB(n) => g.noises[*n as usize].get(z as f64 * 0.25, x as f64 * 0.25, 0.0) * 4.0,
            Node::Shift(n) => g.noises[*n as usize].get(x as f64 * 0.25, y as f64 * 0.25, z as f64 * 0.25) * 4.0,
            Node::RangeChoice { input, min, max, inside, outside } => {
                let d = self.eval(*input, x, y, z);
                if d >= *min && d < *max { self.eval(*inside, x, y, z) } else { self.eval(*outside, x, y, z) }
            }
            Node::IntervalSelect { input, thresholds, fns } => {
                let d = self.eval(*input, x, y, z);
                let i = thresholds.iter().take_while(|t| d >= **t).count();
                self.eval(fns[i], x, y, z)
            }
            Node::Interpolated(a, slot) => {
                if self.mode == Mode::Interpolated { self.interp[*slot as usize] } else { self.eval(*a, x, y, z) }
            }
            Node::FlatCache(a) => {
                let qx = x & !3;
                let qz = z & !3;
                if let Some(v) = self.flat.get(&(id, qx, qz)) { return *v; }
                let v = self.eval(*a, qx, 0, qz);
                self.flat.insert((id, qx, qz), v);
                v
            }
            Node::Cache2d(a) => self.eval(*a, x, y, z),
            Node::CacheOnce(a, slot) => {
                let c = self.once[*slot as usize];
                if c.0 == x && c.1 == y && c.2 == z { return c.3; }
                let v = self.eval(*a, x, y, z);
                self.once[*slot as usize] = (x, y, z, v);
                v
            }
            Node::BlendAlpha => 1.0,
            Node::BlendOffset => 0.0,
            Node::BlendDensity(a) => self.eval(*a, x, y, z),
            Node::Spline(s) => self.spline(s, x, y, z) as f64,
            Node::OldBlendedNoise(b) => g.blended[*b as usize].get(x as f64, y as f64, z as f64),
            Node::FindTopSurface { density, upper, lower, cell } => {
                let up = self.eval(*upper, x, y, z);
                let top = (up / *cell as f64).floor() as i32 * cell;
                let mut yy = top;
                while yy >= *lower {
                    if self.eval(*density, x, yy, z) > 0.0 { return yy as f64; }
                    yy -= cell;
                }
                *lower as f64
            }
        }
    }

    fn spline(&mut self, s: &Spline, x: i32, y: i32, z: i32) -> f32 {
        let f = self.eval(s.coord, x, y, z) as f32;
        let n = s.locs.len();
        let i = s.locs.iter().take_while(|l| f >= **l).count() as isize - 1;
        let last = n - 1;
        if i < 0 {
            let v = self.spline_val(&s.vals[0], x, y, z);
            let d = s.ders[0];
            return if d == 0.0 { v } else { v + d * (f - s.locs[0]) };
        }
        let i = i as usize;
        if i == last {
            let v = self.spline_val(&s.vals[last], x, y, z);
            let d = s.ders[last];
            return if d == 0.0 { v } else { v + d * (f - s.locs[last]) };
        }
        let g = s.locs[i];
        let h = s.locs[i + 1];
        let k = (f - g) / (h - g);
        let l = s.ders[i];
        let m = s.ders[i + 1];
        let nv = self.spline_val(&s.vals[i], x, y, z);
        let o = self.spline_val(&s.vals[i + 1], x, y, z);
        let p = l * (h - g) - (o - nv);
        let q = -m * (h - g) + (o - nv);
        (nv + k * (o - nv)) + k * (1.0 - k) * (p + k * (q - p))
    }

    fn spline_val(&mut self, v: &SplineValue, x: i32, y: i32, z: i32) -> f32 {
        match v {
            SplineValue::Const(c) => *c,
            SplineValue::Spline(s) => self.spline(s, x, y, z),
        }
    }
}

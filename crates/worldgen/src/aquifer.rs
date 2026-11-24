//! Noise-driven aquifers: local fluid levels on a coarse grid, separated by barriers.

use crate::density::{clamped_map, Eval, NodeId};
use crate::random::{PosFactory, Rng};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fluid {
    Air,
    Water,
    Lava,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FluidStatus {
    pub level: i32,
    pub fluid: Fluid,
}

impl FluidStatus {
    #[inline]
    pub fn at(&self, y: i32) -> Fluid {
        if y < self.level {
            self.fluid
        } else {
            Fluid::Air
        }
    }
}

const WAY_BELOW_MIN_Y: i32 = -2032 << 4;

#[derive(Clone, Copy)]
pub struct AquiferNodes {
    pub barrier: NodeId,
    pub floodedness: NodeId,
    pub spread: NodeId,
    pub lava: NodeId,
    pub erosion: NodeId,
    pub depth: NodeId,
    pub prelim: NodeId,
}

pub struct Aquifer {
    nodes: AquiferNodes,
    rand: PosFactory,
    sea_level: i32,
    sea_fluid: Fluid,
    pub enabled: bool,
    centers: HashMap<(i32, i32, i32), (i32, i32, i32)>,
    status: HashMap<(i32, i32, i32), FluidStatus>,
    prelim: HashMap<(i32, i32), i32>,
}

const SURFACE_SAMPLING: [(i32, i32); 13] = [
    (0, 0),
    (-2, -1),
    (-1, -1),
    (0, -1),
    (1, -1),
    (-3, 0),
    (-2, 0),
    (-1, 0),
    (1, 0),
    (-2, 1),
    (-1, 1),
    (0, 1),
    (1, 1),
];

impl Aquifer {
    pub fn new(nodes: AquiferNodes, rand: PosFactory, sea_level: i32, enabled: bool) -> Self {
        Self {
            nodes,
            rand,
            sea_level,
            sea_fluid: Fluid::Water,
            enabled,
            centers: HashMap::new(),
            status: HashMap::new(),
            prelim: HashMap::new(),
        }
    }

    #[inline]
    pub fn global(&self, y: i32) -> FluidStatus {
        if y < (-54).min(self.sea_level) {
            FluidStatus {
                level: -54,
                fluid: Fluid::Lava,
            }
        } else {
            FluidStatus {
                level: self.sea_level,
                fluid: self.sea_fluid,
            }
        }
    }

    pub fn preliminary_surface(&mut self, e: &mut Eval, x: i32, z: i32) -> i32 {
        let qx = x & !3;
        let qz = z & !3;
        if let Some(v) = self.prelim.get(&(qx, qz)) {
            return *v;
        }
        let v = e.eval(self.nodes.prelim, qx, 0, qz).floor() as i32;
        self.prelim.insert((qx, qz), v);
        v
    }

    /// Returns None when the position should stay solid (barrier or positive density).
    pub fn compute(&mut self, e: &mut Eval, x: i32, y: i32, z: i32, density: f64) -> Option<Fluid> {
        if density > 0.0 {
            return None;
        }
        let g = self.global(y);
        if !self.enabled {
            return Some(g.at(y));
        }
        if g.at(y) == Fluid::Lava {
            return Some(Fluid::Lava);
        }
        let l = (x - 5).div_euclid(16);
        let m = (y + 1).div_euclid(12);
        let n = (z - 5).div_euclid(16);
        let (mut o, mut p, mut q) = (i32::MAX, i32::MAX, i32::MAX);
        let (mut r, mut s, mut t) = ((0, 0, 0), (0, 0, 0), (0, 0, 0));
        for u in 0..=1 {
            for v in -1..=1 {
                for w in 0..=1 {
                    let key = (l + u, m + v, n + w);
                    let c = match self.centers.get(&key) {
                        Some(c) => *c,
                        None => {
                            let mut rs = self.rand.at(key.0, key.1, key.2);
                            let cx = key.0 * 16 + rs.next_int_bounded(10);
                            let cy = key.1 * 12 + rs.next_int_bounded(9);
                            let cz = key.2 * 16 + rs.next_int_bounded(10);
                            self.centers.insert(key, (cx, cy, cz));
                            (cx, cy, cz)
                        }
                    };
                    let dx = c.0 - x;
                    let dy = c.1 - y;
                    let dz = c.2 - z;
                    let ag = dx * dx + dy * dy + dz * dz;
                    if o >= ag {
                        t = s;
                        s = r;
                        r = c;
                        q = p;
                        p = o;
                        o = ag;
                    } else if p >= ag {
                        t = s;
                        s = c;
                        q = p;
                        p = ag;
                    } else if q >= ag {
                        t = c;
                        q = ag;
                    }
                }
            }
        }
        let fs2 = self.status_at(e, r);
        let d = similarity(o, p);
        let bs = fs2.at(y);
        if d <= 0.0 {
            return Some(bs);
        }
        if bs == Fluid::Water && self.global(y - 1).at(y - 1) == Fluid::Lava {
            return Some(bs);
        }
        let mut barrier = f64::NAN;
        let fs3 = self.status_at(e, s);
        let e1 = d * self.pressure(e, x, y, z, &mut barrier, fs2, fs3);
        if density + e1 > 0.0 {
            return None;
        }
        let fs4 = self.status_at(e, t);
        let f = similarity(o, q);
        if f > 0.0 {
            let g = d * f * self.pressure(e, x, y, z, &mut barrier, fs2, fs4);
            if density + g > 0.0 {
                return None;
            }
        }
        let g = similarity(p, q);
        if g > 0.0 {
            let h = d * g * self.pressure(e, x, y, z, &mut barrier, fs3, fs4);
            if density + h > 0.0 {
                return None;
            }
        }
        Some(bs)
    }

    #[allow(clippy::too_many_arguments)]
    fn pressure(
        &self,
        e: &mut Eval,
        x: i32,
        y: i32,
        z: i32,
        barrier: &mut f64,
        a: FluidStatus,
        b: FluidStatus,
    ) -> f64 {
        let fa = a.at(y);
        let fb = b.at(y);
        if (fa == Fluid::Lava && fb == Fluid::Water) || (fa == Fluid::Water && fb == Fluid::Lava) {
            return 2.0;
        }
        let j = (a.level - b.level).abs();
        if j == 0 {
            return 0.0;
        }
        let d = 0.5 * (a.level + b.level) as f64;
        let e1 = y as f64 + 0.5 - d;
        let f = j as f64 / 2.0;
        let o = f - e1.abs();
        let q = if e1 > 0.0 {
            let p = o;
            if p > 0.0 {
                p / 1.5
            } else {
                p / 2.5
            }
        } else {
            let p = 3.0 + o;
            if p > 0.0 {
                p / 3.0
            } else {
                p / 10.0
            }
        };
        let r = if (-2.0..=2.0).contains(&q) {
            if barrier.is_nan() {
                *barrier = e.eval(self.nodes.barrier, x, y, z);
            }
            *barrier
        } else {
            0.0
        };
        2.0 * (r + q)
    }

    fn status_at(&mut self, e: &mut Eval, c: (i32, i32, i32)) -> FluidStatus {
        if let Some(s) = self.status.get(&c) {
            return *s;
        }
        let s = self.compute_fluid(e, c.0, c.1, c.2);
        self.status.insert(c, s);
        s
    }

    fn compute_fluid(&mut self, e: &mut Eval, x: i32, y: i32, z: i32) -> FluidStatus {
        let global = self.global(y);
        let mut min_surface = i32::MAX;
        let j = y + 12;
        let k = y - 12;
        let mut flag = false;
        for (ox, oz) in SURFACE_SAMPLING {
            let lx = x + ox * 16;
            let lz = z + oz * 16;
            let n = self.preliminary_surface(e, lx, lz);
            let o = n + 8;
            let center = ox == 0 && oz == 0;
            if center && k > o {
                return global;
            }
            let above = j > o;
            if above || center {
                let fs = self.global(o);
                if fs.at(o) != Fluid::Air {
                    if center {
                        flag = true;
                    }
                    if above {
                        return fs;
                    }
                }
            }
            min_surface = min_surface.min(n);
        }
        let level = self.surface_level(e, x, y, z, global, min_surface, flag);
        FluidStatus {
            level,
            fluid: self.fluid_type(e, x, y, z, global, level),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn surface_level(
        &mut self,
        e: &mut Eval,
        x: i32,
        y: i32,
        z: i32,
        global: FluidStatus,
        min_surface: i32,
        flag: bool,
    ) -> i32 {
        let (d, ee);
        let deep_dark =
            e.eval(self.nodes.erosion, x, y, z) < -0.225 && e.eval(self.nodes.depth, x, y, z) > 0.9;
        if deep_dark {
            d = -1.0;
            ee = -1.0;
        } else {
            let i = min_surface + 8 - y;
            let f = if flag {
                clamped_map(i as f64, 0.0, 64.0, 1.0, 0.0)
            } else {
                0.0
            };
            let g = e.eval(self.nodes.floodedness, x, y, z).clamp(-1.0, 1.0);
            let h = map(f, 1.0, 0.0, -0.3, 0.8);
            let k = map(f, 1.0, 0.0, -0.8, 0.4);
            d = g - k;
            ee = g - h;
        }
        if ee > 0.0 {
            global.level
        } else if d > 0.0 {
            let k = x.div_euclid(16);
            let l = y.div_euclid(40);
            let m = z.div_euclid(16);
            let n = l * 40 + 20;
            let dd = e.eval(self.nodes.spread, k, l, m) * 10.0;
            let o = (dd / 3.0).floor() as i32 * 3;
            min_surface.min(n + o)
        } else {
            WAY_BELOW_MIN_Y
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn fluid_type(
        &mut self,
        e: &mut Eval,
        x: i32,
        y: i32,
        z: i32,
        global: FluidStatus,
        level: i32,
    ) -> Fluid {
        if level <= -10 && level != WAY_BELOW_MIN_Y && global.fluid != Fluid::Lava {
            let k = x.div_euclid(64);
            let l = y.div_euclid(40);
            let m = z.div_euclid(64);
            let d = e.eval(self.nodes.lava, k, l, m);
            if d.abs() > 0.3 {
                return Fluid::Lava;
            }
        }
        global.fluid
    }
}

#[inline]
fn similarity(a: i32, b: i32) -> f64 {
    1.0 - (b - a).abs() as f64 / 25.0
}

#[inline]
pub fn map(v: f64, a: f64, b: f64, c: f64, d: f64) -> f64 {
    c + (v - a) / (b - a) * (d - c)
}

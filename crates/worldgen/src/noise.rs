//! Gradient noise: Ken Perlin's "improved noise", octave sums of it, the
//! two-sample "normal" noise and the legacy blended terrain noise.

use crate::random::{Rng, Xoroshiro};

const GRAD: [[f64; 3]; 16] = [
    [1.0, 1.0, 0.0],
    [-1.0, 1.0, 0.0],
    [1.0, -1.0, 0.0],
    [-1.0, -1.0, 0.0],
    [1.0, 0.0, 1.0],
    [-1.0, 0.0, 1.0],
    [1.0, 0.0, -1.0],
    [-1.0, 0.0, -1.0],
    [0.0, 1.0, 1.0],
    [0.0, -1.0, 1.0],
    [0.0, 1.0, -1.0],
    [0.0, -1.0, -1.0],
    [1.0, 1.0, 0.0],
    [0.0, -1.0, 1.0],
    [-1.0, 1.0, 0.0],
    [0.0, -1.0, -1.0],
];

#[inline]
pub fn lerp(t: f64, a: f64, b: f64) -> f64 {
    a + t * (b - a)
}
#[inline]
pub fn lerp2(tx: f64, ty: f64, a: f64, b: f64, c: f64, d: f64) -> f64 {
    lerp(ty, lerp(tx, a, b), lerp(tx, c, d))
}
#[allow(clippy::too_many_arguments)]
#[inline]
pub fn lerp3(
    tx: f64,
    ty: f64,
    tz: f64,
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
    g: f64,
    h: f64,
) -> f64 {
    lerp(tz, lerp2(tx, ty, a, b, c, d), lerp2(tx, ty, e, f, g, h))
}
#[inline]
pub fn smoothstep(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}
#[inline]
pub fn clamped_lerp(a: f64, b: f64, t: f64) -> f64 {
    if t < 0.0 {
        a
    } else if t > 1.0 {
        b
    } else {
        lerp(t, a, b)
    }
}
#[inline]
pub fn floor(v: f64) -> i32 {
    v.floor() as i32
}
#[inline]
pub fn lfloor(v: f64) -> i64 {
    v.floor() as i64
}
#[inline]
pub fn wrap(v: f64) -> f64 {
    v - (lfloor(v / 33554432.0 + 0.5) as f64) * 33554432.0
}

#[derive(Clone)]
pub struct ImprovedNoise {
    p: [u8; 256],
    pub xo: f64,
    pub yo: f64,
    pub zo: f64,
}

impl ImprovedNoise {
    pub fn new<R: Rng>(r: &mut R) -> Self {
        let xo = r.next_double() * 256.0;
        let yo = r.next_double() * 256.0;
        let zo = r.next_double() * 256.0;
        let mut p = [0u8; 256];
        for (i, v) in p.iter_mut().enumerate() {
            *v = i as u8;
        }
        for i in 0..256 {
            let j = r.next_int_bounded(256 - i as i32) as usize;
            p.swap(i, i + j);
        }
        Self { p, xo, yo, zo }
    }
    #[inline]
    fn perm(&self, i: i32) -> i32 {
        self.p[(i & 255) as usize] as i32
    }

    pub fn noise(&self, x: f64, y: f64, z: f64, y_scale: f64, y_max: f64) -> f64 {
        let d = x + self.xo;
        let e = y + self.yo;
        let f = z + self.zo;
        let i = floor(d);
        let j = floor(e);
        let k = floor(f);
        let g = d - i as f64;
        let h = e - j as f64;
        let l = f - k as f64;
        let o = if y_scale != 0.0 {
            let m = if y_max >= 0.0 && y_max < h { y_max } else { h };
            (floor(m / y_scale + 1.0e-7_f32 as f64) as f64) * y_scale
        } else {
            0.0
        };
        self.sample_and_lerp(i, j, k, g, h - o, l, h)
    }

    #[inline]
    fn grad(hash: i32, x: f64, y: f64, z: f64) -> f64 {
        let g = &GRAD[(hash & 15) as usize];
        g[0] * x + g[1] * y + g[2] * z
    }

    #[allow(clippy::too_many_arguments)]
    fn sample_and_lerp(&self, i: i32, j: i32, k: i32, d: f64, e: f64, f: f64, g: f64) -> f64 {
        let l = self.perm(i);
        let m = self.perm(i + 1);
        let n = self.perm(l + j);
        let o = self.perm(l + j + 1);
        let p = self.perm(m + j);
        let q = self.perm(m + j + 1);
        let h = Self::grad(self.perm(n + k), d, e, f);
        let r = Self::grad(self.perm(p + k), d - 1.0, e, f);
        let s = Self::grad(self.perm(o + k), d, e - 1.0, f);
        let t = Self::grad(self.perm(q + k), d - 1.0, e - 1.0, f);
        let u = Self::grad(self.perm(n + k + 1), d, e, f - 1.0);
        let v = Self::grad(self.perm(p + k + 1), d - 1.0, e, f - 1.0);
        let w = Self::grad(self.perm(o + k + 1), d, e - 1.0, f - 1.0);
        let x = Self::grad(self.perm(q + k + 1), d - 1.0, e - 1.0, f - 1.0);
        let y = smoothstep(d);
        let z = smoothstep(g);
        let aa = smoothstep(f);
        lerp3(y, z, aa, h, r, s, t, u, v, w, x)
    }
}

/// Sum of octaves of improved noise.
#[derive(Clone)]
pub struct PerlinNoise {
    levels: Vec<Option<ImprovedNoise>>,
    amps: Vec<f64>,
    lowest_input: f64,
    lowest_value: f64,
}

impl PerlinNoise {
    /// Modern construction: each octave seeded from a positional fork by name.
    pub fn new(r: &mut Xoroshiro, first_octave: i32, amps: &[f64]) -> Self {
        let n = amps.len();
        let pf = r.fork_positional();
        let levels = (0..n)
            .map(|k| {
                if amps[k] != 0.0 {
                    Some(ImprovedNoise::new(
                        &mut pf.from_hash(&format!("octave_{}", first_octave + k as i32)),
                    ))
                } else {
                    None
                }
            })
            .collect();
        Self::finish(levels, amps, first_octave)
    }

    /// Modern construction over a legacy (java.util.Random) source: the positional fork
    /// seeds each octave with `String.hashCode("octave_N") ^ seed`.
    pub fn new_legacy_positional(
        r: &mut crate::random::JavaRandom,
        first_octave: i32,
        amps: &[f64],
    ) -> Self {
        let seed = r.next_long();
        let levels = (0..amps.len())
            .map(|k| {
                if amps[k] != 0.0 {
                    let h = java_hash(&format!("octave_{}", first_octave + k as i32)) as i64;
                    Some(ImprovedNoise::new(&mut crate::random::JavaRandom::new(
                        h ^ seed,
                    )))
                } else {
                    None
                }
            })
            .collect();
        Self::finish(levels, amps, first_octave)
    }

    /// Legacy construction (octaves drawn sequentially, highest frequency first).
    pub fn new_legacy<R: Rng>(r: &mut R, first_octave: i32, amps: &[f64]) -> Self {
        let n = amps.len() as i32;
        let j = -first_octave;
        let mut levels: Vec<Option<ImprovedNoise>> = vec![None; n as usize];
        let first = ImprovedNoise::new(r);
        if j >= 0 && j < n && amps[j as usize] != 0.0 {
            levels[j as usize] = Some(first);
        }
        for k in (0..j).rev() {
            if k < n && amps[k as usize] != 0.0 {
                levels[k as usize] = Some(ImprovedNoise::new(r));
            } else {
                r.consume(262);
            }
        }
        Self::finish(levels, amps, first_octave)
    }

    fn finish(levels: Vec<Option<ImprovedNoise>>, amps: &[f64], first_octave: i32) -> Self {
        let n = amps.len() as i32;
        Self {
            levels,
            amps: amps.to_vec(),
            lowest_input: 2f64.powi(first_octave),
            lowest_value: 2f64.powi(n - 1) / (2f64.powi(n) - 1.0),
        }
    }

    pub fn get(&self, x: f64, y: f64, z: f64) -> f64 {
        let mut d = 0.0;
        let mut e = self.lowest_input;
        let mut f = self.lowest_value;
        for (lv, a) in self.levels.iter().zip(&self.amps) {
            if let Some(n) = lv {
                let g = n.noise(wrap(x * e), wrap(y * e), wrap(z * e), 0.0, 0.0);
                d += a * g * f;
            }
            e *= 2.0;
            f /= 2.0;
        }
        d
    }

    /// Octave `i` counted from the highest frequency.
    pub fn octave(&self, i: usize) -> Option<&ImprovedNoise> {
        let n = self.levels.len();
        if i < n {
            self.levels[n - 1 - i].as_ref()
        } else {
            None
        }
    }
}

/// Parameters of a named noise (data/worldgen/noise/*.json).
#[derive(Clone, Debug)]
pub struct NoiseParams {
    pub first_octave: i32,
    pub amplitudes: Vec<f64>,
}

#[derive(Clone)]
pub struct NormalNoise {
    first: PerlinNoise,
    second: PerlinNoise,
    factor: f64,
}

impl NormalNoise {
    pub fn new(r: &mut Xoroshiro, p: &NoiseParams) -> Self {
        let first = PerlinNoise::new(r, p.first_octave, &p.amplitudes);
        let second = PerlinNoise::new(r, p.first_octave, &p.amplitudes);
        let mut lo = i32::MAX;
        let mut hi = i32::MIN;
        for (i, a) in p.amplitudes.iter().enumerate() {
            if *a != 0.0 {
                lo = lo.min(i as i32);
                hi = hi.max(i as i32);
            }
        }
        let octaves = hi - lo;
        let dev = 0.1 * (1.0 + 1.0 / (octaves + 1) as f64);
        Self {
            first,
            second,
            factor: 0.16666666666666666 / dev,
        }
    }

    #[inline]
    pub fn get(&self, x: f64, y: f64, z: f64) -> f64 {
        const K: f64 = 1.0181268882175227;
        (self.first.get(x, y, z) + self.second.get(x * K, y * K, z * K)) * self.factor
    }
}

/// java.lang.String#hashCode
pub fn java_hash(s: &str) -> i32 {
    s.encode_utf16()
        .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(c as i32))
}

impl NormalNoise {
    /// `NormalNoise.create` over a random of either flavour (legacy: octaves seeded by
    /// `String.hashCode("octave_N") ^ nextLong()`).
    pub fn new_any(r: &mut crate::random::AnyRng, p: &NoiseParams) -> Self {
        match r {
            crate::random::AnyRng::X(x) => Self::new(x, p),
            crate::random::AnyRng::J(j) => {
                let first = PerlinNoise::new_legacy_positional(j, p.first_octave, &p.amplitudes);
                let second = PerlinNoise::new_legacy_positional(j, p.first_octave, &p.amplitudes);
                Self::from_parts(first, second, &p.amplitudes)
            }
        }
    }

    /// The 1.16 nether biome noise: both halves built with the legacy octave order.
    pub fn new_legacy_nether_biome(r: &mut crate::random::JavaRandom, p: &NoiseParams) -> Self {
        let first = PerlinNoise::new_legacy(r, p.first_octave, &p.amplitudes);
        let second = PerlinNoise::new_legacy(r, p.first_octave, &p.amplitudes);
        Self::from_parts(first, second, &p.amplitudes)
    }

    /// Normal noise seeded the way block-state providers do (legacy random with a seed).
    pub fn new_seeded_legacy(seed: i64, p: &NoiseParams) -> Self {
        let mut r = crate::random::JavaRandom::new(seed);
        let first = PerlinNoise::new_legacy_positional(&mut r, p.first_octave, &p.amplitudes);
        let second = PerlinNoise::new_legacy_positional(&mut r, p.first_octave, &p.amplitudes);
        Self::from_parts(first, second, &p.amplitudes)
    }

    fn from_parts(first: PerlinNoise, second: PerlinNoise, amps: &[f64]) -> Self {
        let mut lo = i32::MAX;
        let mut hi = i32::MIN;
        for (i, a) in amps.iter().enumerate() {
            if *a != 0.0 {
                lo = lo.min(i as i32);
                hi = hi.max(i as i32);
            }
        }
        let octaves = hi - lo;
        let dev = 0.1 * (1.0 + 1.0 / (octaves + 1) as f64);
        Self {
            first,
            second,
            factor: 0.16666666666666666 / dev,
        }
    }
}

/// The legacy 3D terrain noise ("old_blended_noise").
#[derive(Clone)]
pub struct BlendedNoise {
    min_limit: PerlinNoise,
    max_limit: PerlinNoise,
    main: PerlinNoise,
    xz_mul: f64,
    y_mul: f64,
    xz_factor: f64,
    y_factor: f64,
    smear: f64,
}

impl BlendedNoise {
    pub fn new<R: Rng>(
        r: &mut R,
        xz_scale: f64,
        y_scale: f64,
        xz_factor: f64,
        y_factor: f64,
        smear_mult: f64,
    ) -> Self {
        let ones16 = [1.0; 16];
        let ones8 = [1.0; 8];
        let min_limit = PerlinNoise::new_legacy(r, -15, &ones16);
        let max_limit = PerlinNoise::new_legacy(r, -15, &ones16);
        let main = PerlinNoise::new_legacy(r, -7, &ones8);
        let y_mul = 684.412 * y_scale;
        Self {
            min_limit,
            max_limit,
            main,
            xz_mul: 684.412 * xz_scale,
            y_mul,
            xz_factor,
            y_factor,
            smear: y_mul * smear_mult,
        }
    }

    pub fn get(&self, x: f64, y: f64, z: f64) -> f64 {
        let d = x * self.xz_mul;
        let e = y * self.y_mul;
        let f = z * self.xz_mul;
        let g = d / self.xz_factor;
        let h = e / self.y_factor;
        let i = f / self.xz_factor;
        let j = self.smear;
        let k = j / self.y_factor;
        let mut l = 0.0;
        let mut m = 0.0;
        let mut n = 0.0;
        let mut o = 1.0;
        for p in 0..8 {
            if let Some(nz) = self.main.octave(p) {
                n += nz.noise(wrap(g * o), wrap(h * o), wrap(i * o), k * o, h * o) / o;
            }
            o /= 2.0;
        }
        let q = (n / 10.0 + 1.0) / 2.0;
        let above = q >= 1.0;
        let below = q <= 0.0;
        o = 1.0;
        for r in 0..16 {
            let s = wrap(d * o);
            let t = wrap(e * o);
            let u = wrap(f * o);
            let v = j * o;
            if !above {
                if let Some(nz) = self.min_limit.octave(r) {
                    l += nz.noise(s, t, u, v, e * o) / o;
                }
            }
            if !below {
                if let Some(nz) = self.max_limit.octave(r) {
                    m += nz.noise(s, t, u, v, e * o) / o;
                }
            }
            o /= 2.0;
        }
        clamped_lerp(l / 512.0, m / 512.0, q) / 128.0
    }
}

/// 2D simplex noise (used by the biome temperature variation).
#[derive(Clone)]
pub struct SimplexNoise {
    p: [i32; 512],
    pub xo: f64,
    pub yo: f64,
    pub zo: f64,
}

impl SimplexNoise {
    pub fn new<R: Rng>(r: &mut R) -> Self {
        let xo = r.next_double() * 256.0;
        let yo = r.next_double() * 256.0;
        let zo = r.next_double() * 256.0;
        let mut p = [0i32; 512];
        for (i, v) in p.iter_mut().enumerate().take(256) {
            *v = i as i32;
        }
        for i in 0..256 {
            let j = r.next_int_bounded(256 - i as i32) as usize;
            p.swap(i, i + j);
        }
        Self { p, xo, yo, zo }
    }
    fn perm(&self, i: i32) -> i32 {
        self.p[(i & 255) as usize]
    }
    fn corner(&self, idx: i32, x: f64, y: f64, z: f64, off: f64) -> f64 {
        let mut d = off - x * x - y * y - z * z;
        if d < 0.0 {
            0.0
        } else {
            d *= d;
            let g = &GRAD[(idx % 12) as usize];
            d * d * (g[0] * x + g[1] * y + g[2] * z)
        }
    }
    pub fn get_2d(&self, x: f64, y: f64) -> f64 {
        let sqrt3 = 3f64.sqrt();
        let f2 = 0.5 * (sqrt3 - 1.0);
        let g2 = (3.0 - sqrt3) / 6.0;
        let e = (x + y) * f2;
        let i = floor(x + e);
        let j = floor(y + e);
        let g = (i + j) as f64 * g2;
        let h = i as f64 - g;
        let k = j as f64 - g;
        let l = x - h;
        let m = y - k;
        let (n, o) = if l > m { (1, 0) } else { (0, 1) };
        let p = l - n as f64 + g2;
        let q = m - o as f64 + g2;
        let r = l - 1.0 + 2.0 * g2;
        let s = m - 1.0 + 2.0 * g2;
        let t = i & 255;
        let u = j & 255;
        let v = self.perm(t + self.perm(u)) % 12;
        let w = self.perm(t + n + self.perm(u + o)) % 12;
        let z = self.perm(t + 1 + self.perm(u + 1)) % 12;
        let aa = self.corner(v, l, m, 0.0, 0.5);
        let ab = self.corner(w, p, q, 0.0, 0.5);
        let ac = self.corner(z, r, s, 0.0, 0.5);
        70.0 * (aa + ab + ac)
    }
}

/// Octaves of simplex noise, legacy construction (used for biome temperature noise).
pub struct PerlinSimplexNoise {
    levels: Vec<Option<SimplexNoise>>,
    highest_freq_value: f64,
    highest_freq_input: f64,
}

impl PerlinSimplexNoise {
    /// `octaves` must be a contiguous list; only the [0] case is needed.
    pub fn new<R: Rng>(r: &mut R, octaves: &[i32]) -> Self {
        let lo = -*octaves.iter().min().unwrap();
        let hi = *octaves.iter().max().unwrap();
        let n = (lo + hi + 1) as usize;
        let first = SimplexNoise::new(r);
        let mut levels: Vec<Option<SimplexNoise>> = (0..n).map(|_| None).collect();
        let l = hi;
        if l >= 0 && (l as usize) < n && octaves.contains(&0) {
            levels[l as usize] = Some(first);
        }
        for m in (l + 1) as usize..n {
            if octaves.contains(&(l - m as i32)) {
                levels[m] = Some(SimplexNoise::new(r));
            } else {
                r.consume(262);
            }
        }
        Self {
            levels,
            highest_freq_input: 2f64.powi(hi),
            highest_freq_value: 1.0 / (2f64.powi(n as i32) - 1.0),
        }
    }
    pub fn get(&self, x: f64, y: f64, use_offsets: bool) -> f64 {
        let mut d = 0.0;
        let mut e = self.highest_freq_input;
        let mut f = self.highest_freq_value;
        for lv in &self.levels {
            if let Some(n) = lv {
                d += n.get_2d(
                    x * e + if use_offsets { n.xo } else { 0.0 },
                    y * e + if use_offsets { n.yo } else { 0.0 },
                ) * f;
            }
            e /= 2.0;
            f *= 2.0;
        }
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn improved_noise_zero_at_lattice() {
        let mut r = Xoroshiro::new(1);
        let n = ImprovedNoise::new(&mut r);
        // At integer lattice points (relative to the offset) the value is exactly 0.
        let v = n.noise(3.0 - n.xo, 5.0 - n.yo, 7.0 - n.zo, 0.0, 0.0);
        assert!(v.abs() < 1e-9, "{v}");
    }

    #[test]
    fn normal_noise_range() {
        let mut r = Xoroshiro::new(99);
        let p = NoiseParams {
            first_octave: -3,
            amplitudes: vec![1.0, 1.0, 1.0],
        };
        let n = NormalNoise::new(&mut r, &p);
        let mut maxv: f64 = 0.0;
        for i in 0..1000 {
            let v = n.get(i as f64 * 1.37, 2.0, i as f64 * 0.71);
            maxv = maxv.max(v.abs());
        }
        assert!(maxv > 0.1 && maxv < 2.0, "{maxv}");
    }
}

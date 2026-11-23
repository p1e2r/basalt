//! Random number generators used by world generation: Xoroshiro128++ (with the
//! SplitMix/Stafford "seed upgrade" and positional forking) and the classic Java
//! 48-bit linear congruential generator.

use md5::{Digest, Md5};

const GOLDEN: i64 = 0x9E37_79B9_7F4A_7C15u64 as i64;
const SILVER: i64 = 0x6A09_E667_F3BC_C909u64 as i64;

pub fn mix_stafford13(mut z: i64) -> i64 {
    z = (z ^ ((z as u64) >> 30) as i64).wrapping_mul(0xBF58_476D_1CE4_E5B9u64 as i64);
    z = (z ^ ((z as u64) >> 27) as i64).wrapping_mul(0x94D0_49BB_1331_11EBu64 as i64);
    z ^ ((z as u64) >> 31) as i64
}

/// Upgrade a 64-bit seed to a 128-bit Xoroshiro state (lo, hi).
pub fn upgrade_seed(seed: i64) -> (i64, i64) {
    let lo = seed ^ SILVER;
    let hi = lo.wrapping_add(GOLDEN);
    (mix_stafford13(lo), mix_stafford13(hi))
}

/// MD5 of a string, as two big-endian longs.
pub fn seed_from_hash(s: &str) -> (i64, i64) {
    let h = Md5::digest(s.as_bytes());
    let lo = i64::from_be_bytes(h[0..8].try_into().unwrap());
    let hi = i64::from_be_bytes(h[8..16].try_into().unwrap());
    (lo, hi)
}

/// Positional hash of a block position.
pub fn pos_seed(x: i32, y: i32, z: i32) -> i64 {
    let mut l = (x.wrapping_mul(3129871) as i64) ^ (z as i64).wrapping_mul(116129781) ^ (y as i64);
    l = l.wrapping_mul(l).wrapping_mul(42317861).wrapping_add(l.wrapping_mul(11));
    l >> 16
}

/// Common interface of the generators.
pub trait Rng {
    fn next_long(&mut self) -> i64;
    fn next_int(&mut self) -> i32;
    fn next_int_bounded(&mut self, bound: i32) -> i32;
    fn next_double(&mut self) -> f64;
    fn next_float(&mut self) -> f32;
    fn next_bool(&mut self) -> bool;
    fn consume(&mut self, n: usize);
    fn set_seed(&mut self, seed: i64);
    fn next_int_between(&mut self, a: i32, b: i32) -> i32 { self.next_int_bounded(b - a + 1) + a }
    fn next_int_between_exclusive(&mut self, a: i32, b: i32) -> i32 { a + self.next_int_bounded(b - a) }
}

#[derive(Clone, Debug)]
pub struct Xoroshiro {
    lo: i64,
    hi: i64,
}

impl Xoroshiro {
    pub fn new(seed: i64) -> Self {
        let (lo, hi) = upgrade_seed(seed);
        Self::from_state(lo, hi)
    }
    pub fn from_state(lo: i64, hi: i64) -> Self {
        if lo == 0 && hi == 0 { Self { lo: GOLDEN, hi: SILVER } } else { Self { lo, hi } }
    }
    #[inline]
    fn raw_next(&mut self) -> i64 {
        let s0 = self.lo;
        let mut s1 = self.hi;
        let r = s0.wrapping_add(s1).rotate_left(17).wrapping_add(s0);
        s1 ^= s0;
        self.lo = s0.rotate_left(49) ^ s1 ^ (s1 << 21);
        self.hi = s1.rotate_left(28);
        r
    }
    fn next_bits(&mut self, n: u32) -> u64 { (self.raw_next() as u64) >> (64 - n) }
    pub fn fork_positional(&mut self) -> PosFactory {
        let lo = self.next_long();
        let hi = self.next_long();
        PosFactory { lo, hi }
    }
}

impl Rng for Xoroshiro {
    fn next_long(&mut self) -> i64 { self.raw_next() }
    fn next_int(&mut self) -> i32 { self.raw_next() as i32 }
    fn next_int_bounded(&mut self, bound: i32) -> i32 {
        assert!(bound > 0);
        let b = bound as u64;
        let mut l = (self.next_int() as u32) as u64;
        let mut m = l.wrapping_mul(b);
        let mut low = m & 0xFFFF_FFFF;
        if low < b {
            let t = ((!bound).wrapping_add(1) as u32 % bound as u32) as u64;
            while low < t {
                l = (self.next_int() as u32) as u64;
                m = l.wrapping_mul(b);
                low = m & 0xFFFF_FFFF;
            }
        }
        (m >> 32) as i32
    }
    fn next_double(&mut self) -> f64 { self.next_bits(53) as f64 * 1.110_223e-16_f32 as f64 }
    fn next_float(&mut self) -> f32 { self.next_bits(24) as f32 * 5.960_464_5e-8_f32 }
    fn next_bool(&mut self) -> bool { self.raw_next() & 1 != 0 }
    fn consume(&mut self, n: usize) { for _ in 0..n { self.raw_next(); } }
    fn set_seed(&mut self, seed: i64) { *self = Xoroshiro::new(seed); }
}

/// Positional random factory (Xoroshiro flavour).
#[derive(Clone, Copy, Debug)]
pub struct PosFactory {
    pub lo: i64,
    pub hi: i64,
}

impl PosFactory {
    pub fn at(&self, x: i32, y: i32, z: i32) -> Xoroshiro { Xoroshiro::from_state(pos_seed(x, y, z) ^ self.lo, self.hi) }
    pub fn from_hash(&self, s: &str) -> Xoroshiro {
        let (a, b) = seed_from_hash(s);
        Xoroshiro::from_state(a ^ self.lo, b ^ self.hi)
    }
    pub fn from_seed(&self, seed: i64) -> Xoroshiro { Xoroshiro::from_state(seed ^ self.lo, self.hi) }
}

/// java.util.Random-compatible LCG.
#[derive(Clone, Debug)]
pub struct JavaRandom {
    seed: i64,
}

const MULT: i64 = 0x5_DEEC_E66D;
const MASK: i64 = (1 << 48) - 1;

impl JavaRandom {
    pub fn new(seed: i64) -> Self { Self { seed: (seed ^ MULT) & MASK } }
    #[inline]
    pub fn next(&mut self, bits: u32) -> i32 {
        self.seed = (self.seed.wrapping_mul(MULT).wrapping_add(11)) & MASK;
        (self.seed >> (48 - bits)) as i32
    }
}

impl Rng for JavaRandom {
    fn next_long(&mut self) -> i64 { ((self.next(32) as i64) << 32).wrapping_add(self.next(32) as i64) }
    fn next_int(&mut self) -> i32 { self.next(32) }
    fn next_int_bounded(&mut self, bound: i32) -> i32 {
        assert!(bound > 0);
        if bound & bound.wrapping_neg() == bound {
            return ((bound as i64 * self.next(31) as i64) >> 31) as i32;
        }
        loop {
            let i = self.next(31);
            let j = i % bound;
            if i.wrapping_sub(j).wrapping_add(bound - 1) >= 0 { return j; }
        }
    }
    fn next_double(&mut self) -> f64 {
        let a = self.next(26) as i64;
        let b = self.next(27) as i64;
        ((a << 27) + b) as f64 * (1.0 / (1u64 << 53) as f64)
    }
    fn next_float(&mut self) -> f32 { self.next(24) as f32 / (1 << 24) as f32 }
    fn next_bool(&mut self) -> bool { self.next(1) != 0 }
    fn consume(&mut self, n: usize) { for _ in 0..n { self.next(32); } }
    fn set_seed(&mut self, seed: i64) { *self = JavaRandom::new(seed); }
}

/// World-generation seeding helpers layered over any generator.
pub fn set_large_feature_seed<R: Rng>(r: &mut R, seed: i64, x: i32, z: i32) {
    r.set_seed(seed);
    let a = r.next_long();
    let b = r.next_long();
    r.set_seed((x as i64).wrapping_mul(a) ^ (z as i64).wrapping_mul(b) ^ seed);
}

pub fn set_decoration_seed<R: Rng>(r: &mut R, seed: i64, x: i32, z: i32) -> i64 {
    r.set_seed(seed);
    let a = r.next_long() | 1;
    let b = r.next_long() | 1;
    let s = ((x as i64).wrapping_mul(a).wrapping_add((z as i64).wrapping_mul(b))) ^ seed;
    r.set_seed(s);
    s
}

pub fn set_feature_seed<R: Rng>(r: &mut R, deco_seed: i64, index: i32, step: i32) {
    r.set_seed(deco_seed.wrapping_add(index as i64).wrapping_add(10000 * step as i64));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_random_matches_jdk() {
        // Well-known outputs of java.util.Random(0).
        let mut r = JavaRandom::new(0);
        assert_eq!(r.next_int(), -1155484576);
        assert_eq!(r.next_int(), -723955400);
    }

    #[test]
    fn xoroshiro_basic() {
        let mut a = Xoroshiro::new(12345);
        let mut b = Xoroshiro::new(12345);
        for _ in 0..10 { assert_eq!(a.next_long(), b.next_long()); }
        for _ in 0..1000 {
            let v = a.next_int_bounded(7);
            assert!((0..7).contains(&v));
            let d = a.next_double();
            assert!((0.0..1.0).contains(&d));
        }
    }

    #[test]
    fn md5_seed() {
        // md5("") = d41d8cd98f00b204e9800998ecf8427e
        let (a, b) = seed_from_hash("");
        assert_eq!(a as u64, 0xd41d8cd98f00b204);
        assert_eq!(b as u64, 0xe9800998ecf8427e);
    }
}

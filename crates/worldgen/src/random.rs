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

/// SHA-256 (used for the biome zoom seed).
pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 { msg.push(0); }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 { w[i] = u32::from_be_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap()); }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
        }
        for i in 0..8 { h[i] = h[i].wrapping_add(v[i]); }
    }
    let mut out = [0u8; 32];
    for i in 0..8 { out[i * 4..i * 4 + 4].copy_from_slice(&h[i].to_be_bytes()); }
    out
}

/// Seed used by the biome "zoom" (hash of the world seed).
pub fn biome_zoom_seed(seed: i64) -> i64 {
    let h = sha256(&seed.to_le_bytes());
    i64::from_le_bytes(h[0..8].try_into().unwrap())
}

#[inline]
pub fn lcg_next(a: i64, b: i64) -> i64 {
    a.wrapping_mul(a.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407)).wrapping_add(b)
}

#[cfg(test)]
mod tests {
    #[test]
    fn sha256_abc() {
        let h = sha256(b"abc");
        assert_eq!(h[0..4], [0xba, 0x78, 0x16, 0xbf]);
        assert_eq!(h[28..32], [0xf2, 0x00, 0x15, 0xad]);
    }

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

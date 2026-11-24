use anyhow::{bail, Result};
use basalt_nbt::Tag;
use uuid::Uuid;

#[derive(Default, Clone)]
pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(64),
        }
    }
    pub fn with_id(id: i32) -> Self {
        let mut w = Self::new();
        w.varint(id);
        w
    }
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }
    pub fn i8(&mut self, v: i8) -> &mut Self {
        self.buf.push(v as u8);
        self
    }
    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.buf.push(v as u8);
        self
    }
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn i16(&mut self, v: i16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn i64(&mut self, v: i64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn f32(&mut self, v: f32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn f64(&mut self, v: f64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn varint(&mut self, v: i32) -> &mut Self {
        let mut v = v as u32;
        loop {
            if v & !0x7F == 0 {
                self.buf.push(v as u8);
                return self;
            }
            self.buf.push((v & 0x7F | 0x80) as u8);
            v >>= 7;
        }
    }
    pub fn varlong(&mut self, v: i64) -> &mut Self {
        let mut v = v as u64;
        loop {
            if v & !0x7F == 0 {
                self.buf.push(v as u8);
                return self;
            }
            self.buf.push((v & 0x7F | 0x80) as u8);
            v >>= 7;
        }
    }
    pub fn str(&mut self, s: &str) -> &mut Self {
        self.varint(s.len() as i32);
        self.buf.extend_from_slice(s.as_bytes());
        self
    }
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(b);
        self
    }
    pub fn byte_array(&mut self, b: &[u8]) -> &mut Self {
        self.varint(b.len() as i32);
        self.bytes(b)
    }
    pub fn uuid(&mut self, u: Uuid) -> &mut Self {
        self.buf.extend_from_slice(u.as_bytes());
        self
    }
    pub fn nbt(&mut self, t: &Tag) -> &mut Self {
        basalt_nbt::write_network(&mut self.buf, t);
        self
    }
    pub fn position(&mut self, x: i32, y: i32, z: i32) -> &mut Self {
        let v =
            ((x as i64 & 0x3FFFFFF) << 38) | ((z as i64 & 0x3FFFFFF) << 12) | (y as i64 & 0xFFF);
        self.i64(v)
    }
    /// Low-precision vector (entity velocity in add_entity / set_entity_motion): 15 bits per axis
    /// scaled by a shared integer, 6 bytes (+ varint when the scale exceeds 3), or a single 0 byte.
    pub fn lpvec3(&mut self, v: [f64; 3]) -> &mut Self {
        let max = v.iter().fold(0.0f64, |m, c| m.max(c.abs()));
        if !(max >= 3.0517578125e-5) {
            return self.u8(0);
        }
        let scale = max.ceil() as u64;
        let more = scale > 3;
        let pack = |c: f64| -> u64 {
            (((c / scale as f64) * 0.5 + 0.5) * 32766.0)
                .round()
                .clamp(0.0, 32766.0) as u64
        };
        let bits = (scale & 3)
            | (more as u64) << 2
            | pack(v[0]) << 3
            | pack(v[1]) << 18
            | pack(v[2]) << 33;
        self.u8((bits & 0xff) as u8)
            .u8((bits >> 8 & 0xff) as u8)
            .bytes(&((bits >> 16) as u32).to_be_bytes());
        if more {
            self.varint((scale >> 2) as i32);
        }
        self
    }
    pub fn angle(&mut self, deg: f32) -> &mut Self {
        self.u8(((deg / 360.0 * 256.0) as i32 & 0xFF) as u8)
    }
    pub fn len(&self) -> usize {
        self.buf.len()
    }
}

pub fn varint_len(v: i32) -> usize {
    let mut w = Writer::new();
    w.varint(v);
    w.buf.len()
}

pub struct Reader<'a> {
    pub buf: &'a [u8],
    pub pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }
    pub fn rest(&mut self) -> &'a [u8] {
        let s = &self.buf[self.pos..];
        self.pos = self.buf.len();
        s
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos + n > self.buf.len() {
            bail!(
                "unexpected end of packet (need {n}, have {})",
                self.remaining()
            )
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn i8(&mut self) -> Result<i8> {
        Ok(self.u8()? as i8)
    }
    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into()?))
    }
    pub fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_be_bytes(self.take(2)?.try_into()?))
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into()?))
    }
    pub fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_be_bytes(self.take(8)?.try_into()?))
    }
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_be_bytes(self.take(4)?.try_into()?))
    }
    pub fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_be_bytes(self.take(8)?.try_into()?))
    }
    pub fn varint(&mut self) -> Result<i32> {
        let mut v: u32 = 0;
        for i in 0..5 {
            let b = self.u8()?;
            v |= ((b & 0x7F) as u32) << (7 * i);
            if b & 0x80 == 0 {
                return Ok(v as i32);
            }
        }
        bail!("varint too long")
    }
    pub fn varlong(&mut self) -> Result<i64> {
        let mut v: u64 = 0;
        for i in 0..10 {
            let b = self.u8()?;
            v |= ((b & 0x7F) as u64) << (7 * i);
            if b & 0x80 == 0 {
                return Ok(v as i64);
            }
        }
        bail!("varlong too long")
    }
    pub fn str(&mut self) -> Result<String> {
        let n = self.varint()?;
        if n < 0 || n > 1 << 21 {
            bail!("bad string length {n}")
        }
        Ok(String::from_utf8_lossy(self.take(n as usize)?).into_owned())
    }
    pub fn byte_array(&mut self) -> Result<&'a [u8]> {
        let n = self.varint()?;
        if n < 0 {
            bail!("neg len")
        }
        self.take(n as usize)
    }
    pub fn uuid(&mut self) -> Result<Uuid> {
        Ok(Uuid::from_bytes(self.take(16)?.try_into()?))
    }
    pub fn nbt(&mut self) -> Result<Tag> {
        let mut r = basalt_nbt::Reader::new(&self.buf[self.pos..]);
        let t = r.network()?;
        self.pos += r.pos;
        Ok(t)
    }
    pub fn position(&mut self) -> Result<(i32, i32, i32)> {
        let v = self.i64()?;
        Ok((
            (v >> 38) as i32,
            (v << 52 >> 52) as i32,
            (v << 26 >> 38) as i32,
        ))
    }
}

#[cfg(test)]
mod lpvec3_tests {
    use super::Writer;
    #[test]
    fn matches_vanilla_samples() {
        // bytes captured from a vanilla 26.2 server for dropped-item velocities
        let enc = |v: [f64; 3]| {
            let mut w = Writer::new();
            w.lpvec3(v);
            w.buf
        };
        assert_eq!(enc([0.0, 0.0, 0.0]), vec![0]);
        let b = enc([-0.0025, 0.0, -0.00317]);
        assert_eq!(b.len(), 6);
        let dec = |b: &[u8]| {
            let bits = b[0] as u64
                | (b[1] as u64) << 8
                | (u32::from_be_bytes([b[2], b[3], b[4], b[5]]) as u64) << 16;
            let u =
                |x: u64| ((x & 0x7fff).min(32766) as f64 * 2.0 / 32766.0 - 1.0) * (bits & 3) as f64;
            [u(bits >> 3), u(bits >> 18), u(bits >> 33)]
        };
        let v = dec(&hex("41fd7f233331"));
        let round = enc(v);
        assert_eq!(round, hex("41fd7f233331"));
        let big = enc([0.0, -5.5, 0.0]);
        assert_eq!(big.len(), 7);
    }
    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }
}

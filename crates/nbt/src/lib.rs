//! NBT (Named Binary Tag) encoding/decoding, including the "network" form
//! (nameless root compound) used by the protocol since 1.20.2.

use anyhow::{bail, Result};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    End,
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    List(Vec<Tag>),
    Compound(Compound),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

pub type Compound = BTreeMap<String, Tag>;

impl Tag {
    pub fn id(&self) -> u8 {
        match self {
            Tag::End => 0, Tag::Byte(_) => 1, Tag::Short(_) => 2, Tag::Int(_) => 3,
            Tag::Long(_) => 4, Tag::Float(_) => 5, Tag::Double(_) => 6, Tag::ByteArray(_) => 7,
            Tag::String(_) => 8, Tag::List(_) => 9, Tag::Compound(_) => 10,
            Tag::IntArray(_) => 11, Tag::LongArray(_) => 12,
        }
    }
    pub fn as_compound(&self) -> Option<&Compound> { if let Tag::Compound(c) = self { Some(c) } else { None } }
    pub fn as_list(&self) -> Option<&Vec<Tag>> { if let Tag::List(c) = self { Some(c) } else { None } }
    pub fn as_str(&self) -> Option<&str> { if let Tag::String(c) = self { Some(c) } else { None } }
    pub fn as_i64(&self) -> Option<i64> {
        Some(match self { Tag::Byte(v) => *v as i64, Tag::Short(v) => *v as i64, Tag::Int(v) => *v as i64, Tag::Long(v) => *v, _ => return None })
    }
    pub fn as_f64(&self) -> Option<f64> {
        Some(match self { Tag::Float(v) => *v as f64, Tag::Double(v) => *v, _ => return self.as_i64().map(|v| v as f64) })
    }
    pub fn get(&self, k: &str) -> Option<&Tag> { self.as_compound()?.get(k) }

    /// Convert from JSON (as found in vanilla data packs). Numbers become Int/Double,
    /// booleans become Byte; arrays become lists.
    pub fn from_json(v: &serde_json::Value) -> Tag {
        use serde_json::Value as V;
        match v {
            V::Null => Tag::End,
            V::Bool(b) => Tag::Byte(*b as i8),
            V::Number(n) => {
                if let Some(i) = n.as_i64() {
                    if i >= i32::MIN as i64 && i <= i32::MAX as i64 { Tag::Int(i as i32) } else { Tag::Long(i) }
                } else { Tag::Double(n.as_f64().unwrap_or(0.0)) }
            }
            V::String(s) => Tag::String(s.clone()),
            V::Array(a) => {
                let items: Vec<Tag> = a.iter().map(Tag::from_json).collect();
                // Lists must be homogeneous; if mixed, wrap each in a compound with "" key.
                if items.windows(2).all(|w| w[0].id() == w[1].id()) {
                    // promote mixed int/double? keep as-is
                    Tag::List(items)
                } else if items.iter().all(|t| matches!(t, Tag::Int(_) | Tag::Double(_) | Tag::Long(_))) {
                    Tag::List(items.iter().map(|t| Tag::Double(t.as_f64().unwrap())).collect())
                } else {
                    Tag::List(items.into_iter().map(|t| { let mut c = Compound::new(); c.insert(String::new(), t); Tag::Compound(c) }).collect())
                }
            }
            V::Object(o) => Tag::Compound(o.iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k.clone(), Tag::from_json(v))).collect()),
        }
    }
}

// ---------- writing ----------

fn write_str(out: &mut Vec<u8>, s: &str) {
    let b = mutf8_encode(s);
    out.extend_from_slice(&(b.len() as u16).to_be_bytes());
    out.extend_from_slice(&b);
}

fn mutf8_encode(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for ch in s.encode_utf16() {
        let c = ch as u32;
        if c != 0 && c < 0x80 { out.push(c as u8); }
        else if c < 0x800 { out.push((0xC0 | (c >> 6)) as u8); out.push((0x80 | (c & 0x3F)) as u8); }
        else { out.push((0xE0 | (c >> 12)) as u8); out.push((0x80 | ((c >> 6) & 0x3F)) as u8); out.push((0x80 | (c & 0x3F)) as u8); }
    }
    out
}

fn mutf8_decode(b: &[u8]) -> String {
    let mut units = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c & 0x80 == 0 { units.push(c as u16); i += 1; }
        else if c & 0xE0 == 0xC0 && i + 1 < b.len() { units.push((((c & 0x1F) as u16) << 6) | (b[i + 1] & 0x3F) as u16); i += 2; }
        else if i + 2 < b.len() { units.push((((c & 0x0F) as u16) << 12) | (((b[i + 1] & 0x3F) as u16) << 6) | (b[i + 2] & 0x3F) as u16); i += 3; }
        else { i += 1; }
    }
    String::from_utf16_lossy(&units)
}

pub fn write_payload(out: &mut Vec<u8>, t: &Tag) {
    match t {
        Tag::End => {}
        Tag::Byte(v) => out.push(*v as u8),
        Tag::Short(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Int(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Long(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Float(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Double(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::ByteArray(v) => { out.extend_from_slice(&(v.len() as i32).to_be_bytes()); out.extend(v.iter().map(|b| *b as u8)); }
        Tag::String(s) => write_str(out, s),
        Tag::List(items) => {
            let id = items.first().map(|t| t.id()).unwrap_or(0);
            out.push(id);
            out.extend_from_slice(&(items.len() as i32).to_be_bytes());
            for i in items { write_payload(out, i); }
        }
        Tag::Compound(c) => {
            for (k, v) in c { out.push(v.id()); write_str(out, k); write_payload(out, v); }
            out.push(0);
        }
        Tag::IntArray(v) => { out.extend_from_slice(&(v.len() as i32).to_be_bytes()); for x in v { out.extend_from_slice(&x.to_be_bytes()); } }
        Tag::LongArray(v) => { out.extend_from_slice(&(v.len() as i32).to_be_bytes()); for x in v { out.extend_from_slice(&x.to_be_bytes()); } }
    }
}

/// Network NBT: type id + payload, no root name.
pub fn write_network(out: &mut Vec<u8>, t: &Tag) { out.push(t.id()); write_payload(out, t); }

/// File NBT: type id + name + payload.
pub fn write_named(out: &mut Vec<u8>, name: &str, t: &Tag) { out.push(t.id()); write_str(out, name); write_payload(out, t); }

// ---------- reading ----------

pub struct Reader<'a> { pub buf: &'a [u8], pub pos: usize }

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self { Self { buf, pos: 0 } }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos + n > self.buf.len() { bail!("nbt: unexpected eof") }
        let s = &self.buf[self.pos..self.pos + n]; self.pos += n; Ok(s)
    }
    fn u8(&mut self) -> Result<u8> { Ok(self.take(1)?[0]) }
    fn i16(&mut self) -> Result<i16> { Ok(i16::from_be_bytes(self.take(2)?.try_into()?)) }
    fn i32(&mut self) -> Result<i32> { Ok(i32::from_be_bytes(self.take(4)?.try_into()?)) }
    fn i64(&mut self) -> Result<i64> { Ok(i64::from_be_bytes(self.take(8)?.try_into()?)) }
    fn string(&mut self) -> Result<String> { let n = self.i16()? as u16 as usize; Ok(mutf8_decode(self.take(n)?)) }
    fn len(&mut self) -> Result<usize> { let n = self.i32()?; if n < 0 { bail!("nbt: negative length") } Ok(n as usize) }

    pub fn payload(&mut self, id: u8, depth: u32) -> Result<Tag> {
        if depth > 512 { bail!("nbt: too deep") }
        Ok(match id {
            0 => Tag::End,
            1 => Tag::Byte(self.u8()? as i8),
            2 => Tag::Short(self.i16()?),
            3 => Tag::Int(self.i32()?),
            4 => Tag::Long(self.i64()?),
            5 => Tag::Float(f32::from_bits(self.i32()? as u32)),
            6 => Tag::Double(f64::from_bits(self.i64()? as u64)),
            7 => { let n = self.len()?; Tag::ByteArray(self.take(n)?.iter().map(|b| *b as i8).collect()) }
            8 => Tag::String(self.string()?),
            9 => {
                let el = self.u8()?; let n = self.len()?;
                let mut v = Vec::with_capacity(n.min(65536));
                for _ in 0..n { v.push(self.payload(el, depth + 1)?); }
                Tag::List(v)
            }
            10 => {
                let mut c = Compound::new();
                loop {
                    let t = self.u8()?;
                    if t == 0 { break; }
                    let k = self.string()?;
                    c.insert(k, self.payload(t, depth + 1)?);
                }
                Tag::Compound(c)
            }
            11 => { let n = self.len()?; let mut v = Vec::with_capacity(n.min(65536)); for _ in 0..n { v.push(self.i32()?); } Tag::IntArray(v) }
            12 => { let n = self.len()?; let mut v = Vec::with_capacity(n.min(65536)); for _ in 0..n { v.push(self.i64()?); } Tag::LongArray(v) }
            _ => bail!("nbt: bad tag id {id}"),
        })
    }
    pub fn network(&mut self) -> Result<Tag> { let id = self.u8()?; self.payload(id, 0) }
    pub fn named(&mut self) -> Result<(String, Tag)> {
        let id = self.u8()?; if id == 0 { return Ok((String::new(), Tag::End)); }
        let n = self.string()?; Ok((n, self.payload(id, 0)?))
    }
}

/// Helper macro-free builder.
pub fn compound<const N: usize>(items: [(&str, Tag); N]) -> Tag {
    Tag::Compound(items.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

/// Plain text component as NBT.
pub fn text(s: &str) -> Tag { Tag::String(s.to_string()) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let t = compound([("a", Tag::Int(5)), ("b", Tag::List(vec![Tag::String("x".into())])), ("c", Tag::LongArray(vec![1, -2]))]);
        let mut b = Vec::new(); write_network(&mut b, &t);
        assert_eq!(Reader::new(&b).network().unwrap(), t);
    }
}

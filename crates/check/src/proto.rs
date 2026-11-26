//! Wire helpers the bot needs beyond basalt-protocol: text flattening, clientbound item stacks,
//! entity data, hashed stacks for container_click, and an SNBT reader for vanilla command replies.
use anyhow::{anyhow, bail, Result};
use basalt_nbt::Tag;
use basalt_protocol::{Reader, Writer};
use serde_json::{json, Map, Value};

// ------------------------------------------------------------------ text components

/// A received chat/system line: flattened text plus the top-level translation key and its args.
#[derive(Clone, Debug, Default)]
pub struct Text {
    pub plain: String,
    pub key: Option<String>,
    pub args: Vec<String>,
}

fn scalar(t: &Tag) -> String {
    match t {
        Tag::String(s) => s.clone(),
        Tag::Byte(v) => v.to_string(),
        Tag::Short(v) => v.to_string(),
        Tag::Int(v) => v.to_string(),
        Tag::Long(v) => v.to_string(),
        Tag::Float(v) => v.to_string(),
        Tag::Double(v) => v.to_string(),
        other => flatten(other),
    }
}

/// Concatenate all literal text in a component; translations render as `key(arg, ...)`.
pub fn flatten(t: &Tag) -> String {
    match t {
        Tag::Compound(c) => {
            let mut s = String::new();
            if let Some(x) = c.get("text").or_else(|| c.get("")) {
                s.push_str(&scalar(x));
            }
            if let Some(Tag::String(k)) = c.get("translate") {
                let args: Vec<String> = match c.get("with") {
                    Some(Tag::List(l)) => l.iter().map(flatten).collect(),
                    _ => Vec::new(),
                };
                s.push_str(k);
                if !args.is_empty() {
                    s.push('(');
                    s.push_str(&args.join(", "));
                    s.push(')');
                }
            }
            if let Some(Tag::List(ex)) = c.get("extra") {
                for e in ex {
                    s.push_str(&flatten(e));
                }
            }
            s
        }
        Tag::List(l) => l.iter().map(flatten).collect(),
        other => scalar(other),
    }
}

pub fn text_of(t: &Tag) -> Text {
    // errors arrive wrapped ({"text": "", "extra": [{"translate": ...}], "color": "red"}): use the first translation
    if let Tag::Compound(c) = t {
        if c.get("translate").is_none() {
            if let Some(Tag::List(ex)) = c.get("extra") {
                if let Some(inner) = ex
                    .iter()
                    .find(|e| matches!(e, Tag::Compound(x) if x.contains_key("translate")))
                {
                    let mut x = text_of(inner);
                    x.plain = flatten(t);
                    return x;
                }
            }
        }
    }
    let (key, args) = match t {
        Tag::Compound(c) => match c.get("translate") {
            Some(Tag::String(k)) => (
                Some(k.clone()),
                match c.get("with") {
                    Some(Tag::List(l)) => l.iter().map(flatten).collect(),
                    _ => Vec::new(),
                },
            ),
            _ => (None, Vec::new()),
        },
        _ => (None, Vec::new()),
    };
    Text {
        plain: flatten(t),
        key,
        args,
    }
}

// ------------------------------------------------------------------ item stacks

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stack {
    pub item: i32,
    pub count: i32,
    pub damage: i32,
    /// (enchantment registry id, level)
    pub ench: Vec<(i32, i32)>,
    /// repair_cost, custom name, potion registry id (-1 none), trim (material, pattern) holder ids (0 none)
    pub repair: i32,
    pub name: String,
    pub potion: i32,
    pub trim: (i32, i32),
    /// charged_projectiles (loaded crossbow): item ids
    pub charged: Vec<i32>,
}

impl Stack {
    pub fn is_empty(&self) -> bool {
        self.count <= 0
    }
    pub fn name(&self) -> &'static str {
        if self.count <= 0 {
            "air"
        } else {
            basalt_data::registries::ITEM
                .get(self.item as usize)
                .copied()
                .unwrap_or("?")
        }
    }
}

fn component_id(name: &str) -> i32 {
    basalt_data::registry_id(basalt_data::registries::DATA_COMPONENT_TYPE, name).unwrap_or(-1)
}

/// Clientbound item stack (no length prefix on component values: only known types can be read).
pub fn read_slot(r: &mut Reader) -> Result<Stack> {
    let count = r.varint()?;
    if count <= 0 {
        return Ok(Stack::default());
    }
    let item = r.varint()?;
    let (add, rem) = (r.varint()?, r.varint()?);
    let mut s = Stack {
        item,
        count,
        potion: -1,
        ..Default::default()
    };
    for _ in 0..add {
        let ty = r.varint()?;
        if ty == component_id("damage") {
            s.damage = r.varint()?;
        } else if ty == component_id("enchantments") || ty == component_id("stored_enchantments") {
            let n = r.varint()?;
            for _ in 0..n {
                let e = r.varint()?;
                let l = r.varint()?;
                s.ench.push((e, l));
            }
        } else if ty == component_id("repair_cost") {
            s.repair = r.varint()?;
        } else if ty == component_id("max_damage") || ty == component_id("max_stack_size") {
            r.varint()?;
        } else if ty == component_id("custom_name") {
            s.name = text_of(&r.nbt()?).plain;
        } else if ty == component_id("item_name") {
            r.nbt()?;
        } else if ty == component_id("potion_contents") {
            if r.bool()? {
                s.potion = r.varint()?;
            }
            if r.bool()? {
                r.i32()?;
            }
            let n = r.varint()?;
            if n != 0 {
                bail!("potion custom effects unsupported");
            }
            if r.bool()? {
                r.str()?;
            }
        } else if ty == component_id("trim") {
            let m = r.varint()?;
            let p = r.varint()?;
            s.trim = (m, p);
        } else if ty == component_id("charged_projectiles") {
            let n = r.varint()?;
            for _ in 0..n {
                let c = read_slot(r)?;
                s.charged.push(c.item);
            }
        } else if ty == component_id("unbreakable") || ty == component_id("glider") {
        } else {
            bail!("unknown component type {ty} on {}", s.name());
        }
    }
    for _ in 0..rem {
        r.varint()?;
    }
    Ok(s)
}

/// Hashed stack for container_click: optional {item, count, added component hashes, removed}.
/// We never claim component patches; vanilla treats a mismatch as "resync", which is what we want.
pub fn write_hashed(w: &mut Writer, s: &Stack) {
    if s.count <= 0 {
        w.bool(false);
        return;
    }
    w.bool(true)
        .varint(s.item)
        .varint(s.count)
        .varint(0)
        .varint(0);
}

// ------------------------------------------------------------------ entity data

#[derive(Clone, Debug, PartialEq)]
pub enum Meta {
    Byte(u8),
    Int(i32),
    Long(i64),
    Float(f32),
    Str(String),
    Text(String),
    Item(Stack),
    Bool(bool),
    Other,
}

/// Parse set_entity_data entries after the entity id. Stops (keeping what was read) at a
/// serializer we can't size.
pub fn read_meta(r: &mut Reader) -> Vec<(u8, Meta)> {
    let mut out = Vec::new();
    loop {
        let Ok(idx) = r.u8() else { break };
        if idx == 0xff {
            break;
        }
        let Ok(ser) = r.varint() else { break };
        let v: Result<Meta> = (|| {
            Ok(match ser {
                0 => Meta::Byte(r.u8()?),
                1 => Meta::Int(r.varint()?),
                2 => Meta::Long(r.varlong()?),
                3 => Meta::Float(r.f32()?),
                4 => Meta::Str(r.str()?),
                5 => Meta::Text(flatten(&r.nbt()?)),
                6 => {
                    if r.bool()? {
                        Meta::Text(flatten(&r.nbt()?))
                    } else {
                        Meta::Other
                    }
                }
                7 => Meta::Item(read_slot(r)?),
                8 => Meta::Bool(r.bool()?),
                9 => {
                    r.f32()?;
                    r.f32()?;
                    r.f32()?;
                    Meta::Other
                }
                10 => {
                    r.i64()?;
                    Meta::Other
                }
                11 => {
                    if r.bool()? {
                        r.i64()?;
                    }
                    Meta::Other
                }
                12 => {
                    r.varint()?;
                    Meta::Other
                }
                13 => {
                    if r.bool()? {
                        r.uuid()?;
                    }
                    Meta::Other
                }
                14 | 15 => {
                    r.varint()?;
                    Meta::Other
                }
                _ => return Err(anyhow!("serializer {ser}")),
            })
        })();
        match v {
            Ok(m) => out.push((idx, m)),
            Err(_) => break,
        }
    }
    out
}

/// LpVec3 (entity velocity) decode; see NOTES.md.
pub fn read_lpvec3(r: &mut Reader) -> Result<[f64; 3]> {
    let b0 = r.u8()?;
    if b0 == 0 {
        return Ok([0.0; 3]);
    }
    let b1 = r.u8()?;
    let rest = r.take(4)?;
    let bits = b0 as u64 | (b1 as u64) << 8 | (u32::from_be_bytes(rest.try_into()?) as u64) << 16;
    let mut scale = bits & 3;
    if bits & 4 != 0 {
        scale |= (r.varint()? as u32 as u64) << 2;
    }
    let u = |x: u64| ((x & 0x7fff).min(32766) as f64 * 2.0 / 32766.0 - 1.0) * scale as f64;
    Ok([u(bits >> 3), u(bits >> 18), u(bits >> 33)])
}

// ------------------------------------------------------------------ SNBT (vanilla `data get` replies)

/// Parse SNBT text into JSON (numbers lose their type suffix; arrays become lists).
pub fn parse_snbt(s: &str) -> Option<Value> {
    let mut p = Snbt {
        b: s.as_bytes(),
        i: 0,
    };
    let v = p.value()?;
    p.ws();
    Some(v)
}

struct Snbt<'a> {
    b: &'a [u8],
    i: usize,
}

impl Snbt<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && (self.b[self.i] as char).is_whitespace() {
            self.i += 1;
        }
    }
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }
    fn quoted(&mut self) -> Option<String> {
        let q = self.peek()?;
        self.i += 1;
        let mut out = Vec::new();
        while let Some(c) = self.peek() {
            self.i += 1;
            if c == b'\\' {
                if let Some(n) = self.peek() {
                    out.push(n);
                    self.i += 1;
                }
                continue;
            }
            if c == q {
                return String::from_utf8(out).ok();
            }
            out.push(c);
        }
        None
    }
    fn bare(&mut self) -> String {
        let st = self.i;
        while let Some(c) = self.peek() {
            if matches!(c, b',' | b']' | b'}' | b':' | b';') || (c as char).is_whitespace() {
                break;
            }
            self.i += 1;
        }
        String::from_utf8_lossy(&self.b[st..self.i]).into_owned()
    }
    fn value(&mut self) -> Option<Value> {
        self.ws();
        match self.peek()? {
            b'{' => {
                self.i += 1;
                let mut m = Map::new();
                loop {
                    self.ws();
                    if self.peek()? == b'}' {
                        self.i += 1;
                        break;
                    }
                    let k = if matches!(self.peek()?, b'"' | b'\'') {
                        self.quoted()?
                    } else {
                        self.bare()
                    };
                    self.ws();
                    if self.peek()? != b':' {
                        return None;
                    }
                    self.i += 1;
                    let v = self.value()?;
                    m.insert(k, v);
                    self.ws();
                    match self.peek()? {
                        b',' => {
                            self.i += 1;
                        }
                        b'}' => {
                            self.i += 1;
                            break;
                        }
                        _ => return None,
                    }
                }
                Some(Value::Object(m))
            }
            b'[' => {
                self.i += 1;
                self.ws();
                // typed arrays: [I; 1, 2]
                if self.b.len() > self.i + 1
                    && self.b[self.i + 1] == b';'
                    && matches!(self.b[self.i], b'B' | b'I' | b'L')
                {
                    self.i += 2;
                }
                let mut v = Vec::new();
                loop {
                    self.ws();
                    if self.peek()? == b']' {
                        self.i += 1;
                        break;
                    }
                    v.push(self.value()?);
                    self.ws();
                    match self.peek()? {
                        b',' => {
                            self.i += 1;
                        }
                        b']' => {
                            self.i += 1;
                            break;
                        }
                        _ => return None,
                    }
                }
                Some(Value::Array(v))
            }
            b'"' | b'\'' => self.quoted().map(Value::String),
            _ => {
                let t = self.bare();
                if t == "true" {
                    return Some(json!(true));
                }
                if t == "false" {
                    return Some(json!(false));
                }
                let num = t.trim_end_matches(['b', 'B', 's', 'S', 'l', 'L', 'f', 'F', 'd', 'D']);
                match num.parse::<f64>() {
                    Ok(n) if !num.is_empty() => Some(json!(n)),
                    _ => Some(Value::String(t)),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snbt() {
        assert_eq!(parse_snbt("20.0f"), Some(json!(20.0)));
        assert_eq!(parse_snbt("-20s"), Some(json!(-20.0)));
        let v = parse_snbt(
            r#"[{count: 1, Slot: 0b, id: "minecraft:stone", components: {"minecraft:damage": 5}}]"#,
        )
        .unwrap();
        assert_eq!(v[0]["id"], json!("minecraft:stone"));
        assert_eq!(v[0]["components"]["minecraft:damage"], json!(5.0));
        assert_eq!(parse_snbt("[I; 1, 2]"), Some(json!([1.0, 2.0])));
        assert_eq!(
            parse_snbt("[0.5d, 100.0d, 0.5d]"),
            Some(json!([0.5, 100.0, 0.5]))
        );
    }
    #[test]
    fn lpvec3_roundtrip() {
        for v in [[0.1, 0.2, -0.1], [0.0, -5.5, 0.0], [1.0, 0.0, 0.25]] {
            let mut w = Writer::new();
            w.lpvec3(v);
            let d = read_lpvec3(&mut Reader::new(&w.buf)).unwrap();
            for i in 0..3 {
                assert!(
                    (d[i] - v[i]).abs() < 1e-3 * v[i].abs().max(1.0) * 6.0,
                    "{v:?} -> {d:?}"
                );
            }
        }
    }
}

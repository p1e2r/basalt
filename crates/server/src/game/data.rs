//! `/data get entity|block`: the entity / block-entity NBT, an NBT path into it, and vanilla's SNBT
//! text. Reply forms measured on vanilla 26.2 (2026-11-24): `commands.data.entity.query` (name,
//! SNBT), `commands.data.entity.get` (path, name, value*scale "%.2f", floor), numbers with type
//! suffixes (`20.0f`, `201.0d`, `1b`), lists `[a, b]`, missing paths `arguments.nbtpath.nothing_found`.
use super::{err, s, Target, R};
use crate::cmdparse::Parsed;
use crate::game::Game;
use basalt_nbt::{Compound, Tag};
use basalt_protocol::text;

/// Vanilla-style SNBT.
pub fn snbt(t: &Tag) -> String {
    fn num<T: std::fmt::Debug>(v: T) -> String {
        format!("{v:?}")
    }
    match t {
        Tag::Byte(v) => format!("{v}b"),
        Tag::Short(v) => format!("{v}s"),
        Tag::Int(v) => format!("{v}"),
        Tag::Long(v) => format!("{v}L"),
        Tag::Float(v) => format!("{}f", num(v)),
        Tag::Double(v) => format!("{}d", num(v)),
        Tag::String(v) => format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\"")),
        Tag::List(l) => format!("[{}]", l.iter().map(snbt).collect::<Vec<_>>().join(", ")),
        Tag::Compound(c) => format!(
            "{{{}}}",
            c.iter()
                .map(|(k, v)| {
                    let plain = !k.is_empty()
                        && k.chars().all(|c| {
                            c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+')
                        });
                    if plain {
                        format!("{k}: {}", snbt(v))
                    } else {
                        format!("\"{k}\": {}", snbt(v))
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Tag::ByteArray(a) => format!(
            "[B; {}]",
            a.iter()
                .map(|v| format!("{v}B"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Tag::IntArray(a) => format!(
            "[I; {}]",
            a.iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Tag::LongArray(a) => format!(
            "[L; {}]",
            a.iter()
                .map(|v| format!("{v}L"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Tag::End => String::new(),
    }
}

/// Follow a simple NBT path (`a.b`, `a[0]`, `[1]`, `"quoted key"`); `None` if nothing matches.
pub fn lookup<'a>(root: &'a Tag, path: &str) -> Option<&'a Tag> {
    let b = path.as_bytes();
    let (mut cur, mut i) = (root, 0);
    while i < b.len() {
        match b[i] {
            b'.' => i += 1,
            b'[' => {
                let e = path[i..].find(']')? + i;
                let idx: i64 = path[i + 1..e].trim().parse().ok()?;
                let n = match cur {
                    Tag::List(l) => l.len(),
                    Tag::IntArray(a) => a.len(),
                    Tag::ByteArray(a) => a.len(),
                    Tag::LongArray(a) => a.len(),
                    _ => return None,
                } as i64;
                let k = if idx < 0 { n + idx } else { idx };
                if k < 0 || k >= n {
                    return None;
                }
                cur = match cur {
                    Tag::List(l) => &l[k as usize],
                    _ => return None, // array elements are not tags; vanilla returns them as numbers (not modelled)
                };
                i = e + 1;
            }
            b'"' => {
                let e = path[i + 1..].find('"')? + i + 1;
                cur = cur.as_compound()?.get(&path[i + 1..e])?;
                i = e + 1;
            }
            _ => {
                let e = path[i..].find(['.', '[']).map(|x| x + i).unwrap_or(b.len());
                cur = cur.as_compound()?.get(&path[i..e])?;
                i = e;
            }
        }
    }
    Some(cur)
}

fn numeric(t: &Tag) -> Option<f64> {
    match t {
        Tag::Byte(v) => Some(*v as f64),
        Tag::Short(v) => Some(*v as f64),
        Tag::Int(v) => Some(*v as f64),
        Tag::Long(v) => Some(*v as f64),
        Tag::Float(v) => Some(*v as f64),
        Tag::Double(v) => Some(*v),
        _ => None,
    }
}

impl Game {
    pub(super) fn cmd_data_get(&mut self, eid: i32, p: &Parsed) -> R<()> {
        let entity = p.path.get(2).map(|s| s.as_str()) == Some("entity");
        let (root, name, kind) = if entity {
            let t = self.resolve(eid, p.arg("target"), false)?;
            let Some(Target::Player(e)) = t.first().copied() else {
                return Err(err("commands.data.entity.invalid", &[]));
            };
            let pl = &self.players[&e];
            (
                Tag::Compound(self.player_nbt(pl, Compound::new())),
                text::plain(&pl.name),
                "entity",
            )
        } else {
            let b = self.block_pos_from(eid, p.arg("targetPos"))?;
            let Some(be) = self.bes.get(&b) else {
                return Err(err("commands.data.block.invalid", &[]));
            };
            (
                self.be_to_nbt(b, be),
                s(format!("{}, {}, {}", b[0], b[1], b[2])),
                "block",
            )
        };
        if !p.has("path") {
            self.ok(
                eid,
                &format!("commands.data.{kind}.query"),
                vec![name, s(snbt(&root))],
            );
            return Ok(());
        }
        let path = p.arg("path").to_string();
        let v =
            lookup(&root, &path).ok_or_else(|| err("arguments.nbtpath.nothing_found", &[&path]))?;
        if p.has("scale") {
            let scale: f64 = p.arg("scale").parse().unwrap_or(1.0);
            let n = numeric(v).ok_or_else(|| err("commands.data.get.invalid", &[&path]))?;
            let r = (n * scale).floor() as i64;
            self.ok(
                eid,
                &format!("commands.data.{kind}.get"),
                vec![s(&path), name, s(format!("{scale:.2}")), s(r)],
            );
        } else {
            self.ok(
                eid,
                &format!("commands.data.{kind}.query"),
                vec![name, s(snbt(v))],
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snbt_and_paths() {
        let t = basalt_nbt::compound([
            ("Health", Tag::Float(20.0)),
            (
                "Pos",
                Tag::List(vec![Tag::Double(3.5), Tag::Double(201.0), Tag::Double(0.5)]),
            ),
        ]);
        assert_eq!(snbt(lookup(&t, "Health").unwrap()), "20.0f");
        assert_eq!(snbt(lookup(&t, "Pos").unwrap()), "[3.5d, 201.0d, 0.5d]");
        assert_eq!(snbt(lookup(&t, "Pos[1]").unwrap()), "201.0d");
        assert_eq!(snbt(lookup(&t, "Pos[-1]").unwrap()), "0.5d");
        assert!(lookup(&t, "Nope").is_none());
    }
}

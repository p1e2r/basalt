//! SNBT → NBT for command arguments that carry data (text components in /tellraw, /title).
//! Strings, quoted or not; numbers with the b/s/l/f/d suffixes (plain integers → int, plain
//! decimals → double); true/false → byte; compounds; lists; typed arrays B;/I;/L;.
use basalt_nbt::Tag;
use std::collections::BTreeMap;

pub fn parse(s: &str) -> Option<Tag> {
    let (t, i) = value(s.as_bytes(), 0)?;
    (ws(s.as_bytes(), i) == s.len()).then_some(t)
}

fn ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

fn unquoted(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b'+')
}

fn quoted(b: &[u8], i: usize) -> Option<(String, usize)> {
    let q = b[i];
    let mut out = Vec::new();
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' if j + 1 < b.len() => {
                out.push(b[j + 1]);
                j += 2;
            }
            c if c == q => return Some((String::from_utf8(out).ok()?, j + 1)),
            c => {
                out.push(c);
                j += 1;
            }
        }
    }
    None
}

fn scalar(w: &str) -> Tag {
    match w {
        "true" => return Tag::Byte(1),
        "false" => return Tag::Byte(0),
        _ => {}
    }
    let (num, suf) = match w.chars().last() {
        Some(c) if "bBsSlLfFdD".contains(c) && w.len() > 1 => {
            (&w[..w.len() - 1], Some(c.to_ascii_lowercase()))
        }
        _ => (w, None),
    };
    match suf {
        Some('b') => {
            if let Ok(v) = num.parse::<i8>() {
                return Tag::Byte(v);
            }
        }
        Some('s') => {
            if let Ok(v) = num.parse::<i16>() {
                return Tag::Short(v);
            }
        }
        Some('l') => {
            if let Ok(v) = num.parse::<i64>() {
                return Tag::Long(v);
            }
        }
        Some('f') => {
            if let Ok(v) = num.parse::<f32>() {
                return Tag::Float(v);
            }
        }
        Some('d') => {
            if let Ok(v) = num.parse::<f64>() {
                return Tag::Double(v);
            }
        }
        _ => {}
    }
    if let Ok(v) = w.parse::<i32>() {
        return Tag::Int(v);
    }
    if w.contains('.') {
        if let Ok(v) = w.parse::<f64>() {
            return Tag::Double(v);
        }
    }
    Tag::String(w.to_string())
}

fn value(b: &[u8], i: usize) -> Option<(Tag, usize)> {
    let i = ws(b, i);
    match *b.get(i)? {
        b'{' => {
            let mut m = BTreeMap::new();
            let mut j = ws(b, i + 1);
            if b.get(j) == Some(&b'}') {
                return Some((Tag::Compound(m), j + 1));
            }
            loop {
                j = ws(b, j);
                let (k, e) = if matches!(b.get(j), Some(b'"') | Some(b'\'')) {
                    quoted(b, j)?
                } else {
                    let e = (j..b.len()).find(|&x| !unquoted(b[x])).unwrap_or(b.len());
                    (String::from_utf8(b[j..e].to_vec()).ok()?, e)
                };
                j = ws(b, e);
                if b.get(j) != Some(&b':') {
                    return None;
                }
                let (v, e) = value(b, j + 1)?;
                m.insert(k, v);
                j = ws(b, e);
                match b.get(j)? {
                    b',' => j += 1,
                    b'}' => return Some((Tag::Compound(m), j + 1)),
                    _ => return None,
                }
            }
        }
        b'[' => {
            let mut j = ws(b, i + 1);
            let typed = matches!(b.get(j), Some(b'B' | b'I' | b'L')) && b.get(j + 1) == Some(&b';');
            let kind = if typed {
                let k = b[j];
                j += 2;
                Some(k)
            } else {
                None
            };
            let mut items = Vec::new();
            j = ws(b, j);
            if b.get(j) != Some(&b']') {
                loop {
                    let (v, e) = value(b, j)?;
                    items.push(v);
                    j = ws(b, e);
                    match b.get(j)? {
                        b',' => j += 1,
                        b']' => break,
                        _ => return None,
                    }
                }
            }
            let end = j + 1;
            let t = match kind {
                Some(b'B') => Tag::ByteArray(
                    items
                        .iter()
                        .map(|t| {
                            if let Tag::Byte(v) = t {
                                *v
                            } else if let Tag::Int(v) = t {
                                *v as i8
                            } else {
                                0
                            }
                        })
                        .collect(),
                ),
                Some(b'I') => Tag::IntArray(
                    items
                        .iter()
                        .map(|t| if let Tag::Int(v) = t { *v } else { 0 })
                        .collect(),
                ),
                Some(b'L') => Tag::LongArray(
                    items
                        .iter()
                        .map(|t| match t {
                            Tag::Long(v) => *v,
                            Tag::Int(v) => *v as i64,
                            _ => 0,
                        })
                        .collect(),
                ),
                _ => Tag::List(items),
            };
            Some((t, end))
        }
        b'"' | b'\'' => {
            let (s, e) = quoted(b, i)?;
            Some((Tag::String(s), e))
        }
        _ => {
            let e = (i..b.len()).find(|&x| !unquoted(b[x])).unwrap_or(b.len());
            if e == i {
                return None;
            }
            Some((scalar(std::str::from_utf8(&b[i..e]).ok()?), e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn components() {
        assert_eq!(parse("\"hi\""), Some(Tag::String("hi".into())));
        let t = parse("{text:'a',color:red,bold:true,extra:[{text:\"b\"}]}").unwrap();
        let Tag::Compound(m) = t else { panic!() };
        assert_eq!(m["text"], Tag::String("a".into()));
        assert_eq!(m["bold"], Tag::Byte(1));
        assert!(matches!(m["extra"], Tag::List(_)));
        assert_eq!(parse("[I;1,2]"), Some(Tag::IntArray(vec![1, 2])));
        assert_eq!(parse("5b"), Some(Tag::Byte(5)));
    }
}

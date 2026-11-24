//! Text components (sent as NBT on the wire).
use basalt_nbt::{Compound, Tag};

pub fn plain(s: &str) -> Tag {
    Tag::String(s.into())
}

pub fn colored(s: &str, color: &str) -> Tag {
    let mut c = Compound::new();
    c.insert("text".into(), Tag::String(s.into()));
    c.insert("color".into(), Tag::String(color.into()));
    Tag::Compound(c)
}

pub fn translate(key: &str, args: Vec<Tag>) -> Tag {
    let mut c = Compound::new();
    c.insert("translate".into(), Tag::String(key.into()));
    if !args.is_empty() {
        c.insert("with".into(), Tag::List(args));
    }
    Tag::Compound(c)
}

/// Flatten a text component NBT to a plain string (best effort).
pub fn to_plain(t: &Tag) -> String {
    match t {
        Tag::String(s) => s.clone(),
        Tag::Compound(c) => {
            let mut s = String::new();
            if let Some(Tag::String(x)) = c.get("text") {
                s.push_str(x);
            }
            if let Some(Tag::String(x)) = c.get("translate") {
                s.push_str(x);
                if let Some(Tag::List(w)) = c.get("with") {
                    s.push_str(" [");
                    for a in w {
                        s.push_str(&to_plain(a));
                        s.push(' ');
                    }
                    s.push(']');
                }
            }
            if let Some(Tag::List(ex)) = c.get("extra") {
                for e in ex {
                    s.push_str(&to_plain(e));
                }
            }
            if let Some(Tag::String(x)) = c.get("") {
                s.push_str(x);
            }
            s
        }
        Tag::List(l) => l.iter().map(to_plain).collect(),
        _ => String::new(),
    }
}

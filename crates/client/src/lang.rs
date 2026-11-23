//! Text components → display strings, using the en_us language file from the client jar.
use basalt_nbt::Tag;
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

/// (en_us from the jar, the selected language on top of it)
type Tables = (HashMap<String, String>, HashMap<String, String>);
static LANG: OnceLock<RwLock<Tables>> = OnceLock::new();

fn table() -> &'static RwLock<Tables> { LANG.get_or_init(|| RwLock::new((HashMap::new(), HashMap::new()))) }

/// Install the language table (called once after the jar is read).
pub fn init(json: Option<&[u8]>) {
    let map = json.and_then(|j| serde_json::from_slice::<HashMap<String, String>>(j).ok()).unwrap_or_default();
    table().write().unwrap().0 = map;
}

/// Switch to another language (its json from the launcher asset store); None = back to en_us.
pub fn set_language(json: Option<&[u8]>) {
    let map = json.and_then(|j| serde_json::from_slice::<HashMap<String, String>>(j).ok()).unwrap_or_default();
    table().write().unwrap().1 = map;
}

/// Translate a key with positional arguments (`%s`, `%1$s`, `%%`).
pub fn tr(key: &str, args: &[String]) -> String {
    let t = table().read().unwrap();
    let fmt = t.1.get(key).or_else(|| t.0.get(key)).map(|s| s.as_str());
    let Some(fmt) = fmt else {
        return if args.is_empty() { key.to_string() } else { format!("{key} {}", args.join(" ")) };
    };
    let mut out = String::new();
    let mut next = 0usize;
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' { out.push(c); continue; }
        // %% | %s | %N$s
        let mut num = String::new();
        while let Some(d) = chars.peek().copied().filter(|d| d.is_ascii_digit()) { num.push(d); chars.next(); }
        match chars.next() {
            Some('%') => out.push('%'),
            Some('s') | Some('d') => { out.push_str(args.get(next).map(|s| s.as_str()).unwrap_or("")); next += 1; }
            Some('$') => {
                let i: usize = num.parse().unwrap_or(1);
                chars.next(); // 's'
                out.push_str(args.get(i.saturating_sub(1)).map(|s| s.as_str()).unwrap_or(""));
            }
            Some(o) => { out.push('%'); out.push_str(&num); out.push(o); }
            None => out.push('%'),
        }
    }
    out
}

/// Flatten a text component to a display string.
pub fn text(t: &Tag) -> String {
    match t {
        Tag::String(s) => s.clone(),
        Tag::Compound(c) => {
            let mut s = String::new();
            if let Some(Tag::String(x)) = c.get("text") { s.push_str(x); }
            if let Some(Tag::String(x)) = c.get("") { s.push_str(x); }
            if let Some(Tag::String(k)) = c.get("translate") {
                let args: Vec<String> = match c.get("with") { Some(Tag::List(w)) => w.iter().map(text).collect(), _ => Vec::new() };
                let fallback = match c.get("fallback") { Some(Tag::String(f)) => Some(f.clone()), _ => None };
                let r = tr(k, &args);
                if r == *k && fallback.is_some() { s.push_str(&fallback.unwrap()); } else { s.push_str(&r); }
            }
            if let Some(Tag::List(ex)) = c.get("extra") { for e in ex { s.push_str(&text(e)); } }
            s
        }
        Tag::List(l) => l.iter().map(text).collect(),
        Tag::Byte(v) => v.to_string(), Tag::Short(v) => v.to_string(), Tag::Int(v) => v.to_string(), Tag::Long(v) => v.to_string(),
        Tag::Float(v) => v.to_string(), Tag::Double(v) => v.to_string(),
        _ => String::new(),
    }
}

/// Display name of an item (from the language file).
pub fn item_name(item: i32) -> String {
    let n = basalt_data::registries::ITEM.get(item as usize).copied().unwrap_or("air");
    let k1 = format!("item.minecraft.{n}");
    let r = tr(&k1, &[]);
    if r != k1 { return r; }
    let k2 = format!("block.minecraft.{n}");
    let r = tr(&k2, &[]);
    if r != k2 { r } else { n.to_string() }
}

#[cfg(test)]
mod tests {
    #[test]
    fn formats() {
        super::init(Some(br#"{"a":"%s was slain by %s","b":"%2$s then %1$s","c":"100%%"}"#));
        assert_eq!(super::tr("a", &["X".into(), "Y".into()]), "X was slain by Y");
        assert_eq!(super::tr("b", &["X".into(), "Y".into()]), "Y then X");
        assert_eq!(super::tr("c", &[]), "100%");
        assert_eq!(super::tr("missing", &[]), "missing");
    }
}

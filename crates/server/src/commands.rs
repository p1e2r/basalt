//! Brigadier command tree for the clientbound `commands` packet, built from the data generator's
//! `reports/commands.json` (the same tree vanilla sends), plus the answers to `command_suggestion`.
//!
//! Wire format (26.2, verified by decoding a vanilla op player's packet with tools/decode-commands.py):
//! varint node count; per node: u8 flags (bits 0-1 type root/literal/argument, 0x04 executable,
//! 0x08 redirect, 0x10 custom suggestions, 0x20 restricted), varint child count + child indices,
//! [varint redirect index], [string name for literal/argument], [argument: varint parser id +
//! parser properties + (0x10) string suggestion type]; then varint root index.
use basalt_data::{registries as reg, registry_id};
use basalt_protocol::Writer;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

/// Commands Basalt implements (top-level names present in vanilla's tree). Redirect targets
/// (e.g. `tp` → `teleport`) are pulled in automatically.
pub const IMPLEMENTED: &[&str] = &["gamemode", "tp", "teleport", "give", "time", "say", "list", "save-all", "effect", "enchant", "setblock", "fill", "kill", "summon", "help", "weather", "clear", "item"];
/// Basalt-only commands (not in vanilla): name, optional integer argument.
const EXTRA: &[(&str, Option<&str>)] = &[("spawn", None), ("save", None), ("mobs", Some("radius"))];

struct Node {
    flags: u8,
    children: Vec<usize>,
    redirect: Option<usize>,
    name: Option<String>,
    parser: Option<(i32, Vec<u8>)>,
    suggestions: Option<&'static str>,
}

struct Builder<'a> { nodes: Vec<Node>, redirects: Vec<(usize, Vec<String>)>, root: &'a Value }

fn parser_id(name: &str) -> Option<i32> { registry_id(reg::COMMAND_ARGUMENT_TYPE, name) }

/// Parser properties exactly as the client expects them (see the module docs / decode-commands.py).
fn encode_props(parser: &str, p: &Value) -> Option<Vec<u8>> {
    let mut w = Writer::new();
    match parser {
        "brigadier:integer" | "brigadier:long" | "brigadier:float" | "brigadier:double" => {
            let (min, max) = (p.get("min"), p.get("max"));
            w.u8(min.is_some() as u8 | (max.is_some() as u8) << 1);
            for v in [min, max].into_iter().flatten() {
                match parser {
                    "brigadier:integer" => { w.i32(v.as_i64()? as i32); }
                    "brigadier:long" => { w.i64(v.as_i64()?); }
                    "brigadier:float" => { w.f32(v.as_f64()? as f32); }
                    _ => { w.f64(v.as_f64()?); }
                }
            }
        }
        "brigadier:string" => { w.varint(match p["type"].as_str()? { "word" => 0, "phrase" => 1, _ => 2 }); }
        "minecraft:entity" => { w.u8((p["amount"].as_str()? == "single") as u8 | ((p["type"].as_str()? == "players") as u8) << 1); }
        "minecraft:score_holder" => { w.u8((p["amount"].as_str()? == "multiple") as u8); }
        "minecraft:time" => { w.i32(p["min"].as_i64().unwrap_or(0) as i32); }
        "minecraft:resource_or_tag" | "minecraft:resource_or_tag_key" | "minecraft:resource" | "minecraft:resource_key" | "minecraft:resource_selector" => { w.str(p["registry"].as_str()?); }
        _ => {}
    }
    Some(w.buf)
}

/// Custom suggestion types vanilla attaches (found by decoding its tree for the commands above).
fn suggestion_for(parser: &str, arg: &str, props: &Value) -> Option<&'static str> {
    match (parser, arg) {
        ("minecraft:resource", "entity") if props["registry"] == "minecraft:entity_type" => Some("minecraft:summonable_entities"),
        ("minecraft:resource_location" | "minecraft:resource", "timemarker" | "timeline") => Some("minecraft:ask_server"),
        _ => None,
    }
}

impl Builder<'_> {
    /// Returns None if this subtree needs a parser the registry doesn't know (the caller drops the command).
    fn add(&mut self, name: &str, j: &Value, path: &[String]) -> Option<usize> {
        let is_arg = j["type"] == "argument";
        let mut flags = if is_arg { 2 } else { 1 };
        if j["executable"].as_bool().unwrap_or(false) { flags |= 0x04; }
        if j.get("permissions").is_some() { flags |= 0x20; }
        let mut node = Node { flags, children: vec![], redirect: None, name: Some(name.to_string()), parser: None, suggestions: None };
        if is_arg {
            let parser = j["parser"].as_str()?;
            node.parser = Some((parser_id(parser)?, encode_props(parser, j.get("properties").unwrap_or(&Value::Null))?));
            if let Some(s) = suggestion_for(parser, name, j.get("properties").unwrap_or(&Value::Null)) { node.suggestions = Some(s); node.flags |= 0x10; }
        }
        let idx = self.nodes.len();
        self.nodes.push(node);
        if let Some(r) = j.get("redirect").and_then(|r| r.as_array()) {
            self.nodes[idx].flags |= 0x08;
            self.redirects.push((idx, r.iter().filter_map(|s| s.as_str().map(String::from)).collect()));
        }
        let mut p = path.to_vec(); p.push(name.to_string());
        if let Some(ch) = j.get("children").and_then(|c| c.as_object()) {
            for (k, v) in ch { let c = self.add(k, v, &p)?; self.nodes[idx].children.push(c); }
        }
        Some(idx)
    }
}

/// Build the `commands` packet body (everything after the packet id).
pub fn build_packet(data_dir: &Path, names: &[&str]) -> Option<Vec<u8>> {
    let json: Value = serde_json::from_slice(&std::fs::read(data_dir.join("reports/commands.json")).ok()?).ok()?;
    let top = json["children"].as_object()?;
    let mut want: Vec<String> = names.iter().map(|s| s.to_string()).collect();
    // pull in redirect targets
    let mut i = 0;
    while i < want.len() {
        if let Some(r) = top.get(&want[i]).and_then(|n| n.get("redirect")).and_then(|r| r.as_array()) {
            if let Some(t) = r.first().and_then(|t| t.as_str()) { if !want.iter().any(|w| w == t) { want.push(t.to_string()); } }
        }
        i += 1;
    }
    let mut b = Builder { nodes: vec![Node { flags: 0, children: vec![], redirect: None, name: None, parser: None, suggestions: None }], redirects: vec![], root: &json };
    let mut index: HashMap<String, usize> = HashMap::new();
    for n in &want {
        let Some(j) = top.get(n) else { continue };
        let before = (b.nodes.len(), b.redirects.len());
        match b.add(n, j, &[]) {
            Some(idx) => { index.insert(n.clone(), idx); b.nodes[0].children.push(idx); }
            None => { b.nodes.truncate(before.0); b.redirects.truncate(before.1); }   // unknown parser: leave the command out
        }
    }
    for (idx, path) in std::mem::take(&mut b.redirects) {
        // redirect path = names from the root: the first must be an included top-level command
        let target = path.first().and_then(|t| index.get(t)).copied();
        match target { Some(t) => b.nodes[idx].redirect = Some(t), None => b.nodes[idx].flags &= !0x08 }
    }
    let _ = b.root;
    for (name, arg) in EXTRA {
        let idx = b.nodes.len();
        b.nodes.push(Node { flags: 0x01 | 0x04, children: vec![], redirect: None, name: Some(name.to_string()), parser: None, suggestions: None });
        b.nodes[0].children.push(idx);
        if let (Some(a), Some(pid)) = (arg, parser_id("brigadier:integer")) {
            let c = b.nodes.len();
            b.nodes.push(Node { flags: 0x02 | 0x04, children: vec![], redirect: None, name: Some(a.to_string()), parser: Some((pid, vec![1, 0, 0, 0, 1])), suggestions: None });
            b.nodes[idx].children.push(c);
        }
    }
    let mut w = Writer::new();
    w.varint(b.nodes.len() as i32);
    for n in &b.nodes {
        w.u8(n.flags).varint(n.children.len() as i32);
        for c in &n.children { w.varint(*c as i32); }
        if let Some(r) = n.redirect { w.varint(r as i32); }
        if let Some(name) = &n.name { w.str(name); }
        if let Some((id, props)) = &n.parser { w.varint(*id).bytes(props); }
        if let Some(s) = n.suggestions { w.str(s); }
    }
    w.varint(0);
    Some(w.buf)
}

/// Answer for an `ask_server` suggestion request: `(start, length, suggestions)` over the text
/// (the client sends it with the leading slash). Only the places our tree uses ask_server.
pub fn suggest(text: &str) -> (i32, i32, Vec<String>) {
    let t = text.trim_start_matches('/');
    let words: Vec<&str> = t.split(' ').collect();
    let last = words.last().copied().unwrap_or("");
    let start = (text.len() - last.len()) as i32;
    let pool: &[&str] = match words.as_slice() {
        ["time", "set" | "add", _] => &["day", "noon", "night", "midnight"],
        _ => &[],
    };
    let v: Vec<String> = pool.iter().filter(|s| s.starts_with(last)).map(|s| s.to_string()).collect();
    (start, last.len() as i32, v)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn data() -> Option<std::path::PathBuf> { let d = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../data")); d.join("reports/commands.json").exists().then_some(d) }

    /// Decode our own packet with an independent reader and check the graph is well-formed.
    #[test]
    fn tree_is_well_formed() {
        let Some(d) = data() else { return };
        let b = build_packet(&d, IMPLEMENTED).expect("tree");
        let mut r = basalt_protocol::Reader::new(&b);
        let n = r.varint().unwrap() as usize;
        let (mut names, mut kids) = (vec![], vec![]);
        for _ in 0..n {
            let fl = r.u8().unwrap(); let c: Vec<i32> = (0..r.varint().unwrap()).map(|_| r.varint().unwrap()).collect();
            if fl & 8 != 0 { assert!((r.varint().unwrap() as usize) < n); }
            if fl & 3 != 0 { names.push(r.str().unwrap()); } else { names.push(String::new()); }
            if fl & 3 == 2 {
                let pid = r.varint().unwrap();
                let raw = basalt_data::registries::COMMAND_ARGUMENT_TYPE[pid as usize];
                let full = if raw.contains(':') { raw.to_string() } else { format!("minecraft:{raw}") };
                let pname = full.as_str();
                match pname { "brigadier:integer" => { let f = r.u8().unwrap(); for _ in 0..(f & 1) + (f >> 1 & 1) { r.i32().unwrap(); } } "minecraft:entity" | "minecraft:score_holder" => { r.u8().unwrap(); } "minecraft:time" => { r.i32().unwrap(); } "minecraft:resource" | "minecraft:resource_key" | "minecraft:resource_or_tag" | "minecraft:resource_or_tag_key" | "minecraft:resource_selector" => { r.str().unwrap(); } "brigadier:string" => { r.varint().unwrap(); } "brigadier:float" | "brigadier:double" | "brigadier:long" => { let f = r.u8().unwrap(); let sz = if pname == "brigadier:float" { 4 } else { 8 }; r.take(((f & 1) + (f >> 1 & 1)) as usize * sz).unwrap(); } _ => {} }
                if fl & 0x10 != 0 { r.str().unwrap(); }
            }
            assert!(c.iter().all(|&i| (i as usize) < n && i != 0));
            kids.push(c);
        }
        assert_eq!(r.varint().unwrap(), 0, "root index");
        assert_eq!(r.remaining(), 0, "no trailing bytes");
        let top: Vec<&str> = kids[0].iter().map(|&i| names[i as usize].as_str()).collect();
        for c in IMPLEMENTED { assert!(top.contains(c), "{c} missing from {top:?}"); }
        for c in ["spawn", "save", "mobs"] { assert!(top.contains(&c)); }
    }

    #[test]
    fn time_set_suggestions() {
        assert_eq!(suggest("/time set n"), (10, 1, vec!["noon".into(), "night".into()]));
        assert!(suggest("/gamemode c").2.is_empty());
    }
}

//! Generates Rust tables from the reports produced by Mojang's data generator
//! (run `cargo xtask extract-data` first). Nothing from Mojang is committed.
use serde_json::Value;
use std::fmt::Write;
use std::path::PathBuf;

fn ident(s: &str) -> String {
    let s = s
        .trim_start_matches("minecraft:")
        .to_uppercase()
        .replace(['/', '.', '-'], "_");
    if s.chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false)
    {
        format!("_{s}")
    } else {
        s
    }
}

fn main() {
    let root =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../data/reports");
    let root = std::env::var("BASALT_DATA")
        .map(|p| PathBuf::from(p).join("reports"))
        .unwrap_or(root);
    println!("cargo:rerun-if-changed={}", root.display());
    let rd =
        |n: &str| -> Value {
            let p = root.join(n);
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|_| {
                panic!("missing {} — run `cargo xtask extract-data`", p.display())
            }))
            .unwrap()
        };
    let mut o = String::new();

    // ---- packets ----
    let packets = rd("packets.json");
    writeln!(o, "pub mod packets {{").unwrap();
    for (state, dirs) in packets.as_object().unwrap() {
        writeln!(o, "pub mod {state} {{").unwrap();
        for (dir, pk) in dirs.as_object().unwrap() {
            writeln!(o, "pub mod {dir} {{").unwrap();
            for (name, v) in pk.as_object().unwrap() {
                writeln!(o, "pub const {}: i32 = {};", ident(name), v["protocol_id"]).unwrap();
            }
            writeln!(o, "pub fn name(id: i32) -> &'static str {{ match id {{").unwrap();
            for (name, v) in pk.as_object().unwrap() {
                writeln!(o, "{} => {:?},", v["protocol_id"], name).unwrap();
            }
            writeln!(o, "_ => \"?\" }} }}").unwrap();
            writeln!(o, "}}").unwrap();
        }
        writeln!(o, "}}").unwrap();
    }
    writeln!(o, "}}").unwrap();

    // ---- registries ----
    let regs = rd("registries.json");
    writeln!(o, "pub mod registries {{").unwrap();
    let mut all = Vec::new();
    for (reg, v) in regs.as_object().unwrap() {
        let mut entries: Vec<(i64, String)> = v["entries"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, e)| (e["protocol_id"].as_i64().unwrap(), k.clone()))
            .collect();
        entries.sort();
        let id = ident(reg);
        write!(o, "pub const {id}: &[&str] = &[").unwrap();
        for (_, n) in &entries {
            write!(o, "{:?},", n.trim_start_matches("minecraft:")).unwrap();
        }
        writeln!(o, "];").unwrap();
        all.push((reg.clone(), id));
    }
    write!(o, "pub const ALL: &[(&str, &[&str])] = &[").unwrap();
    for (r, id) in &all {
        write!(o, "({r:?}, {id}),").unwrap();
    }
    writeln!(o, "];\n}}").unwrap();

    // ---- blocks ----
    let blocks = rd("blocks.json");
    let mut names = String::new();
    let mut info = String::new();
    let mut state_block = String::new();
    let mut state_vals = String::new();
    let mut state_off = String::new();
    let mut nstates = 0usize;
    let mut voff = 0usize;
    let mut states_all: Vec<(i64, usize, Vec<u8>)> = Vec::new();
    for (bi, (name, b)) in blocks.as_object().unwrap().iter().enumerate() {
        let props: Vec<(String, Vec<String>)> = b
            .get("properties")
            .and_then(|p| p.as_object())
            .map(|p| {
                p.iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            v.as_array()
                                .unwrap()
                                .iter()
                                .map(|x| x.as_str().unwrap().to_string())
                                .collect(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let states = b["states"].as_array().unwrap();
        let mut def = 0;
        let mut min = i64::MAX;
        for s in states {
            let id = s["id"].as_i64().unwrap();
            min = min.min(id);
            if s.get("default").and_then(|d| d.as_bool()).unwrap_or(false) {
                def = id;
            }
            let vals: Vec<u8> = props
                .iter()
                .map(|(k, vs)| {
                    vs.iter()
                        .position(|x| x == s["properties"][k].as_str().unwrap())
                        .unwrap() as u8
                })
                .collect();
            states_all.push((id, bi, vals));
        }
        write!(names, "{:?},", name.trim_start_matches("minecraft:")).unwrap();
        write!(info, "BlockInfo {{ name: {:?}, kind: {:?}, default_state: {def}, first_state: {min}, num_states: {}, props: &[", name.trim_start_matches("minecraft:"), b["definition"]["type"].as_str().unwrap_or("").trim_start_matches("minecraft:"), states.len()).unwrap();
        for (k, vs) in &props {
            write!(info, "({k:?}, &{vs:?}),").unwrap();
        }
        writeln!(info, "] }},").unwrap();
    }
    states_all.sort_by_key(|s| s.0);
    for (i, (id, bi, vals)) in states_all.iter().enumerate() {
        assert_eq!(*id as usize, i, "state ids must be dense");
        write!(state_block, "{bi},").unwrap();
        write!(state_off, "{voff},").unwrap();
        for v in vals {
            write!(state_vals, "{v},").unwrap();
        }
        voff += vals.len();
        nstates += 1;
    }
    write!(state_off, "{voff},").unwrap();
    writeln!(o, "pub mod blocks {{ use super::BlockInfo;").unwrap();
    writeln!(o, "pub const BLOCKS: &[BlockInfo] = &[{info}];").unwrap();
    writeln!(o, "pub const STATE_COUNT: usize = {nstates};").unwrap();
    writeln!(
        o,
        "pub static STATE_BLOCK: [u16; {nstates}] = [{state_block}];"
    )
    .unwrap();
    writeln!(
        o,
        "pub static STATE_VAL_OFF: [u32; {}] = [{state_off}];",
        nstates + 1
    )
    .unwrap();
    writeln!(o, "pub static STATE_VALS: [u8; {voff}] = [{state_vals}];").unwrap();
    writeln!(o, "}}").unwrap();
    let _ = names;

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("generated.rs");
    std::fs::write(out, o).unwrap();
}

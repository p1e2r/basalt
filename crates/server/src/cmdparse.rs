//! Brigadier-style command parser driven by the same tree Basalt sends to clients (`reports/commands.json`).
//!
//! The walk mirrors what vanilla was observed to do over the wire (see the `commands` fuzzer in crates/check):
//! at each node a literal that matches the next word wins, otherwise every argument child is tried; the best
//! candidate is the one that consumed all input, then the one without errors; trailing input after a complete
//! parse is `command.unknown.argument`, an incomplete parse (node not executable) is `command.unknown.command`,
//! and a single argument error is reported as that error. Argument parsers cover exactly what the implemented
//! commands use (numbers with min/max, time units, vec3/block_pos coordinates, entity selectors with options,
//! block states, resources, snbt compounds, ...); each error carries the translation key vanilla sends.
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

/// A failure with the translation key + arguments vanilla replies with.
#[derive(Clone, Debug, PartialEq)]
pub struct CmdError {
    pub key: String,
    pub args: Vec<String>,
}

impl CmdError {
    pub fn new(key: &str, args: &[&str]) -> CmdError {
        CmdError {
            key: key.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
        }
    }
}

type R<T> = Result<T, CmdError>;

/// Result of a successful parse: the node names walked (literals and argument names) and the raw text of
/// every argument by name.
#[derive(Clone, Debug, Default)]
pub struct Parsed {
    pub path: Vec<String>,
    pub args: HashMap<String, String>,
}

impl Parsed {
    pub fn arg(&self, name: &str) -> &str {
        self.args.get(name).map(|s| s.as_str()).unwrap_or("")
    }
    pub fn has(&self, name: &str) -> bool {
        self.args.contains_key(name)
    }
    pub fn path_str(&self) -> String {
        self.path.join(" ")
    }
}

/// Whether a registry entry exists (`registry` like `minecraft:mob_effect`, `id` like `minecraft:speed`).
pub type Resources<'a> = &'a dyn Fn(&str, &str) -> bool;

pub struct Tree {
    root: Value,
}

static TREE: OnceLock<Option<Tree>> = OnceLock::new();

/// The tree from `<data_dir>/reports/commands.json` (loaded once).
pub fn tree(data_dir: &Path) -> Option<&'static Tree> {
    TREE.get_or_init(|| {
        let v: Value =
            serde_json::from_slice(&std::fs::read(data_dir.join("reports/commands.json")).ok()?)
                .ok()?;
        Some(Tree { root: v })
    })
    .as_ref()
}

#[derive(Clone, Default)]
struct Ctx {
    path: Vec<String>,
    args: HashMap<String, String>,
    pos: usize,
    errors: Vec<CmdError>,
    executable: bool,
}

impl Tree {
    fn node<'a>(&'a self, path: &[&str]) -> Option<&'a Value> {
        let mut n = &self.root;
        for p in path {
            n = n.get("children")?.get(*p)?;
        }
        Some(n)
    }

    /// Children of `n`, following a redirect (`tp` -> `teleport`) when it has none of its own.
    fn children<'a>(&'a self, n: &'a Value) -> Vec<(&'a str, &'a Value)> {
        let src = match (
            n.get("children").and_then(|c| c.as_object()),
            n.get("redirect").and_then(|r| r.as_array()),
        ) {
            (Some(c), _) if !c.is_empty() => n,
            (_, Some(r)) => {
                let p: Vec<&str> = r.iter().filter_map(|s| s.as_str()).collect();
                self.node(&p).unwrap_or(n)
            }
            _ => n,
        };
        src.get("children")
            .and_then(|c| c.as_object())
            .map(|c| c.iter().map(|(k, v)| (k.as_str(), v)).collect())
            .unwrap_or_default()
    }

    pub fn has_command(&self, name: &str) -> bool {
        self.node(&[name]).is_some()
    }

    /// Implemented top-level commands, sorted.
    pub fn command_names(&self) -> Vec<String> {
        let mut v: Vec<String> = crate::commands::IMPLEMENTED
            .iter()
            .filter(|n| self.node(&[n]).is_some())
            .map(|n| n.to_string())
            .collect();
        v.sort();
        v
    }

    /// Brigadier-style smart usage of a top-level command (`<targets> <item> [<count>]`, `(a|b)` for alternatives).
    pub fn usage(&self, name: &str) -> String {
        fn rec(t: &Tree, n: &Value, depth: usize) -> String {
            // a redirect ends the usage like Brigadier's ("..." back to the root, "-> x" otherwise);
            // without this, execute's subcommands (which redirect to execute) recurse forever
            let own = n
                .get("children")
                .and_then(|c| c.as_object())
                .map(|c| !c.is_empty())
                .unwrap_or(false);
            if let (false, Some(r)) = (own, n.get("redirect").and_then(|r| r.as_array())) {
                let p: Vec<&str> = r.iter().filter_map(|s| s.as_str()).collect();
                return if p.is_empty() {
                    "...".into()
                } else {
                    format!("-> {}", p.join(" "))
                };
            }
            if depth > 32 {
                return String::new();
            }
            let kids = t.children(n);
            if kids.is_empty() {
                return String::new();
            }
            let label = |(k, v): &(&str, &Value)| {
                if v["type"] == "argument" {
                    format!("<{k}>")
                } else {
                    k.to_string()
                }
            };
            let part = |c: &(&str, &Value)| {
                let sub = rec(t, c.1, depth + 1);
                if sub.is_empty() {
                    label(c)
                } else {
                    format!("{} {sub}", label(c))
                }
            };
            let optional = n["executable"].as_bool().unwrap_or(false);
            let parts: Vec<String> = kids.iter().map(part).collect();
            let body = if parts.len() == 1 {
                parts[0].clone()
            } else {
                parts.join("|")
            };
            if optional {
                format!("[{body}]")
            } else if parts.len() == 1 {
                body
            } else {
                format!("({body})")
            }
        }
        self.node(&[name])
            .map(|n| rec(self, n, 0))
            .unwrap_or_default()
    }

    /// Parse `input` (without the leading slash).
    pub fn parse(&self, input: &str, res: Resources) -> R<Parsed> {
        let base = Ctx::default();
        let c = self.parse_nodes(&self.root, input, 0, &base, res);
        if c.pos < input.len() {
            if c.errors.len() == 1 {
                return Err(c.errors[0].clone());
            }
            return Err(CmdError::new(
                if c.path.is_empty() {
                    "command.unknown.command"
                } else {
                    "command.unknown.argument"
                },
                &[],
            ));
        }
        if !c.executable {
            return Err(CmdError::new("command.unknown.command", &[]));
        }
        let mut path = c.path;
        if path.first().map(|s| s.as_str()) == Some("tp") {
            path[0] = "teleport".into();
        } // alias: one executor path
        Ok(Parsed { path, args: c.args })
    }

    fn parse_nodes(&self, node: &Value, s: &str, pos: usize, base: &Ctx, res: Resources) -> Ctx {
        let kids = self.children(node);
        let word_end = s[pos..].find(' ').map(|i| pos + i).unwrap_or(s.len());
        let word = &s[pos..word_end];
        let lit = kids
            .iter()
            .find(|(k, v)| v["type"] == "literal" && *k == word);
        let relevant: Vec<(&str, &Value)> = match lit {
            Some(l) => vec![*l],
            None => kids
                .iter()
                .filter(|(_, v)| v["type"] == "argument")
                .copied()
                .collect(),
        };
        let mut potentials: Vec<Ctx> = Vec::new();
        let mut errors: Vec<CmdError> = Vec::new();
        for (name, child) in relevant {
            let mut c = base.clone();
            c.errors.clear();
            let end = if child["type"] == "literal" {
                Ok(word_end)
            } else {
                parse_arg(
                    child["parser"].as_str().unwrap_or(""),
                    child.get("properties").unwrap_or(&Value::Null),
                    s,
                    pos,
                    res,
                )
                .map(|(_, e)| e)
            };
            let end = match end {
                Ok(e) => e,
                Err(e) => {
                    errors.push(e);
                    continue;
                }
            };
            if end < s.len() && !s[end..].starts_with(' ') {
                errors.push(CmdError::new("command.expected.separator", &[]));
                continue;
            }
            c.path.push(name.to_string());
            if child["type"] == "argument" {
                c.args.insert(name.to_string(), s[pos..end].to_string());
            }
            c.pos = end;
            c.executable = child["executable"].as_bool().unwrap_or(false);
            let has_redirect = child.get("redirect").is_some();
            let need = if has_redirect { 1 } else { 2 };
            if s.len() - end >= need {
                c.pos = end + 1;
                // a node with neither children nor redirect: the walk stops with input left over
                potentials.push(self.parse_nodes(child, s, end + 1, &c, res));
            } else {
                potentials.push(c);
            }
        }
        if potentials.is_empty() {
            let mut c = base.clone();
            c.pos = pos;
            c.errors = errors;
            return c;
        }
        // fully consumed first, then error-free; ties keep the earlier child
        let key = |c: &Ctx| (c.pos < s.len(), !c.errors.is_empty());
        let mut best = 0;
        for (i, c) in potentials.iter().enumerate() {
            if key(c) < key(&potentials[best]) {
                best = i;
            }
        }
        potentials.swap_remove(best)
    }
}

// ------------------------------------------------------------------ low-level reader helpers

fn peek(s: &str, p: usize) -> Option<char> {
    s[p..].chars().next()
}
fn num_char(c: char) -> bool {
    c.is_ascii_digit() || c == '.' || c == '-'
}
fn unquoted_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+')
}
fn take_while(s: &str, p: usize, f: impl Fn(char) -> bool) -> usize {
    p + s[p..]
        .chars()
        .take_while(|c| f(*c))
        .map(|c| c.len_utf8())
        .sum::<usize>()
}
fn word_end(s: &str, p: usize) -> usize {
    take_while(s, p, |c| c != ' ')
}

fn read_int(s: &str, p: usize) -> R<(i64, usize)> {
    let e = take_while(s, p, num_char);
    if e == p {
        return Err(CmdError::new("parsing.int.expected", &[]));
    }
    s[p..e]
        .parse::<i32>()
        .map(|v| (v as i64, e))
        .map_err(|_| CmdError::new("parsing.int.invalid", &[&s[p..e]]))
}
fn read_long(s: &str, p: usize) -> R<(i64, usize)> {
    let e = take_while(s, p, num_char);
    if e == p {
        return Err(CmdError::new("parsing.long.expected", &[]));
    }
    s[p..e]
        .parse::<i64>()
        .map(|v| (v, e))
        .map_err(|_| CmdError::new("parsing.long.invalid", &[&s[p..e]]))
}
fn read_float(s: &str, p: usize, what: &str) -> R<(f64, usize)> {
    let e = take_while(s, p, num_char);
    if e == p {
        return Err(CmdError::new(&format!("parsing.{what}.expected"), &[]));
    }
    s[p..e]
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .map(|v| (v, e))
        .ok_or_else(|| CmdError::new(&format!("parsing.{what}.invalid"), &[&s[p..e]]))
}

fn bounds(props: &Value, key: &str) -> Option<f64> {
    props.get(key).and_then(|v| v.as_f64())
}

/// Parse one argument at `p`; returns a short description and the end offset.
fn parse_arg(parser: &str, props: &Value, s: &str, p: usize, res: Resources) -> R<((), usize)> {
    let parser = parser.trim_start_matches("minecraft:");
    let end = match parser {
        "brigadier:integer" | "brigadier:long" => {
            let (v, e) = if parser == "brigadier:long" {
                read_long(s, p)?
            } else {
                read_int(s, p)?
            };
            let what = if parser == "brigadier:long" {
                "long"
            } else {
                "integer"
            };
            if let Some(m) = bounds(props, "min") {
                if (v as f64) < m {
                    return Err(CmdError::new(
                        &format!("argument.{what}.low"),
                        &[&(m as i64).to_string(), &v.to_string()],
                    ));
                }
            }
            if let Some(m) = bounds(props, "max") {
                if (v as f64) > m {
                    return Err(CmdError::new(
                        &format!("argument.{what}.big"),
                        &[&(m as i64).to_string(), &v.to_string()],
                    ));
                }
            }
            e
        }
        "brigadier:float" | "brigadier:double" => {
            let what = if parser == "brigadier:float" {
                "float"
            } else {
                "double"
            };
            let (v, e) = read_float(s, p, what)?;
            if let Some(m) = bounds(props, "min") {
                if v < m {
                    return Err(CmdError::new(
                        &format!("argument.{what}.low"),
                        &[&m.to_string(), &v.to_string()],
                    ));
                }
            }
            if let Some(m) = bounds(props, "max") {
                if v > m {
                    return Err(CmdError::new(
                        &format!("argument.{what}.big"),
                        &[&m.to_string(), &v.to_string()],
                    ));
                }
            }
            e
        }
        "brigadier:bool" => {
            let e = take_while(s, p, unquoted_char);
            match &s[p..e] {
                "true" | "false" => e,
                "" => return Err(CmdError::new("parsing.bool.expected", &[])),
                w => return Err(CmdError::new("parsing.bool.invalid", &[w])),
            }
        }
        "brigadier:string" => match props["type"].as_str().unwrap_or("word") {
            "greedy" => s.len(),
            "phrase" if peek(s, p) == Some('"') => read_quoted(s, p)?,
            _ => take_while(s, p, unquoted_char),
        },
        "message" => s.len(),
        "gamemode" => {
            let e = take_while(s, p, unquoted_char);
            if !matches!(
                &s[p..e],
                "survival" | "creative" | "adventure" | "spectator"
            ) {
                return Err(CmdError::new("argument.gamemode.invalid", &[&s[p..e]]));
            }
            e
        }
        "time" => {
            let (_, e) = parse_time(s, p, bounds(props, "min").unwrap_or(0.0) as i64)?;
            e
        }
        "entity" => {
            let single = props["amount"] == "single";
            let players = props["type"] == "players";
            parse_selector(s, p, single, players, res)?.1
        }
        "vec3" => {
            let (_, e) = parse_pos(s, p, PosKind::Vec3)?;
            e
        }
        "vec2" => {
            let (_, e) = parse_pos(s, p, PosKind::Vec2)?;
            e
        }
        "rotation" => {
            let (_, e) = parse_pos(s, p, PosKind::Rot)?;
            e
        }
        "block_pos" => {
            let (_, e) = parse_pos(s, p, PosKind::Block)?;
            e
        }
        "block_state" | "block_predicate" => parse_block(s, p, parser == "block_predicate")?,
        "item_stack" | "item_predicate" => parse_item(s, p, parser == "item_predicate", res)?,
        "resource" | "resource_key" => {
            let (id, e) = read_id(s, p)?;
            let reg = props["registry"].as_str().unwrap_or("");
            if !res(reg, &id) {
                return Err(CmdError::new("argument.resource.not_found", &[&id, reg]));
            }
            e
        }
        "resource_location" => read_id(s, p)?.1,
        "nbt_compound_tag" => parse_snbt_compound(s, p)?,
        // JSON/SNBT text component: a quoted string, a balanced {..} / [..] (spaces allowed inside), else a word
        "component" => match peek(s, p) {
            Some('"') => read_quoted(s, p)?,
            Some(open @ ('{' | '[')) => {
                let (mut depth, mut i) = (0i32, p);
                let mut quote = false;
                let mut esc = false;
                let mut end = None;
                for c in s[p..].chars() {
                    i += c.len_utf8();
                    if quote {
                        if esc {
                            esc = false;
                        } else if c == '\\' {
                            esc = true;
                        } else if c == '"' {
                            quote = false;
                        }
                    } else if c == '"' {
                        quote = true;
                    } else if c == '{' || c == '[' {
                        depth += 1;
                    } else if c == '}' || c == ']' {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(i);
                            break;
                        }
                    }
                }
                let _ = open;
                end.ok_or_else(|| CmdError::new("argument.component.invalid", &[&s[p..]]))?
            }
            _ => word_end(s, p),
        },
        "entity_anchor" => {
            let e = take_while(s, p, unquoted_char);
            if !matches!(&s[p..e], "feet" | "eyes") {
                return Err(CmdError::new("argument.anchor.invalid", &[&s[p..e]]));
            }
            e
        }
        // everything else the implemented commands never use: one word
        _ => word_end(s, p),
    };
    Ok(((), end))
}

fn read_quoted(s: &str, p: usize) -> R<usize> {
    let mut i = p + 1;
    let mut esc = false;
    for c in s[i..].chars() {
        i += c.len_utf8();
        if esc {
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == '"' {
            return Ok(i);
        }
    }
    Err(CmdError::new("parsing.quote.expected.end", &[]))
}

/// `namespace:path` with the characters resource locations allow; returns the id with the namespace filled in.
pub fn read_id(s: &str, p: usize) -> R<(String, usize)> {
    let e = take_while(s, p, |c| {
        c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.' | '/' | ':')
    });
    let raw = &s[p..e];
    // an uppercase / foreign character right after the id is an invalid id (vanilla stops reading there and fails)
    if e < s.len()
        && !s[e..].starts_with(' ')
        && !matches!(peek(s, e), Some('[' | '{' | ']' | ',' | '}' | '='))
    {
        return Err(CmdError::new("argument.id.invalid", &[]));
    }
    let id = if raw.contains(':') {
        raw.to_string()
    } else {
        format!("minecraft:{raw}")
    };
    Ok((id, e))
}

// ------------------------------------------------------------------ time

/// `<float>[d|s|t]` in ticks, with the argument's minimum.
pub fn parse_time(s: &str, p: usize, min: i64) -> R<(i64, usize)> {
    let (f, e) = read_float(s, p, "float")?;
    let ue = take_while(s, e, unquoted_char);
    let factor = match &s[e..ue] {
        "" | "t" => 1.0,
        "s" => 20.0,
        "d" => 24000.0,
        u => return Err(CmdError::new("argument.time.invalid_unit", &[u])),
    };
    let ticks = ((f * factor) + 0.5)
        .floor()
        .clamp(i32::MIN as f64, i32::MAX as f64) as i64;
    if ticks < min {
        return Err(CmdError::new(
            "argument.time.tick_count_too_low",
            &[&min.to_string(), &ticks.to_string()],
        ));
    }
    Ok((ticks, ue))
}

// ------------------------------------------------------------------ coordinates

#[derive(Clone, Copy, PartialEq)]
pub enum PosKind {
    Vec3,
    Vec2,
    Rot,
    Block,
}

/// One coordinate: `~` relative (offset), absolute, or `^` local.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coord {
    pub rel: bool,
    pub local: bool,
    pub v: f64,
    pub dotted: bool,
}

/// Parse `n` coordinates (`vec2`/`rotation`: 2, `vec3`/`block_pos`: 3).
pub fn parse_pos(s: &str, p: usize, kind: PosKind) -> R<(Vec<Coord>, usize)> {
    let n = if matches!(kind, PosKind::Vec3 | PosKind::Block) {
        3
    } else {
        2
    };
    let incomplete = if n == 3 {
        "argument.pos3d.incomplete"
    } else {
        "argument.pos2d.incomplete"
    };
    let local = peek(s, p) == Some('^');
    let mut out = Vec::new();
    let mut i = p;
    for k in 0..n {
        let c;
        if local {
            if peek(s, i) != Some('^') {
                return Err(CmdError::new("argument.pos.mixed", &[]));
            }
            i += 1;
            let (v, e) = if i < s.len() && peek(s, i) != Some(' ') {
                read_float(s, i, "double")?
            } else {
                (0.0, i)
            };
            i = e;
            c = Coord {
                rel: false,
                local: true,
                v,
                dotted: true,
            };
        } else {
            if peek(s, i) == Some('^') {
                return Err(CmdError::new("argument.pos.mixed", &[]));
            }
            if i >= s.len() {
                return Err(CmdError::new(
                    if kind == PosKind::Block {
                        "argument.pos.missing.int"
                    } else {
                        "argument.pos.missing.double"
                    },
                    &[],
                ));
            }
            let rel = peek(s, i) == Some('~');
            if rel {
                i += 1;
            }
            let start = i;
            let v = if i < s.len() && peek(s, i) != Some(' ') {
                if kind == PosKind::Block && !rel {
                    let (v, e) = read_int(s, i)?;
                    i = e;
                    v as f64
                } else {
                    let (v, e) = read_float(s, i, "double")?;
                    i = e;
                    v
                }
            } else {
                0.0
            };
            c = Coord {
                rel,
                local: false,
                v,
                dotted: s[start..i].contains('.'),
            };
        }
        out.push(c);
        if k + 1 < n {
            if peek(s, i) == Some(' ') {
                i += 1;
            } else {
                return Err(CmdError::new(incomplete, &[]));
            }
        }
    }
    Ok((out, i))
}

// ------------------------------------------------------------------ entity selectors

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sel {
    Me,
    Nearest,
    Random,
    All,
    Entities,
    NearestEntity,
    Name,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Sort {
    Nearest,
    Furthest,
    Random,
    Arbitrary,
}

/// `lo..hi` with either end open.
pub type Range = (Option<f64>, Option<f64>);

#[derive(Clone, Debug)]
pub struct Selector {
    pub kind: Sel,
    pub name: String,
    pub limit: Option<usize>,
    pub types: Vec<(bool, String)>,
    pub distance: Option<(Option<f64>, Option<f64>)>,
    pub gamemodes: Vec<(bool, String)>,
    pub names: Vec<(bool, String)>,
    pub sort: Option<Sort>,
    /// tag=[!]name (empty name: no tags / any tag)
    pub tags: Vec<(bool, String)>,
    /// scores={objective=range,...}
    pub scores: Vec<(String, Range)>,
    /// level=range (players' experience level)
    pub level: Option<(Option<f64>, Option<f64>)>,
}

impl Selector {
    fn new(kind: Sel) -> Selector {
        Selector {
            kind,
            name: String::new(),
            limit: None,
            types: vec![],
            distance: None,
            gamemodes: vec![],
            names: vec![],
            sort: None,
            tags: vec![],
            scores: vec![],
            level: None,
        }
    }
    pub fn max_results(&self) -> usize {
        let base = match self.kind {
            Sel::Me | Sel::Nearest | Sel::NearestEntity | Sel::Name => 1,
            _ => usize::MAX,
        };
        self.limit.map(|l| l.min(base)).unwrap_or(base)
    }
}

const OPTION_KEYS: &[&str] = &[
    "x",
    "y",
    "z",
    "dx",
    "dy",
    "dz",
    "x_rotation",
    "y_rotation",
    "level",
    "team",
    "tag",
    "nbt",
    "scores",
    "advancements",
    "predicate",
];

fn parse_range(v: &str) -> R<(Option<f64>, Option<f64>)> {
    let f = |t: &str| -> R<Option<f64>> {
        if t.is_empty() {
            Ok(None)
        } else {
            t.parse::<f64>()
                .ok()
                .filter(|x| x.is_finite())
                .map(Some)
                .ok_or_else(|| CmdError::new("argument.range.invalid", &[]))
        }
    };
    match v.split_once("..") {
        Some((a, b)) => {
            let (a, b) = (f(a)?, f(b)?);
            if a.is_none() && b.is_none() {
                return Err(CmdError::new("argument.range.empty", &[]));
            }
            if let (Some(a), Some(b)) = (a, b) {
                if a > b {
                    return Err(CmdError::new("argument.range.swapped", &[]));
                }
            }
            Ok((a, b))
        }
        None => {
            let a = f(v)?;
            if a.is_none() {
                return Err(CmdError::new("argument.range.empty", &[]));
            }
            Ok((a, a))
        }
    }
}

/// A selector (`@s`, `@a[distance=..3]`) or a player name / uuid. `single`: at most one entity allowed;
/// `players`: only players allowed. Returns the end offset.
pub fn parse_selector(
    s: &str,
    p: usize,
    single: bool,
    players: bool,
    res: Resources,
) -> R<(Selector, usize)> {
    if peek(s, p) != Some('@') {
        // names read like unquoted strings: `~` and `^` are not name characters, so `tp ~ ~ ~ 90` cannot take `~` as a target
        let e = take_while(s, p, unquoted_char);
        let name = &s[p..e];
        if name.is_empty() || (name.len() > 16 && uuid::Uuid::parse_str(name).is_err()) {
            return Err(CmdError::new("argument.entity.invalid", &[]));
        }
        let mut sel = Selector::new(Sel::Name);
        sel.name = name.to_string();
        return Ok((sel, e));
    }
    let mut i = p + 1;
    let Some(c) = peek(s, i) else {
        return Err(CmdError::new("argument.entity.selector.missing", &[]));
    };
    let kind = match c {
        's' => Sel::Me,
        'p' => Sel::Nearest,
        'r' => Sel::Random,
        'a' => Sel::All,
        'e' => Sel::Entities,
        'n' => Sel::NearestEntity,
        _ => {
            return Err(CmdError::new(
                "argument.entity.selector.unknown",
                &[&s[p..p + 1 + c.len_utf8()]],
            ))
        }
    };
    i += 1;
    let mut sel = Selector::new(kind);
    // @a/@p/@r select players only; @e/@n/@s may select any entity until `type=player` restricts them
    let mut includes_entities = matches!(kind, Sel::Entities | Sel::NearestEntity);
    if kind == Sel::Nearest {
        sel.sort = Some(Sort::Nearest);
    } else if kind == Sel::Random {
        sel.sort = Some(Sort::Random);
    }
    let mut seen_type_pos = false;
    let mut seen_gm_pos = false;
    let mut seen_name_pos = false;
    let mut sorted = matches!(kind, Sel::Nearest | Sel::Random | Sel::NearestEntity);
    if kind == Sel::NearestEntity {
        sel.sort = Some(Sort::Nearest);
    }
    if peek(s, i) == Some('[') {
        i += 1;
        loop {
            while peek(s, i) == Some(' ') {
                i += 1;
            }
            if peek(s, i) == Some(']') {
                i += 1;
                break;
            }
            let ke = take_while(s, i, unquoted_char);
            if ke == i {
                return Err(CmdError::new("argument.entity.options.unterminated", &[]));
            }
            let key = &s[i..ke];
            i = ke;
            while peek(s, i) == Some(' ') {
                i += 1;
            }
            let known = matches!(
                key,
                "type" | "limit" | "distance" | "gamemode" | "name" | "sort"
            ) || OPTION_KEYS.contains(&key);
            if !known {
                return Err(CmdError::new("argument.entity.options.unknown", &[key]));
            }
            let inapplicable = match key {
                "limit" => {
                    kind == Sel::Me
                        || kind == Sel::Nearest
                        || kind == Sel::NearestEntity
                        || sel.limit.is_some()
                }
                "sort" => kind == Sel::Me || sorted,
                "distance" => sel.distance.is_some(),
                _ => false,
            };
            if inapplicable {
                return Err(CmdError::new(
                    "argument.entity.options.inapplicable",
                    &[key],
                ));
            }
            if peek(s, i) != Some('=') {
                return Err(CmdError::new("argument.entity.options.valueless", &[key]));
            }
            i += 1;
            while peek(s, i) == Some(' ') {
                i += 1;
            }
            let neg = matches!(
                key,
                "type" | "gamemode" | "name" | "tag" | "team" | "predicate"
            ) && peek(s, i) == Some('!');
            if neg {
                i += 1;
            }
            match key {
                "type" => {
                    if peek(s, i) == Some('#') {
                        i = take_while(s, i + 1, |c| {
                            c.is_ascii_lowercase()
                                || c.is_ascii_digit()
                                || matches!(c, '_' | '-' | '.' | '/' | ':')
                        });
                    } else {
                        if !neg && seen_type_pos {
                            return Err(CmdError::new(
                                "argument.entity.options.inapplicable",
                                &[key],
                            ));
                        }
                        let (id, e) = read_id(s, i)?;
                        if !res("minecraft:entity_type", &id) {
                            return Err(CmdError::new(
                                "argument.entity.options.type.invalid",
                                &[&id],
                            ));
                        }
                        if !neg {
                            seen_type_pos = true;
                            if id == "minecraft:player" {
                                includes_entities = false;
                            }
                        }
                        sel.types.push((neg, id));
                        i = e;
                    }
                }
                "limit" => {
                    let (v, e) = read_int(s, i)?;
                    if v < 1 {
                        return Err(CmdError::new("argument.entity.options.limit.toosmall", &[]));
                    }
                    sel.limit = Some(v as usize);
                    i = e;
                }
                "distance" => {
                    let e = take_while(s, i, num_char);
                    let r = parse_range(&s[i..e])?;
                    if r.0.is_some_and(|v| v < 0.0) || r.1.is_some_and(|v| v < 0.0) {
                        return Err(CmdError::new(
                            "argument.entity.options.distance.negative",
                            &[],
                        ));
                    }
                    sel.distance = Some(r);
                    i = e;
                }
                "gamemode" => {
                    let e = take_while(s, i, unquoted_char);
                    let g = &s[i..e];
                    if !matches!(g, "survival" | "creative" | "adventure" | "spectator") {
                        return Err(CmdError::new("argument.entity.options.mode.invalid", &[g]));
                    }
                    if !neg && seen_gm_pos {
                        return Err(CmdError::new(
                            "argument.entity.options.inapplicable",
                            &[key],
                        ));
                    }
                    if !neg {
                        seen_gm_pos = true;
                    }
                    sel.gamemodes.push((neg, g.to_string()));
                    i = e;
                }
                "sort" => {
                    let e = take_while(s, i, unquoted_char);
                    sel.sort = Some(match &s[i..e] {
                        "nearest" => Sort::Nearest,
                        "furthest" => Sort::Furthest,
                        "random" => Sort::Random,
                        "arbitrary" => Sort::Arbitrary,
                        o => {
                            return Err(CmdError::new(
                                "argument.entity.options.sort.irreversible",
                                &[o],
                            ))
                        }
                    });
                    sorted = true;
                    i = e;
                }
                "name" => {
                    if !neg && seen_name_pos {
                        return Err(CmdError::new(
                            "argument.entity.options.inapplicable",
                            &[key],
                        ));
                    }
                    let e = if peek(s, i) == Some('"') {
                        read_quoted(s, i)?
                    } else {
                        take_while(s, i, unquoted_char)
                    };
                    if !neg {
                        seen_name_pos = true;
                    }
                    sel.names.push((neg, s[i..e].trim_matches('"').to_string()));
                    i = e;
                }
                "tag" => {
                    let e = take_while(s, i, unquoted_char);
                    sel.tags.push((neg, s[i..e].to_string()));
                    i = e;
                }
                "level" => {
                    let e = take_while(s, i, num_char);
                    sel.level = Some(parse_range(&s[i..e])?);
                    i = e;
                }
                "scores" => {
                    let e = skip_value(s, i);
                    let body = s[i..e].trim().trim_start_matches('{').trim_end_matches('}');
                    for part in body.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                        let (obj, r) = part.split_once('=').ok_or_else(|| {
                            CmdError::new("argument.entity.options.valueless", &["scores"])
                        })?;
                        sel.scores
                            .push((obj.trim().to_string(), parse_range(r.trim())?));
                    }
                    i = e;
                }
                _ => i = skip_value(s, i), // position/rotation/team/nbt/...: accepted, not evaluated
            }
            while peek(s, i) == Some(' ') {
                i += 1;
            }
            match peek(s, i) {
                Some(',') => {
                    i += 1;
                }
                Some(']') => {
                    i += 1;
                    break;
                }
                _ => return Err(CmdError::new("argument.entity.options.unterminated", &[])),
            }
        }
    }
    if single && sel.max_results() > 1 {
        return Err(CmdError::new("argument.entity.toomany", &[]));
    }
    if players && includes_entities && kind != Sel::Me {
        return Err(CmdError::new("argument.player.entities", &[]));
    }
    Ok((sel, i))
}

/// Skip a selector option value up to the next top-level `,` or `]`.
fn skip_value(s: &str, mut i: usize) -> usize {
    let (mut depth, mut quote) = (0i32, None::<char>);
    while let Some(c) = peek(s, i) {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '{' | '[' => depth += 1,
                '}' => depth -= 1,
                ']' if depth > 0 => depth -= 1,
                ']' | ',' if depth == 0 => return i,
                _ => {}
            },
        }
        i += c.len_utf8();
    }
    i
}

// ------------------------------------------------------------------ block states / items / snbt

/// `id[prop=value,...]{nbt}`; with `predicate`, `#tag` is also allowed. Validates ids, properties and values.
fn parse_block(s: &str, p: usize, predicate: bool) -> R<usize> {
    if predicate && peek(s, p) == Some('#') {
        let mut e = take_while(s, p + 1, |c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.' | '/' | ':')
        });
        if peek(s, e) == Some('[') {
            e = s[e..]
                .find(']')
                .map(|x| e + x + 1)
                .ok_or_else(|| CmdError::new("argument.block.property.unclosed", &[]))?;
        }
        return Ok(e);
    }
    let (id, mut e) = read_id(s, p)?;
    let Some(block) = basalt_data::blocks::BLOCKS
        .iter()
        .find(|b| format!("minecraft:{}", b.name) == id)
    else {
        return Err(CmdError::new("argument.block.id.invalid", &[&id]));
    };
    if peek(s, e) == Some('[') {
        e += 1;
        let mut seen: Vec<String> = Vec::new();
        loop {
            while peek(s, e) == Some(' ') {
                e += 1;
            }
            if peek(s, e) == Some(']') {
                e += 1;
                break;
            }
            if e >= s.len() {
                return Err(CmdError::new("argument.block.property.unclosed", &[]));
            }
            let ke = take_while(s, e, unquoted_char);
            let key = s[e..ke].to_string();
            e = ke;
            let Some((_, values)) = block.props.iter().find(|(n, _)| *n == key) else {
                return Err(CmdError::new(
                    "argument.block.property.unknown",
                    &[&id, &key],
                ));
            };
            if seen.contains(&key) {
                return Err(CmdError::new(
                    "argument.block.property.duplicate",
                    &[&key, &id],
                ));
            }
            seen.push(key.clone());
            while peek(s, e) == Some(' ') {
                e += 1;
            }
            if peek(s, e) != Some('=') {
                return Err(CmdError::new(
                    "argument.block.property.novalue",
                    &[&id, &key],
                ));
            }
            e += 1;
            while peek(s, e) == Some(' ') {
                e += 1;
            }
            let ve = take_while(s, e, unquoted_char);
            let val = &s[e..ve];
            if !values.contains(&val) {
                return Err(CmdError::new(
                    "argument.block.property.invalid",
                    &[&id, val, &key],
                ));
            }
            e = ve;
            while peek(s, e) == Some(' ') {
                e += 1;
            }
            match peek(s, e) {
                Some(',') => {
                    e += 1;
                }
                Some(']') => {
                    e += 1;
                    break;
                }
                _ => return Err(CmdError::new("argument.block.property.unclosed", &[])),
            }
        }
    }
    if peek(s, e) == Some('{') {
        e = parse_snbt_compound(s, e)?;
    }
    Ok(e)
}

/// `id` (or `#tag` / `*` for predicates) followed by an optional `[components]` (accepted, not evaluated).
fn parse_item(s: &str, p: usize, predicate: bool, res: Resources) -> R<usize> {
    if predicate && peek(s, p) == Some('#') {
        return Ok(take_while(s, p + 1, |c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.' | '/' | ':')
        }));
    }
    if predicate && peek(s, p) == Some('*') {
        return Ok(p + 1);
    }
    let (id, mut e) = read_id(s, p)?;
    if !res("minecraft:item", &id) {
        return Err(CmdError::new("argument.item.id.invalid", &[&id]));
    }
    if peek(s, e) == Some('[') {
        // components: balanced brackets; first key must be a component id (cheap sanity check for the fuzzed `stone[`)
        let close = {
            let mut d = 0;
            let mut end = None;
            for (k, c) in s[e..].char_indices() {
                match c {
                    '[' => d += 1,
                    ']' => {
                        d -= 1;
                        if d == 0 {
                            end = Some(e + k + 1);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            end
        };
        let Some(c) = close else {
            let key = take_while(s, e + 1, |c| {
                c.is_ascii_lowercase()
                    || c.is_ascii_digit()
                    || matches!(c, '_' | '-' | '.' | '/' | ':')
            });
            let key = &s[e + 1..key];
            return Err(CmdError::new(
                "arguments.item.component.unknown",
                &[&if key.contains(':') {
                    key.to_string()
                } else {
                    format!("minecraft:{key}")
                }],
            ));
        };
        e = c;
    }
    Ok(e)
}

/// A balanced `{key:value,...}` SNBT compound; returns the end offset.
pub fn parse_snbt_compound(s: &str, p: usize) -> R<usize> {
    if peek(s, p) != Some('{') {
        return Err(CmdError::new("snbt.parser.expected_compound", &[]));
    }
    snbt_value(s, p)
}

fn snbt_ws(s: &str, mut i: usize) -> usize {
    while peek(s, i).is_some_and(|c| c.is_whitespace()) {
        i += 1;
    }
    i
}

fn snbt_value(s: &str, i: usize) -> R<usize> {
    let i = snbt_ws(s, i);
    match peek(s, i) {
        Some('{') => {
            let mut i = snbt_ws(s, i + 1);
            if peek(s, i) == Some('}') {
                return Ok(i + 1);
            }
            loop {
                i = snbt_ws(s, i);
                let ke = match peek(s, i) {
                    Some('"') | Some('\'') => read_quoted_any(s, i)?,
                    _ => take_while(s, i, unquoted_char),
                };
                if ke == i {
                    return Err(CmdError::new("snbt.parser.expected_unquoted_string", &[]));
                }
                i = snbt_ws(s, ke);
                if peek(s, i) != Some(':') {
                    return Err(CmdError::new(
                        "snbt.parser.expected_key_value_separator",
                        &[],
                    ));
                }
                i = snbt_value(s, i + 1)?;
                i = snbt_ws(s, i);
                match peek(s, i) {
                    Some(',') => i += 1,
                    Some('}') => return Ok(i + 1),
                    _ => return Err(CmdError::new("snbt.parser.expected_unquoted_string", &[])),
                }
            }
        }
        Some('[') => {
            let mut i = snbt_ws(s, i + 1);
            // typed array prefix `B;` `I;` `L;`
            if matches!(peek(s, i), Some('B' | 'I' | 'L')) && peek(s, i + 1) == Some(';') {
                i += 2;
            }
            if peek(s, snbt_ws(s, i)) == Some(']') {
                return Ok(snbt_ws(s, i) + 1);
            }
            loop {
                i = snbt_value(s, i)?;
                i = snbt_ws(s, i);
                match peek(s, i) {
                    Some(',') => i += 1,
                    Some(']') => return Ok(i + 1),
                    _ => return Err(CmdError::new("snbt.parser.expected_unquoted_string", &[])),
                }
            }
        }
        Some('"') | Some('\'') => read_quoted_any(s, i),
        Some(_) => {
            let e = take_while(s, i, |c| unquoted_char(c) || c == '+');
            if e == i {
                Err(CmdError::new("snbt.parser.expected_unquoted_string", &[]))
            } else {
                Ok(e)
            }
        }
        None => Err(CmdError::new("snbt.parser.expected_unquoted_string", &[])),
    }
}

fn read_quoted_any(s: &str, p: usize) -> R<usize> {
    let q = peek(s, p).unwrap_or('"');
    let mut i = p + 1;
    let mut esc = false;
    for c in s[i..].chars() {
        i += c.len_utf8();
        if esc {
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == q {
            return Ok(i);
        }
    }
    Err(CmdError::new("parsing.quote.expected.end", &[]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree_for_tests() -> Option<Tree> {
        let d = std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../data/reports/commands.json"
        ));
        Some(Tree {
            root: serde_json::from_slice(&std::fs::read(d).ok()?).ok()?,
        })
    }
    fn res(r: &str, id: &str) -> bool {
        match r {
            "minecraft:entity_type" => {
                basalt_data::registry_id(basalt_data::registries::ENTITY_TYPE, id).is_some()
            }
            "minecraft:item" => {
                basalt_data::registry_id(basalt_data::registries::ITEM, id).is_some()
            }
            _ => id != "minecraft:foo",
        }
    }
    fn key(t: &Tree, c: &str) -> String {
        match t.parse(c, &res) {
            Ok(p) => format!("ok:{}", p.path_str()),
            Err(e) => e.key,
        }
    }

    /// Every expectation below was observed on the vanilla 26.2 server by the `commands` fuzzer.
    #[test]
    fn vanilla_failure_classes() {
        let Some(t) = tree_for_tests() else { return };
        for (cmd, want) in [
            ("", "command.unknown.command"),
            ("frobnicate", "command.unknown.command"),
            ("gamemode", "command.unknown.command"),
            ("gamemode 1", "argument.gamemode.invalid"),
            ("gamemode s", "argument.gamemode.invalid"),
            (
                "gamemode survival survival survival",
                "command.unknown.argument",
            ),
            ("tp", "command.unknown.command"),
            ("tp ~ ~ ~ 90", "command.unknown.argument"),
            ("tp @s", "ok:teleport destination"),
            ("tp 1 2 3", "ok:teleport location"),
            ("tp @s @s", "ok:teleport targets destination"),
            ("tp @s ~ ~ ~ 90 0", "ok:teleport targets location rotation"),
            (
                "tp @s ~ ~ ~ facing 0 0 0",
                "ok:teleport targets location facing facingLocation",
            ),
            ("teleport ~1 ~", "command.unknown.argument"),
            ("kill", "ok:kill"),
            ("kill @s @s", "command.unknown.argument"),
            ("kill @s[", "argument.entity.options.unterminated"),
            ("kill @e[limit=0]", "argument.entity.options.limit.toosmall"),
            (
                "kill @e[distance=..-1]",
                "argument.entity.options.distance.negative",
            ),
            (
                "give @s[limit=1] stone",
                "argument.entity.options.inapplicable",
            ),
            (
                "give @e[sort=xyz] stone 1",
                "argument.entity.options.sort.irreversible",
            ),
            (
                "give @e[type=] stone 1",
                "argument.entity.options.type.invalid",
            ),
            ("give @e stone", "argument.player.entities"),
            (
                "give @e[type=minecraft:player,distance=..3] stone",
                "ok:give targets item",
            ),
            ("give @s stone 1 2", "command.unknown.argument"),
            ("give @s", "command.unknown.command"),
            ("give @s nonexist 1", "argument.item.id.invalid"),
            ("give @s stone[ 1", "arguments.item.component.unknown"),
            ("clear @s stone -1", "argument.integer.low"),
            ("clear @s stone 1.5", "parsing.int.invalid"),
            ("clear @s stone abc", "parsing.int.expected"),
            ("clear @s stone 0", "ok:clear targets item maxCount"),
            ("setblock ~ ~2", "argument.pos3d.incomplete"),
            ("setblock ~ ~3 0.5 stone", "parsing.int.invalid"),
            ("setblock ~ ~2 ~ stone foo", "command.unknown.argument"),
            ("setblock ~ ~2 ~ stone keep", "ok:setblock pos block keep"),
            ("fill ~ ~ ~ ^1 ~ ~ stone", "argument.pos.mixed"),
            ("fill ~ ~ ~ ~~ ~ ~ stone", "parsing.double.expected"),
            (
                "fill ~ ~ ~ ~1 ~ ~ oak_stairs[facing=up]",
                "argument.block.property.invalid",
            ),
            ("fill ~ ~ ~ ~1 ~ ~ []", "argument.block.id.invalid"),
            ("fill ~ ~ ~ ~1 ~ ~", "command.unknown.command"),
            ("summon pig ~ ~2", "argument.pos3d.incomplete"),
            (
                "summon pig ~ ~2 ~ {NoAI:",
                "snbt.parser.expected_unquoted_string",
            ),
            ("summon pig ~ ~2 ~ {} extra", "command.unknown.argument"),
            ("summon", "command.unknown.command"),
            ("time set 368t", "ok:time set time"),
            ("time set 1x", "ok:time set timemarker"),
            ("time set noon", "ok:time set timemarker"),
            ("time add", "command.unknown.command"),
            ("time foo 99999999999", "command.unknown.argument"),
            ("weather rain -5", "argument.time.tick_count_too_low"),
            ("weather rain ", "command.unknown.argument"),
            ("weather thunder 0", "argument.time.tick_count_too_low"),
            ("weather rain 1d", "ok:weather rain duration"),
            ("say", "command.unknown.command"),
            ("say hi", "ok:say message"),
            ("list uuids", "ok:list uuids"),
            ("list a b", "command.unknown.argument"),
            ("list foo", "command.unknown.argument"),
            ("save-all foo", "command.unknown.argument"),
            ("save-all flush", "ok:save-all flush"),
            ("effect", "command.unknown.command"),
            ("effect clear", "ok:effect clear"),
            ("effect give @s speed 1000001", "argument.integer.big"),
            ("effect give @s foo", "argument.resource.not_found"),
            ("effect give @s speed 5 256", "argument.integer.big"),
            ("enchant sharpness", "command.unknown.command"),
            ("help foo", "ok:help command"),
        ] {
            assert_eq!(key(&t, cmd), want, "`{cmd}`");
        }
    }

    #[test]
    fn time_units() {
        assert_eq!(parse_time("368t", 0, 0).unwrap().0, 368);
        assert_eq!(parse_time("1.5", 0, 0).unwrap().0, 2);
        assert_eq!(parse_time("1d", 0, 0).unwrap().0, 24000);
        assert_eq!(parse_time("2s", 0, 0).unwrap().0, 40);
        assert_eq!(parse_time("0.4t", 0, 0).unwrap().0, 0);
        assert_eq!(
            parse_time("1x", 0, 0).unwrap_err().key,
            "argument.time.invalid_unit"
        );
        assert_eq!(
            parse_time("-5", 0, 1).unwrap_err().key,
            "argument.time.tick_count_too_low"
        );
    }
}

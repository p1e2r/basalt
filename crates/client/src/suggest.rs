//! Command suggestions: parses the server's Brigadier tree (clientbound `commands`) and walks it
//! along what has been typed, like the vanilla chat box. Literals and enum-like arguments are
//! suggested locally; nodes marked `ask_server` are answered by the server (`command_suggestion`).
use anyhow::{bail, Result};
use basalt_data::registries as reg;
use basalt_protocol::Reader;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Root,
    Literal,
    Argument,
}

#[derive(Debug)]
pub struct Node {
    pub kind: Kind,
    pub exec: bool,
    pub children: Vec<usize>,
    pub redirect: Option<usize>,
    pub name: String,
    pub parser: String,
    pub registry: String,
    pub players_only: bool,
    pub suggestions: Option<String>,
}

#[derive(Debug)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub root: usize,
}

fn parser_name(id: i32) -> String {
    let raw = reg::COMMAND_ARGUMENT_TYPE
        .get(id as usize)
        .copied()
        .unwrap_or("?");
    if raw.contains(':') {
        raw.to_string()
    } else {
        format!("minecraft:{raw}")
    }
}

impl Tree {
    /// Parse the packet body (after the packet id). Same layout the server writes; see
    /// crates/server/src/commands.rs.
    pub fn parse(body: &[u8]) -> Result<Tree> {
        let mut r = Reader::new(body);
        let n = r.varint()? as usize;
        let mut nodes = Vec::with_capacity(n);
        for _ in 0..n {
            let fl = r.u8()?;
            let kind = match fl & 3 {
                0 => Kind::Root,
                1 => Kind::Literal,
                2 => Kind::Argument,
                _ => bail!("bad node type"),
            };
            let cn = r.varint()?;
            let mut children = Vec::new();
            for _ in 0..cn {
                children.push(r.varint()? as usize);
            }
            let redirect = if fl & 8 != 0 {
                Some(r.varint()? as usize)
            } else {
                None
            };
            let name = if kind != Kind::Root {
                r.str()?
            } else {
                String::new()
            };
            let (mut parser, mut registry, mut players_only, mut suggestions) =
                (String::new(), String::new(), false, None);
            if kind == Kind::Argument {
                parser = parser_name(r.varint()?);
                match parser.as_str() {
                    "brigadier:integer" | "brigadier:long" | "brigadier:float"
                    | "brigadier:double" => {
                        let f = r.u8()?;
                        let sz = if parser == "brigadier:integer" || parser == "brigadier:float" {
                            4
                        } else {
                            8
                        };
                        r.take((f & 1) as usize * sz + (f >> 1 & 1) as usize * sz)?;
                    }
                    "brigadier:string" => {
                        r.varint()?;
                    }
                    "minecraft:entity" => {
                        players_only = r.u8()? & 2 != 0;
                    }
                    "minecraft:score_holder" => {
                        r.u8()?;
                    }
                    "minecraft:time" => {
                        r.i32()?;
                    }
                    "minecraft:resource_or_tag"
                    | "minecraft:resource_or_tag_key"
                    | "minecraft:resource"
                    | "minecraft:resource_key"
                    | "minecraft:resource_selector" => {
                        registry = r.str()?;
                    }
                    _ => {}
                }
                if fl & 0x10 != 0 {
                    suggestions = Some(r.str()?);
                }
            }
            nodes.push(Node {
                kind,
                exec: fl & 4 != 0,
                children,
                redirect,
                name,
                parser,
                registry,
                players_only,
                suggestions,
            });
        }
        let root = r.varint()? as usize;
        if root >= nodes.len()
            || nodes.iter().any(|n| {
                n.children.iter().any(|&c| c >= nodes.len())
                    || n.redirect.map(|x| x >= nodes.len()).unwrap_or(false)
            })
        {
            bail!("bad node index");
        }
        Ok(Tree { nodes, root })
    }

    /// Children of `i`, following its redirect.
    fn kids(&self, i: usize) -> Vec<usize> {
        let n = &self.nodes[i];
        let mut v = n.children.clone();
        if let Some(r) = n.redirect {
            v.extend(self.nodes[r].children.iter().copied());
        }
        v
    }
}

/// How many whitespace-separated words an argument of this parser takes (`None` = the rest of the line).
fn arity(n: &Node) -> Option<usize> {
    match n.parser.as_str() {
        "minecraft:block_pos" | "minecraft:vec3" => Some(3),
        "minecraft:vec2" | "minecraft:column_pos" | "minecraft:rotation" => Some(2),
        "minecraft:message"
        | "minecraft:nbt_compound_tag"
        | "minecraft:nbt_tag"
        | "minecraft:nbt_path"
        | "minecraft:component"
        | "minecraft:style" => None,
        _ => Some(1),
    }
}

pub struct Outcome {
    /// Byte offset in the typed text where the replaced word starts (after the leading slash).
    pub anchor: usize,
    pub list: Vec<String>,
    /// The deciding node wants the server to answer (`ask_server`).
    pub ask_server: bool,
}

fn matches(cand: &str, prefix: &str) -> bool {
    let (c, p) = (cand.to_ascii_lowercase(), prefix.to_ascii_lowercase());
    c.starts_with(&p)
        || c.strip_prefix("minecraft:")
            .map(|s| s.starts_with(&p))
            .unwrap_or(false)
}

fn registry_names(registry: &str) -> Vec<String> {
    let names: &[&str] = match registry.trim_start_matches("minecraft:") {
        "entity_type" => reg::ENTITY_TYPE,
        "mob_effect" => reg::MOB_EFFECT,
        "item" => reg::ITEM,
        "block" => reg::BLOCK,
        "attribute" => reg::ATTRIBUTE,
        "potion" => reg::POTION,
        "sound_event" => reg::SOUND_EVENT,
        "particle_type" => reg::PARTICLE_TYPE,
        _ => &[],
    };
    names.iter().map(|n| format!("minecraft:{n}")).collect()
}

fn argument_candidates(n: &Node, players: &[String], prefix: &str) -> Vec<String> {
    let mut v: Vec<String> = match n.parser.as_str() {
        "minecraft:gamemode" => ["survival", "creative", "adventure", "spectator"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        "brigadier:bool" => vec!["true".into(), "false".into()],
        "minecraft:entity" | "minecraft:score_holder" | "minecraft:game_profile" => {
            let mut s: Vec<String> = ["@a", "@e", "@p", "@r", "@s"]
                .iter()
                .filter(|s| !n.players_only || **s != "@e")
                .map(|s| s.to_string())
                .collect();
            s.extend(players.iter().cloned());
            s
        }
        "minecraft:resource" | "minecraft:resource_key" | "minecraft:resource_or_tag" => {
            let mut r = registry_names(&n.registry);
            if n.suggestions.as_deref() == Some("minecraft:summonable_entities") {
                r.retain(|e| e != "minecraft:player");
            }
            r
        }
        "minecraft:item_stack" | "minecraft:item_predicate" => registry_names("item"),
        "minecraft:block_state" | "minecraft:block_predicate" if !prefix.contains('[') => {
            registry_names("block")
        }
        "minecraft:block_pos" | "minecraft:vec3" | "minecraft:vec2" | "minecraft:column_pos" => {
            vec!["~".into()]
        }
        _ => vec![],
    };
    v.retain(|c| matches(c, prefix));
    v
}

/// Suggestions for `input` (what is in the chat box, with the leading `/`). `None` when the text
/// doesn't parse against the tree (nothing to suggest).
pub fn suggest(tree: &Tree, input: &str, players: &[String]) -> Option<Outcome> {
    let text = input.strip_prefix('/')?;
    let words: Vec<&str> = text.split(' ').collect();
    let (prefix, done) = (*words.last()?, &words[..words.len() - 1]);
    let anchor = text.len() - prefix.len();
    let (mut cur, mut i) = (tree.root, 0usize);
    // words belonging to a multi-word argument that is still being typed
    let mut partial: Option<usize> = None;
    while i < done.len() {
        let kids = tree.kids(cur);
        let w = done[i];
        if let Some(&c) = kids
            .iter()
            .find(|&&c| tree.nodes[c].kind == Kind::Literal && tree.nodes[c].name == w)
        {
            cur = c;
            i += 1;
            continue;
        }
        let Some(&c) = kids.iter().find(|&&c| tree.nodes[c].kind == Kind::Argument) else {
            return None;
        };
        match arity(&tree.nodes[c]) {
            None => {
                return Some(Outcome {
                    anchor,
                    list: vec![],
                    ask_server: false,
                })
            } // greedy: the rest is free text
            Some(n) if i + n > done.len() => {
                partial = Some(c);
                break;
            } // vec3 / pos half typed
            Some(n) => {
                cur = c;
                i += n;
            }
        }
    }
    if let Some(c) = partial {
        return Some(Outcome {
            anchor,
            list: argument_candidates(&tree.nodes[c], players, prefix),
            ask_server: false,
        });
    }
    let mut list: Vec<String> = Vec::new();
    let mut ask = false;
    for &c in &tree.kids(cur) {
        let n = &tree.nodes[c];
        match n.kind {
            Kind::Literal => {
                if matches(&n.name, prefix) {
                    list.push(n.name.clone());
                }
            }
            Kind::Argument => {
                if n.suggestions.as_deref() == Some("minecraft:ask_server") {
                    ask = true;
                }
                list.extend(argument_candidates(n, players, prefix));
            }
            Kind::Root => {}
        }
    }
    list.sort();
    list.dedup();
    Some(Outcome {
        anchor,
        list,
        ask_server: ask,
    })
}

/// Draws nothing itself: the HUD asks for the visible window of the list.
pub fn window(len: usize, sel: usize, rows: usize) -> (usize, usize) {
    let start = if sel >= rows { sel + 1 - rows } else { 0 };
    (start, (start + rows).min(len))
}

#[cfg(test)]
mod tests {
    use super::*;
    use basalt_protocol::Writer;

    /// gamemode <gamemode> [target]; give <targets> <item>; summon <entity>; time set <timemarker:ask_server>
    fn sample() -> Tree {
        let pid = |n: &str| basalt_data::registry_id(reg::COMMAND_ARGUMENT_TYPE, n).unwrap();
        let mut w = Writer::new();
        w.varint(9);
        let lit = |w: &mut Writer, name: &str, kids: &[i32], exec: bool| {
            w.u8(1 | (exec as u8) << 2).varint(kids.len() as i32);
            for k in kids {
                w.varint(*k);
            }
            w.str(name);
        };
        w.u8(0).varint(4).varint(1).varint(3).varint(5).varint(7); // 0 root
        lit(&mut w, "gamemode", &[2], false); // 1
        w.u8(2 | 4)
            .varint(0)
            .str("gamemode")
            .varint(pid("minecraft:gamemode")); // 2
        lit(&mut w, "give", &[4], false); // 3
        w.u8(2 | 4)
            .varint(0)
            .str("item")
            .varint(pid("minecraft:item_stack")); // 4
        lit(&mut w, "summon", &[6], false); // 5
        w.u8(2 | 4 | 0x10)
            .varint(0)
            .str("entity")
            .varint(pid("minecraft:resource"))
            .str("minecraft:entity_type")
            .str("minecraft:summonable_entities"); // 6
        lit(&mut w, "time", &[8], false); // 7
        w.u8(2 | 4 | 0x10)
            .varint(0)
            .str("timemarker")
            .varint(pid("minecraft:resource_location"))
            .str("minecraft:ask_server"); // 8
        w.varint(0);
        Tree::parse(&w.buf).unwrap()
    }

    #[test]
    fn literals_and_enums() {
        let t = sample();
        let o = suggest(&t, "/g", &[]).unwrap();
        assert_eq!(o.list, vec!["gamemode", "give"]);
        assert_eq!(o.anchor, 0);
        let o = suggest(&t, "/gamemode c", &[]).unwrap();
        assert_eq!(o.list, vec!["creative"]);
        assert_eq!(o.anchor, 9);
        assert_eq!(
            suggest(&t, "/gamemode ", &[]).unwrap().list,
            vec!["adventure", "creative", "spectator", "survival"]
        );
        assert!(suggest(&t, "/nope", &[]).unwrap().list.is_empty());
        assert!(suggest(&t, "/gamemode creative extra", &[])
            .map(|o| o.list.is_empty())
            .unwrap_or(true)); // too many words: nothing to offer
    }

    #[test]
    fn registries_and_server_asked() {
        let t = sample();
        let o = suggest(&t, "/summon zomb", &[]).unwrap();
        assert!(
            o.list.contains(&"minecraft:zombie".to_string())
                && o.list.contains(&"minecraft:zombie_villager".to_string())
        );
        assert!(suggest(&t, "/time ", &[]).unwrap().ask_server);
    }
}

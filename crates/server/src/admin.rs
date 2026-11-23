//! Server admin files in the server root (the folder holding the world), in vanilla's formats:
//! server.properties, ops.json, whitelist.json, banned-players.json, banned-ips.json.
//! Without an ops.json every player counts as an operator (Basalt's behaviour before these files;
//! the test harness relies on it); once ops.json exists it is enforced like vanilla.
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The properties Basalt writes when creating server.properties (vanilla keys and defaults).
const DEFAULTS: &[(&str, &str)] = &[
    ("difficulty", "easy"), ("enforce-whitelist", "false"), ("gamemode", "survival"), ("hardcore", "false"), ("level-name", "world"), ("level-seed", ""),
    ("max-players", "20"), ("motd", "A Minecraft Server"), ("network-compression-threshold", "256"), ("online-mode", "false"), ("op-permission-level", "4"),
    ("pvp", "true"), ("server-port", "25565"), ("simulation-distance", "10"), ("spawn-protection", "16"), ("view-distance", "10"), ("white-list", "false"),
];

pub struct Properties { pub map: BTreeMap<String, String> }

impl Properties {
    /// Read `root/server.properties`, creating it with vanilla's defaults when missing.
    pub fn load_or_create(root: &Path) -> Properties {
        let path = root.join("server.properties");
        let mut map: BTreeMap<String, String> = DEFAULTS.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                for l in text.lines() {
                    let l = l.trim();
                    if l.is_empty() || l.starts_with('#') { continue; }
                    if let Some((k, v)) = l.split_once('=') { map.insert(k.trim().to_string(), unescape(v.trim())); }
                }
            }
            Err(_) => {
                let mut out = String::from("#Minecraft server properties (written by Basalt)\n");
                for (k, v) in &map { out.push_str(&format!("{k}={v}\n")); }
                let _ = std::fs::create_dir_all(root);
                if std::fs::write(&path, out).is_ok() { log::info!("created {}", path.display()); }
            }
        }
        Properties { map }
    }
    pub fn get(&self, k: &str) -> Option<&str> { self.map.get(k).map(|s| s.as_str()).filter(|s| !s.is_empty()) }
    pub fn bool(&self, k: &str) -> bool { self.get(k) == Some("true") }
}

fn unescape(v: &str) -> String { v.replace("\\:", ":").replace("\\=", "=").replace("\\n", "\n").replace("\\\\", "\\") }

#[derive(Clone, Debug)]
pub struct Op { pub uuid: String, pub name: String, pub level: u8 }

/// ops / whitelist / bans, kept in memory and written back on change.
pub struct Lists {
    root: Option<PathBuf>,
    pub ops: Option<Vec<Op>>,
    pub whitelist: Vec<(String, String)>,
    pub banned: Vec<(String, String, String)>,
    pub banned_ips: Vec<(String, String)>,
    pub whitelist_on: bool,
    pub op_level: u8,
}

fn read(root: &Path, f: &str) -> Option<Vec<Value>> { serde_json::from_slice::<Value>(&std::fs::read(root.join(f)).ok()?).ok()?.as_array().cloned() }
fn s(v: &Value, k: &str) -> String { v[k].as_str().unwrap_or("").to_string() }

impl Lists {
    pub fn load(root: Option<&Path>, whitelist_on: bool, op_level: u8) -> Lists {
        let mut l = Lists { root: root.map(Path::to_path_buf), ops: None, whitelist: Vec::new(), banned: Vec::new(), banned_ips: Vec::new(), whitelist_on, op_level };
        if let Some(r) = root {
            l.ops = read(r, "ops.json").map(|a| a.iter().map(|v| Op { uuid: s(v, "uuid"), name: s(v, "name"), level: v["level"].as_u64().unwrap_or(4).min(4) as u8 }).collect());
            l.whitelist = read(r, "whitelist.json").unwrap_or_default().iter().map(|v| (s(v, "uuid"), s(v, "name"))).collect();
            l.banned = read(r, "banned-players.json").unwrap_or_default().iter().map(|v| (s(v, "uuid"), s(v, "name"), s(v, "reason"))).collect();
            l.banned_ips = read(r, "banned-ips.json").unwrap_or_default().iter().map(|v| (s(v, "ip"), s(v, "reason"))).collect();
        }
        l
    }

    /// Permission level of a player (0-4).
    pub fn level(&self, uuid: &str, name: &str) -> u8 {
        match &self.ops {
            None => 4,
            Some(ops) => ops.iter().find(|o| o.uuid == uuid || o.name.eq_ignore_ascii_case(name)).map(|o| o.level).unwrap_or(0),
        }
    }

    /// Why a joining player is refused (translation key + reason), if they are.
    pub fn refuse(&self, uuid: &str, name: &str, ip: &str) -> Option<(String, String)> {
        if let Some(b) = self.banned.iter().find(|b| b.0 == uuid || b.1.eq_ignore_ascii_case(name)) { return Some(("multiplayer.disconnect.banned.reason".into(), b.2.clone())); }
        if let Some(b) = self.banned_ips.iter().find(|b| b.0 == ip) { return Some(("multiplayer.disconnect.banned_ip.reason".into(), b.1.clone())); }
        // with the whitelist on, listed players and operators get in
        let listed = self.whitelist.iter().any(|w| w.0 == uuid || w.1.eq_ignore_ascii_case(name));
        let op = self.ops.as_ref().map(|o| o.iter().any(|o| o.uuid == uuid || o.name.eq_ignore_ascii_case(name))).unwrap_or(false);
        if self.whitelist_on && !listed && !op { return Some(("multiplayer.disconnect.not_whitelisted".into(), String::new())); }
        None
    }

    fn write(&self, file: &str, v: Value) {
        let Some(r) = &self.root else { return };
        if let Ok(b) = serde_json::to_vec_pretty(&v) { let _ = std::fs::write(r.join(file), b); }
    }
    pub fn save(&self) {
        if let Some(ops) = &self.ops { self.write("ops.json", Value::Array(ops.iter().map(|o| json!({"uuid": o.uuid, "name": o.name, "level": o.level, "bypassesPlayerLimit": false})).collect())); }
        self.write("whitelist.json", Value::Array(self.whitelist.iter().map(|w| json!({"uuid": w.0, "name": w.1})).collect()));
        self.write("banned-players.json", Value::Array(self.banned.iter().map(|b| json!({"uuid": b.0, "name": b.1, "created": "", "source": "Server", "expires": "forever", "reason": b.2})).collect()));
        self.write("banned-ips.json", Value::Array(self.banned_ips.iter().map(|b| json!({"ip": b.0, "created": "", "source": "Server", "expires": "forever", "reason": b.1})).collect()));
    }

    pub fn op(&mut self, uuid: &str, name: &str) -> bool {
        let lvl = self.op_level;
        let ops = self.ops.get_or_insert_with(Vec::new);
        if ops.iter().any(|o| o.uuid == uuid) { return false; }
        ops.push(Op { uuid: uuid.into(), name: name.into(), level: lvl });
        true
    }
    pub fn deop(&mut self, uuid: &str) -> bool {
        let ops = self.ops.get_or_insert_with(Vec::new);
        let n = ops.len(); ops.retain(|o| o.uuid != uuid); ops.len() != n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn levels_and_refusals() {
        let mut l = Lists::load(None, false, 4);
        assert_eq!(l.level("u1", "A"), 4, "no ops.json: everyone is op");
        assert!(l.op("u1", "A"));
        assert_eq!(l.level("u1", "A"), 4);
        assert_eq!(l.level("u2", "B"), 0);
        l.banned.push(("u2".into(), "B".into(), "grief".into()));
        assert_eq!(l.refuse("u2", "B", "1.2.3.4").map(|r| r.1), Some("grief".into()));
        l.whitelist_on = true;
        assert!(l.refuse("u3", "C", "1.2.3.4").is_some());
        l.whitelist.push(("u3".into(), "C".into()));
        assert!(l.refuse("u3", "C", "1.2.3.4").is_none());
    }
}

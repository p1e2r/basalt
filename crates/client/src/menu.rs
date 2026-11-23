//! Menu screens (title, multiplayer list, direct connect, pause, options, controls, death) built
//! from the jar's widget sprites, plus the local config file (options + saved servers).
use crate::container::Open;
use crate::options::{self, OptKind, Page};
use crate::mesh::MeshData;
use crate::Game;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use winit::keyboard::KeyCode;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ServerEntry { pub name: String, pub addr: String }

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    pub fov: f32,
    pub render_distance: u32,
    /// mouse sensitivity 0..1 (vanilla default 0.5)
    pub sensitivity: f32,
    /// 0 = auto
    pub gui_scale: u32,
    pub master_volume: f32,
    pub fullscreen: bool,
    pub name: String,
    pub servers: Vec<ServerEntry>,
    /// language code ("en_us" = the jar's own)
    pub language: String,
    /// every other option by id (see options.rs; bools 0/1, cycles as index)
    pub extra: std::collections::BTreeMap<String, f32>,
}

impl Default for Config {
    fn default() -> Self { Config { fov: 70.0, render_distance: 10, sensitivity: 0.5, gui_scale: 0, master_volume: 1.0, fullscreen: false, name: "Basalt".into(), servers: Vec::new(), language: "en_us".into(), extra: Default::default() } }
}

impl Config {
    pub fn dir() -> std::path::PathBuf {
        if let Ok(d) = std::env::var("BASALT_CONFIG_DIR") { return d.into(); }
        basalt_paths::home_dir().join(".basalt")
    }
    pub fn path() -> std::path::PathBuf { Self::dir().join("options.json") }
    pub fn load() -> Config { std::fs::read(Self::path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default() }
    pub fn save(&self) {
        let _ = std::fs::create_dir_all(Self::dir());
        if let Ok(j) = serde_json::to_vec_pretty(self) { let _ = std::fs::write(Self::path(), j); }
    }
    /// Degrees of rotation per mouse count (vanilla-like curve).
    pub fn mouse_factor(&self) -> f32 { let f = self.sensitivity * 0.6 + 0.2; f * f * f * 8.0 * 0.15 }
}

pub enum Screen {
    Container(Open),
    Title,
    Multiplayer { selected: Option<usize>, last_click: Option<(usize, Instant)> },
    EditServer { index: Option<usize>, name: String, addr: String, focus: u8 },
    Direct { addr: String },
    Connecting { addr: String, error: Option<String> },
    Pause,
    Options { from_pause: bool },
    Page { page: Page, from_pause: bool, scroll: f32 },
    Worlds { selected: Option<usize>, last_click: Option<(usize, Instant)>, search: String, scroll: f32 },
    CreateWorld { meta: crate::worlds::WorldMeta, seed: String, focus: u8 },
    EditWorld { index: usize, name: String },
    DeleteWorld { index: usize },
    /// a one-line status ("Reading world data...", "Saving world")
    Message { text: String },
    /// info screens behind the pause menu (no server data yet)
    Info { title: String, text: String },
    Death { msg: String, at: Instant },
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum Kind { Button { label: String, enabled: bool }, Slider { label: String, value: f32 }, Field { text: String, focused: bool, hint: String }, Label { text: String, color: [u8; 4], scale: f32 }, Row { title: String, sub: String, selected: bool }, Icon { sprite: String, tooltip: String }, Server { index: usize, selected: bool }, World { index: usize, selected: bool } }

/// A widget in GUI units.
#[derive(Clone, Debug)]
pub struct Widget { pub id: String, pub x: f32, pub y: f32, pub w: f32, pub h: f32, pub kind: Kind }

fn button(id: &str, label: &str, x: f32, y: f32, w: f32) -> Widget { Widget { id: id.into(), x, y, w, h: 20.0, kind: Kind::Button { label: label.into(), enabled: true } } }
fn icon(id: &str, sprite: &str, tooltip: &str, x: f32, y: f32) -> Widget { Widget { id: id.into(), x, y, w: 20.0, h: 20.0, kind: Kind::Icon { sprite: sprite.into(), tooltip: tooltip.into() } } }
fn disabled(mut w: Widget) -> Widget { if let Kind::Button { enabled, .. } = &mut w.kind { *enabled = false; } w }
fn label(text: &str, cx: f32, y: f32, color: [u8; 4], scale: f32) -> Widget { Widget { id: String::new(), x: cx, y, w: 0.0, h: 0.0, kind: Kind::Label { text: text.into(), color, scale } } }
fn t(k: &str) -> String { crate::lang::tr(k, &[]) }

pub const BINDINGS: &[(&str, &str)] = &[("key.forward", "W"), ("key.left", "A"), ("key.back", "S"), ("key.right", "D"), ("key.jump", "Space"), ("key.sneak", "Left Shift"), ("key.sprint", "Left Control"),
    ("key.inventory", "E"), ("key.drop", "Q"), ("key.swapOffhand", "F"), ("key.chat", "T"), ("key.command", "/"), ("key.attack", "Left Button"), ("key.use", "Right Button"),
    ("key.pickItem", "Middle Button"), ("key.hotbar.1", "1-9"), ("key.screenshot", "F2"), ("key.togglePerspective", "F5"), ("key.fullscreen", "F11")];

impl Game {
    pub(crate) fn gui_size(&self) -> (f32, f32) { let s = self.gui_scale(); (self.gpu.size.0 as f32 / s, self.gpu.size.1 as f32 / s) }

    pub(crate) fn widgets(&self) -> Vec<Widget> {
        let (gw, gh) = self.gui_size();
        let cx = (gw / 2.0).floor();
        let mut v = Vec::new();
        let white = [255; 4];
        let gray = [160, 160, 160, 255];
        match &self.screen {
            // singleplayer screens: layouts measured from a vanilla 26.2 recording (2026-11-23)
            Some(Screen::Worlds { selected, search, scroll, .. }) => {
                v.push(label(&t("selectWorld.title"), cx, 8.0, white, 1.0));
                v.push(Widget { id: "search".into(), x: cx - 100.0, y: 22.0, w: 200.0, h: 20.0, kind: Kind::Field { text: search.clone(), focused: true, hint: t("gui.language.search") } });
                for (i, w) in self.worlds.iter().enumerate().filter(|(_, w)| search.is_empty() || w.meta.name.to_lowercase().contains(&search.to_lowercase())) {
                    let y = 52.0 + v.iter().filter(|w| w.id.starts_with("world")).count() as f32 * 36.0 - scroll;
                    if y < 48.0 || y + 36.0 > gh - 60.0 { continue; }
                    let _ = w;
                    v.push(Widget { id: format!("world{i}"), x: cx - 135.0, y, w: 270.0, h: 36.0, kind: Kind::World { index: i, selected: *selected == Some(i) } });
                }
                let sel = selected.is_some();
                let en = |b: Widget, on: bool| if on { b } else { disabled(b) };
                v.push(en(button("play", &t("selectWorld.select"), cx - 154.0, gh - 52.0, 150.0), sel));
                v.push(button("create", &t("selectWorld.create"), cx + 4.0, gh - 52.0, 150.0));
                v.push(en(button("edit", &t("selectWorld.edit"), cx - 154.0, gh - 28.0, 72.0), sel));
                v.push(en(button("delete", &t("selectWorld.delete"), cx - 76.0, gh - 28.0, 72.0), sel));
                v.push(en(button("recreate", &t("selectWorld.recreate"), cx + 4.0, gh - 28.0, 72.0), sel));
                v.push(button("back", &t("gui.back"), cx + 82.0, gh - 28.0, 72.0));
            }
            Some(Screen::CreateWorld { meta, seed, focus }) => {
                v.push(label(&t("selectWorld.create"), cx, 15.0, white, 1.0));
                v.push(label(&t("selectWorld.enterName"), cx - 100.0 + self.ui.text_width(&t("selectWorld.enterName"), 1.0) / 2.0, 33.0, gray, 1.0));
                v.push(Widget { id: "name".into(), x: cx - 100.0, y: 43.0, w: 200.0, h: 20.0, kind: Kind::Field { text: meta.name.clone(), focused: *focus == 0, hint: String::new() } });
                let gm = ["selectWorld.gameMode.survival", "selectWorld.gameMode.creative", "selectWorld.gameMode.hardcore"][(meta.gamemode as usize).min(2)];
                v.push(button("gamemode", &format!("{}: {}", t("selectWorld.gameMode"), t(gm)), cx - 100.0, 70.0, 200.0));
                let df = ["options.difficulty.peaceful", "options.difficulty.easy", "options.difficulty.normal", "options.difficulty.hard"][(meta.difficulty as usize).min(3)];
                v.push(button("difficulty", &format!("{}: {}", t("options.difficulty"), t(df)), cx - 100.0, 95.0, 200.0));
                v.push(button("commands", &format!("{}: {}", t("selectWorld.allowCommands"), t(if meta.commands { "options.on" } else { "options.off" })), cx - 100.0, 120.0, 200.0));
                v.push(button("worldtype", &format!("{}: {}", t("selectWorld.mapType"), t(if meta.flat { "generator.minecraft.flat" } else { "generator.minecraft.normal" })), cx - 100.0, 145.0, 200.0));
                v.push(label(&t("selectWorld.enterSeed"), cx - 100.0 + self.ui.text_width(&t("selectWorld.enterSeed"), 1.0) / 2.0, 170.0, gray, 1.0));
                v.push(Widget { id: "seed".into(), x: cx - 100.0, y: 180.0, w: 200.0, h: 20.0, kind: Kind::Field { text: seed.clone(), focused: *focus == 1, hint: t("selectWorld.seedInfo") } });
                v.push(button("create", &t("selectWorld.create"), cx - 155.0, gh - 28.0, 150.0));
                v.push(button("cancel", &t("gui.cancel"), cx + 5.0, gh - 28.0, 150.0));
            }
            Some(Screen::EditWorld { name, .. }) => {
                v.push(label(&t("selectWorld.edit.title"), cx, 15.0, white, 1.0));
                v.push(label(&t("selectWorld.enterName"), cx - 100.0 + self.ui.text_width(&t("selectWorld.enterName"), 1.0) / 2.0, 33.0, gray, 1.0));
                v.push(Widget { id: "name".into(), x: cx - 100.0, y: 43.0, w: 200.0, h: 20.0, kind: Kind::Field { text: name.clone(), focused: true, hint: String::new() } });
                for (i, (id, k)) in [("reset_icon", "selectWorld.edit.resetIcon"), ("open_folder", "selectWorld.edit.openFolder"), ("backup", "selectWorld.edit.backup"),
                    ("backup_folder", "selectWorld.edit.backupFolder"), ("optimize", "selectWorld.edit.optimize")].iter().enumerate() {
                    let b = button(id, &t(k), cx - 100.0, 70.0 + i as f32 * 25.0, 200.0);
                    v.push(if *id == "optimize" { disabled(b) } else { b });
                }
                v.push(button("save", &t("selectWorld.edit.save"), cx - 99.0, gh - 22.0, 98.0));
                v.push(button("cancel", &t("gui.cancel"), cx + 2.0, gh - 22.0, 98.0));
            }
            Some(Screen::DeleteWorld { index }) => {
                let name = self.worlds.get(*index).map(|w| w.meta.name.clone()).unwrap_or_default();
                v.push(label(&t("selectWorld.deleteQuestion"), cx, gh / 2.0 - 31.0, white, 1.0));
                v.push(label(&crate::lang::tr("selectWorld.deleteWarning", &[name]), cx, gh / 2.0 - 15.0, white, 1.0));
                v.push(button("confirm", &t("selectWorld.deleteButton"), cx - 155.0, gh / 2.0 + 16.0, 150.0));
                v.push(button("cancel", &t("gui.cancel"), cx + 5.0, gh / 2.0 + 16.0, 150.0));
            }
            Some(Screen::Message { text }) => {
                v.push(label(text, cx, gh / 2.0 - 4.0, white, 1.0));
            }
            Some(Screen::Title) => {
                // layout measured from a vanilla 26.2 client recording (2026-11-23), GUI units
                let y = (gh / 4.0 + 48.0).floor();
                v.push(button("singleplayer", &t("menu.singleplayer"), cx - 100.0, y, 200.0));
                v.push(button("multiplayer", &t("menu.multiplayer"), cx - 100.0, y + 24.0, 200.0));
                v.push(disabled(button("realms", &t("menu.online"), cx - 100.0, y + 48.0, 200.0)));
                v.push(icon("friends", "sprites/friends/friends", &t("gui.friends.open"), cx - 34.0, y + 72.0));
                v.push(icon("language", "sprites/icon/language", &t("options.language"), cx - 10.0, y + 72.0));
                v.push(icon("accessibility", "sprites/icon/accessibility", &t("options.accessibility"), cx + 14.0, y + 72.0));
                v.push(button("options", &t("menu.options"), cx - 100.0, y + 96.0, 98.0));
                v.push(button("quit", &t("menu.quit"), cx + 2.0, y + 96.0, 98.0));
                v.push(label("Basalt 26.2", 2.0 + self.ui.text_width("Basalt 26.2", 1.0) / 2.0, gh - 10.0, white, 1.0));
            }
            Some(Screen::Multiplayer { selected, .. }) => {
                // vanilla 26.2 server list (recording 2026-11-23): 304-wide rows 36 apart under the header
                v.push(label(&t("multiplayer.title"), cx, 13.0, white, 1.0));
                for i in 0..self.cfg.servers.len() {
                    let y = 36.0 + i as f32 * 36.0;
                    if y + 36.0 > gh - 64.0 { break; }
                    v.push(Widget { id: format!("row{i}"), x: cx - 152.0, y, w: 304.0, h: 36.0, kind: Kind::Server { index: i, selected: *selected == Some(i) } });
                }
                let by = gh - 52.0;
                let sel = selected.is_some();
                let en = |b: Widget, on: bool| if on { b } else { disabled(b) };
                v.push(en(button("join", &t("selectServer.select"), cx - 154.0, by, 100.0), sel));
                v.push(button("direct", &t("selectServer.direct"), cx - 50.0, by, 100.0));
                v.push(button("add", &t("selectServer.add"), cx + 54.0, by, 100.0));
                v.push(en(button("edit", &t("selectServer.edit"), cx - 154.0, by + 24.0, 74.0), sel));
                v.push(en(button("delete", &t("selectServer.delete"), cx - 76.0, by + 24.0, 74.0), sel));
                v.push(button("refresh", &t("selectServer.refresh"), cx + 2.0, by + 24.0, 74.0));
                v.push(button("back", &t("gui.back"), cx + 80.0, by + 24.0, 74.0));
            }
            Some(Screen::EditServer { name, addr, focus, .. }) => {
                v.push(label(&t("manageServer.edit"), cx, 17.0, white, 1.0));
                v.push(label(&t("addServer.enterName"), cx - 100.0 + self.ui.text_width(&t("addServer.enterName"), 1.0) / 2.0, 53.0, [160, 160, 160, 255], 1.0));
                v.push(Widget { id: "name".into(), x: cx - 100.0, y: 66.0, w: 200.0, h: 20.0, kind: Kind::Field { text: name.clone(), focused: *focus == 0, hint: String::new() } });
                v.push(label(&t("addServer.enterIp"), cx - 100.0 + self.ui.text_width(&t("addServer.enterIp"), 1.0) / 2.0, 94.0, [160, 160, 160, 255], 1.0));
                v.push(Widget { id: "addr".into(), x: cx - 100.0, y: 106.0, w: 200.0, h: 20.0, kind: Kind::Field { text: addr.clone(), focused: *focus == 1, hint: String::new() } });
                v.push(button("done", &t("gui.done"), cx - 100.0, gh / 4.0 + 96.0 + 18.0, 200.0));
                v.push(button("cancel", &t("gui.cancel"), cx - 100.0, gh / 4.0 + 120.0 + 18.0, 200.0));
            }
            Some(Screen::Direct { addr }) => {
                v.push(label(&t("selectServer.direct"), cx, 20.0, white, 1.0));
                v.push(label(&t("addServer.enterIp"), cx - 100.0 + self.ui.text_width(&t("addServer.enterIp"), 1.0) / 2.0, 100.0, [160, 160, 160, 255], 1.0));
                v.push(Widget { id: "addr".into(), x: cx - 100.0, y: 116.0, w: 200.0, h: 20.0, kind: Kind::Field { text: addr.clone(), focused: true, hint: String::new() } });
                v.push(button("join", &t("selectServer.select"), cx - 100.0, gh / 4.0 + 96.0 + 12.0, 200.0));
                v.push(button("cancel", &t("gui.cancel"), cx - 100.0, gh / 4.0 + 120.0 + 12.0, 200.0));
            }
            Some(Screen::Connecting { addr, error }) => {
                match error {
                    None => { v.push(label(&t("connect.connecting"), cx, gh / 2.0 - 30.0, white, 1.0)); v.push(label(addr, cx, gh / 2.0 - 15.0, [160, 160, 160, 255], 1.0)); v.push(button("cancel", &t("gui.cancel"), cx - 100.0, gh / 2.0 + 30.0, 200.0)); }
                    Some(e) => {
                        v.push(label(&t("disconnect.lost"), cx, gh / 2.0 - 40.0, [170, 170, 170, 255], 1.0));
                        for (i, line) in wrap_text(e, 300.0, |s| self.ui.text_width(s, 1.0)).iter().enumerate() { v.push(label(line, cx, gh / 2.0 - 15.0 + i as f32 * 10.0, white, 1.0)); }
                        v.push(button("back", &t("gui.toTitle"), cx - 100.0, gh / 2.0 + 40.0, 200.0));
                    }
                }
            }
            Some(Screen::Pause) => {
                v.push(label(&t("menu.game"), cx, 40.0, white, 1.0));
                let y = (gh / 4.0 + 8.0).floor();
                v.push(button("resume", &t("menu.returnToGame"), cx - 102.0, y, 204.0));
                v.push(button("advancements", &t("gui.advancements"), cx - 102.0, y + 24.0, 98.0));
                v.push(button("stats", &t("gui.stats"), cx + 4.0, y + 24.0, 98.0));
                for (i, (id, spr, tip)) in [("bug", "sprites/pause_menu/bug", "menu.reportBugs"), ("feedback", "sprites/pause_menu/social_interactions", "menu.sendFeedback"),
                    ("friends", "sprites/friends/friends", "gui.friends.open"), ("reporting", "sprites/pause_menu/player_reporting", "menu.playerReporting"), ("links", "list", "menu.server_links")].into_iter().enumerate() {
                    v.push(icon(id, spr, &t(tip), cx - 58.0 + i as f32 * 24.0, y + 48.0));
                }
                if self.integrated.is_some() {
                    v.push(button("options", &t("menu.options"), cx - 102.0, y + 72.0, 98.0));
                    v.push(disabled(button("lan", &t("menu.shareToLan"), cx + 4.0, y + 72.0, 98.0)));
                    v.push(button("disconnect", &t("menu.returnToMenu"), cx - 102.0, y + 96.0, 204.0));
                } else {
                    v.push(button("options", &t("menu.options"), cx - 102.0, y + 72.0, 204.0));
                    v.push(button("disconnect", &t("menu.disconnect"), cx - 102.0, y + 96.0, 204.0));
                }
            }
            Some(Screen::Options { from_pause }) => {
                // vanilla 26.2 options screen (recording 2026-11-23): FOV + World Options/Online, then five rows
                v.push(label(&t("options.title"), cx, 15.0, white, 1.0));
                let c = &self.cfg;
                let fov_label = if c.fov.round() as i32 == 70 { format!("{}: {}", t("options.fov"), t("options.fov.min")) } else { format!("{}: {}", t("options.fov"), c.fov.round()) };
                v.push(Widget { id: "fov".into(), x: cx - 155.0, y: 30.0, w: 150.0, h: 20.0, kind: Kind::Slider { label: fov_label, value: ((c.fov - 30.0) / 80.0).clamp(0.0, 1.0) } });
                let _ = from_pause;
                v.push(button("page:online", &t("options.online"), cx + 5.0, 30.0, 150.0));
                let rows: [[(&str, &str); 2]; 5] = [[("page:skin", "options.skinCustomisation"), ("page:sounds", "options.sounds")], [("page:video", "options.video"), ("page:controls", "options.controls")],
                    [("page:language", "options.language"), ("page:chat", "options.chat")], [("page:packs", "options.resourcepack"), ("page:accessibility", "options.accessibility")],
                    [("telemetry", "options.telemetry"), ("page:credits", "options.credits_and_attribution")]];
                for (r, pair) in rows.iter().enumerate() {
                    for (k, (id, key)) in pair.iter().enumerate() {
                        let b = button(id, &t(key), if k == 0 { cx - 155.0 } else { cx + 5.0 }, 88.0 + r as f32 * 24.0, 150.0);
                        v.push(if *id == "telemetry" { disabled(b) } else { b });
                    }
                }
                v.push(button("done", &t("gui.done"), cx - 100.0, gh - 27.0, 200.0));
            }
            Some(Screen::Page { page, scroll, .. }) => {
                v.push(label(&t(options::title(*page)), cx, 13.0, white, 1.0));
                match page {
                    Page::KeyBinds => {
                        for (i, (k, key)) in BINDINGS.iter().enumerate() {
                            let y = 37.0 + i as f32 * 20.0 - scroll;
                            if y < 33.0 || y + 20.0 > gh - 33.0 { continue; }
                            v.push(label(&t(k), cx - 90.0 - self.ui.text_width(&t(k), 1.0) / 2.0 + 60.0, y + 6.0, white, 1.0));
                            v.push(button(&format!("key:{i}"), key, cx + 5.0, y, 75.0));
                            v.push(disabled(button(&format!("reset:{i}"), &t("controls.reset"), cx + 85.0, y, 50.0)));
                        }
                        v.push(disabled(button("reset_all", &t("controls.resetAll"), cx - 155.0, gh - 27.0, 150.0)));
                        v.push(button("done", &t("gui.done"), cx + 5.0, gh - 27.0, 150.0));
                    }
                    Page::Language => {
                        for (i, (code, name)) in self.languages().iter().enumerate() {
                            let y = 37.0 + i as f32 * 18.0 - scroll;
                            if y < 33.0 || y + 18.0 > gh - 53.0 { continue; }
                            v.push(Widget { id: format!("lang:{code}"), x: cx - 135.0, y, w: 270.0, h: 18.0, kind: Kind::Row { title: name.clone(), sub: "\u{0}center".into(), selected: *code == self.cfg.language } });
                        }
                        v.push(label(&t("options.languageAccuracyWarning"), cx, gh - 46.0, [160, 160, 160, 255], 1.0));
                        v.push(disabled(button("font", &t("options.font"), cx - 155.0, gh - 27.0, 150.0)));
                        v.push(button("done", &t("gui.done"), cx + 5.0, gh - 27.0, 150.0));
                    }
                    Page::Packs => {
                        v.push(label(&t("pack.available.title"), cx - 94.0, 38.0, white, 1.0));
                        v.push(label(&t("pack.selected.title"), cx + 94.0, 38.0, white, 1.0));
                        v.push(Widget { id: "pack:default".into(), x: cx + 16.0, y: 50.0, w: 172.0, h: 34.0, kind: Kind::Row { title: t("pack.name.default").replace("pack.name.default", "Default"), sub: t("pack.description.default").replace("pack.description.default", "The default look and feel of Minecraft"), selected: false } });
                        v.push(button("open_folder", &t("pack.openFolder"), cx - 155.0, gh - 27.0, 150.0));
                        v.push(button("done", &t("gui.done"), cx + 5.0, gh - 27.0, 150.0));
                    }
                    Page::Credits => {
                        for (i, k) in ["credits_and_attribution.button.credits", "credits_and_attribution.button.attribution", "credits_and_attribution.button.licenses"].iter().enumerate() {
                            v.push(button(&format!("credits:{i}"), &t(k), cx - 85.0, 40.0 + i as f32 * 24.0, 170.0));
                        }
                        v.push(button("done", &t("gui.done"), cx - 100.0, gh - 27.0, 200.0));
                    }
                    p => {
                        v.extend(options::layout(&self.cfg, *p, cx, gh, *scroll).0);
                        if *p == Page::Accessibility {
                            v.push(disabled(button("guide", &t("options.accessibility.link"), cx - 155.0, gh - 27.0, 150.0)));
                            v.push(button("done", &t("gui.done"), cx + 5.0, gh - 27.0, 150.0));
                        } else {
                            v.push(button("done", &t("gui.done"), cx - 100.0, gh - 27.0, 200.0));
                        }
                    }
                }
            }
            Some(Screen::Info { title, text }) => {
                v.push(label(title, cx, 13.0, white, 1.0));
                v.push(label(text, cx, gh / 2.0 - 4.0, white, 1.0));
                v.push(button("done", &t("gui.done"), cx - 100.0, gh - 27.0, 200.0));
            }
            Some(Screen::Death { msg, at }) => {
                v.push(label(&t("deathScreen.title"), cx, 30.0, white, 2.0));
                v.push(label(msg, cx, 85.0, white, 1.0));
                v.push(label(&format!("{}: \u{a7}e0", t("deathScreen.score")).replace("\u{a7}e", ""), cx, 100.0, white, 1.0));
                let enabled = at.elapsed().as_secs_f32() >= 1.0;
                let mut r = button("respawn", &t("deathScreen.respawn"), cx - 100.0, gh / 4.0 + 72.0, 200.0); r.kind = Kind::Button { label: t("deathScreen.respawn"), enabled }; v.push(r);
                let mut tt = button("title", &t("deathScreen.titleScreen"), cx - 100.0, gh / 4.0 + 96.0, 200.0); tt.kind = Kind::Button { label: t("deathScreen.titleScreen"), enabled }; v.push(tt);
            }
            _ => {}
        }
        v
    }

    fn widget_at(&self, mx: f32, my: f32) -> Option<Widget> {
        let s = self.gui_scale();
        let (gx, gy) = (mx / s, my / s);
        self.widgets().into_iter().find(|w| !w.id.is_empty() && gx >= w.x && gx < w.x + w.w && gy >= w.y && gy < w.y + w.h)
    }

    /// Mouse press/release on a menu screen.
    pub(crate) fn menu_mouse(&mut self, button: u8, pressed: bool) {
        if !pressed { self.slider_drag = None; return; }
        if button != 0 { return; }
        let (mx, my) = self.mouse_pos;
        let Some(w) = self.widget_at(mx, my) else { return };
        match &w.kind {
            Kind::Button { enabled: false, .. } => return,
            Kind::Button { .. } => { self.sounds.play_ui("ui.button.click"); self.menu_action(&w.id); }
            Kind::Slider { .. } => { self.slider_drag = Some(w.id.clone()); self.slider_set(&w, mx); }
            Kind::Field { .. } => {
                if let Some(Screen::EditServer { focus, .. }) = &mut self.screen { *focus = if w.id == "name" { 0 } else { 1 }; }
                if let Some(Screen::CreateWorld { focus, .. }) = &mut self.screen { *focus = if w.id == "name" { 0 } else { 1 }; }
            }
            Kind::Row { .. } | Kind::Server { .. } if w.id.starts_with("row") => {
                let i: usize = w.id[3..].parse().unwrap_or(0);
                if let Some(Screen::Multiplayer { selected, last_click }) = &mut self.screen {
                    let dbl = last_click.map(|(j, t)| j == i && t.elapsed().as_millis() < 300).unwrap_or(false);
                    *selected = Some(i); *last_click = Some((i, Instant::now()));
                    if dbl { self.menu_action("join"); }
                }
            }
            Kind::Label { .. } => {}
            Kind::Icon { .. } => { self.sounds.play_ui("ui.button.click"); self.menu_action(&w.id); }
            Kind::World { index, .. } => {
                let i = *index;
                if let Some(Screen::Worlds { selected, last_click, .. }) = &mut self.screen {
                    let dbl = last_click.map(|(j, t)| j == i && t.elapsed().as_millis() < 300).unwrap_or(false);
                    *selected = Some(i); *last_click = Some((i, Instant::now()));
                    if dbl { self.menu_action("play"); }
                }
            }
            Kind::Row { .. } | Kind::Server { .. } => { if w.id.starts_with("lang:") { self.menu_action(&w.id); } }
        }
    }
    pub(crate) fn menu_motion(&mut self) {
        let Some(id) = self.slider_drag.clone() else { return };
        let (mx, _) = self.mouse_pos;
        if let Some(w) = self.widgets().into_iter().find(|w| w.id == id) { self.slider_set(&w, mx); }
    }
    fn slider_set(&mut self, w: &Widget, mx: f32) {
        let s = self.gui_scale();
        let f = ((mx / s - w.x - 4.0) / (w.w - 8.0)).clamp(0.0, 1.0);
        match w.id.as_str() {
            "fov" => self.cfg.fov = (30.0 + f * 80.0).round(),
            "render" => self.cfg.render_distance = (2.0 + f * 30.0).round() as u32,
            "sens" => self.cfg.sensitivity = (f * 200.0).round() / 200.0,
            "volume" => { self.cfg.master_volume = (f * 100.0).round() / 100.0; self.sounds.master = self.cfg.master_volume; }
            id if id.starts_with("opt:") => {
                let page = match &self.screen { Some(Screen::Page { page, .. }) => *page, _ => return };
                if let Some(o) = options::opts(page).iter().find(|o| o.id == &id[4..]) {
                    if let OptKind::Slider(min, max, step, ..) = o.kind { let v = (min + f * (max - min)) / step; self.set_opt(o.id, v.round() * step); }
                }
            }
            _ => {}
        }
        self.cfg.save();
    }

    pub(crate) fn menu_action(&mut self, id: &str) {
        let was_list = matches!(self.screen, Some(Screen::Multiplayer { .. }));
        self.menu_action_inner(id);
        if (!was_list || id == "refresh") && matches!(self.screen, Some(Screen::Multiplayer { .. })) { self.refresh_pings(); }
        if matches!(self.screen, Some(Screen::Worlds { .. })) { self.load_world_icons(); }
    }

    /// Ping every saved server again (results arrive in Game::pump).
    pub(crate) fn refresh_pings(&mut self) {
        self.ping_rounds += 1;
        for s in self.cfg.servers.clone() {
            self.pings.insert(s.addr.clone(), crate::status::PingState::Pinging(Instant::now()));
            crate::status::spawn(s.addr, self.ping_chan_tx());
        }
    }

    fn menu_action_inner(&mut self, id: &str) {
        let screen = self.screen.take();
        self.screen = match (screen, id) {
            (Some(Screen::Title), "singleplayer") => {
                self.worlds = crate::worlds::list();
                if self.worlds.is_empty() { Some(Screen::CreateWorld { meta: Default::default(), seed: String::new(), focus: 0 }) }
                else { Some(Screen::Worlds { selected: None, last_click: None, search: String::new(), scroll: 0.0 }) }
            }
            (Some(Screen::Worlds { .. }), "back") => Some(Screen::Title),
            (Some(Screen::Worlds { .. }), "create") => Some(Screen::CreateWorld { meta: Default::default(), seed: String::new(), focus: 0 }),
            (Some(Screen::Worlds { selected: Some(i), .. }), "edit") => self.worlds.get(i).map(|w| Screen::EditWorld { index: i, name: w.meta.name.clone() }),
            (Some(Screen::Worlds { selected: Some(i), .. }), "delete") => Some(Screen::DeleteWorld { index: i }),
            (Some(Screen::Worlds { selected: Some(i), .. }), "recreate") => self.worlds.get(i).map(|w| {
                let mut meta = w.meta.clone(); meta.last_played = 0;
                Screen::CreateWorld { seed: meta.seed.to_string(), meta, focus: 0 }
            }),
            (Some(Screen::Worlds { selected: Some(i), .. }), "play") => match self.worlds.get(i).cloned() { Some(w) => { self.play_world(w); self.screen.take() } None => Some(Screen::Title) },
            (Some(Screen::CreateWorld { mut meta, seed, focus }), id) if matches!(id, "gamemode" | "difficulty" | "commands" | "worldtype") => {
                match id { "gamemode" => meta.gamemode = (meta.gamemode + 1) % 3, "difficulty" => meta.difficulty = (meta.difficulty + 1) % 4, "commands" => meta.commands = !meta.commands, _ => meta.flat = !meta.flat }
                Some(Screen::CreateWorld { meta, seed, focus })
            }
            (Some(Screen::CreateWorld { mut meta, seed, .. }), "create") => {
                meta.seed = crate::worlds::parse_seed(&seed);
                if meta.name.trim().is_empty() { meta.name = t("selectWorld.newWorld"); }
                match crate::worlds::create(&meta) {
                    Ok(dir) => {
                        let folder = dir.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
                        self.play_world(crate::worlds::WorldEntry { dir, folder, meta });
                        self.screen.take()
                    }
                    Err(e) => Some(Screen::Connecting { addr: String::new(), error: Some(format!("{e:#}")) }),
                }
            }
            (Some(Screen::CreateWorld { .. }), "cancel") | (Some(Screen::EditWorld { .. }), "cancel") | (Some(Screen::DeleteWorld { .. }), "cancel") => {
                self.worlds = crate::worlds::list();
                Some(if self.worlds.is_empty() { Screen::Title } else { Screen::Worlds { selected: None, last_click: None, search: String::new(), scroll: 0.0 } })
            }
            (Some(Screen::EditWorld { index, name }), "save") => {
                if let Some(w) = self.worlds.get_mut(index) { w.meta.name = name; let _ = crate::worlds::save_meta(&w.dir, &w.meta); }
                Some(Screen::Worlds { selected: Some(index), last_click: None, search: String::new(), scroll: 0.0 })
            }
            (Some(Screen::EditWorld { index, name }), id) if matches!(id, "reset_icon" | "open_folder" | "backup" | "backup_folder") => {
                if let Some(w) = self.worlds.get(index).cloned() {
                    match id {
                        "reset_icon" => { let _ = std::fs::remove_file(w.dir.join("icon.png")); self.icon_slots.remove(&format!("world:{}", w.folder)); }
                        "open_folder" => crate::worlds::open_folder(&w.dir),
                        "backup" => { if let Err(e) = crate::worlds::backup(&w) { log::warn!("backup: {e:#}"); } }
                        _ => crate::worlds::open_folder(&crate::worlds::backups_dir()),
                    }
                }
                Some(Screen::EditWorld { index, name })
            }
            (Some(Screen::DeleteWorld { index }), "confirm") => {
                if let Some(w) = self.worlds.get(index) { if let Err(e) = crate::worlds::delete(&w.dir) { log::warn!("{e:#}"); } }
                self.worlds = crate::worlds::list();
                Some(if self.worlds.is_empty() { Screen::Title } else { Screen::Worlds { selected: None, last_click: None, search: String::new(), scroll: 0.0 } })
            }
            (Some(Screen::Title), "multiplayer") => Some(Screen::Multiplayer { selected: None, last_click: None }),
            (Some(Screen::Title), "options") => Some(Screen::Options { from_pause: false }),
            (Some(Screen::Title), "quit") => { self.quit = true; Some(Screen::Title) }
            (Some(Screen::Multiplayer { selected, .. }), "join") => {
                match selected.and_then(|i| self.cfg.servers.get(i)).map(|s| s.addr.clone()) { Some(a) => { self.connect(&a); self.screen.take() } None => Some(Screen::Multiplayer { selected, last_click: None }) }
            }
            (Some(Screen::Multiplayer { .. }), "direct") => Some(Screen::Direct { addr: self.last_addr.clone() }),
            (Some(Screen::Multiplayer { .. }), "add") => Some(Screen::EditServer { index: None, name: t("selectServer.defaultName"), addr: String::new(), focus: 0 }),
            (Some(Screen::Multiplayer { selected: Some(i), .. }), "edit") => { let s = self.cfg.servers[i].clone(); Some(Screen::EditServer { index: Some(i), name: s.name, addr: s.addr, focus: 0 }) }
            (Some(Screen::Multiplayer { selected: Some(i), .. }), "delete") => { if i < self.cfg.servers.len() { self.cfg.servers.remove(i); self.cfg.save(); } Some(Screen::Multiplayer { selected: None, last_click: None }) }
            (Some(Screen::Multiplayer { .. }), "back") => Some(Screen::Title),
            (Some(Screen::EditServer { index, name, addr, .. }), "done") => {
                let e = ServerEntry { name, addr };
                match index { Some(i) if i < self.cfg.servers.len() => self.cfg.servers[i] = e, _ => self.cfg.servers.push(e) }
                self.cfg.save();
                Some(Screen::Multiplayer { selected: None, last_click: None })
            }
            (Some(Screen::EditServer { .. }), "cancel") | (Some(Screen::Direct { .. }), "cancel") => Some(Screen::Multiplayer { selected: None, last_click: None }),
            (Some(Screen::Direct { addr }), "join") => { self.last_addr = addr.clone(); self.connect(&addr); self.screen.take() }
            (Some(Screen::Connecting { .. }), "cancel") | (Some(Screen::Connecting { .. }), "back") => { self.disconnect(); Some(Screen::Multiplayer { selected: None, last_click: None }) }
            (Some(Screen::Pause), "resume") => None,
            (Some(Screen::Pause), "options") => Some(Screen::Options { from_pause: true }),
            (Some(Screen::Pause), "disconnect") => { self.leave_world(); Some(Screen::Title) }
            (Some(Screen::Options { from_pause }), "gui") => { self.cfg.gui_scale = (self.cfg.gui_scale + 1) % 5; self.cfg.save(); Some(Screen::Options { from_pause }) }
            (Some(Screen::Options { from_pause }), "fullscreen") => { self.cfg.fullscreen = !self.cfg.fullscreen; self.cfg.save(); self.fullscreen_changed = true; Some(Screen::Options { from_pause }) }
            (Some(Screen::Options { from_pause }), id) if id.starts_with("page:") => {
                let page = match &id[5..] { "skin" => Page::Skin, "sounds" => Page::Sounds, "video" => Page::Video, "controls" => Page::Controls, "language" => Page::Language,
                    "chat" => Page::Chat, "packs" => Page::Packs, "accessibility" => Page::Accessibility, "credits" => Page::Credits, _ => Page::Online };
                let scroll = if page == Page::Language { self.language_scroll() } else { 0.0 };
                Some(Screen::Page { page, from_pause, scroll })
            }
            (Some(Screen::Page { page, from_pause, scroll }), id) if id.starts_with("opt:") => {
                let id = &id[4..];
                match id {
                    "mouse" => Some(Screen::Page { page: Page::Mouse, from_pause, scroll: 0.0 }),
                    "keybinds" => Some(Screen::Page { page: Page::KeyBinds, from_pause, scroll: 0.0 }),
                    "controls" => Some(Screen::Page { page: Page::Controls, from_pause, scroll: 0.0 }),
                    "gui_scale" => { self.cfg.gui_scale = (self.cfg.gui_scale + 1) % 5; self.cfg.save(); Some(Screen::Page { page, from_pause, scroll }) }
                    "fullscreen" => { self.cfg.fullscreen = !self.cfg.fullscreen; self.cfg.save(); self.fullscreen_changed = true; Some(Screen::Page { page, from_pause, scroll }) }
                    _ => {
                        if let Some(o) = options::opts(page).iter().find(|o| o.id == id) {
                            let v = options::value(&self.cfg, o);
                            let nv = match o.kind { OptKind::Toggle(_) => if v > 0.5 { 0.0 } else { 1.0 }, OptKind::Cycle(vals, _) => ((v as usize + 1) % vals.len()) as f32, _ => v };
                            self.set_opt(o.id, nv);
                        }
                        Some(Screen::Page { page, from_pause, scroll })
                    }
                }
            }
            (Some(Screen::Page { from_pause, scroll, .. }), id) if id.starts_with("lang:") => {
                self.set_language(&id[5..]);
                Some(Screen::Page { page: Page::Language, from_pause, scroll })
            }
            (Some(Screen::Page { from_pause, .. }), "open_folder") => {
                let dir = Config::dir().join("resourcepacks");
                let _ = std::fs::create_dir_all(&dir);
                let opener = if cfg!(target_os = "macos") { "open" } else if cfg!(target_os = "windows") { "explorer" } else { "xdg-open" };
                let _ = std::process::Command::new(opener).arg(&dir).spawn();
                Some(Screen::Page { page: Page::Packs, from_pause, scroll: 0.0 })
            }
            (Some(Screen::Page { page, from_pause, .. }), "done") => {
                self.cfg.save();
                Some(match page { Page::Mouse | Page::KeyBinds => Screen::Page { page: Page::Controls, from_pause, scroll: 0.0 }, _ => Screen::Options { from_pause } })
            }
            (Some(Screen::Pause), "advancements") => Some(Screen::Info { title: t("gui.advancements"), text: t("advancements.empty") }),
            (Some(Screen::Pause), "stats") => Some(Screen::Info { title: t("gui.stats"), text: t("multiplayer.downloadingStats") }),
            (Some(Screen::Info { .. }), "done") => Some(Screen::Pause),
            (Some(Screen::Title), "language") => Some(Screen::Page { page: Page::Language, from_pause: false, scroll: self.language_scroll() }),
            (Some(Screen::Title), "accessibility") => Some(Screen::Page { page: Page::Accessibility, from_pause: false, scroll: 0.0 }),
            (Some(Screen::Options { from_pause }), "done") => { self.cfg.save(); Some(if from_pause { Screen::Pause } else { Screen::Title }) }
            (Some(Screen::Death { .. }), "respawn") => { self.respawn(); None }
            (Some(Screen::Death { .. }), "title") => { self.disconnect(); Some(Screen::Title) }
            (s, _) => s,
        };
    }

    /// Keyboard on a menu screen (text fields, Esc). Returns true if consumed.
    pub(crate) fn menu_key(&mut self, code: KeyCode, text: Option<&str>) -> bool {
        let esc = code == KeyCode::Escape;
        match &mut self.screen {
            Some(Screen::EditServer { name, addr, focus, .. }) => {
                match code {
                    KeyCode::Tab => *focus = 1 - *focus,
                    KeyCode::Backspace => { if *focus == 0 { name.pop(); } else { addr.pop(); } }
                    KeyCode::Enter | KeyCode::NumpadEnter => self.menu_action("done"),
                    KeyCode::Escape => self.menu_action("cancel"),
                    _ => if let Some(t) = text { let f = if *focus == 0 { name } else { addr }; for c in t.chars() { if !c.is_control() && f.len() < 64 { f.push(c); } } },
                }
                true
            }
            Some(Screen::Worlds { search, .. }) => {
                match code {
                    KeyCode::Escape => self.menu_action("back"),
                    KeyCode::Backspace => { search.pop(); }
                    KeyCode::Enter | KeyCode::NumpadEnter => self.menu_action("play"),
                    _ => if let Some(t) = text { for c in t.chars() { if !c.is_control() && search.len() < 32 { search.push(c); } } },
                }
                true
            }
            Some(Screen::CreateWorld { meta, seed, focus }) => {
                match code {
                    KeyCode::Escape => self.menu_action("cancel"),
                    KeyCode::Tab => *focus = 1 - *focus,
                    KeyCode::Backspace => { if *focus == 0 { meta.name.pop(); } else { seed.pop(); } }
                    KeyCode::Enter | KeyCode::NumpadEnter => self.menu_action("create"),
                    _ => if let Some(t) = text { let f = if *focus == 0 { &mut meta.name } else { seed }; for c in t.chars() { if !c.is_control() && f.len() < 32 { f.push(c); } } },
                }
                true
            }
            Some(Screen::EditWorld { name, .. }) => {
                match code {
                    KeyCode::Escape => self.menu_action("cancel"),
                    KeyCode::Backspace => { name.pop(); }
                    KeyCode::Enter | KeyCode::NumpadEnter => self.menu_action("save"),
                    _ => if let Some(t) = text { for c in t.chars() { if !c.is_control() && name.len() < 32 { name.push(c); } } },
                }
                true
            }
            Some(Screen::DeleteWorld { .. }) if esc => { self.menu_action("cancel"); true }
            Some(Screen::Message { .. }) => true,
            Some(Screen::Direct { addr }) => {
                match code {
                    KeyCode::Backspace => { addr.pop(); }
                    KeyCode::Enter | KeyCode::NumpadEnter => self.menu_action("join"),
                    KeyCode::Escape => self.menu_action("cancel"),
                    _ => if let Some(t) = text { for c in t.chars() { if !c.is_control() && addr.len() < 64 { addr.push(c); } } },
                }
                true
            }
            Some(Screen::Pause) if esc => { self.screen = None; true }
            Some(Screen::Options { .. }) if esc => { self.menu_action("done"); true }
            Some(Screen::Page { .. }) | Some(Screen::Info { .. }) if esc => { self.menu_action("done"); true }
            Some(Screen::Multiplayer { .. }) if esc => { self.menu_action("back"); true }
            Some(Screen::Death { .. }) => true,
            Some(Screen::Title) => true,
            _ => false,
        }
    }

    /// Store an option; the ones Basalt acts on take effect right away.
    pub(crate) fn set_opt(&mut self, id: &str, v: f32) {
        match id {
            "render_distance" => self.cfg.render_distance = v as u32,
            "sensitivity" => self.cfg.sensitivity = v,
            "vol.master" => { self.cfg.master_volume = v; self.sounds.master = v; }
            _ => { self.cfg.extra.insert(id.to_string(), v); }
        }
        if let Some(cat) = id.strip_prefix("vol.") { self.sounds.set_category(cat, v); }
        self.cfg.save();
    }

    /// (code, "Name (Region)") of every language in the launcher asset store, en_us first.
    pub(crate) fn languages(&self) -> Vec<(String, String)> {
        let mut v = self.sounds.languages();
        if !v.iter().any(|(c, _)| c == "en_us") { v.insert(0, ("en_us".into(), "English (US)".into())); }
        v
    }
    pub(crate) fn set_language(&mut self, code: &str) {
        let json = if code == "en_us" { None } else { self.sounds.read_object(&format!("minecraft/lang/{code}.json")) };
        crate::lang::set_language(json.as_deref());
        self.cfg.language = code.to_string();
        self.cfg.save();
    }

    /// Scroll that puts the selected language in the middle of the list (like vanilla on open).
    fn language_scroll(&self) -> f32 {
        let gh = self.gui_size().1;
        let langs = self.languages();
        let i = langs.iter().position(|(c, _)| *c == self.cfg.language).unwrap_or(0) as f32;
        let max = (4.0 + langs.len() as f32 * 18.0 - (gh - 86.0)).max(0.0);
        (i * 18.0 - (gh - 86.0) / 2.0 + 9.0).clamp(0.0, max)
    }

    /// Mouse wheel over a scrolling page.
    pub(crate) fn menu_scroll(&mut self, dy: f32) {
        let gh = self.gui_size().1;
        let langs = self.languages().len();
        if let Some(Screen::Page { page, scroll, .. }) = &mut self.screen {
            let max = match page {
                Page::KeyBinds => (4.0 + BINDINGS.len() as f32 * 20.0 - (gh - 66.0)).max(0.0),
                Page::Language => (4.0 + langs as f32 * 18.0 - (gh - 86.0)).max(0.0),
                p => options::layout(&self.cfg, *p, 0.0, gh, 0.0).1,
            };
            *scroll = (*scroll - dy * 20.0).clamp(0.0, max);
        }
    }

    pub(crate) fn draw_menu(&self, ui: &mut MeshData) {
        let s = self.gui_scale();
        let a = &self.ui;
        let (w, h) = (self.gpu.size.0 as f32, self.gpu.size.1 as f32);
        // behind every other menu: the blurred world (in game) or the blurred panorama (render.rs Backdrop)
        if let Some(Screen::Death { .. }) = &self.screen { a.gradient(ui, 0.0, 0.0, w, h, [80, 0, 0, 96], [128, 48, 48, 160]); }
        if let Some(Screen::Message { text }) = &self.screen {
            let tw = a.text_width(text, 1.0);
            let (gw, gh) = self.gui_size();
            let (bx, by) = (((gw - tw) / 2.0 - 6.0).floor(), (gh / 2.0 - 10.0).floor());
            a.rect(ui, bx * s, by * s, (tw + 12.0) * s, 20.0 * s, [255; 4]);
            a.rect(ui, (bx + 1.0) * s, (by + 1.0) * s, (tw + 10.0) * s, 18.0 * s, [0, 0, 0, 255]);
        }
        if matches!(self.screen, Some(Screen::Worlds { .. })) {
            let iw = if self.connected && self.spawned { "inworld_" } else { "" };
            let (top, bottom) = (48.0 * s, h - 60.0 * s);
            let tile = 16.0 * s;
            let mut y = top; while y < bottom { let mut x = 0.0; while x < w { a.sprite_sized(ui, &format!("{iw}menu_list_background"), x, y, 16.0, ((bottom - y) / s).min(16.0), s, [255; 4]); x += tile; } y += tile; }
            let mut x = 0.0; while x < w { a.sprite_sized(ui, &format!("{iw}header_separator"), x, top - 2.0 * s, 32.0, 2.0, s, [255; 4]); a.sprite_sized(ui, &format!("{iw}footer_separator"), x, bottom, 32.0, 2.0, s, [255; 4]); x += 32.0 * s; }
        }
        if matches!(self.screen, Some(Screen::Page { .. }) | Some(Screen::Multiplayer { .. })) {
            // list area between a 33-unit header and footer: darker tiles + separator lines
            let iw = if self.connected && self.spawned { "inworld_" } else { "" };
            let foot = if matches!(self.screen, Some(Screen::Multiplayer { .. })) { 64.0 } else { 33.0 };
            let (top, bottom) = (33.0 * s, h - foot * s);
            let tile = 16.0 * s;
            let mut y = top; while y < bottom { let mut x = 0.0; while x < w { a.sprite_sized(ui, &format!("{iw}menu_list_background"), x, y, 16.0, ((bottom - y) / s).min(16.0), s, [255; 4]); x += tile; } y += tile; }
            let mut x = 0.0; while x < w { a.sprite_sized(ui, &format!("{iw}header_separator"), x, top - 2.0 * s, 32.0, 2.0, s, [255; 4]); a.sprite_sized(ui, &format!("{iw}footer_separator"), x, bottom, 32.0, 2.0, s, [255; 4]); x += 32.0 * s; }
        }
        if matches!(self.screen, Some(Screen::Title)) {
            let lx = ((w / s - 256.0) / 2.0).floor() * s;
            if a.size("title/minecraft").is_some() { a.sprite_sized(ui, "title/minecraft", lx, 30.0 * s, 256.0, 64.0, s, [255; 4]); }
            if a.size("title/edition").is_some() { a.sprite_sized(ui, "title/edition", lx + 64.0 * s, 67.0 * s, 128.0, 16.0, s, [255; 4]); }
            // splash: yellow, tilted 20°, pulsing, shrunk to fit (by eye against a vanilla recording)
            if !self.splash.is_empty() {
                let ms = self.menu_t0.elapsed().as_millis() % 1000;
                let pulse = 1.8 - ((ms as f32 / 1000.0 * std::f32::consts::TAU).sin()).abs() * 0.1;
                let k = pulse * 100.0 / (a.text_width(&self.splash, 1.0) + 32.0);
                let mut m = MeshData::default();
                let tw = a.text_width(&self.splash, 1.0);
                a.text(&mut m, &self.splash, -tw / 2.0, -4.0, 1.0, [255, 255, 0, 255]);
                let (sn, cs) = (-20f32).to_radians().sin_cos();
                let (ox, oy) = ((w / s / 2.0).floor() + 90.0, 70.0);
                let base = ui.verts.len() as u32;
                for vx in &mut m.verts {
                    let (x, y) = (vx.pos[0] * k, vx.pos[1] * k);
                    vx.pos = [(ox + x * cs - y * sn) * s, (oy + x * sn + y * cs) * s, 0.0];
                }
                ui.verts.extend(m.verts);
                ui.idx.extend(m.idx.iter().map(|i| i + base));
            }
        }
        let (mx, my) = self.mouse_pos;
        let hov = |wd: &Widget| { let (gx, gy) = (mx / s, my / s); gx >= wd.x && gx < wd.x + wd.w && gy >= wd.y && gy < wd.y + wd.h };
        for wd in self.widgets() {
            let (x, y) = (wd.x * s, wd.y * s);
            match &wd.kind {
                Kind::Button { label, enabled } => {
                    let spr = if !enabled { "sprites/widget/button_disabled" } else if hov(&wd) { "sprites/widget/button_highlighted" } else { "sprites/widget/button" };
                    a.nine(ui, spr, x, y, wd.w, wd.h, 3.0, s);
                    let col = if *enabled { [255; 4] } else { [160, 160, 160, 255] };
                    a.text_centered(ui, label, x + wd.w * s / 2.0, y + (wd.h - 8.0) / 2.0 * s, s, col);
                }
                Kind::Slider { label, value } => {
                    a.nine(ui, if hov(&wd) { "sprites/widget/slider_highlighted" } else { "sprites/widget/slider" }, x, y, wd.w, wd.h, 1.0, s);
                    a.nine(ui, if hov(&wd) { "sprites/widget/slider_handle_highlighted" } else { "sprites/widget/slider_handle" }, x + (value * (wd.w - 8.0)) * s, y, 8.0, 20.0, 1.0, s);
                    a.text_centered(ui, label, x + wd.w * s / 2.0, y + 6.0 * s, s, [255; 4]);
                }
                Kind::Field { text, focused, hint } => {
                    a.nine(ui, if *focused { "sprites/widget/text_field_highlighted" } else { "sprites/widget/text_field" }, x, y, wd.w, wd.h, 1.0, s);
                    let cursor = if *focused && (self.frames.1.elapsed().as_millis() / 300) % 2 == 0 { "_" } else { "" };
                    if text.is_empty() && !hint.is_empty() { a.text(ui, hint, x + 4.0 * s, y + 6.0 * s, s, [128, 128, 128, 255]); }
                    a.text(ui, &format!("{text}{cursor}"), x + 4.0 * s, y + 6.0 * s, s, [224, 224, 224, 255]);
                }
                Kind::Label { text, color, scale } => a.text_centered(ui, text, wd.x * s, y, s * scale, *color),
                Kind::Icon { sprite, .. } => {
                    a.nine(ui, if hov(&wd) { "sprites/widget/button_highlighted" } else { "sprites/widget/button" }, x, y, wd.w, wd.h, 3.0, s);
                    if sprite == "list" {
                        for r in 0..4 { a.rect(ui, x + 5.0 * s, y + (5.0 + r as f32 * 3.0) * s, s, 2.0 * s, [255; 4]); a.rect(ui, x + 7.0 * s, y + (5.0 + r as f32 * 3.0) * s, 8.0 * s, 2.0 * s, [255; 4]); }
                    } else if let Some((iw, ih)) = a.size(sprite) {
                        a.sprite(ui, sprite, x + ((wd.w - iw) / 2.0).floor() * s, y + ((wd.h - ih) / 2.0).floor() * s, s);
                    }
                }
                Kind::World { index, selected } => {
                    let Some(wl) = self.worlds.get(*index) else { continue };
                    if *selected { a.rect(ui, x, y, wd.w * s, wd.h * s, [160, 160, 160, 255]); a.rect(ui, x + s, y + s, (wd.w - 2.0) * s, (wd.h - 2.0) * s, [0, 0, 0, 255]); }
                    else if hov(&wd) { a.rect(ui, x, y, wd.w * s, wd.h * s, [255, 255, 255, 24]); }
                    let tx = x + 35.0 * s;
                    a.text(ui, &wl.meta.name, tx, y + 1.0 * s, s, [255; 4]);
                    let when = crate::worlds::format_time(wl.meta.last_played);
                    a.text(ui, &format!("{} ({when})", wl.folder), tx, y + 12.0 * s, s, [128, 128, 128, 255]);
                    let gm = ["gameMode.survival", "gameMode.creative", "gameMode.hardcore", "gameMode.adventure"][(wl.meta.gamemode as usize).min(3)];
                    let mut info = t(gm);
                    if wl.meta.commands { info.push_str(&format!(", {}", t("selectWorld.commands"))); }
                    info.push_str(&format!(", {} 26.2", t("selectWorld.version")));
                    a.text(ui, &info, tx, y + 23.0 * s, s, [128, 128, 128, 255]);
                }
                Kind::Server { index, selected } => {
                    let Some(srv) = self.cfg.servers.get(*index) else { continue };
                    if *selected { a.rect(ui, x, y, wd.w * s, wd.h * s, [160, 160, 160, 255]); a.rect(ui, x + s, y + s, (wd.w - 2.0) * s, (wd.h - 2.0) * s, [0, 0, 0, 255]); }
                    else if hov(&wd) { a.rect(ui, x, y, wd.w * s, wd.h * s, [255, 255, 255, 24]); }
                    let (tx, right) = (x + 35.0 * s, x + (wd.w - 2.0) * s);
                    a.text(ui, &srv.name, tx, y + 3.0 * s, s, [255; 4]);
                    let ping_spr: String = match self.pings.get(&srv.addr) {
                        Some(crate::status::PingState::Done(st)) => {
                            for (li, line) in st.motd.iter().enumerate() {
                                let mut lx = tx;
                                for (txt, col) in line { a.text(ui, txt, lx, y + (14.0 + li as f32 * 9.0) * s, s, *col); lx += a.text_width(txt, s); }
                            }
                            let compatible = st.protocol == basalt_protocol::PROTOCOL_VERSION as i64;
                            let (info, col) = if compatible { (format!("{}/{}", st.online, st.max), [128, 128, 128, 255]) } else { (st.version.clone(), [255, 85, 85, 255]) };
                            a.text(ui, &info, right - 13.0 * s - a.text_width(&info, s), y + 3.0 * s, s, col);
                            let bars = match st.ping_ms { 0..=149 => 5, 150..=299 => 4, 300..=599 => 3, 600..=999 => 2, _ => 1 };
                            if compatible { format!("sprites/server_list/ping_{bars}") } else { "sprites/server_list/incompatible".into() }
                        }
                        Some(crate::status::PingState::Failed(_)) => {
                            a.text(ui, &t("multiplayer.status.cannot_connect"), tx, y + 14.0 * s, s, [255, 85, 85, 255]);
                            "sprites/server_list/unreachable".into()
                        }
                        Some(crate::status::PingState::Pinging(t0)) => {
                            a.text(ui, &t("multiplayer.status.pinging"), tx, y + 14.0 * s, s, [128, 128, 128, 255]);
                            let f = (t0.elapsed().as_millis() / 100 % 8) as usize;
                            format!("sprites/server_list/pinging_{}", if f > 4 { 8 - f } else { f + 1 })
                        }
                        None => "sprites/server_list/pinging_1".into(),
                    };
                    a.sprite(ui, &ping_spr, right - 10.0 * s, y + 3.0 * s, s);
                }
                Kind::Row { title, sub, selected } if sub == "\u{0}center" => {
                    // single-line centred row (language list): white frame when selected
                    if *selected { a.rect(ui, x, y, wd.w * s, wd.h * s, [255; 4]); a.rect(ui, x + s, y + s, (wd.w - 2.0) * s, (wd.h - 2.0) * s, [0, 0, 0, 255]); }
                    else if hov(&wd) { a.rect(ui, x, y, wd.w * s, wd.h * s, [255, 255, 255, 32]); }
                    a.text_centered(ui, title, x + wd.w * s / 2.0, y + 5.0 * s, s, [255; 4]);
                }
                Kind::Row { title, sub, selected } => {
                    if *selected { a.rect(ui, x - s, y - s, (wd.w + 2.0) * s, (wd.h + 2.0) * s, [128, 128, 128, 255]); a.rect(ui, x, y, wd.w * s, wd.h * s, [0, 0, 0, 255]); }
                    else if hov(&wd) { a.rect(ui, x, y, wd.w * s, wd.h * s, [255, 255, 255, 32]); }
                    a.text(ui, title, x + 4.0 * s, y + 4.0 * s, s, [255; 4]);
                    a.text(ui, sub, x + 4.0 * s, y + 16.0 * s, s, [128, 128, 128, 255]);
                }
            }
        }
        // tooltip of the hovered icon button
        if let Some(Kind::Icon { tooltip, .. }) = self.widgets().into_iter().find(|wd| hov(wd) && matches!(wd.kind, Kind::Icon { .. })).map(|wd| wd.kind) {
            self.tooltip(ui, &tooltip, mx / s, my / s);
        }
    }

    /// Server favicons (icon texture layer, drawn after `draw_menu`).
    pub(crate) fn draw_menu_icons(&self, m: &mut MeshData) {
        let s = self.gui_scale();
        for wd in self.widgets() {
            let key = match wd.kind {
                Kind::Server { index, .. } => match self.cfg.servers.get(index) { Some(srv) => srv.addr.clone(), None => continue },
                Kind::World { index, .. } => match self.worlds.get(index) { Some(w) => format!("world:{}", w.folder), None => continue },
                _ => continue,
            };
            if let Some(&slot) = self.icon_slots.get(&key) {
                self.ui.quad(m, (wd.x + 2.0) * s, (wd.y + 2.0) * s, 32.0 * s, 32.0 * s, crate::render::icon_uv(slot), [255; 4]);
            }
        }
    }

    /// Vanilla-style tooltip box (dark purple frame) at the mouse, GUI units.
    pub(crate) fn tooltip(&self, ui: &mut MeshData, text: &str, gx: f32, gy: f32) {
        let (s, a) = (self.gui_scale(), &self.ui);
        let (tw, (gw, _)) = (a.text_width(text, 1.0), self.gui_size());
        let x = (gx + 12.0).min(gw - tw - 4.0);
        let y = gy - 12.0;
        a.rect(ui, (x - 3.0) * s, (y - 4.0) * s, (tw + 6.0) * s, 16.0 * s, [16, 0, 16, 240]);
        a.rect(ui, (x - 2.0) * s, (y - 3.0) * s, (tw + 4.0) * s, s, [80, 0, 255, 80]);
        a.rect(ui, (x - 2.0) * s, (y + 10.0) * s, (tw + 4.0) * s, s, [40, 0, 127, 80]);
        a.text(ui, text, x * s, y * s, s, [255; 4]);
    }
}

pub fn wrap_text(s: &str, max: f32, width: impl Fn(&str) -> f32) -> Vec<String> {
    let mut out = Vec::new(); let mut line = String::new();
    for word in s.split(' ') {
        let cand = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if width(&cand) > max && !line.is_empty() { out.push(std::mem::take(&mut line)); line = word.to_string(); } else { line = cand; }
    }
    if !line.is_empty() { out.push(line); }
    out
}

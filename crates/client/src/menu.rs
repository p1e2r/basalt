//! Menu screens (title, multiplayer list, direct connect, pause, options, controls, death) built
//! from the jar's widget sprites, plus the local config file (options + saved servers).
use crate::container::Open;
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
}

impl Default for Config {
    fn default() -> Self { Config { fov: 70.0, render_distance: 10, sensitivity: 0.5, gui_scale: 0, master_volume: 1.0, fullscreen: false, name: "Basalt".into(), servers: Vec::new() } }
}

impl Config {
    pub fn dir() -> std::path::PathBuf {
        if let Ok(d) = std::env::var("BASALT_CONFIG_DIR") { return d.into(); }
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        std::path::Path::new(&home).join(".basalt")
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
    Controls { from_pause: bool },
    Death { msg: String, at: Instant },
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum Kind { Button { label: String, enabled: bool }, Slider { label: String, value: f32 }, Field { text: String, focused: bool, hint: String }, Label { text: String, color: [u8; 4], scale: f32 }, Row { title: String, sub: String, selected: bool } }

/// A widget in GUI units.
#[derive(Clone, Debug)]
pub struct Widget { pub id: String, pub x: f32, pub y: f32, pub w: f32, pub h: f32, pub kind: Kind }

fn button(id: &str, label: &str, x: f32, y: f32, w: f32) -> Widget { Widget { id: id.into(), x, y, w, h: 20.0, kind: Kind::Button { label: label.into(), enabled: true } } }
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
        match &self.screen {
            Some(Screen::Title) => {
                let y = (gh / 4.0 + 48.0).floor();
                let mut sp = button("singleplayer", &t("menu.singleplayer"), cx - 100.0, y, 200.0);
                sp.kind = Kind::Button { label: t("menu.singleplayer"), enabled: false };
                v.push(sp);
                v.push(button("multiplayer", &t("menu.multiplayer"), cx - 100.0, y + 24.0, 200.0));
                v.push(button("options", &t("menu.options"), cx - 100.0, y + 72.0 + 12.0, 98.0));
                v.push(button("quit", &t("menu.quit"), cx + 2.0, y + 72.0 + 12.0, 98.0));
                v.push(label("Basalt 26.2 (clean-room)", 2.0 + self.ui.text_width("Basalt 26.2 (clean-room)", 1.0) / 2.0, gh - 10.0, white, 1.0));
            }
            Some(Screen::Multiplayer { selected, .. }) => {
                v.push(label(&t("multiplayer.title"), cx, 12.0, white, 1.0));
                for (i, s) in self.cfg.servers.iter().enumerate() {
                    v.push(Widget { id: format!("row{i}"), x: cx - 152.0, y: 32.0 + i as f32 * 36.0, w: 304.0, h: 32.0, kind: Kind::Row { title: s.name.clone(), sub: s.addr.clone(), selected: *selected == Some(i) } });
                }
                let by = gh - 52.0;
                let sel = selected.is_some();
                let mut join = button("join", &t("selectServer.select"), cx - 154.0, by, 100.0); join.kind = Kind::Button { label: t("selectServer.select"), enabled: sel }; v.push(join);
                v.push(button("direct", &t("selectServer.direct"), cx - 50.0, by, 100.0));
                v.push(button("add", &t("selectServer.add"), cx + 54.0, by, 100.0));
                let mut edit = button("edit", &t("selectServer.edit"), cx - 154.0, by + 24.0, 74.0); edit.kind = Kind::Button { label: t("selectServer.edit"), enabled: sel }; v.push(edit);
                let mut del = button("delete", &t("selectServer.delete"), cx - 76.0, by + 24.0, 74.0); del.kind = Kind::Button { label: t("selectServer.delete"), enabled: sel }; v.push(del);
                v.push(button("back", &t("gui.back"), cx + 4.0 + 76.0, by + 24.0, 74.0));
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
                let y = gh / 4.0 + 8.0;
                v.push(button("resume", &t("menu.returnToGame"), cx - 102.0, y, 204.0));
                v.push(button("options", &t("menu.options"), cx - 102.0, y + 96.0, 204.0));
                v.push(button("disconnect", &t("menu.disconnect"), cx - 102.0, y + 120.0, 204.0));
            }
            Some(Screen::Options { .. }) => {
                v.push(label(&t("options.title"), cx, 15.0, white, 1.0));
                let c = &self.cfg;
                let fov_label = if c.fov.round() as i32 == 70 { format!("{}: Normal", t("options.fov")) } else { format!("{}: {}", t("options.fov"), c.fov.round()) };
                let y0 = 40.0;
                let sl = |id: &str, l: String, val: f32, x: f32, y: f32| Widget { id: id.into(), x, y, w: 150.0, h: 20.0, kind: Kind::Slider { label: l, value: val.clamp(0.0, 1.0) } };
                v.push(sl("fov", fov_label, (c.fov - 30.0) / 80.0, cx - 155.0, y0));
                v.push(sl("render", format!("{}: {} chunks", t("options.renderDistance"), c.render_distance), (c.render_distance as f32 - 2.0) / 30.0, cx + 5.0, y0));
                v.push(sl("sens", format!("{}: {}%", t("options.sensitivity"), (c.sensitivity * 200.0).round()), c.sensitivity, cx - 155.0, y0 + 24.0));
                v.push(sl("volume", format!("{}: {}%", t("soundCategory.master"), (c.master_volume * 100.0).round()), c.master_volume, cx + 5.0, y0 + 24.0));
                let gs = if c.gui_scale == 0 { "Auto".to_string() } else { c.gui_scale.to_string() };
                v.push(button("gui", &format!("{}: {gs}", t("options.guiScale")), cx - 155.0, y0 + 48.0, 150.0));
                v.push(button("fullscreen", &format!("{}: {}", t("options.fullscreen"), if c.fullscreen { "ON" } else { "OFF" }), cx + 5.0, y0 + 48.0, 150.0));
                v.push(button("controls", &t("options.controls"), cx - 155.0, y0 + 72.0, 150.0));
                v.push(button("done", &t("gui.done"), cx - 100.0, gh - 27.0, 200.0));
            }
            Some(Screen::Controls { .. }) => {
                v.push(label(&t("controls.title"), cx, 12.0, white, 1.0));
                for (i, (k, key)) in BINDINGS.iter().enumerate() {
                    let y = 30.0 + i as f32 * 11.0;
                    if y > gh - 40.0 { break; }
                    let name = t(k);
                    v.push(label(&name, cx - 80.0, y, white, 1.0));
                    v.push(label(key, cx + 80.0, y, [255, 255, 160, 255], 1.0));
                }
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
            }
            Kind::Row { .. } => {
                let i: usize = w.id[3..].parse().unwrap_or(0);
                if let Some(Screen::Multiplayer { selected, last_click }) = &mut self.screen {
                    let dbl = last_click.map(|(j, t)| j == i && t.elapsed().as_millis() < 300).unwrap_or(false);
                    *selected = Some(i); *last_click = Some((i, Instant::now()));
                    if dbl { self.menu_action("join"); }
                }
            }
            Kind::Label { .. } => {}
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
            _ => {}
        }
        self.cfg.save();
    }

    pub(crate) fn menu_action(&mut self, id: &str) {
        let screen = self.screen.take();
        self.screen = match (screen, id) {
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
            (Some(Screen::Pause), "disconnect") => { self.disconnect(); Some(Screen::Title) }
            (Some(Screen::Options { from_pause }), "gui") => { self.cfg.gui_scale = (self.cfg.gui_scale + 1) % 5; self.cfg.save(); Some(Screen::Options { from_pause }) }
            (Some(Screen::Options { from_pause }), "fullscreen") => { self.cfg.fullscreen = !self.cfg.fullscreen; self.cfg.save(); self.fullscreen_changed = true; Some(Screen::Options { from_pause }) }
            (Some(Screen::Options { from_pause }), "controls") => Some(Screen::Controls { from_pause }),
            (Some(Screen::Options { from_pause }), "done") => { self.cfg.save(); Some(if from_pause { Screen::Pause } else { Screen::Title }) }
            (Some(Screen::Controls { from_pause }), "done") => Some(Screen::Options { from_pause }),
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
            Some(Screen::Controls { .. }) if esc => { self.menu_action("done"); true }
            Some(Screen::Multiplayer { .. }) if esc => { self.menu_action("back"); true }
            Some(Screen::Death { .. }) => true,
            Some(Screen::Title) => true,
            _ => false,
        }
    }

    pub(crate) fn draw_menu(&self, ui: &mut MeshData) {
        let s = self.gui_scale();
        let a = &self.ui;
        let (w, h) = (self.gpu.size.0 as f32, self.gpu.size.1 as f32);
        let in_world = self.connected && self.spawned;
        match &self.screen {
            Some(Screen::Death { .. }) => a.gradient(ui, 0.0, 0.0, w, h, [80, 0, 0, 96], [128, 48, 48, 160]),
            Some(Screen::Pause) | Some(Screen::Options { from_pause: true }) | Some(Screen::Controls { from_pause: true }) if in_world => a.gradient(ui, 0.0, 0.0, w, h, [16, 16, 16, 192], [16, 16, 16, 208]),
            _ => {
                // tiled menu background, darkened
                let tile = 32.0 * s;
                let mut y = 0.0;
                while y < h { let mut x = 0.0; while x < w { a.sprite_sized(ui, "menu_background", x, y, 32.0, 32.0, s, [90, 90, 90, 255]); x += tile; } y += tile; }
            }
        }
        if matches!(self.screen, Some(Screen::Title)) {
            if let Some((lw, lh)) = a.size("title/minecraft") {
                let (dw, dh) = (256.0, 256.0 * lh / lw);
                a.sprite_sized(ui, "title/minecraft", ((w / s - dw) / 2.0).floor() * s, 30.0 * s, dw, dh, s, [255; 4]);
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
                Kind::Field { text, focused, .. } => {
                    a.nine(ui, if *focused { "sprites/widget/text_field_highlighted" } else { "sprites/widget/text_field" }, x, y, wd.w, wd.h, 1.0, s);
                    let cursor = if *focused && (self.frames.1.elapsed().as_millis() / 300) % 2 == 0 { "_" } else { "" };
                    a.text(ui, &format!("{text}{cursor}"), x + 4.0 * s, y + 6.0 * s, s, [224, 224, 224, 255]);
                }
                Kind::Label { text, color, scale } => a.text_centered(ui, text, wd.x * s, y, s * scale, *color),
                Kind::Row { title, sub, selected } => {
                    if *selected { a.rect(ui, x - s, y - s, (wd.w + 2.0) * s, (wd.h + 2.0) * s, [128, 128, 128, 255]); a.rect(ui, x, y, wd.w * s, wd.h * s, [0, 0, 0, 255]); }
                    else if hov(&wd) { a.rect(ui, x, y, wd.w * s, wd.h * s, [255, 255, 255, 32]); }
                    a.text(ui, title, x + 4.0 * s, y + 4.0 * s, s, [255; 4]);
                    a.text(ui, sub, x + 4.0 * s, y + 16.0 * s, s, [128, 128, 128, 255]);
                }
            }
        }
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

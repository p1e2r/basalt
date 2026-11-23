//! Container screens: the player inventory (E), crafting table and generic chests opened by
//! open_screen. Slot positions follow the jar's container textures (the well-known 176×166 layout).
//! Clicks are predicted locally and sent as the 26.2 container_click (window, state id, slot,
//! button, mode, changed slots as hashed stacks, carried hashed stack); the server's
//! container_set_content / set_slot / set_cursor_item are authoritative.
use crate::mesh::MeshData;
use crate::net::ItemStack;
use crate::Game;
use basalt_protocol::{ids::play::serverbound as sb, Writer};
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Debug)]
/// `Furnace(0)` furnace, `Furnace(1)` blast furnace, `Furnace(2)` smoker (same layout, different background/sprites).
pub enum Kind { Inventory, Crafting, Generic(u32), Furnace(u8), Other }

pub struct Open {
    pub window: i32,
    pub kind: Kind,
    pub title: String,
    /// slots of this window (for the inventory, mirrors Game::inventory)
    pub slots: Vec<ItemStack>,
    pub state: i32,
    /// pending drag: (button 0/1/2, slots visited)
    pub drag: Option<(u8, Vec<i16>)>,
    pub last_click: Option<(i16, Instant)>,
    pub hover: Option<i16>,
    /// container_set_data properties (furnace: 0 burn time left, 1 burn duration, 2 cook progress, 3 cook total)
    pub props: [i16; 4],
}

impl Kind {
    pub fn from_menu(menu: i32) -> Kind {
        match basalt_data::registries::MENU.get(menu as usize).copied().unwrap_or("") {
            "crafting" => Kind::Crafting,
            "generic_9x1" => Kind::Generic(1), "generic_9x2" => Kind::Generic(2), "generic_9x3" => Kind::Generic(3),
            "furnace" => Kind::Furnace(0), "blast_furnace" => Kind::Furnace(1), "smoker" => Kind::Furnace(2),
            "generic_9x4" => Kind::Generic(4), "generic_9x5" => Kind::Generic(5), "generic_9x6" => Kind::Generic(6),
            _ => Kind::Other,
        }
    }
    pub fn size(self) -> (f32, f32) { match self { Kind::Generic(r) => (176.0, 114.0 + r as f32 * 18.0), _ => (176.0, 166.0) } }
    pub fn slot_count(self) -> usize { match self { Kind::Inventory | Kind::Crafting => 46, Kind::Generic(r) => r as usize * 9 + 36, Kind::Furnace(_) => 39, Kind::Other => 36 } }
    /// first slot index of the player's main inventory (27 slots, then 9 hotbar)
    pub fn player_start(self) -> usize { match self { Kind::Inventory => 9, Kind::Crafting => 10, Kind::Generic(r) => r as usize * 9, Kind::Furnace(_) => 3, Kind::Other => 0 } }
    /// GUI-local top-left of slot i (16×16 item area).
    pub fn slot_pos(self, i: usize) -> Option<(f32, f32)> {
        let ps = self.player_start();
        let inv_y = match self { Kind::Generic(r) => 18.0 + r as f32 * 18.0 + 13.0, _ => 84.0 };
        if i >= ps && i < ps + 36 {
            let j = i - ps;
            return Some(if j < 27 { (8.0 + (j % 9) as f32 * 18.0, inv_y + (j / 9) as f32 * 18.0) } else { (8.0 + (j - 27) as f32 * 18.0, inv_y + 58.0) });
        }
        match self {
            Kind::Inventory => match i {
                0 => Some((154.0, 28.0)),
                1..=4 => Some((98.0 + ((i - 1) % 2) as f32 * 18.0, 18.0 + ((i - 1) / 2) as f32 * 18.0)),
                5..=8 => Some((8.0, 8.0 + (i - 5) as f32 * 18.0)),
                45 => Some((77.0, 62.0)),
                _ => None,
            },
            Kind::Crafting => match i {
                0 => Some((124.0, 35.0)),
                1..=9 => Some((30.0 + ((i - 1) % 3) as f32 * 18.0, 17.0 + ((i - 1) / 3) as f32 * 18.0)),
                _ => None,
            },
            Kind::Generic(_) => Some((8.0 + (i % 9) as f32 * 18.0, 18.0 + (i / 9) as f32 * 18.0)),
            // furnace.png: input above the flame, fuel below it, result right of the arrow
            Kind::Furnace(_) => match i { 0 => Some((56.0, 17.0)), 1 => Some((56.0, 53.0)), 2 => Some((116.0, 35.0)), _ => None },
            Kind::Other => None,
        }
    }
    pub fn is_result(self, i: i16) -> bool { (matches!(self, Kind::Inventory | Kind::Crafting) && i == 0) || (matches!(self, Kind::Furnace(_)) && i == 2) }
}

/// Hashed stack as sent in container_click: present flag, item, count, component hashes.
/// Only component-free stacks can be hashed exactly; for others the server resyncs the slot.
pub fn write_hashed(w: &mut Writer, s: &ItemStack) {
    if s.count <= 0 { w.bool(false); return; }
    w.bool(true).varint(s.item).varint(s.count).varint(0).varint(0);
}

impl Game {
    pub(crate) fn gui_scale(&self) -> f32 {
        let (w, h) = (self.gpu.size.0 as f32, self.gpu.size.1 as f32);
        let auto = (w / 320.0).min(h / 240.0).floor().max(1.0);
        if self.cfg.gui_scale > 0 { (self.cfg.gui_scale as f32).min(auto) } else { auto }
    }
    /// Screen-pixel origin of the open container's background.
    pub(crate) fn container_origin(&self, kind: Kind) -> (f32, f32) {
        let s = self.gui_scale();
        let (w, h) = kind.size();
        (((self.gpu.size.0 as f32 / s - w) / 2.0).floor() * s, ((self.gpu.size.1 as f32 / s - h) / 2.0).floor() * s)
    }
    pub(crate) fn slot_at(&self, c: &Open, mx: f32, my: f32) -> Option<i16> {
        let s = self.gui_scale();
        let (ox, oy) = self.container_origin(c.kind);
        for i in 0..c.slots.len() {
            if let Some((x, y)) = c.kind.slot_pos(i) {
                let (sx, sy) = (ox + (x - 1.0) * s, oy + (y - 1.0) * s);
                if mx >= sx && mx < sx + 18.0 * s && my >= sy && my < sy + 18.0 * s { return Some(i as i16); }
            }
        }
        None
    }
    fn inside_container(&self, c: &Open, mx: f32, my: f32) -> bool {
        let s = self.gui_scale(); let (ox, oy) = self.container_origin(c.kind); let (w, h) = c.kind.size();
        mx >= ox && my >= oy && mx < ox + w * s && my < oy + h * s
    }

    pub(crate) fn open_inventory(&mut self) {
        let slots = self.inventory.clone();
        self.screen = Some(crate::menu::Screen::Container(Open { window: 0, kind: Kind::Inventory, title: crate::lang::tr("container.crafting", &[]), slots, state: self.inv_state, drag: None, last_click: None, hover: None, props: [0; 4] }));
    }
    pub(crate) fn close_container(&mut self, send_close: bool) {
        if let Some(crate::menu::Screen::Container(c)) = &self.screen {
            if send_close { let mut w = Writer::with_id(sb::CONTAINER_CLOSE); w.varint(c.window); crate::send(&self.out, w); }
            // vanilla clears the cursor client-side on close (the server returns it to the inventory)
            self.carried = ItemStack::default();
            self.screen = None;
        }
    }

    fn max_stack(&self, item: i32) -> i32 { self.rules.as_ref().and_then(|r| r.item(item)).map(|i| i.max_stack).unwrap_or(64).max(1) }
    pub(crate) fn max_damage(&self, item: i32) -> i32 { self.rules.as_ref().and_then(|r| r.item(item)).map(|i| i.max_damage).unwrap_or(0) }

    /// Send a click with local prediction. `slot` -999 = outside.
    pub(crate) fn click(&mut self, slot: i16, button: i8, mode: i32) {
        let Some(crate::menu::Screen::Container(c)) = &self.screen else { return };
        let (window, state, kind) = (c.window, c.state, c.kind);
        let before = c.slots.clone();
        let mut slots = before.clone();
        let mut carried = self.carried.clone();
        let max = |g: &Game, it: &ItemStack| g.max_stack(it.item);
        let idx = slot as usize;
        let valid = slot >= 0 && idx < slots.len();
        match mode {
            0 if slot == -999 => { if !carried.is_empty() { carried = if button == 0 { ItemStack::default() } else { carried.with_count(carried.count - 1) }; } }
            0 if valid && kind.is_result(slot) => {} // crafting result: server decides
            0 if valid => {
                let s = slots[idx].clone();
                if button == 0 {
                    if carried.is_empty() { slots[idx] = ItemStack::default(); carried = s; }
                    else if s.is_empty() || s.same_item(&carried) {
                        let n = (max(self, &carried) - s.count.max(0)).min(carried.count).max(0);
                        slots[idx] = carried.with_count(s.count.max(0) + n); carried = carried.with_count(carried.count - n);
                    } else { slots[idx] = carried; carried = s; }
                } else if carried.is_empty() {
                    let half = (s.count + 1) / 2; slots[idx] = s.with_count(s.count - half); carried = s.with_count(half);
                } else if s.is_empty() || (s.same_item(&carried) && s.count < max(self, &carried)) {
                    slots[idx] = carried.with_count(s.count.max(0) + 1); carried = carried.with_count(carried.count - 1);
                } else if !s.same_item(&carried) { slots[idx] = carried; carried = s; }
            }
            2 if valid && !kind.is_result(slot) => {
                let hb = if button == 40 { if kind == Kind::Inventory { Some(45) } else { None } } else { Some(kind.player_start() + 27 + button as usize) };
                if let Some(hb) = hb { if hb < slots.len() { slots.swap(idx, hb); } }
            }
            4 if valid && !kind.is_result(slot) && carried.is_empty() => { let s = &slots[idx]; slots[idx] = if button == 1 { ItemStack::default() } else { s.with_count(s.count - 1) }; }
            6 if !carried.is_empty() => {
                let m = max(self, &carried);
                for pass in 0..2 { for i in 0..slots.len() {
                    if carried.count >= m || kind.is_result(i as i16) { continue; }
                    let t = &slots[i];
                    if t.is_empty() || !t.same_item(&carried) || (pass == 0 && t.count >= m) { continue; }
                    let n = (m - carried.count).min(t.count);
                    carried.count += n; slots[i] = t.with_count(t.count - n);
                } }
            }
            _ => {}
        }
        self.send_click(window, state, slot, button, mode, &before, &slots, &carried);
        self.apply_prediction(slots, carried);
    }

    fn send_click(&mut self, window: i32, state: i32, slot: i16, button: i8, mode: i32, before: &[ItemStack], after: &[ItemStack], carried: &ItemStack) {
        let changed: Vec<usize> = (0..after.len()).filter(|&i| before.get(i) != after.get(i)).collect();
        let mut w = Writer::with_id(sb::CONTAINER_CLICK);
        w.varint(window).varint(state).i16(slot).i8(button).varint(mode).varint(changed.len() as i32);
        for i in changed { w.i16(i as i16); write_hashed(&mut w, &after[i]); }
        write_hashed(&mut w, carried);
        crate::send(&self.out, w);
        self.clicks_sent += 1;
    }

    fn apply_prediction(&mut self, slots: Vec<ItemStack>, carried: ItemStack) {
        self.carried = carried;
        if let Some(crate::menu::Screen::Container(c)) = &mut self.screen {
            if c.window == 0 { self.inventory = slots.clone(); for i in 0..9 { self.hotbar[i] = self.inventory[36 + i].clone(); } }
            c.slots = slots;
        }
    }

    /// Finish a drag (quick-craft) over `slots`: start, one packet per slot, end — like vanilla.
    pub(crate) fn send_drag(&mut self, button: u8, visited: &[i16]) {
        let Some(crate::menu::Screen::Container(c)) = &self.screen else { return };
        let (window, state, kind) = (c.window, c.state, c.kind);
        let before = c.slots.clone();
        let mut slots = before.clone();
        let mut carried = self.carried.clone();
        let targets: Vec<usize> = visited.iter().map(|&s| s as usize).filter(|&i| i < slots.len() && !kind.is_result(i as i16) && (slots[i].is_empty() || slots[i].same_item(&carried))).collect();
        if !carried.is_empty() && !targets.is_empty() {
            let m = self.max_stack(carried.item);
            let per = match button { 0 => (carried.count / targets.len() as i32).max(1), 1 => 1, _ => m };
            let creative_fill = button == 2;
            let mut left = carried.count;
            for &t in &targets {
                if left == 0 && !creative_fill { break; }
                let s = slots[t].count.max(0);
                let n = if creative_fill { m - s } else { per.min(m - s).min(left) };
                if n <= 0 { continue; }
                slots[t] = carried.with_count(s + n);
                if !creative_fill { left -= n; }
            }
            carried = carried.with_count(left);
        }
        let b0 = button as i8 * 4;
        // start, add each slot, end (the end carries the predicted changes)
        let mut w = Writer::with_id(sb::CONTAINER_CLICK); w.varint(window).varint(state).i16(-999).i8(b0).varint(5).varint(0); write_hashed(&mut w, &self.carried); crate::send(&self.out, w);
        for &s in visited { let mut w = Writer::with_id(sb::CONTAINER_CLICK); w.varint(window).varint(state).i16(s).i8(b0 + 1).varint(5).varint(0); write_hashed(&mut w, &self.carried); crate::send(&self.out, w); }
        self.send_click(window, state, -999, b0 + 2, 5, &before, &slots, &carried);
        self.apply_prediction(slots, carried);
    }

    // ------------------------------------------------------------ input

    pub(crate) fn container_mouse(&mut self, button: u8, pressed: bool) {
        let (mx, my) = self.mouse_pos;
        let Some(crate::menu::Screen::Container(c)) = &self.screen else { return };
        let slot = self.slot_at(c, mx, my);
        let outside = !self.inside_container(c, mx, my);
        let shift = self.keys.contains(&winit::keyboard::KeyCode::ShiftLeft) || self.keys.contains(&winit::keyboard::KeyCode::ShiftRight);
        let carrying = !self.carried.is_empty();
        if pressed {
            if button > 2 { return; }
            // double click: gather all of the carried item
            let dbl = button == 0 && c.last_click.map(|(s, t)| Some(s) == slot && t.elapsed().as_millis() < 250).unwrap_or(false);
            if let Some(crate::menu::Screen::Container(c)) = &mut self.screen { c.last_click = if button == 0 && !dbl { slot.map(|s| (s, Instant::now())) } else { None }; }
            if dbl && carrying { if let Some(s) = slot { self.click(s, 0, 6); } return; }
            if button == 2 { if let Some(s) = slot { if self.player.gamemode == 1 { self.click(s, 2, 3); } } return; }
            if carrying && slot.is_some() && !shift {
                if let Some(crate::menu::Screen::Container(c)) = &mut self.screen { c.drag = Some((button, vec![slot.unwrap()])); }
                return;
            }
            match slot {
                Some(s) => self.click(s, button as i8, if shift { 1 } else { 0 }),
                None if outside => self.click(-999, button as i8, 0),
                None => {}
            }
        } else {
            let drag = if let Some(crate::menu::Screen::Container(c)) = &mut self.screen { c.drag.take() } else { None };
            if let Some((b, visited)) = drag {
                if visited.len() >= 2 { self.send_drag(b, &visited); }
                else if let Some(&s) = visited.first() { self.click(s, b as i8, 0); }
            }
        }
    }

    pub(crate) fn container_motion(&mut self) {
        let (mx, my) = self.mouse_pos;
        let Some(crate::menu::Screen::Container(c)) = &self.screen else { return };
        let slot = self.slot_at(c, mx, my);
        if let Some(crate::menu::Screen::Container(c)) = &mut self.screen {
            c.hover = slot;
            if let (Some((_, v)), Some(s)) = (&mut c.drag, slot) { if !v.contains(&s) && !c.kind.is_result(s) { v.push(s); } }
        }
    }

    /// Keys while a container is open: number keys swap with the hotbar, Q throws, E/Esc close.
    pub(crate) fn container_key(&mut self, code: winit::keyboard::KeyCode) {
        use winit::keyboard::KeyCode as K;
        let hover = if let Some(crate::menu::Screen::Container(c)) = &self.screen { c.hover } else { return };
        let digits = [K::Digit1, K::Digit2, K::Digit3, K::Digit4, K::Digit5, K::Digit6, K::Digit7, K::Digit8, K::Digit9];
        if let Some(i) = digits.iter().position(|d| *d == code) { if let Some(s) = hover { self.click(s, i as i8, 2); } return; }
        match code {
            K::KeyF => { if let Some(s) = hover { self.click(s, 40, 2); } }
            K::KeyQ => { if let Some(s) = hover { let ctrl = self.keys.contains(&K::ControlLeft) || self.keys.contains(&K::SuperLeft); self.click(s, ctrl as i8, 4); } }
            K::KeyE | K::Escape => self.close_container(true),
            _ => {}
        }
    }

    // ------------------------------------------------------------ drawing

    pub(crate) fn draw_container(&self, c: &Open, ui: &mut MeshData, ents: &mut MeshData, items: &mut MeshData, top: &mut MeshData, top_items: &mut MeshData) {
        let s = self.gui_scale();
        let a = &self.ui;
        let (w, h) = (self.gpu.size.0 as f32, self.gpu.size.1 as f32);
        a.gradient(ui, 0.0, 0.0, w, h, [16, 16, 16, 192], [16, 16, 16, 208]);
        let (ox, oy) = self.container_origin(c.kind);
        let (cw, ch) = c.kind.size();
        match c.kind {
            Kind::Inventory => {
                a.region(ui, "container/inventory", ox, oy, 0.0, 0.0, cw, ch, s);
                // the player preview in the black box
                let pid = basalt_data::registry_id(basalt_data::registries::ENTITY_TYPE, "player").unwrap_or(0);
                let me = crate::entity::Entity::new(self.my_eid, crate::net::offline_uuid(&self.name), pid, [0.0; 3], [0.0; 3], 0.0, 0.0, 0.0);
                crate::entity::build_model_gui(&self.entity_atlas, &me, ox + 51.0 * s, oy + 75.0 * s, 30.0 * s, ents);
            }
            Kind::Crafting => a.region(ui, "container/crafting_table", ox, oy, 0.0, 0.0, cw, ch, s),
            Kind::Furnace(v) => {
                let (bg, sp) = [("furnace", "furnace"), ("blast_furnace", "blast_furnace"), ("smoker", "smoker")][v as usize % 3];
                a.region(ui, &format!("container/{bg}"), ox, oy, 0.0, 0.0, cw, ch, s);
                // flame (14x14 at 56,36) shrinks from the top as the fuel burns down; arrow (24x16 at 79,34) fills left to right
                let [lit, lit_total, cook, cook_total] = c.props.map(|p| p as i32);
                if lit > 0 {
                    let dur = if lit_total == 0 { 200 } else { lit_total };
                    let h = ((lit as f32 * 13.0 / dur as f32).ceil() as i32 + 1).clamp(0, 14) as f32;
                    a.region(ui, &format!("sprites/container/{sp}/lit_progress"), ox + 56.0 * s, oy + (36.0 + 14.0 - h) * s, 0.0, 14.0 - h, 14.0, h, s);
                }
                let w = if cook_total > 0 && cook > 0 { (cook as f32 * 24.0 / cook_total as f32).ceil().clamp(0.0, 24.0) } else { 0.0 };
                if w > 0.0 { a.region(ui, &format!("sprites/container/{sp}/burn_progress"), ox + 79.0 * s, oy + 34.0 * s, 0.0, 0.0, w, 16.0, s); }
            }
            Kind::Generic(r) => {
                let top_h = r as f32 * 18.0 + 17.0;
                a.region(ui, "container/generic_54", ox, oy, 0.0, 0.0, cw, top_h, s);
                a.region(ui, "container/generic_54", ox, oy + top_h * s, 0.0, 126.0, cw, 96.0, s);
            }
            Kind::Other => a.region(ui, "container/inventory", ox, oy, 0.0, 0.0, cw, ch, s),
        }
        let gray = [64, 64, 64, 255];
        match c.kind {
            Kind::Inventory => a.text_ex(ui, &c.title, ox + 97.0 * s, oy + 8.0 * s, s, gray, false),
            _ => {
                a.text_ex(ui, &c.title, ox + 8.0 * s, oy + 6.0 * s, s, gray, false);
                let inv_label_y = ch - 94.0;
                a.text_ex(ui, &crate::lang::tr("container.inventory", &[]), ox + 8.0 * s, oy + (inv_label_y + 2.0) * s, s, gray, false);
            }
        }
        for (i, it) in c.slots.iter().enumerate() {
            let Some((x, y)) = c.kind.slot_pos(i) else { continue };
            let (sx, sy) = (ox + x * s, oy + y * s);
            if c.hover == Some(i as i16) { a.rect(ui, sx, sy, 16.0 * s, 16.0 * s, [255, 255, 255, 128]); }
            if it.count > 0 {
                crate::ui::item_icon(&self.assets, items, it.item, sx, sy, s);
                crate::ui::item_decorations(a, top, it, self.max_damage(it.item), sx, sy, s);
            }
        }
        // drag preview: slots being painted
        if let Some((_, v)) = &c.drag { if v.len() >= 2 { for &sl in v { if let Some((x, y)) = c.kind.slot_pos(sl as usize) { a.rect(ui, ox + x * s, oy + y * s, 16.0 * s, 16.0 * s, [255, 255, 255, 90]); } } } }
        // carried stack follows the mouse, drawn above everything
        if !self.carried.is_empty() {
            let (mx, my) = self.mouse_pos;
            crate::ui::item_icon(&self.assets, top_items, self.carried.item, mx - 8.0 * s, my - 8.0 * s, s);
            let mut dec = MeshData::default();
            crate::ui::item_decorations(a, &mut dec, &self.carried, self.max_damage(self.carried.item), mx - 8.0 * s, my - 8.0 * s, s);
            append(top, dec);
        } else if let Some(hv) = c.hover {
            // tooltip
            if let Some(it) = c.slots.get(hv as usize).filter(|it| it.count > 0) {
                let name = crate::lang::item_name(it.item);
                let (mx, my) = self.mouse_pos;
                let tw = a.text_width(&name, s);
                let (tx, ty) = (mx + 12.0 * s, my - 12.0 * s);
                a.rect(top, tx - 3.0 * s, ty - 4.0 * s, tw + 6.0 * s, 16.0 * s, [16, 0, 16, 240]);
                a.text(top, &name, tx, ty, s, [255; 4]);
            }
        }
    }
}

pub fn append(dst: &mut MeshData, src: MeshData) {
    let b = dst.verts.len() as u32;
    dst.verts.extend(src.verts);
    dst.idx.extend(src.idx.into_iter().map(|i| i + b));
}

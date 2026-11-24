//! Inventory and container interaction: container_click modes, 2×2 and crafting-table grids.
//!
//! Player inventory (window 0): 0 craft result, 1-4 craft grid, 5-8 armor, 9-35 main,
//! 36-44 hotbar, 45 offhand — stored 1:1 in `Player::inv` (slot 0 unused).
//! Crafting table window: 0 result, 1-9 grid (`Player::table_grid`), 10-36 main, 37-45 hotbar.
use super::{Game, Player, Stack, NO_ENCH};
use anyhow::Result;
use basalt_protocol::{ids::play::clientbound as cb, text, Reader, Writer};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Window {
    Inventory,
    Crafting(i32),
    Cont(super::container::Cw),
}

#[derive(Clone, Copy, PartialEq)]
enum Loc {
    Result,
    Inv(usize),
    Grid(usize),
}

fn loc(win: Window, slot: i16) -> Option<Loc> {
    let s = slot as i32;
    match win {
        Window::Inventory => match s {
            0 => Some(Loc::Result),
            1..=45 => Some(Loc::Inv(s as usize)),
            _ => None,
        },
        Window::Crafting(_) => match s {
            0 => Some(Loc::Result),
            1..=9 => Some(Loc::Grid(s as usize - 1)),
            10..=45 => Some(Loc::Inv(s as usize - 1)),
            _ => None,
        },
        Window::Cont(_) => None, // container windows are handled by Game::cont_click
    }
}

fn slot_count(win: Window) -> i16 {
    match win {
        Window::Inventory => 46,
        Window::Crafting(_) => 46,
        Window::Cont(cw) => cw.total() as i16,
    }
}

impl Player {
    fn get(&self, l: Loc) -> Stack {
        match l {
            Loc::Result => self.craft_result,
            Loc::Inv(i) => self.inv[i],
            Loc::Grid(i) => self.table_grid[i],
        }
    }
    fn set(&mut self, l: Loc, s: Stack) {
        let s = if s.count <= 0 { Stack::default() } else { s };
        match l {
            Loc::Result => self.craft_result = s,
            Loc::Inv(i) => self.inv[i] = s,
            Loc::Grid(i) => self.table_grid[i] = s,
        }
    }
    pub(super) fn window_id(&self) -> i32 {
        match self.window {
            Window::Inventory => 0,
            Window::Crafting(id) => id,
            Window::Cont(cw) => cw.id,
        }
    }
    fn grid(&self) -> (Vec<Option<i32>>, usize) {
        match self.window {
            Window::Inventory => (
                (1..5)
                    .map(|i| (self.inv[i].count > 0).then_some(self.inv[i].item))
                    .collect(),
                2,
            ),
            Window::Cont(_) => (Vec::new(), 0),
            Window::Crafting(_) => (
                self.table_grid
                    .iter()
                    .map(|s| (s.count > 0).then_some(s.item))
                    .collect(),
                3,
            ),
        }
    }
    fn grid_loc(&self, i: usize) -> Loc {
        match self.window {
            Window::Inventory => Loc::Inv(1 + i),
            _ => Loc::Grid(i),
        }
    }
}

impl Game {
    pub(super) fn max_stack(&self, item: i32) -> i32 {
        self.data
            .item(item)
            .map(|i| i.max_stack)
            .unwrap_or(64)
            .max(1)
    }

    /// Insert into the inventory like vanilla's pickup / give (order to be confirmed by the click
    /// fuzzer's pickup cases): first merge into incomplete stacks of the same item with identical
    /// components — the selected hotbar slot, the offhand, hotbar 0-8, then main 9-35 — then fill
    /// empty slots, hotbar 0-8 left to right, then main 9-35. Returns what didn't fit.
    pub(super) fn give_stack(&mut self, eid: i32, s: Stack) -> i32 {
        let (left, changed) = self.give_stack_quiet(eid, s);
        if changed {
            if let Some(p) = self.players.get_mut(&eid) {
                Self::send_window(p);
            }
        }
        left
    }

    /// [`give_stack`] without sending the window (pickups send take_item_entity first, like vanilla).
    pub(super) fn give_stack_quiet(&mut self, eid: i32, s: Stack) -> (i32, bool) {
        let max = self.max_stack(s.item);
        let Some(p) = self.players.get_mut(&eid) else {
            return (s.count, false);
        };
        let mut count = s.count;
        let same =
            |t: &Stack| t.count > 0 && t.item == s.item && t.damage == s.damage && t.ench == s.ench;
        let mut merge: Vec<usize> = vec![36 + p.held, 45];
        merge.extend((36..45).filter(|&i| i != 36 + p.held));
        merge.extend(9..36);
        for &i in &merge {
            if count == 0 {
                break;
            }
            let t = &mut p.inv[i];
            if same(t) && t.count < max {
                let n = (max - t.count).min(count);
                t.count += n;
                count -= n;
            }
        }
        for i in (36..45).chain(9..36) {
            if count == 0 {
                break;
            }
            let t = &mut p.inv[i];
            if t.count == 0 {
                let n = max.min(count);
                *t = Stack {
                    item: s.item,
                    count: n,
                    damage: s.damage,
                    ench: s.ench,
                };
                count -= n;
            }
        }
        (count, count != s.count)
    }

    fn update_craft(&mut self, eid: i32) {
        let Some(p) = self.players.get(&eid) else {
            return;
        };
        let (grid, w) = p.grid();
        let r = if grid.iter().all(|g| g.is_none()) {
            None
        } else {
            self.data.craft(&grid, w, w)
        };
        let p = self.players.get_mut(&eid).unwrap();
        p.craft_result = r
            .map(|r| Stack {
                item: r.item,
                count: r.count,
                damage: 0,
                ench: NO_ENCH,
            })
            .unwrap_or_default();
    }

    /// Take one craft: consume one of each grid ingredient (leaving remainders). Returns the result.
    fn take_craft(&mut self, eid: i32) -> Option<Stack> {
        let p = self.players.get(&eid)?;
        let (grid, w) = p.grid();
        let r = self.data.craft(&grid, w, w)?;
        let p = self.players.get_mut(&eid).unwrap();
        for i in 0..w * w {
            let l = p.grid_loc(i);
            let mut s = p.get(l);
            if s.count <= 0 {
                continue;
            }
            s.count -= 1;
            if s.count == 0 {
                if let Some(Some(rem)) = r.remainders.get(i) {
                    s = Stack {
                        item: *rem,
                        count: 1,
                        damage: 0,
                        ench: NO_ENCH,
                    };
                }
            }
            p.set(l, s);
        }
        Some(Stack {
            item: r.item,
            count: r.count,
            damage: 0,
            ench: NO_ENCH,
        })
    }

    pub(super) fn send_window(p: &mut Player) {
        p.state_id = p.state_id.wrapping_add(1) & 0x7fff;
        let mut w = Writer::with_id(cb::CONTAINER_SET_CONTENT);
        w.varint(p.window_id())
            .varint(p.state_id)
            .varint(slot_count(p.window) as i32);
        if let Window::Cont(cw) = p.window {
            let n = cw.n_cont();
            for i in 0..n {
                super::write_slot(&mut w, p.cont_items.get(i).copied().unwrap_or_default());
            }
            for j in 0..36 {
                super::write_slot(
                    &mut w,
                    if j < 27 {
                        p.inv[9 + j]
                    } else {
                        p.inv[36 + j - 27]
                    },
                );
            }
        } else {
            for s in 0..slot_count(p.window) {
                super::write_slot(&mut w, p.get(loc(p.window, s).unwrap()));
            }
        }
        super::write_slot(&mut w, p.cursor);
        p.send(w);
    }

    pub(super) fn open_crafting_table(&mut self, eid: i32) {
        let Some(p) = self.players.get_mut(&eid) else {
            return;
        };
        p.next_window = p.next_window % 100 + 1;
        let id = p.next_window;
        p.window = Window::Crafting(id);
        let menu =
            basalt_data::registry_id(basalt_data::registries::MENU, "crafting").unwrap_or(11);
        let mut w = Writer::with_id(cb::OPEN_SCREEN);
        w.varint(id)
            .varint(menu)
            .nbt(&text::translate("container.crafting", vec![]));
        p.send(w);
        Self::send_window(p);
    }

    /// Return grid and cursor contents to the inventory (dropping what doesn't fit).
    pub(super) fn close_window(&mut self, eid: i32) {
        if let Some(Window::Cont(cw)) = self.players.get(&eid).map(|p| p.window) {
            self.cont_closed(eid, cw);
        }
        let Some(p) = self.players.get_mut(&eid) else {
            return;
        };
        let mut back: Vec<Stack> = Vec::new();
        if p.cursor.count > 0 {
            back.push(p.cursor);
            p.cursor = Stack::default();
        }
        match p.window {
            Window::Inventory => {
                for i in 1..5 {
                    if p.inv[i].count > 0 {
                        back.push(p.inv[i]);
                        p.inv[i] = Stack::default();
                    }
                }
            }
            Window::Crafting(_) => {
                for s in p.table_grid.iter_mut() {
                    if s.count > 0 {
                        back.push(*s);
                        *s = Stack::default();
                    }
                }
            }
            Window::Cont(_) => {}
        }
        p.window = Window::Inventory;
        p.craft_result = Stack::default();
        for s in back {
            let left = self.give_stack(eid, s);
            if left > 0 {
                self.throw_item(
                    eid,
                    Stack {
                        item: s.item,
                        count: left,
                        damage: s.damage,
                        ench: s.ench,
                    },
                    false,
                );
            }
        }
        if let Some(p) = self.players.get_mut(&eid) {
            Self::send_window(p);
        }
    }

    /// Move a stack into the crafting table grid (merge first, then empty cells). Returns (leftover, anything moved).
    fn move_into_grid(&mut self, eid: i32, mut s: Stack) -> (Stack, bool) {
        let max = self.max_stack(s.item);
        let p = self.players.get_mut(&eid).unwrap();
        let before = s.count;
        for pass in 0..2 {
            for t in p.table_grid.iter_mut() {
                if s.count == 0 {
                    break;
                }
                if pass == 0 && t.count > 0 && t.item == s.item && t.count < max {
                    let n = (max - t.count).min(s.count);
                    t.count += n;
                    s.count -= n;
                }
                if pass == 1 && t.count == 0 {
                    let n = max.min(s.count);
                    *t = Stack { count: n, ..s };
                    s.count -= n;
                }
            }
        }
        if s.count == 0 {
            s = Stack::default();
        }
        (s, s.count != before)
    }

    /// Move a stack into the inventory range [lo, hi) (merge first, then empty slots). Returns leftover.
    fn move_into(&mut self, eid: i32, mut s: Stack, ranges: &[(usize, usize)]) -> Stack {
        let max = self.max_stack(s.item);
        let p = self.players.get_mut(&eid).unwrap();
        for pass in 0..2 {
            for &(lo, hi) in ranges {
                for i in lo..hi {
                    if s.count == 0 {
                        return s;
                    }
                    let t = &mut p.inv[i];
                    if pass == 0 && t.count > 0 && t.item == s.item && t.count < max {
                        let n = (max - t.count).min(s.count);
                        t.count += n;
                        s.count -= n;
                    }
                    if pass == 1 && t.count == 0 {
                        *t = s;
                        s.count = 0;
                    }
                }
            }
        }
        s
    }

    fn armor_ok(&self, slot: usize, item: i32) -> bool {
        let want = match slot {
            5 => "head",
            6 => "chest",
            7 => "legs",
            8 => "feet",
            _ => return true,
        };
        self.data.item(item).and_then(|i| i.equip_slot.as_deref()) == Some(want)
    }

    pub(super) fn container_click(&mut self, eid: i32, r: &mut Reader) -> Result<()> {
        let window = r.varint()?;
        let _state = r.varint()?;
        let slot = r.i16()?;
        let button = r.i8()?;
        let mode = r.varint()?;
        // the client's predicted changes (hashed) are ignored: the server is authoritative and resyncs
        let Some(p) = self.players.get(&eid) else {
            return Ok(());
        };
        if window != p.window_id() || p.dead {
            return Ok(());
        }
        let win = p.window;
        if let Window::Cont(cw) = win {
            self.cont_click(eid, cw, slot, button, mode);
            return Ok(());
        }
        let creative = p.gamemode == 1;
        let l = loc(win, slot);
        match mode {
            0 => {
                if slot == -999 {
                    let p = self.players.get_mut(&eid).unwrap();
                    let c = p.cursor;
                    if c.count > 0 {
                        let n = if button == 0 { c.count } else { 1 };
                        p.cursor.count -= n;
                        if p.cursor.count <= 0 {
                            p.cursor = Stack::default();
                        }
                        self.throw_item(
                            eid,
                            Stack {
                                item: c.item,
                                count: n,
                                damage: c.damage,
                                ench: c.ench,
                            },
                            false,
                        );
                    }
                } else if let Some(l) = l {
                    if l == Loc::Result {
                        let res = self.players[&eid].craft_result;
                        let cur = self.players[&eid].cursor;
                        if res.count > 0
                            && (cur.count == 0
                                || (cur.item == res.item
                                    && cur.count + res.count <= self.max_stack(res.item)))
                        {
                            if let Some(got) = self.take_craft(eid) {
                                let p = self.players.get_mut(&eid).unwrap();
                                p.cursor = Stack {
                                    item: got.item,
                                    count: cur.count + got.count,
                                    damage: got.damage,
                                    ench: got.ench,
                                };
                            }
                        }
                    } else {
                        let p = &self.players[&eid];
                        let (s, c) = (p.get(l), p.cursor);
                        let max_c = self.max_stack(c.item);
                        let ok_here = |g: &Game, item: i32| match l {
                            Loc::Inv(i) => g.armor_ok(i, item),
                            _ => true,
                        };
                        let (ns, nc) = if button == 0 {
                            if c.count == 0 {
                                (Stack::default(), s)
                            } else if s.count == 0 || s.item == c.item {
                                if !ok_here(self, c.item) {
                                    (s, c)
                                } else {
                                    let n = (max_c - s.count.max(0)).min(c.count).max(0);
                                    (
                                        Stack {
                                            item: c.item,
                                            count: s.count + n,
                                            damage: c.damage,
                                            ench: c.ench,
                                        },
                                        Stack {
                                            item: c.item,
                                            count: c.count - n,
                                            damage: c.damage,
                                            ench: c.ench,
                                        },
                                    )
                                }
                            } else if ok_here(self, c.item) {
                                (c, s)
                            } else {
                                (s, c)
                            }
                        } else if c.count == 0 {
                            let half = (s.count + 1) / 2;
                            (
                                Stack {
                                    item: s.item,
                                    count: s.count - half,
                                    damage: s.damage,
                                    ench: s.ench,
                                },
                                Stack {
                                    item: s.item,
                                    count: half,
                                    damage: s.damage,
                                    ench: s.ench,
                                },
                            )
                        } else if (s.count == 0 || (s.item == c.item && s.count < max_c))
                            && ok_here(self, c.item)
                        {
                            (
                                Stack {
                                    item: c.item,
                                    count: s.count + 1,
                                    damage: c.damage,
                                    ench: c.ench,
                                },
                                Stack {
                                    item: c.item,
                                    count: c.count - 1,
                                    damage: c.damage,
                                    ench: c.ench,
                                },
                            )
                        } else {
                            (s, c)
                        };
                        let p = self.players.get_mut(&eid).unwrap();
                        p.set(l, ns);
                        p.cursor = if nc.count > 0 { nc } else { Stack::default() };
                    }
                }
            }
            1 => {
                if let Some(l) = l {
                    let main_hot = |win: Window, from_inv: Option<usize>| -> Vec<(usize, usize)> {
                        match (win, from_inv) {
                            (_, Some(i)) if (36..45).contains(&i) => vec![(9, 36)],
                            (Window::Inventory, Some(i)) if (9..36).contains(&i) => vec![(36, 45)],
                            (Window::Crafting(_), Some(i)) if (9..36).contains(&i) => {
                                vec![(36, 45)]
                            }
                            _ => vec![(9, 36), (36, 45)],
                        }
                    };
                    if l == Loc::Result {
                        // craft as many as fit
                        for _ in 0..64 {
                            let Some(got) = self.take_craft(eid) else {
                                break;
                            };
                            let left = self.move_into(eid, got, &[(36, 45), (9, 36)]);
                            if left.count > 0 {
                                self.throw_item(eid, left, false);
                                break;
                            }
                            self.update_craft(eid);
                            if self.players[&eid].craft_result.item != got.item {
                                break;
                            }
                        }
                    } else {
                        let s = self.players[&eid].get(l);
                        if s.count > 0 {
                            let from = if let Loc::Inv(i) = l { Some(i) } else { None };
                            // armor goes to its slot first
                            let mut ranges = main_hot(win, from);
                            if win == Window::Inventory && from.map(|i| i >= 9).unwrap_or(false) {
                                if let Some(eq) =
                                    self.data.item(s.item).and_then(|i| i.equip_slot.clone())
                                {
                                    let a = match eq.as_str() {
                                        "head" => 5,
                                        "chest" => 6,
                                        "legs" => 7,
                                        "feet" => 8,
                                        "offhand" => 45,
                                        _ => 0,
                                    };
                                    if a != 0 && self.players[&eid].inv[a].count == 0 {
                                        ranges.insert(0, (a, a + 1));
                                    }
                                }
                            }
                            // crafting table: shift-clicking from the player inventory fills the 3x3 grid first; if anything fit there
                            // the rest stays in the source slot (no fallback to the other inventory section)
                            let to_grid =
                                matches!(win, Window::Crafting(_)) && matches!(l, Loc::Inv(_));
                            let (s, moved) = if to_grid {
                                self.move_into_grid(eid, s)
                            } else {
                                (s, false)
                            };
                            let left = if moved {
                                s
                            } else {
                                self.move_into(eid, s, &ranges)
                            };
                            self.players.get_mut(&eid).unwrap().set(l, left);
                        }
                    }
                }
            }
            2 => {
                if let Some(l) = l {
                    let hb = if button == 40 {
                        45
                    } else if (0..9).contains(&button) {
                        36 + button as usize
                    } else {
                        return Ok(());
                    };
                    let p = self.players.get_mut(&eid).unwrap();
                    if l == Loc::Result {
                        if p.inv[hb].count == 0 {
                            if let Some(got) = self.take_craft(eid) {
                                self.players.get_mut(&eid).unwrap().inv[hb] = got;
                            }
                        }
                    } else {
                        let a = p.get(l);
                        let b = p.inv[hb];
                        p.set(l, b);
                        p.inv[hb] = a;
                    }
                }
            }
            3 => {
                if let (Some(l), true) = (l, creative) {
                    let s = self.players[&eid].get(l);
                    if s.count > 0 && self.players[&eid].cursor.count == 0 {
                        let m = self.max_stack(s.item);
                        self.players.get_mut(&eid).unwrap().cursor = Stack {
                            item: s.item,
                            count: m,
                            damage: s.damage,
                            ench: s.ench,
                        };
                    }
                }
            }
            4 => {
                if let Some(l) = l {
                    if l == Loc::Result {
                        if let Some(got) = self.take_craft(eid) {
                            self.throw_item(eid, got, false);
                        }
                    } else {
                        let s = self.players[&eid].get(l);
                        if s.count > 0 {
                            let n = if button == 1 { s.count } else { 1 };
                            self.players.get_mut(&eid).unwrap().set(
                                l,
                                Stack {
                                    item: s.item,
                                    count: s.count - n,
                                    damage: s.damage,
                                    ench: s.ench,
                                },
                            );
                            self.throw_item(
                                eid,
                                Stack {
                                    item: s.item,
                                    count: n,
                                    damage: s.damage,
                                    ench: s.ench,
                                },
                                false,
                            );
                        }
                    }
                }
            }
            5 => {
                let p = self.players.get_mut(&eid).unwrap();
                match button {
                    0 | 4 | 8 => {
                        p.drag = (button, Vec::new());
                    }
                    1 | 5 | 9 => {
                        if let Some(l) = l {
                            if l != Loc::Result && !p.drag.1.contains(&slot) {
                                p.drag.1.push(slot);
                            }
                        }
                    }
                    2 | 6 | 10 => {
                        let (start, slots) = std::mem::take(&mut p.drag);
                        let c = p.cursor;
                        if c.count > 0 && !slots.is_empty() {
                            let max = self.max_stack(c.item);
                            let targets: Vec<Loc> = slots
                                .iter()
                                .filter_map(|&s| loc(win, s))
                                .filter(|&l| {
                                    let s = self.players[&eid].get(l);
                                    s.count == 0 || s.item == c.item
                                })
                                .collect();
                            if !targets.is_empty() {
                                let per = match start {
                                    0 => (c.count / targets.len() as i32).max(1),
                                    4 => 1,
                                    _ => max,
                                };
                                let mut left = c.count;
                                let p = self.players.get_mut(&eid).unwrap();
                                for t in targets {
                                    if left == 0 {
                                        break;
                                    }
                                    let s = p.get(t);
                                    let room = max - s.count.max(0);
                                    let n = if start == 8 && creative {
                                        room
                                    } else {
                                        per.min(room).min(left)
                                    };
                                    if n <= 0 {
                                        continue;
                                    }
                                    p.set(
                                        t,
                                        Stack {
                                            item: c.item,
                                            count: s.count.max(0) + n,
                                            damage: c.damage,
                                            ench: c.ench,
                                        },
                                    );
                                    if !(start == 8 && creative) {
                                        left -= n;
                                    }
                                }
                                p.cursor = if left > 0 {
                                    Stack {
                                        item: c.item,
                                        count: left,
                                        damage: c.damage,
                                        ench: c.ench,
                                    }
                                } else {
                                    Stack::default()
                                };
                            }
                        }
                    }
                    _ => {}
                }
            }
            6 => {
                let c = self.players[&eid].cursor;
                if c.count > 0 {
                    let max = self.max_stack(c.item);
                    let p = self.players.get_mut(&eid).unwrap();
                    let mut cur = c.count;
                    for s in 1..slot_count(win) {
                        if cur >= max {
                            break;
                        }
                        let l = loc(win, s).unwrap();
                        let t = p.get(l);
                        if t.count > 0 && t.item == c.item {
                            let n = (max - cur).min(t.count);
                            cur += n;
                            p.set(
                                l,
                                Stack {
                                    item: t.item,
                                    count: t.count - n,
                                    damage: t.damage,
                                    ench: t.ench,
                                },
                            );
                        }
                    }
                    p.cursor.count = cur;
                }
            }
            _ => {}
        }
        self.update_craft(eid);
        if let Some(p) = self.players.get_mut(&eid) {
            Self::send_window(p);
        }
        Ok(())
    }
}

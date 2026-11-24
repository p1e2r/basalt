//! Scoreboard (sidebar, tab-list score, below-name score) and boss bars: state kept from the
//! set_objective / set_score / reset_score / set_display_objective / boss_event packets, and the HUD
//! drawing. Layout constants were measured against the real 26.2 client (see NOTES.md, "Scoreboard
//! and boss bar HUD").
use crate::mesh::MeshData;
use crate::ui::UiAtlas;
use std::collections::HashMap;
use uuid::Uuid;

/// How a score is shown: the number (default), nothing (blank) or a fixed text.
#[derive(Debug, Clone, PartialEq)]
pub enum Fmt {
    Default,
    Blank,
    Fixed(String),
}

#[derive(Debug, Clone)]
pub enum BossOp {
    Add {
        name: String,
        progress: f32,
        color: i32,
        overlay: i32,
        flags: u8,
    },
    Remove,
    Progress(f32),
    Name(String),
    Style {
        color: i32,
        overlay: i32,
    },
    Flags(u8),
}

#[derive(Debug, Clone)]
pub enum Update {
    /// mode 0 add, 1 remove, 2 change; render type 0 integer, 1 hearts
    Objective {
        name: String,
        mode: u8,
        title: String,
        render: i32,
    },
    Display {
        slot: i32,
        name: String,
    },
    Score {
        holder: String,
        objective: String,
        value: i32,
        display: Option<String>,
        fmt: Fmt,
    },
    Reset {
        holder: String,
        objective: Option<String>,
    },
    Boss {
        uuid: Uuid,
        op: BossOp,
    },
}

/// (score, custom display name, number format)
pub type Score = (i32, Option<String>, Fmt);

#[derive(Debug, Clone)]
pub struct Objective {
    pub title: String,
    /// render type (0 integer, 1 hearts): for the tab-list score, not drawn yet
    #[allow(dead_code)]
    pub render: i32,
}

#[derive(Debug, Clone)]
pub struct BossBar {
    pub uuid: Uuid,
    pub name: String,
    pub progress: f32,
    pub color: i32,
    pub overlay: i32,
    pub flags: u8,
}

#[derive(Default)]
pub struct Board {
    pub objectives: HashMap<String, Objective>,
    /// objective -> holder -> (score, custom display name, number format)
    pub scores: HashMap<String, HashMap<String, Score>>,
    /// display slot (0 list, 1 sidebar, 2 below_name, 3.. sidebar.team.*) -> objective
    pub display: HashMap<i32, String>,
    /// boss bars in the order they were added
    pub bars: Vec<BossBar>,
}

impl Board {
    pub fn apply(&mut self, u: Update) {
        match u {
            Update::Objective {
                name,
                mode,
                title,
                render,
            } => match mode {
                1 => {
                    self.objectives.remove(&name);
                    self.scores.remove(&name);
                    self.display.retain(|_, v| *v != name);
                }
                _ => {
                    self.objectives.insert(name, Objective { title, render });
                }
            },
            Update::Display { slot, name } => {
                if name.is_empty() {
                    self.display.remove(&slot);
                } else {
                    self.display.insert(slot, name);
                }
            }
            Update::Score {
                holder,
                objective,
                value,
                display,
                fmt,
            } => {
                self.scores
                    .entry(objective)
                    .or_default()
                    .insert(holder, (value, display, fmt));
            }
            Update::Reset { holder, objective } => match objective {
                Some(o) => {
                    if let Some(m) = self.scores.get_mut(&o) {
                        m.remove(&holder);
                    }
                }
                None => {
                    for m in self.scores.values_mut() {
                        m.remove(&holder);
                    }
                }
            },
            Update::Boss { uuid, op } => match op {
                BossOp::Add {
                    name,
                    progress,
                    color,
                    overlay,
                    flags,
                } => {
                    self.bars.retain(|b| b.uuid != uuid);
                    self.bars.push(BossBar {
                        uuid,
                        name,
                        progress,
                        color,
                        overlay,
                        flags,
                    });
                }
                BossOp::Remove => self.bars.retain(|b| b.uuid != uuid),
                other => {
                    if let Some(b) = self.bars.iter_mut().find(|b| b.uuid == uuid) {
                        match other {
                            BossOp::Progress(p) => b.progress = p,
                            BossOp::Name(n) => b.name = n,
                            BossOp::Style { color, overlay } => {
                                b.color = color;
                                b.overlay = overlay;
                            }
                            BossOp::Flags(f) => b.flags = f,
                            _ => {}
                        }
                    }
                }
            },
        }
    }

    /// Scores of an objective as shown: highest first, ties by name (reversed alphabetical order is what
    /// vanilla's list yields before it takes the last 15), at most 15.
    pub fn sorted(&self, objective: &str) -> Vec<(String, i32, Option<String>, Fmt)> {
        let Some(m) = self.scores.get(objective) else {
            return vec![];
        };
        let mut v: Vec<(String, i32, Option<String>, Fmt)> = m
            .iter()
            .filter(|(h, _)| !h.starts_with('#'))
            .map(|(h, (s, d, f))| (h.clone(), *s, d.clone(), f.clone()))
            .collect();
        v.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
        });
        v.truncate(15);
        v
    }

    /// Score of a holder in the objective shown in a slot (tab list / below name), if any.
    #[allow(dead_code)] // for the tab list / below_name, which basalt-client doesn't draw yet
    pub fn slot_score(&self, slot: i32, holder: &str) -> Option<(i32, Fmt)> {
        let o = self.display.get(&slot)?;
        self.scores
            .get(o)?
            .get(holder)
            .map(|(v, _, f)| (*v, f.clone()))
    }

    /// The sidebar: right edge, title centred over the entries, names left, scores right in red.
    pub fn draw_sidebar(&self, a: &UiAtlas, ui: &mut MeshData, w: f32, h: f32, s: f32) {
        let Some(name) = self.display.get(&1) else {
            return;
        };
        let Some(obj) = self.objectives.get(name) else {
            return;
        };
        let rows = self.sorted(name);
        let title_w = a.text_width(&obj.title, 1.0);
        let spacer = a.text_width(": ", 1.0);
        let shown: Vec<(String, String)> = rows
            .iter()
            .map(|(hld, v, d, f)| {
                let nm = d.clone().unwrap_or_else(|| hld.clone());
                let sc = match f {
                    Fmt::Default => v.to_string(),
                    Fmt::Blank => String::new(),
                    Fmt::Fixed(t) => t.clone(),
                };
                (nm, sc)
            })
            .collect();
        let mut max_w = title_w;
        for (n, sc) in &shown {
            max_w = max_w.max(
                a.text_width(n, 1.0)
                    + if sc.is_empty() {
                        0.0
                    } else {
                        spacer + a.text_width(sc, 1.0)
                    },
            );
        }
        let (gw, gh) = (w / s, h / s);
        let n = shown.len() as f32;
        let bottom = (gh / 2.0 + n * 9.0 / 3.0).floor();
        let top = bottom - n * 9.0;
        let left = gw - max_w - 3.0;
        let right = gw - 1.0;
        a.rect(
            ui,
            (left - 2.0) * s,
            (top - 9.0 - 1.0) * s,
            (right - left + 2.0) * s,
            9.0 * s,
            [0, 0, 0, 102],
        );
        a.rect(
            ui,
            (left - 2.0) * s,
            (top - 1.0) * s,
            (right - left + 2.0) * s,
            (bottom - top + 1.0) * s,
            [0, 0, 0, 76],
        );
        a.text_ex(
            ui,
            &obj.title,
            (left + max_w / 2.0 - title_w / 2.0) * s,
            (top - 9.0) * s,
            s,
            [255; 4],
            false,
        );
        for (i, (n, sc)) in shown.iter().enumerate() {
            let y = (top + i as f32 * 9.0) * s;
            a.text_ex(ui, n, left * s, y, s, [255; 4], false);
            if !sc.is_empty() {
                a.text_ex(
                    ui,
                    sc,
                    (right - a.text_width(sc, 1.0)) * s,
                    y,
                    s,
                    [255, 85, 85, 255],
                    false,
                );
            }
        }
    }

    /// Boss bars stacked from the top centre: name above, 182x5 bar, 19 px per bar, until a third of the screen.
    pub fn draw_bars(&self, a: &UiAtlas, ui: &mut MeshData, w: f32, h: f32, s: f32) {
        const COLORS: [&str; 7] = ["pink", "blue", "red", "green", "yellow", "purple", "white"];
        const OVERLAYS: [&str; 5] = ["", "notched_6", "notched_10", "notched_12", "notched_20"];
        let gh = h / s;
        let mut y = 12.0;
        for b in &self.bars {
            let x = ((w / s - 182.0) / 2.0).floor();
            let col = COLORS[(b.color.max(0) as usize).min(6)];
            let ov = OVERLAYS[(b.overlay.max(0) as usize).min(4)];
            a.sprite(
                ui,
                &format!("sprites/boss_bar/{col}_background"),
                x * s,
                y * s,
                s,
            );
            if !ov.is_empty() {
                a.sprite(
                    ui,
                    &format!("sprites/boss_bar/{ov}_background"),
                    x * s,
                    y * s,
                    s,
                );
            }
            // measured: progress 0.5 fills 91 px, 0.25 fills 46 (round half up of progress * 182)
            let fill = (b.progress.clamp(0.0, 1.0) * 182.0 + 0.5)
                .floor()
                .min(182.0);
            if fill > 0.0 {
                a.region(
                    ui,
                    &format!("sprites/boss_bar/{col}_progress"),
                    x * s,
                    y * s,
                    0.0,
                    0.0,
                    fill,
                    5.0,
                    s,
                );
                if !ov.is_empty() {
                    a.region(
                        ui,
                        &format!("sprites/boss_bar/{ov}_progress"),
                        x * s,
                        y * s,
                        0.0,
                        0.0,
                        fill,
                        5.0,
                        s,
                    );
                }
            }
            let tw = a.text_width(&b.name, 1.0);
            a.text(
                ui,
                &b.name,
                ((w / s - tw) / 2.0).floor() * s,
                (y - 9.0) * s,
                s,
                [255; 4],
            );
            y += 19.0;
            if y >= gh / 3.0 {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn score(b: &mut Board, h: &str, o: &str, v: i32) {
        b.apply(Update::Score {
            holder: h.into(),
            objective: o.into(),
            value: v,
            display: None,
            fmt: Fmt::Default,
        });
    }
    #[test]
    fn sidebar_order_and_limit() {
        let mut b = Board::default();
        b.apply(Update::Objective {
            name: "o".into(),
            mode: 0,
            title: "T".into(),
            render: 0,
        });
        for (h, v) in [("b", 1), ("a", 1), ("c", 5), ("#hidden", 9)] {
            score(&mut b, h, "o", v);
        }
        let v: Vec<String> = b.sorted("o").into_iter().map(|r| r.0).collect();
        assert_eq!(v, ["c", "a", "b"]);
        for i in 0..20 {
            score(&mut b, &format!("p{i:02}"), "o", 100 + i);
        }
        assert_eq!(b.sorted("o").len(), 15);
    }
    #[test]
    fn reset_and_remove() {
        let mut b = Board::default();
        b.apply(Update::Objective {
            name: "o".into(),
            mode: 0,
            title: "T".into(),
            render: 0,
        });
        score(&mut b, "x", "o", 1);
        b.apply(Update::Display {
            slot: 1,
            name: "o".into(),
        });
        b.apply(Update::Reset {
            holder: "x".into(),
            objective: None,
        });
        assert!(b.sorted("o").is_empty());
        b.apply(Update::Objective {
            name: "o".into(),
            mode: 1,
            title: String::new(),
            render: 0,
        });
        assert!(b.display.is_empty());
    }
    #[test]
    fn boss_bar_updates() {
        let mut b = Board::default();
        let u = Uuid::from_u128(1);
        b.apply(Update::Boss {
            uuid: u,
            op: BossOp::Add {
                name: "n".into(),
                progress: 0.5,
                color: 6,
                overlay: 0,
                flags: 0,
            },
        });
        b.apply(Update::Boss {
            uuid: u,
            op: BossOp::Progress(0.25),
        });
        b.apply(Update::Boss {
            uuid: u,
            op: BossOp::Style {
                color: 1,
                overlay: 2,
            },
        });
        assert_eq!(
            (b.bars[0].progress, b.bars[0].color, b.bars[0].overlay),
            (0.25, 1, 2)
        );
        b.apply(Update::Boss {
            uuid: u,
            op: BossOp::Remove,
        });
        assert!(b.bars.is_empty());
    }
}

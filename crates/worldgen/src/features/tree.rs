//! Trees (trunk placers, foliage placers, decorators, leaf distances) and huge mushrooms.

use super::provider::{IntProvider, StateProvider};
use super::{Features, Pred, R};
use crate::random::Rng;
use crate::region::{flag, Region, F_AIR};
use anyhow::{bail, Result};
use basalt_data::BlockState;
use serde_json::Value;
use std::collections::HashSet;

type Pos = (i32, i32, i32);

/// A set with java.util.HashSet<BlockPos> iteration order (insertion order within
/// hash buckets, buckets in index order, default capacity growth).
#[derive(Default)]
pub struct JSet {
    items: Vec<Pos>,
    index: HashSet<Pos>,
}

impl JSet {
    pub fn insert(&mut self, p: Pos) -> bool {
        if self.index.insert(p) { self.items.push(p); true } else { false }
    }
    pub fn contains(&self, p: &Pos) -> bool { self.index.contains(p) }
    pub fn is_empty(&self) -> bool { self.items.is_empty() }
    pub fn len(&self) -> usize { self.items.len() }
    /// Elements in Java iteration order.
    pub fn java_order(&self) -> Vec<Pos> {
        let mut cap = 16usize;
        while self.items.len() > cap * 3 / 4 { cap *= 2; }
        let mut v: Vec<(usize, usize, Pos)> = self.items.iter().enumerate().map(|(i, p)| {
            let h = (p.1.wrapping_add(p.2.wrapping_mul(31))).wrapping_mul(31).wrapping_add(p.0);
            let h = h ^ ((h as u32) >> 16) as i32;
            ((h as u32 as usize) & (cap - 1), i, *p)
        }).collect();
        v.sort_by_key(|e| (e.0, e.1));
        v.into_iter().map(|e| e.2).collect()
    }
    /// Java order, then stably sorted by Y (as TreeDecorator.Context does).
    pub fn sorted_by_y(&self) -> Vec<Pos> {
        let mut v = self.java_order();
        v.sort_by_key(|p| p.1);
        v
    }
}

enum Size {
    Two { limit: i32, lower: i32, upper: i32, min_clipped: Option<i32> },
    Three { limit: i32, upper_limit: i32, lower: i32, middle: i32, upper: i32, min_clipped: Option<i32> },
}

impl Size {
    fn parse(v: &Value) -> Result<Size> {
        let g = |k: &str, d: i64| v.get(k).and_then(|x| x.as_i64()).unwrap_or(d) as i32;
        let mc = v.get("min_clipped_height").and_then(|x| x.as_i64()).map(|x| x as i32);
        Ok(match v["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
            "two_layers_feature_size" => Size::Two { limit: g("limit", 1), lower: g("lower_size", 0), upper: g("upper_size", 1), min_clipped: mc },
            "three_layers_feature_size" => Size::Three {
                limit: g("limit", 1), upper_limit: g("upper_limit", 1), lower: g("lower_size", 0), middle: g("middle_size", 1), upper: g("upper_size", 1), min_clipped: mc,
            },
            t => bail!("unsupported feature size {t}"),
        })
    }
    fn at(&self, height: i32, y: i32) -> i32 {
        match *self {
            Size::Two { limit, lower, upper, .. } => if y < limit { lower } else { upper },
            Size::Three { limit, upper_limit, lower, middle, upper, .. } => {
                if y < limit { lower } else if y >= height - upper_limit { upper } else { middle }
            }
        }
    }
    fn min_clipped(&self) -> Option<i32> { match *self { Size::Two { min_clipped, .. } | Size::Three { min_clipped, .. } => min_clipped } }
}

#[derive(Clone, Copy, PartialEq)]
enum TrunkKind { Straight, Fancy, DarkOak, Giant }

struct Trunk { kind: TrunkKind, base: i32, ra: i32, rb: i32 }

enum FoliageKind {
    Blob { height: i32 },
    Fancy { height: i32 },
    Bush { height: i32 },
    Spruce { trunk_height: IntProvider },
    Pine { height: IntProvider },
    MegaPine { crown: IntProvider },
    DarkOak,
    Jungle { height: i32 },
    Acacia,
}

struct Foliage { kind: FoliageKind, radius: IntProvider, offset: IntProvider }

enum Decorator {
    AttachedToLogs { prob: f32, provider: StateProvider, dirs: Vec<usize> },
    Beehive(f32),
    AlterGround(StateProvider),
    LeaveVine(f32),
    TrunkVine,
    Cocoa(f32),
    PlaceOnGround { tries: i32, radius: i32, height: i32, provider: StateProvider },
}

pub struct Tree {
    trunk: Trunk,
    foliage: Foliage,
    trunk_provider: StateProvider,
    foliage_provider: StateProvider,
    below: StateProvider,
    size: Size,
    ignore_vines: bool,
    decorators: Vec<Decorator>,
}

fn parse_decorators(f: &mut Features, v: &Value) -> Result<Vec<Decorator>> {
    let mut decorators = Vec::new();
    for d in v.as_array().cloned().unwrap_or_default() {
            let p = |k: &str| d[k].as_f64().unwrap_or(0.0) as f32;
            decorators.push(match d["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
                "beehive" => Decorator::Beehive(p("probability")),
                "alter_ground" => Decorator::AlterGround(StateProvider::parse(&f.tags, &d["provider"])?),
                "leave_vine" => Decorator::LeaveVine(p("probability")),
                "trunk_vine" => Decorator::TrunkVine,
                "attached_to_logs" => Decorator::AttachedToLogs {
                    prob: p("probability"),
                    provider: StateProvider::parse(&f.tags, &d["block_provider"])?,
                    dirs: d["directions"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().and_then(super::dir_index)).collect()).unwrap_or_default(),
                },
                "cocoa" => Decorator::Cocoa(p("probability")),
                "place_on_ground" => Decorator::PlaceOnGround {
                    tries: d.get("tries").and_then(|x| x.as_i64()).unwrap_or(128) as i32,
                    radius: d.get("radius").and_then(|x| x.as_i64()).unwrap_or(2) as i32,
                    height: d.get("height").and_then(|x| x.as_i64()).unwrap_or(1) as i32,
                    provider: StateProvider::parse(&f.tags, &d["block_state_provider"])?,
                },
                t => bail!("unsupported tree decorator {t}"),
            });
        }
    Ok(decorators)
}

/// Attachment point for foliage.
struct Att { pos: Pos, radius_offset: i32, double: bool }

fn st(name: &str) -> u16 { BlockState::parse(name).map(|s| s.0 as u16).unwrap_or(0) }

impl Tree {
    pub fn parse(f: &mut Features, c: &Value) -> Result<Tree> {
        if c.get("root_placer").map(|r| !r.is_null()).unwrap_or(false) { bail!("root placers not implemented"); }
        let tp = &c["trunk_placer"];
        let kind = match tp["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
            "straight_trunk_placer" => TrunkKind::Straight,
            "fancy_trunk_placer" => TrunkKind::Fancy,
            "dark_oak_trunk_placer" => TrunkKind::DarkOak,
            "giant_trunk_placer" => TrunkKind::Giant,
            t => bail!("unsupported trunk placer {t}"),
        };
        let g = |v: &Value, k: &str| v[k].as_i64().unwrap_or(0) as i32;
        let trunk = Trunk { kind, base: g(tp, "base_height"), ra: g(tp, "height_rand_a"), rb: g(tp, "height_rand_b") };
        let fp = &c["foliage_placer"];
        let fk = match fp["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
            "blob_foliage_placer" => FoliageKind::Blob { height: g(fp, "height") },
            "fancy_foliage_placer" => FoliageKind::Fancy { height: g(fp, "height") },
            "bush_foliage_placer" => FoliageKind::Bush { height: g(fp, "height") },
            "spruce_foliage_placer" => FoliageKind::Spruce { trunk_height: IntProvider::parse(&fp["trunk_height"])? },
            "pine_foliage_placer" => FoliageKind::Pine { height: IntProvider::parse(&fp["height"])? },
            "mega_pine_foliage_placer" => FoliageKind::MegaPine { crown: IntProvider::parse(&fp["crown_height"])? },
            "dark_oak_foliage_placer" => FoliageKind::DarkOak,
            "jungle_foliage_placer" => FoliageKind::Jungle { height: g(fp, "height") },
            "acacia_foliage_placer" => FoliageKind::Acacia,
            t => bail!("unsupported foliage placer {t}"),
        };
        let foliage = Foliage { kind: fk, radius: IntProvider::parse(&fp["radius"])?, offset: IntProvider::parse(&fp["offset"])? };
        let decorators = parse_decorators(f, &c["decorators"])?;
        Ok(Tree {
            trunk, foliage,
            trunk_provider: StateProvider::parse(&f.tags, &c["trunk_provider"])?,
            foliage_provider: StateProvider::parse(&f.tags, &c["foliage_provider"])?,
            below: StateProvider::parse(&f.tags, &c["below_trunk_provider"])?,
            size: Size::parse(&c["minimum_size"])?,
            ignore_vines: c["ignore_vines"].as_bool().unwrap_or(false),
            decorators,
        })
    }

    pub fn place(&self, f: &Features, reg: &mut Region, r: &mut R, x: i32, y: i32, z: i32) -> bool {
        let mut t = Placing { f, tree: Some(self), logs: JSet::default(), leaves: JSet::default(), decos: JSet::default() };
        if !t.do_place(reg, r, (x, y, z)) || (t.logs.is_empty() && t.leaves.is_empty()) { return false; }
        if !self.decorators.is_empty() {
            let logs = t.logs.sorted_by_y();
            let leaves = t.leaves.sorted_by_y();
            for d in &self.decorators { t.decorate(reg, r, d, &logs, &leaves); }
        }
        t.update_leaves(reg);
        true
    }
}

struct Placing<'a> {
    f: &'a Features,
    tree: Option<&'a Tree>,
    logs: JSet,
    leaves: JSet,
    decos: JSet,
}

impl<'a> Placing<'a> {
    fn t(&self) -> &'a Tree { self.tree.expect("tree") }

    fn valid_tree_pos(&self, reg: &Region, p: Pos) -> bool {
        let b = reg.block(p.0, p.1, p.2);
        flag(b, F_AIR) || self.f.tag("replaceable_by_trees")[b as usize]
    }
    fn is_free(&self, reg: &Region, p: Pos) -> bool {
        self.valid_tree_pos(reg, p) || self.f.tag("logs")[reg.block(p.0, p.1, p.2) as usize]
    }
    fn set_trunk(&mut self, reg: &mut Region, p: Pos, s: u16) { self.logs.insert(p); reg.set(p.0, p.1, p.2, s); }

    fn below_trunk(&mut self, reg: &mut Region, r: &mut R, p: Pos) {
        if let Some(s) = self.t().below.get(self.f, reg, r, p.0, p.1, p.2) {
            // The block below the trunk counts as part of the trunk (matters for decorators).
            self.set_trunk(reg, p, s);
        }
    }

    fn place_log(&mut self, reg: &mut Region, r: &mut R, p: Pos, axis: Option<&str>) -> bool {
        if !self.valid_tree_pos(reg, p) { return false; }
        let Some(mut s) = self.t().trunk_provider.get(self.f, reg, r, p.0, p.1, p.2) else { return false };
        if let Some(a) = axis {
            if let Some(n) = BlockState(s as u32).with("axis", a) { s = n.0 as u16; }
        }
        self.set_trunk(reg, p, s);
        true
    }

    fn place_log_if_free(&mut self, reg: &mut Region, r: &mut R, p: Pos) {
        if self.is_free(reg, p) { self.place_log(reg, r, p, None); }
    }

    fn do_place(&mut self, reg: &mut Region, r: &mut R, pos: Pos) -> bool {
        let tr = &self.t().trunk;
        let i = tr.base + r.next_int_bounded(tr.ra + 1) + r.next_int_bounded(tr.rb + 1);
        let j = self.foliage_height(r, i);
        let k = i - j;
        let l = self.foliage_radius(r, k);
        let m = pos.1;
        let n = pos.1 + i + 1;
        if m < crate::MIN_Y + 1 || n > crate::MIN_Y + crate::HEIGHT { return false; }
        let o = self.max_free_height(reg, i, pos);
        if super::debug_filter().map(|d| d == "tree").unwrap_or(false) {
            eprintln!("[tree] at {pos:?} trunk {i} foliage_h {j} radius {l} maxfree {o}");
        }
        if o < i && self.t().size.min_clipped().map(|mc| o < mc).unwrap_or(true) { return false; }
        let atts = match tr.kind {
            TrunkKind::Straight => self.straight(reg, r, o, pos),
            TrunkKind::Fancy => self.fancy(reg, r, o, pos),
            TrunkKind::DarkOak => self.dark_oak(reg, r, o, pos),
            TrunkKind::Giant => self.giant(reg, r, o, pos),
        };
        for a in atts { self.create_foliage(reg, r, o, &a, j, l); }
        true
    }

    fn max_free_height(&self, reg: &Region, h: i32, pos: Pos) -> i32 {
        for j in 0..=h + 1 {
            let k = self.t().size.at(h, j);
            for l in -k..=k {
                for n in -k..=k {
                    let p = (pos.0 + l, pos.1 + j, pos.2 + n);
                    let vine = BlockState(reg.block(p.0, p.1, p.2) as u32).name() == "vine";
                    if self.is_free(reg, p) && (self.t().ignore_vines || !vine) { continue; }
                    return j - 2;
                }
            }
        }
        h
    }

    // ---- trunk placers ----

    fn straight(&mut self, reg: &mut Region, r: &mut R, h: i32, pos: Pos) -> Vec<Att> {
        self.below_trunk(reg, r, (pos.0, pos.1 - 1, pos.2));
        for i in 0..h { self.place_log(reg, r, (pos.0, pos.1 + i, pos.2), None); }
        vec![Att { pos: (pos.0, pos.1 + h, pos.2), radius_offset: 0, double: false }]
    }

    fn giant(&mut self, reg: &mut Region, r: &mut R, h: i32, pos: Pos) -> Vec<Att> {
        let b = (pos.0, pos.1 - 1, pos.2);
        for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] { self.below_trunk(reg, r, (b.0 + dx, b.1, b.2 + dz)); }
        for i in 0..h {
            self.place_log_if_free(reg, r, (pos.0, pos.1 + i, pos.2));
            if i >= h - 1 { continue; }
            self.place_log_if_free(reg, r, (pos.0 + 1, pos.1 + i, pos.2));
            self.place_log_if_free(reg, r, (pos.0 + 1, pos.1 + i, pos.2 + 1));
            self.place_log_if_free(reg, r, (pos.0, pos.1 + i, pos.2 + 1));
        }
        vec![Att { pos: (pos.0, pos.1 + h, pos.2), radius_offset: 0, double: true }]
    }

    fn dark_oak(&mut self, reg: &mut Region, r: &mut R, h: i32, pos: Pos) -> Vec<Att> {
        let b = (pos.0, pos.1 - 1, pos.2);
        for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] { self.below_trunk(reg, r, (b.0 + dx, b.1, b.2 + dz)); }
        let dir = [(0, -1), (1, 0), (0, 1), (-1, 0)][r.next_int_bounded(4) as usize];
        let i = h - r.next_int_bounded(4);
        let mut j = 2 - r.next_int_bounded(3);
        let (k, l, m) = pos;
        let (mut n, mut o) = (k, m);
        let p = l + h - 1;
        for q in 0..h {
            if q >= i && j > 0 { n += dir.0; o += dir.1; j -= 1; }
            let bp = (n, l + q, o);
            let bb = reg.block(bp.0, bp.1, bp.2);
            if flag(bb, F_AIR) || self.f.tag("leaves")[bb as usize] {
                self.place_log(reg, r, bp, None);
                self.place_log(reg, r, (bp.0 + 1, bp.1, bp.2), None);
                self.place_log(reg, r, (bp.0, bp.1, bp.2 + 1), None);
                self.place_log(reg, r, (bp.0 + 1, bp.1, bp.2 + 1), None);
            }
        }
        let mut out = vec![Att { pos: (n, p, o), radius_offset: 0, double: true }];
        for q in -1..=2 {
            for rr in -1..=2 {
                if ((0..=1).contains(&q) && (0..=1).contains(&rr)) || r.next_int_bounded(3) > 0 { continue; }
                let s = r.next_int_bounded(3) + 2;
                for t in 0..s { self.place_log(reg, r, (k + q, p - t - 1, m + rr), None); }
                out.push(Att { pos: (n + q, p, o + rr), radius_offset: 0, double: false });
            }
        }
        out
    }

    fn fancy(&mut self, reg: &mut Region, r: &mut R, height: i32, pos: Pos) -> Vec<Att> {
        let i = height + 2;
        let j = (i as f64 * 0.618).floor() as i32;
        self.below_trunk(reg, r, (pos.0, pos.1 - 1, pos.2));
        let k = 1.min((1.382 + (i as f64 / 13.0).powi(2)).floor() as i32);
        let l = pos.1 + j;
        let mut m = i - 5;
        let mut list: Vec<(Pos, i32)> = vec![((pos.0, pos.1 + m, pos.2), l)];
        while m >= 0 {
            let f = tree_shape(i, m);
            if f >= 0.0 {
                for _ in 0..k {
                    let e = f as f64 * (r.next_float() as f64 + 0.328);
                    let g = (r.next_float() * 2.0) as f64 * std::f64::consts::PI;
                    let h = e * g.sin() + 0.5;
                    let o = e * g.cos() + 0.5;
                    let p2 = (pos.0 + h.floor() as i32, pos.1 + m - 1, pos.2 + o.floor() as i32);
                    let p3 = (p2.0, p2.1 + 5, p2.2);
                    if !self.limb(reg, r, p2, p3, false) { continue; }
                    let dx = pos.0 - p2.0;
                    let dz = pos.2 - p2.2;
                    let rr = p2.1 as f64 - ((dx * dx + dz * dz) as f64).sqrt() * 0.381;
                    let s = if rr > l as f64 { l } else { rr as i32 };
                    let p4 = (pos.0, s, pos.2);
                    if !self.limb(reg, r, p4, p2, false) { continue; }
                    list.push((p2, p4.1));
                }
            }
            m -= 1;
        }
        self.limb(reg, r, pos, (pos.0, pos.1 + j, pos.2), true);
        for (p, base) in &list {
            let bp = (pos.0, *base, pos.2);
            if bp == *p || !trim(i, base - pos.1) { continue; }
            self.limb(reg, r, bp, *p, true);
        }
        list.into_iter().filter(|(_, base)| trim(i, base - pos.1)).map(|(p, _)| Att { pos: p, radius_offset: 0, double: false }).collect()
    }

    fn limb(&mut self, reg: &mut Region, r: &mut R, a: Pos, b: Pos, place: bool) -> bool {
        if !place && a == b { return true; }
        let d = (b.0 - a.0, b.1 - a.1, b.2 - a.2);
        let steps = d.0.abs().max(d.1.abs()).max(d.2.abs());
        let f = d.0 as f32 / steps as f32;
        let g = d.1 as f32 / steps as f32;
        let h = d.2 as f32 / steps as f32;
        for j in 0..=steps {
            let p = (a.0 + (0.5 + j as f32 * f).floor() as i32, a.1 + (0.5 + j as f32 * g).floor() as i32, a.2 + (0.5 + j as f32 * h).floor() as i32);
            if place {
                let ax = (p.0 - a.0).abs();
                let az = (p.2 - a.2).abs();
                let mx = ax.max(az);
                let axis = if mx > 0 { if ax == mx { "x" } else { "z" } } else { "y" };
                self.place_log(reg, r, p, Some(axis));
                continue;
            }
            if !self.is_free(reg, p) { return false; }
        }
        true
    }

    // ---- foliage placers ----

    fn foliage_height(&self, r: &mut R, trunk_height: i32) -> i32 {
        match &self.t().foliage.kind {
            FoliageKind::Blob { height } | FoliageKind::Fancy { height } | FoliageKind::Bush { height } | FoliageKind::Jungle { height } => *height,
            FoliageKind::Spruce { trunk_height: th } => 4.max(trunk_height - th.sample(r)),
            FoliageKind::Pine { height } => height.sample(r),
            FoliageKind::MegaPine { crown } => crown.sample(r),
            FoliageKind::DarkOak => 4,
            FoliageKind::Acacia => 0,
        }
    }

    fn foliage_radius(&self, r: &mut R, trunk_height: i32) -> i32 {
        let base = self.t().foliage.radius.sample(r);
        match self.t().foliage.kind {
            FoliageKind::Pine { .. } => base + r.next_int_bounded((trunk_height + 1).max(1)),
            _ => base,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn create_foliage(&mut self, reg: &mut Region, r: &mut R, _max_free: i32, a: &Att, fh: i32, fr: i32) {
        let offset = self.t().foliage.offset.sample(r);
        let p = a.pos;
        let ro = a.radius_offset;
        let dbl = a.double;
        match &self.t().foliage.kind {
            FoliageKind::Blob { .. } => {
                let mut m = offset;
                while m >= offset - fh {
                    let n = (fr + ro - 1 - m / 2).max(0);
                    self.leaves_row(reg, r, p, n, m, dbl);
                    m -= 1;
                }
            }
            FoliageKind::Fancy { .. } => {
                let mut m = offset;
                while m >= offset - fh {
                    let n = fr + if m == offset || m == offset - fh { 0 } else { 1 };
                    self.leaves_row(reg, r, p, n, m, dbl);
                    m -= 1;
                }
            }
            FoliageKind::Bush { .. } => {
                let mut m = offset;
                while m >= offset - fh {
                    let n = fr + ro - 1 - m;
                    self.leaves_row(reg, r, p, n, m, dbl);
                    m -= 1;
                }
            }
            FoliageKind::Spruce { .. } => {
                let mut i = r.next_int_bounded(2);
                let mut j = 1;
                let mut k = 0;
                let mut l = offset;
                while l >= -fh {
                    self.leaves_row(reg, r, p, i, l, dbl);
                    if i >= j { i = k; k = 1; j = (j + 1).min(fr + ro); } else { i += 1; }
                    l -= 1;
                }
            }
            FoliageKind::Pine { .. } => {
                let mut i = 0;
                let mut j = offset;
                while j >= offset - fh {
                    self.leaves_row(reg, r, p, i, j, dbl);
                    if i >= 1 && j == offset - fh + 1 { i -= 1; } else if i < fr + ro { i += 1; }
                    j -= 1;
                }
            }
            FoliageKind::MegaPine { .. } => {
                let mut i = 0;
                let mut j = p.1 - fh + offset;
                while j <= p.1 + offset {
                    let k = p.1 - j;
                    let l = fr + ro + ((k as f32 / fh as f32) * 3.5).floor() as i32;
                    let m = if k > 0 && l == i && (j & 1) == 0 { l + 1 } else { l };
                    self.leaves_row(reg, r, (p.0, j, p.2), m, 0, dbl);
                    i = l;
                    j += 1;
                }
            }
            FoliageKind::DarkOak => {
                let q = (p.0, p.1 + offset, p.2);
                if dbl {
                    self.leaves_row(reg, r, q, fr + 2, -1, dbl);
                    self.leaves_row(reg, r, q, fr + 3, 0, dbl);
                    self.leaves_row(reg, r, q, fr + 2, 1, dbl);
                    if r.next_bool() { self.leaves_row(reg, r, q, fr, 2, dbl); }
                } else {
                    self.leaves_row(reg, r, q, fr + 2, -1, dbl);
                    self.leaves_row(reg, r, q, fr + 1, 0, dbl);
                }
            }
            FoliageKind::Jungle { .. } => {
                let i = if dbl { fh } else { 1 + r.next_int_bounded(2) };
                let mut j = offset;
                while j >= offset - i {
                    let k = fr + ro + 1 - j;
                    self.leaves_row(reg, r, p, k, j, dbl);
                    j -= 1;
                }
            }
            FoliageKind::Acacia => {
                let q = (p.0, p.1 + offset, p.2);
                self.leaves_row(reg, r, q, fr + ro, -1 - fh, dbl);
                self.leaves_row(reg, r, q, fr - 1, -fh, dbl);
                self.leaves_row(reg, r, q, fr + ro - 1, 0, dbl);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn leaves_row(&mut self, reg: &mut Region, r: &mut R, p: Pos, range: i32, ly: i32, large: bool) {
        let i = if large { 1 } else { 0 };
        for j in -range..=range + i {
            for k in -range..=range + i {
                if self.skip_signed(r, j, ly, k, range, large) { continue; }
                self.try_leaf(reg, r, (p.0 + j, p.1 + ly, p.2 + k));
            }
        }
    }

    fn skip_signed(&self, r: &mut R, dx: i32, y: i32, dz: i32, range: i32, large: bool) -> bool {
        if let FoliageKind::DarkOak = self.t().foliage.kind {
            if y == 0 && large && (dx == -range || dx >= range) && (dz == -range || dz >= range) { return true; }
        }
        let (i, j) = if large { (dx.abs().min((dx - 1).abs()), dz.abs().min((dz - 1).abs())) } else { (dx.abs(), dz.abs()) };
        self.skip(r, i, y, j, range, large)
    }

    fn skip(&self, r: &mut R, dx: i32, y: i32, dz: i32, range: i32, large: bool) -> bool {
        match self.t().foliage.kind {
            FoliageKind::Blob { .. } => dx == range && dz == range && (r.next_int_bounded(2) == 0 || y == 0),
            FoliageKind::Fancy { .. } => (dx as f32 + 0.5).powi(2) + (dz as f32 + 0.5).powi(2) > (range * range) as f32,
            FoliageKind::Bush { .. } => dx == range && dz == range && r.next_int_bounded(2) == 0,
            FoliageKind::Spruce { .. } | FoliageKind::Pine { .. } => dx == range && dz == range && range > 0,
            FoliageKind::MegaPine { .. } | FoliageKind::Jungle { .. } => dx + dz >= 7 || dx * dx + dz * dz > range * range,
            FoliageKind::DarkOak => {
                if y == -1 && !large { return dx == range && dz == range; }
                if y == 1 { return dx + dz > range * 2 - 2; }
                false
            }
            FoliageKind::Acacia => if y == 0 { (dx > 1 || dz > 1) && dx != 0 && dz != 0 } else { dx == range && dz == range && range > 0 },
        }
    }

    fn try_leaf(&mut self, reg: &mut Region, r: &mut R, p: Pos) {
        if !self.valid_tree_pos(reg, p) { return; }
        let Some(mut s) = self.t().foliage_provider.get(self.f, reg, r, p.0, p.1, p.2) else { return };
        let bs = BlockState(s as u32);
        if bs.get("waterlogged").is_some() {
            let here = reg.block(p.0, p.1, p.2);
            let src = BlockState(here as u32).name() == "water" && BlockState(here as u32).get("level") == Some("0")
                || BlockState(here as u32).get("waterlogged") == Some("true");
            if let Some(n) = bs.with("waterlogged", if src { "true" } else { "false" }) { s = n.0 as u16; }
        }
        self.leaves.insert(p);
        reg.set(p.0, p.1, p.2, s);
    }

    // ---- decorators ----

    fn deco_set(&mut self, reg: &mut Region, p: Pos, s: u16) { self.decos.insert(p); reg.set(p.0, p.1, p.2, s); }
    fn is_air(reg: &Region, p: Pos) -> bool { flag(reg.block(p.0, p.1, p.2), F_AIR) }

    fn vine(&mut self, reg: &mut Region, p: Pos, face: &str) {
        if let Some(v) = BlockState::from_name("vine").and_then(|v| v.with(face, "true")) { self.deco_set(reg, p, v.0 as u16); }
    }

    fn hanging_vine(&mut self, reg: &mut Region, p: Pos, face: &str) {
        self.vine(reg, p, face);
        let mut q = (p.0, p.1 - 1, p.2);
        let mut i = 4;
        while Self::is_air(reg, q) && i > 0 { self.vine(reg, q, face); q.1 -= 1; i -= 1; }
    }

    fn decorate(&mut self, reg: &mut Region, r: &mut R, d: &Decorator, logs: &[Pos], leaves: &[Pos]) {
        match d {
            Decorator::Beehive(p) => {
                if logs.is_empty() { return; }
                if r.next_float() >= *p { return; }
                let i = if !leaves.is_empty() { (leaves[0].1 - 1).max(logs[0].1 + 1) } else { (logs[0].1 + 1 + r.next_int_bounded(3)).min(logs[logs.len() - 1].1) };
                let mut cand: Vec<Pos> = Vec::new();
                for l in logs.iter().filter(|l| l.1 == i) {
                    for (dx, dz) in [(1, 0), (0, 1), (-1, 0)] { cand.push((l.0 + dx, l.1, l.2 + dz)); }
                }
                if cand.is_empty() { return; }
                let n = cand.len();
                for k in (2..=n).rev() { let j = r.next_int_bounded(k as i32) as usize; cand.swap(k - 1, j); }
                if let Some(p) = cand.into_iter().find(|p| Self::is_air(reg, *p) && Self::is_air(reg, (p.0, p.1, p.2 + 1))) {
                    self.deco_set(reg, p, st("bee_nest[facing=south]"));
                    let _ = r.next_int_bounded(2);
                }
            }
            Decorator::AlterGround(prov) => {
                let list: Vec<Pos> = logs.to_vec();
                if list.is_empty() { return; }
                let i = list[0].1;
                for p in list.iter().filter(|p| p.1 == i) {
                    self.circle(reg, r, prov, (p.0 - 1, p.1, p.2 - 1));
                    self.circle(reg, r, prov, (p.0 + 2, p.1, p.2 - 1));
                    self.circle(reg, r, prov, (p.0 - 1, p.1, p.2 + 2));
                    self.circle(reg, r, prov, (p.0 + 2, p.1, p.2 + 2));
                    for _ in 0..5 {
                        let k = r.next_int_bounded(64);
                        let l = k % 8;
                        let m = k / 8;
                        if l == 0 || l == 7 || m == 0 || m == 7 { self.circle(reg, r, prov, (p.0 - 3 + l, p.1, p.2 - 3 + m)); }
                    }
                }
            }
            Decorator::LeaveVine(prob) => {
                for l in leaves {
                    if r.next_float() < *prob { let p = (l.0 - 1, l.1, l.2); if Self::is_air(reg, p) { self.hanging_vine(reg, p, "east"); } }
                    if r.next_float() < *prob { let p = (l.0 + 1, l.1, l.2); if Self::is_air(reg, p) { self.hanging_vine(reg, p, "west"); } }
                    if r.next_float() < *prob { let p = (l.0, l.1, l.2 - 1); if Self::is_air(reg, p) { self.hanging_vine(reg, p, "south"); } }
                    if r.next_float() < *prob { let p = (l.0, l.1, l.2 + 1); if Self::is_air(reg, p) { self.hanging_vine(reg, p, "north"); } }
                }
            }
            Decorator::AttachedToLogs { prob, provider, dirs } => {
                let mut list = logs.to_vec();
                let n = list.len();
                for k in (2..=n).rev() { let j = r.next_int_bounded(k as i32) as usize; list.swap(k - 1, j); }
                for p in list {
                    let d = super::DIRS[dirs[r.next_int_bounded(dirs.len() as i32) as usize]];
                    let q = (p.0 + d.0, p.1 + d.1, p.2 + d.2);
                    if !(r.next_float() <= *prob) || !Self::is_air(reg, q) { continue; }
                    if let Some(s) = provider.get(self.f, reg, r, q.0, q.1, q.2) { self.deco_set(reg, q, s); }
                }
            }
            Decorator::TrunkVine => {
                for l in logs {
                    if r.next_int_bounded(3) > 0 { let p = (l.0 - 1, l.1, l.2); if Self::is_air(reg, p) { self.vine(reg, p, "east"); } }
                    if r.next_int_bounded(3) > 0 { let p = (l.0 + 1, l.1, l.2); if Self::is_air(reg, p) { self.vine(reg, p, "west"); } }
                    if r.next_int_bounded(3) > 0 { let p = (l.0, l.1, l.2 - 1); if Self::is_air(reg, p) { self.vine(reg, p, "south"); } }
                    if r.next_int_bounded(3) > 0 { let p = (l.0, l.1, l.2 + 1); if Self::is_air(reg, p) { self.vine(reg, p, "north"); } }
                }
            }
            Decorator::Cocoa(prob) => {
                if r.next_float() >= *prob || logs.is_empty() { return; }
                let i = logs[0].1;
                for l in logs.iter().filter(|l| l.1 - i <= 2) {
                    for (face, (dx, dz)) in [("north", (0, -1)), ("east", (1, 0)), ("south", (0, 1)), ("west", (-1, 0))] {
                        if r.next_float() > 0.25 { continue; }
                        let q = (l.0 - dx, l.1, l.2 - dz);
                        if !Self::is_air(reg, q) { continue; }
                        let age = r.next_int_bounded(3);
                        let s = BlockState::from_name("cocoa").and_then(|c| c.with("age", &age.to_string())).and_then(|c| c.with("facing", face));
                        if let Some(s) = s { self.deco_set(reg, q, s.0 as u16); }
                    }
                }
            }
            Decorator::PlaceOnGround { tries, radius, height, provider } => {
                if logs.is_empty() { return; }
                let p0 = logs[0];
                let i = p0.1;
                let (mut j, mut k, mut l, mut m) = (p0.0, p0.0, p0.2, p0.2);
                for p in logs.iter().filter(|p| p.1 == i) { j = j.min(p.0); k = k.max(p.0); l = l.min(p.2); m = m.max(p.2); }
                let (x0, x1, y0, y1, z0, z1) = (j - radius, k + radius, i - height, i + height, l - radius, m + radius);
                for _ in 0..*tries {
                    let x = r.next_int_between(x0, x1);
                    let y = r.next_int_between(y0, y1);
                    let z = r.next_int_between(z0, z1);
                    let up = (x, y + 1, z);
                    let ub = reg.block(up.0, up.1, up.2);
                    let below = reg.block(x, y, z);
                    if (flag(ub, F_AIR) || BlockState(ub as u32).name() == "vine")
                        && crate::region::flag(below, crate::region::F_FULL) && !self.f.tag("leaves")[below as usize]
                        && reg.height(crate::region::Hm::MotionBlockingNoLeaves, x, z) <= up.1 {
                        if let Some(s) = provider.get(self.f, reg, r, up.0, up.1, up.2) { self.deco_set(reg, up, s); }
                    }
                }
            }
        }
    }

    fn circle(&mut self, reg: &mut Region, r: &mut R, prov: &StateProvider, c: Pos) {
        for i in -2..=2 {
            for j in -2..=2 {
                if i32::abs(i) == 2 && i32::abs(j) == 2 { continue; }
                let p = (c.0 + i, c.1, c.2 + j);
                let mut k = 2;
                while k >= -3 {
                    let q = (p.0, p.1 + k, p.2);
                    let b = reg.block(q.0, q.1, q.2);
                    if self.f.tag("substrate_overworld")[b as usize] {
                        if let Some(s) = prov.get(self.f, reg, r, p.0, p.1, p.2) { self.deco_set(reg, q, s); }
                        break;
                    }
                    if !flag(b, F_AIR) && k < 0 { break; }
                    k -= 1;
                }
            }
        }
    }

    // ---- leaf distances ----

    fn update_leaves(&mut self, reg: &mut Region) {
        let dist_of = |s: u16| BlockState(s as u32).get("distance").and_then(|d| d.parse::<i32>().ok());
        let mut levels: Vec<HashSet<Pos>> = vec![HashSet::new(); 7];
        let logs: Vec<Pos> = self.logs.items.clone();
        let logset: HashSet<Pos> = logs.iter().copied().collect();
        for p in &logs {
            for (dx, dy, dz) in super::DIRS {
                let m = (p.0 + dx, p.1 + dy, p.2 + dz);
                if logset.contains(&m) { continue; }
                let s = reg.block(m.0, m.1, m.2);
                if dist_of(s).is_none() { continue; }
                levels[0].insert(m);
                if let Some(n) = BlockState(s as u32).with("distance", "1") { reg.set(m.0, m.1, m.2, n.0 as u16); }
            }
        }
        for i in 1..7 {
            let prev: Vec<Pos> = levels[i - 1].iter().copied().collect();
            for p in prev {
                for (dx, dy, dz) in super::DIRS {
                    let m = (p.0 + dx, p.1 + dy, p.2 + dz);
                    if levels[i - 1].contains(&m) || levels[i].contains(&m) { continue; }
                    let s = reg.block(m.0, m.1, m.2);
                    let Some(j) = dist_of(s) else { continue };
                    if j <= i as i32 + 1 { continue; }
                    if let Some(n) = BlockState(s as u32).with("distance", &(i + 1).to_string()) { reg.set(m.0, m.1, m.2, n.0 as u16); }
                    levels[i].insert(m);
                }
            }
        }
    }
}

pub struct FallenTree {
    trunk: StateProvider,
    length: IntProvider,
    stump: Vec<Decorator>,
    log: Vec<Decorator>,
}

impl FallenTree {
    pub fn parse(f: &mut Features, c: &Value) -> Result<FallenTree> {
        Ok(FallenTree {
            trunk: StateProvider::parse(&f.tags, &c["trunk_provider"])?,
            length: IntProvider::parse(&c["log_length"])?,
            stump: parse_decorators(f, &c["stump_decorators"])?,
            log: parse_decorators(f, &c["log_decorators"])?,
        })
    }

    pub fn place(&self, f: &Features, reg: &mut Region, r: &mut R, x: i32, y: i32, z: i32) -> bool {
        let mut pl = Placing { f, tree: None, logs: JSet::default(), leaves: JSet::default(), decos: JSet::default() };
        // stump
        if let Some(s) = self.trunk.get(f, reg, r, x, y, z) { reg.set(x, y, z, s); }
        let mut stump = JSet::default();
        stump.insert((x, y, z));
        let sl = stump.sorted_by_y();
        for d in &self.stump { pl.decorate(reg, r, d, &sl, &[]); }
        let (dx, dz, axis) = [(0, -1, "z"), (1, 0, "x"), (0, 1, "z"), (-1, 0, "x")][r.next_int_bounded(4) as usize];
        let len = self.length.sample(r) - 2;
        let k = 2 + r.next_int_bounded(2);
        let mut p = (x + dx * k, y + 1, z + dz * k);
        let sturdy_below = |reg: &Region, p: Pos| crate::region::flag(reg.block(p.0, p.1 - 1, p.2), crate::region::F_FULL);
        for _ in 0..6 {
            if pl.valid_tree_pos(reg, p) && sturdy_below(reg, p) { break; }
            p.1 -= 1;
        }
        let mut q = p;
        let mut gaps = 0;
        for _ in 0..len {
            if !pl.valid_tree_pos(reg, q) { return true; }
            if !sturdy_below(reg, q) { gaps += 1; if gaps > 2 { return true; } } else { gaps = 0; }
            q = (q.0 + dx, q.1, q.2 + dz);
        }
        let mut set = JSet::default();
        let mut q = p;
        for _ in 0..len {
            if let Some(s) = self.trunk.get(f, reg, r, q.0, q.1, q.2) {
                let s = BlockState(s as u32).with("axis", axis).map(|b| b.0 as u16).unwrap_or(s);
                reg.set(q.0, q.1, q.2, s);
            }
            set.insert(q);
            q = (q.0 + dx, q.1, q.2 + dz);
        }
        let ls = set.sorted_by_y();
        for d in &self.log { pl.decorate(reg, r, d, &ls, &[]); }
        true
    }
}

fn tree_shape(height: i32, y: i32) -> f32 {
    if (y as f32) < height as f32 * 0.3 { return -1.0; }
    let f = height as f32 / 2.0;
    let g = f - y as f32;
    let mut h = (f * f - g * g).sqrt();
    if g == 0.0 { h = f; } else if g.abs() >= f { return 0.0; }
    h * 0.5
}

fn trim(height: i32, local_y: i32) -> bool { local_y as f64 >= height as f64 * 0.2 }

/// Huge red / brown mushrooms.
#[allow(clippy::too_many_arguments)]
pub fn huge_mushroom(f: &Features, red: bool, cap: &StateProvider, stem: &StateProvider, radius: i32, on: &Pred, reg: &mut Region, r: &mut R, x: i32, y: i32, z: i32) -> bool {
    let mut h = r.next_int_bounded(3) + 4;
    if r.next_int_bounded(12) == 0 { h *= 2; }
    if y < crate::MIN_Y + 1 || y + h + 1 > crate::MIN_Y + crate::HEIGHT - 1 { return false; }
    if !on.test(f, reg, x, y - 1, z) { return false; }
    for j in 0..=h {
        let k = if red { 0 } else if j <= 3 { 0 } else { radius };
        for l in -k..=k {
            for n in -k..=k {
                let b = reg.block(x + l, y + j, z + n);
                if flag(b, F_AIR) || f.tag("leaves")[b as usize] { continue; }
                return false;
            }
        }
    }
    let place = |reg: &mut Region, p: Pos, s: u16| {
        let b = reg.block(p.0, p.1, p.2);
        if flag(b, F_AIR) || f.tag("replaceable_by_mushrooms")[b as usize] { reg.set(p.0, p.1, p.2, s); }
    };
    let set_faces = |s: u16, faces: &[(&str, bool)]| -> u16 {
        let mut b = BlockState(s as u32);
        for (k, v) in faces { if let Some(n) = b.with(k, if *v { "true" } else { "false" }) { b = n; } }
        b.0 as u16
    };
    if red {
        let mut i = h - 3;
        while i <= h {
            let j = if i < h { radius } else { radius - 1 };
            let k = radius - 2;
            for l in -j..=j {
                for m in -j..=j {
                    let b5 = l == -j || l == j;
                    let b6 = m == -j || m == j;
                    if i < h && b5 == b6 { continue; }
                    let Some(s) = cap.get(f, reg, r, x, y, z) else { continue };
                    let s = set_faces(s, &[("up", i >= h - 1), ("west", l < -k), ("east", l > k), ("north", m < -k), ("south", m > k)]);
                    place(reg, (x + l, y + i, z + m), s);
                }
            }
            i += 1;
        }
    } else {
        let i = radius;
        for j in -i..=i {
            for k in -i..=i {
                let (b1, b2, b3, b4) = (j == -i, j == i, k == -i, k == i);
                let b5 = b1 || b2;
                let b6 = b3 || b4;
                if b5 && b6 { continue; }
                let b7 = b1 || (b6 && j == 1 - i);
                let b8 = b2 || (b6 && j == i - 1);
                let b9 = b3 || (b5 && k == 1 - i);
                let b10 = b4 || (b5 && k == i - 1);
                let Some(s) = cap.get(f, reg, r, x, y, z) else { continue };
                let s = set_faces(s, &[("west", b7), ("east", b8), ("north", b9), ("south", b10)]);
                place(reg, (x + j, y + h, z + k), s);
            }
        }
    }
    for i in 0..h {
        if let Some(s) = stem.get(f, reg, r, x, y, z) { place(reg, (x, y + i, z), s); }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jset_follows_java_hashset_order() {
        let mut s = JSet::default();
        // hashCode(x,y,z) = (y + 31z) * 31 + x -> buckets 15, 1, 0 for these three.
        s.insert((0, 1, 0));
        s.insert((1, 0, 0));
        s.insert((0, 0, 0));
        assert!(!s.insert((1, 0, 0)));
        assert_eq!(s.java_order(), vec![(0, 0, 0), (1, 0, 0), (0, 1, 0)]);
        assert_eq!(s.sorted_by_y(), vec![(0, 0, 0), (1, 0, 0), (0, 1, 0)]);
        // Same bucket keeps insertion order: (16,0,0) and (0,0,0) collide in a 16-slot table.
        let mut t = JSet::default();
        t.insert((16, 0, 0));
        t.insert((0, 0, 0));
        assert_eq!(t.java_order(), vec![(16, 0, 0), (0, 0, 0)]);
    }

    #[test]
    fn fancy_tree_shape() {
        assert_eq!(tree_shape(10, 1), -1.0);
        assert!(tree_shape(10, 5) > 0.0);
        assert!(trim(10, 2) && !trim(10, 1));
    }
}

//! Nether decoration: huge fungi, nether vegetation, vines, glowstone, basalt and blobs.

use super::provider::{IntProvider, StateProvider};
use super::{mth_next_int, Features, Pred, R};
use crate::random::Rng;
use crate::region::{flag, Hm, Region, F_AIR};
use crate::state_from_json;
use anyhow::Result;
use basalt_data::BlockState;
use serde_json::Value;

fn st(name: &str) -> u16 {
    BlockState::parse(name).map(|s| s.0 as u16).unwrap_or(0)
}
fn is(reg: &Region, x: i32, y: i32, z: i32, s: u16) -> bool {
    reg.block(x, y, z) == s
}
fn air(reg: &Region, x: i32, y: i32, z: i32) -> bool {
    flag(reg.block(x, y, z), F_AIR)
}
fn name_is(reg: &Region, x: i32, y: i32, z: i32, n: &str) -> bool {
    BlockState(reg.block(x, y, z) as u32).name() == n
}
fn min_y(reg: &Region) -> i32 {
    reg.g.dim.min_y()
}
fn max_y(reg: &Region) -> i32 {
    reg.g.dim.min_y() + reg.g.dim.sections() as i32 * 16 - 1
}

// ---- count_on_every_layer ----

fn is_empty_layer(s: u16) -> bool {
    flag(s, F_AIR) || matches!(BlockState(s as u32).name(), "water" | "lava")
}

/// Positions of the "count_on_every_layer" placement: per layer, `count` random columns whose n-th
/// floor (air above a non-empty, non-bedrock block, counted from the top) is used.
pub fn every_layer(
    reg: &mut Region,
    r: &mut R,
    count: &IntProvider,
    x0: i32,
    z0: i32,
) -> Vec<(i32, i32, i32)> {
    let mut out = Vec::new();
    let miny = min_y(reg);
    let mut layer = 0;
    loop {
        let mut any = false;
        let mut j = 0;
        while j < count.sample(r) {
            j += 1;
            let x = r.next_int_bounded(16) + x0;
            let z = r.next_int_bounded(16) + z0;
            let h = reg.height(Hm::MotionBlocking, x, z);
            // find the layer-th floor
            let mut remaining = layer;
            let mut prev = reg.block(x, h, z);
            let mut found = None;
            let mut k = h;
            while k > miny {
                let below = reg.block(x, k - 1, z);
                if !is_empty_layer(below)
                    && is_empty_layer(prev)
                    && !name_is(reg, x, k - 1, z, "bedrock")
                {
                    if remaining == 0 {
                        found = Some(k);
                        break;
                    }
                    remaining -= 1;
                }
                prev = below;
                k -= 1;
            }
            if let Some(k) = found {
                out.push((x, k, z));
                any = true;
            }
        }
        layer += 1;
        if !any {
            break;
        }
    }
    out
}

// ---- huge fungus ----

pub struct HugeFungus {
    decor: u16,
    vines: bool,
    hat: u16,
    stem: u16,
    replaceable: Pred,
    valid_base: &'static str,
    planted: bool,
}

impl HugeFungus {
    pub fn parse(f: &mut Features, c: &Value) -> Result<HugeFungus> {
        Ok(HugeFungus {
            decor: state_from_json(&c["decor_state"])?,
            vines: BlockState(state_from_json(&c["hat_state"])? as u32).name()
                == "nether_wart_block",
            hat: state_from_json(&c["hat_state"])?,
            stem: state_from_json(&c["stem_state"])?,
            replaceable: Pred::parse(&f.tags, &c["replaceable_blocks"])?,
            valid_base: BlockState(state_from_json(&c["valid_base_block"])? as u32).name(),
            planted: c["planted"].as_bool().unwrap_or(false),
        })
    }

    fn replaceable(
        &self,
        f: &Features,
        reg: &mut Region,
        x: i32,
        y: i32,
        z: i32,
        plants: bool,
    ) -> bool {
        f.replaceable[reg.block(x, y, z) as usize]
            || (plants && self.replaceable.test(f, reg, x, y, z))
    }

    pub fn place(&self, f: &Features, reg: &mut Region, r: &mut R, x: i32, y: i32, z: i32) -> bool {
        if BlockState(reg.block(x, y - 1, z) as u32).name() != self.valid_base {
            return false;
        }
        let mut h = mth_next_int(r, 4, 13);
        if r.next_int_bounded(12) == 0 {
            h *= 2;
        }
        if !self.planted && y + h + 1 >= reg.g.gen_height {
            return false;
        }
        let thick = !self.planted && r.next_float() < 0.06;
        reg.set(x, y, z, 0);
        self.stem(f, reg, r, x, y, z, h, thick);
        self.hat(f, reg, r, x, y, z, h, thick);
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn stem(
        &self,
        f: &Features,
        reg: &mut Region,
        r: &mut R,
        x: i32,
        y: i32,
        z: i32,
        h: i32,
        thick: bool,
    ) {
        let j = thick as i32;
        for k in -j..=j {
            for l in -j..=j {
                let corner = thick && k.abs() == j && l.abs() == j;
                for m in 0..h {
                    let (px, py, pz) = (x + k, y + m, z + l);
                    if !self.replaceable(f, reg, px, py, pz, true) {
                        continue;
                    }
                    if corner {
                        if r.next_float() < 0.1 {
                            reg.set(px, py, pz, self.stem);
                        }
                    } else {
                        reg.set(px, py, pz, self.stem);
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn hat(
        &self,
        f: &Features,
        reg: &mut Region,
        r: &mut R,
        x: i32,
        y: i32,
        z: i32,
        h: i32,
        thick: bool,
    ) {
        let j = (r.next_int_bounded(1 + h / 3) + 5).min(h);
        let k = h - j;
        for l in k..=h {
            let mut m = if l < h - r.next_int_bounded(3) { 2 } else { 1 };
            if j > 8 && l < k + 4 {
                m = 3;
            }
            if thick {
                m += 1;
            }
            for n in -m..=m {
                for o in -m..=m {
                    let edge_x = n == -m || n == m;
                    let edge_z = o == -m || o == m;
                    let inner = !edge_x && !edge_z && l != h;
                    let corner = edge_x && edge_z;
                    let low = l < k + 3;
                    let (px, py, pz) = (x + n, y + l, z + o);
                    if !self.replaceable(f, reg, px, py, pz, false) {
                        continue;
                    }
                    if low {
                        if !inner {
                            self.hat_drop(reg, r, px, py, pz);
                        }
                    } else if inner {
                        self.hat_block(reg, r, px, py, pz, 0.1, 0.2);
                    } else if corner {
                        self.hat_block(reg, r, px, py, pz, 0.01, 0.7);
                    } else {
                        self.hat_block(reg, r, px, py, pz, 0.0005, 0.98);
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn hat_block(&self, reg: &mut Region, r: &mut R, x: i32, y: i32, z: i32, decor: f32, hat: f32) {
        if r.next_float() < decor {
            reg.set(x, y, z, self.decor);
        } else if r.next_float() < hat {
            reg.set(x, y, z, self.hat);
            if self.vines && r.next_float() < 0.083 {
                weeping_column_below(reg, r, x, y, z);
            }
        }
    }

    fn hat_drop(&self, reg: &mut Region, r: &mut R, x: i32, y: i32, z: i32) {
        if is(reg, x, y - 1, z, self.hat) {
            reg.set(x, y, z, self.hat);
        } else if r.next_float() < 0.15 {
            reg.set(x, y, z, self.hat);
            if self.vines && r.next_int_bounded(11) == 0 {
                weeping_column_below(reg, r, x, y, z);
            }
        }
    }
}

fn weeping_column_below(reg: &mut Region, r: &mut R, x: i32, y: i32, z: i32) {
    if !air(reg, x, y - 1, z) {
        return;
    }
    let mut h = mth_next_int(r, 1, 5);
    if r.next_int_bounded(7) == 0 {
        h *= 2;
    }
    weeping_column(reg, r, x, y - 1, z, h, 23, 25);
}

#[allow(clippy::too_many_arguments)]
fn weeping_column(
    reg: &mut Region,
    r: &mut R,
    x: i32,
    mut y: i32,
    z: i32,
    height: i32,
    min_age: i32,
    max_age: i32,
) {
    let plant = st("weeping_vines_plant");
    let head = BlockState::from_name("weeping_vines").unwrap();
    for i in 0..=height {
        if air(reg, x, y, z) {
            if i == height || !air(reg, x, y - 1, z) {
                let age = mth_next_int(r, min_age, max_age);
                reg.set(
                    x,
                    y,
                    z,
                    head.with("age", &age.to_string())
                        .map(|b| b.0 as u16)
                        .unwrap_or(head.0 as u16),
                );
                break;
            }
            reg.set(x, y, z, plant);
        }
        y -= 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn twisting_column(
    reg: &mut Region,
    r: &mut R,
    x: i32,
    mut y: i32,
    z: i32,
    height: i32,
    min_age: i32,
    max_age: i32,
) {
    let plant = st("twisting_vines_plant");
    let head = BlockState::from_name("twisting_vines").unwrap();
    for i in 0..=height {
        if air(reg, x, y, z) {
            if i == height || !air(reg, x, y + 1, z) {
                let age = mth_next_int(r, min_age, max_age);
                reg.set(
                    x,
                    y,
                    z,
                    head.with("age", &age.to_string())
                        .map(|b| b.0 as u16)
                        .unwrap_or(head.0 as u16),
                );
                break;
            }
            reg.set(x, y, z, plant);
        }
        y += 1;
    }
}

// ---- vegetation ----

pub struct Vegetation {
    pub provider: StateProvider,
    pub width: i32,
    pub height: i32,
}

pub fn nether_vegetation(
    f: &Features,
    c: &Vegetation,
    reg: &mut Region,
    r: &mut R,
    x: i32,
    y: i32,
    z: i32,
) -> bool {
    let below = reg.block(x, y - 1, z);
    if !f.tags.get("nylium")[below as usize] {
        return false;
    }
    if y < 2 || y + 1 >= 256 {
        return false;
    }
    let mut n = 0;
    for _ in 0..c.width * c.width {
        let dx = r.next_int_bounded(c.width) - r.next_int_bounded(c.width);
        let dy = r.next_int_bounded(c.height) - r.next_int_bounded(c.height);
        let dz = r.next_int_bounded(c.width) - r.next_int_bounded(c.width);
        let (px, py, pz) = (x + dx, y + dy, z + dz);
        let Some(s) = c.provider.get(f, reg, r, px, py, pz) else {
            continue;
        };
        if air(reg, px, py, pz) && py > min_y(reg) && f.can_survive(reg, s, px, py, pz) {
            reg.set(px, py, pz, s);
            n += 1;
        }
    }
    n > 0
}

pub fn weeping_vines(reg: &mut Region, r: &mut R, x: i32, y: i32, z: i32) -> bool {
    if !air(reg, x, y, z) {
        return false;
    }
    let (nr, wart) = (st("netherrack"), st("nether_wart_block"));
    let above = reg.block(x, y + 1, z);
    if above != nr && above != wart {
        return false;
    }
    // roof nether wart
    reg.set(x, y, z, wart);
    for _ in 0..200 {
        let px = x + r.next_int_bounded(6) - r.next_int_bounded(6);
        let py = y + r.next_int_bounded(2) - r.next_int_bounded(5);
        let pz = z + r.next_int_bounded(6) - r.next_int_bounded(6);
        if !air(reg, px, py, pz) {
            continue;
        }
        let mut j = 0;
        for (dx, dy, dz) in super::DIRS {
            let b = reg.block(px + dx, py + dy, pz + dz);
            if b == nr || b == wart {
                j += 1;
            }
            if j > 1 {
                break;
            }
        }
        if j == 1 {
            reg.set(px, py, pz, wart);
        }
    }
    // roof weeping vines
    for _ in 0..100 {
        let px = x + r.next_int_bounded(8) - r.next_int_bounded(8);
        let py = y + r.next_int_bounded(2) - r.next_int_bounded(7);
        let pz = z + r.next_int_bounded(8) - r.next_int_bounded(8);
        if !air(reg, px, py, pz) {
            continue;
        }
        let a = reg.block(px, py + 1, pz);
        if a == nr || a == wart {
            let mut h = mth_next_int(r, 1, 8);
            if r.next_int_bounded(6) == 0 {
                h *= 2;
            }
            if r.next_int_bounded(5) == 0 {
                h = 1;
            }
            weeping_column(reg, r, px, py, pz, h, 17, 25);
        }
    }
    true
}

fn twisting_invalid(reg: &Region, x: i32, y: i32, z: i32) -> bool {
    if !air(reg, x, y, z) {
        return true;
    }
    let b = reg.block(x, y - 1, z);
    !(b == st("netherrack") || b == st("warped_nylium") || b == st("warped_wart_block"))
}

#[allow(clippy::too_many_arguments)]
pub fn twisting_vines(
    reg: &mut Region,
    r: &mut R,
    x: i32,
    y: i32,
    z: i32,
    width: i32,
    height: i32,
    max_height: i32,
) -> bool {
    if twisting_invalid(reg, x, y, z) {
        return false;
    }
    for _ in 0..width * width {
        let px = x + mth_next_int(r, -width, width);
        let mut py = y + mth_next_int(r, -height, height);
        let pz = z + mth_next_int(r, -width, width);
        // find the first air block above ground
        let mut ok = true;
        loop {
            py -= 1;
            if py < min_y(reg) || py > max_y(reg) {
                ok = false;
                break;
            }
            if !air(reg, px, py, pz) {
                break;
            }
        }
        if !ok {
            continue;
        }
        py += 1;
        if twisting_invalid(reg, px, py, pz) {
            continue;
        }
        let mut h = mth_next_int(r, 1, max_height);
        if r.next_int_bounded(6) == 0 {
            h *= 2;
        }
        if r.next_int_bounded(5) == 0 {
            h = 1;
        }
        twisting_column(reg, r, px, py, pz, h, 17, 25);
    }
    true
}

// ---- glowstone, basalt ----

pub fn glowstone_blob(reg: &mut Region, r: &mut R, x: i32, y: i32, z: i32) -> bool {
    if !air(reg, x, y, z) {
        return false;
    }
    let a = BlockState(reg.block(x, y + 1, z) as u32).name();
    if !matches!(a, "netherrack" | "basalt" | "blackstone") {
        return false;
    }
    let gs = st("glowstone");
    reg.set(x, y, z, gs);
    for _ in 0..1500 {
        let px = x + r.next_int_bounded(8) - r.next_int_bounded(8);
        let py = y - r.next_int_bounded(12);
        let pz = z + r.next_int_bounded(8) - r.next_int_bounded(8);
        if !air(reg, px, py, pz) {
            continue;
        }
        let mut j = 0;
        for (dx, dy, dz) in super::DIRS {
            if is(reg, px + dx, py + dy, pz + dz, gs) {
                j += 1;
            }
            if j > 1 {
                break;
            }
        }
        if j == 1 {
            reg.set(px, py, pz, gs);
        }
    }
    true
}

pub fn basalt_pillar(reg: &mut Region, r: &mut R, x: i32, y: i32, z: i32) -> bool {
    if !air(reg, x, y, z) || air(reg, x, y + 1, z) {
        return false;
    }
    let basalt = st("basalt");
    let mut yy = y;
    let (mut bn, mut bs, mut bw, mut be) = (true, true, true, true);
    let hang = |reg: &mut Region, r: &mut R, px: i32, py: i32, pz: i32| -> bool {
        if r.next_int_bounded(10) != 0 {
            reg.set(px, py, pz, basalt);
            true
        } else {
            false
        }
    };
    while air(reg, x, yy, z) {
        if yy < min_y(reg) || yy > max_y(reg) {
            return true;
        }
        reg.set(x, yy, z, basalt);
        bn = bn && hang(reg, r, x, yy, z - 1);
        bs = bs && hang(reg, r, x, yy, z + 1);
        bw = bw && hang(reg, r, x - 1, yy, z);
        be = be && hang(reg, r, x + 1, yy, z);
        yy -= 1;
    }
    yy += 1;
    for (dx, dz) in [(0, -1), (0, 1), (-1, 0), (1, 0)] {
        if r.next_bool() {
            reg.set(x + dx, yy, z + dz, basalt);
        }
    }
    yy -= 1;
    for i in -3..4 {
        for j in -3..4 {
            let k = i32::abs(i) * i32::abs(j);
            if r.next_int_bounded(10) >= 10 - k {
                continue;
            }
            let mut py = yy;
            let (px, pz) = (x + i, z + j);
            let mut l = 3;
            while air(reg, px, py - 1, pz) {
                py -= 1;
                l -= 1;
                if l <= 0 {
                    break;
                }
            }
            if !air(reg, px, py - 1, pz) {
                reg.set(px, py, pz, basalt);
            }
        }
    }
    true
}

// ---- delta ----

pub struct Delta {
    contents: u16,
    rim: u16,
    size: IntProvider,
    rim_size: IntProvider,
}

impl Delta {
    pub fn parse(c: &Value) -> Result<Delta> {
        Ok(Delta {
            contents: state_from_json(&c["contents"])?,
            rim: state_from_json(&c["rim"])?,
            size: IntProvider::parse(&c["size"])?,
            rim_size: IntProvider::parse(&c["rim_size"])?,
        })
    }

    fn clear(&self, reg: &Region, x: i32, y: i32, z: i32) -> bool {
        let b = reg.block(x, y, z);
        if BlockState(b as u32).name() == BlockState(self.contents as u32).name() {
            return false;
        }
        if matches!(
            BlockState(b as u32).name(),
            "bedrock"
                | "nether_bricks"
                | "nether_brick_fence"
                | "nether_brick_stairs"
                | "nether_wart"
                | "chest"
                | "spawner"
        ) {
            return false;
        }
        for (i, (dx, dy, dz)) in super::DIRS.iter().enumerate() {
            let a = air(reg, x + dx, y + dy, z + dz);
            let up = i == 1;
            if (a && !up) || (!a && up) {
                return false;
            }
        }
        true
    }

    pub fn place(&self, reg: &mut Region, r: &mut R, x: i32, y: i32, z: i32) -> bool {
        let mut any = false;
        let rim_on = r.next_double() < 0.9;
        let i = if rim_on { self.rim_size.sample(r) } else { 0 };
        let j = if rim_on { self.rim_size.sample(r) } else { 0 };
        let with_rim = rim_on && i != 0 && j != 0;
        let k = self.size.sample(r);
        let l = self.size.sample(r);
        let m = k.max(l);
        // within-Manhattan order is irrelevant: lava always wins over rim, neither creates air.
        let mut ps = Vec::new();
        for dz in -l..=l {
            for dx in -k..=k {
                if dx.abs() + dz.abs() <= m {
                    ps.push((dx.abs() + dz.abs(), dx, dz));
                }
            }
        }
        ps.sort();
        for (_, dx, dz) in ps {
            let (px, pz) = (x + dx, z + dz);
            if !self.clear(reg, px, y, pz) {
                continue;
            }
            if with_rim {
                any = true;
                reg.set(px, y, pz, self.rim);
            }
            let (qx, qz) = (px + i, pz + j);
            if self.clear(reg, qx, y, qz) {
                any = true;
                reg.set(qx, y, qz, self.contents);
            }
        }
        any
    }
}

// ---- basalt columns ----

pub struct BasaltColumns {
    height: IntProvider,
    reach: IntProvider,
}

const CANNOT_PLACE_ON: [&str; 10] = [
    "lava",
    "bedrock",
    "magma_block",
    "soul_sand",
    "nether_bricks",
    "nether_brick_fence",
    "nether_brick_stairs",
    "nether_wart",
    "chest",
    "spawner",
];

impl BasaltColumns {
    pub fn parse(c: &Value) -> Result<BasaltColumns> {
        Ok(BasaltColumns {
            height: IntProvider::parse(&c["height"])?,
            reach: IntProvider::parse(&c["reach"])?,
        })
    }

    fn air_or_lava_ocean(reg: &Region, sea: i32, x: i32, y: i32, z: i32) -> bool {
        air(reg, x, y, z) || (name_is(reg, x, y, z, "lava") && y <= sea)
    }

    fn can_place_at(reg: &Region, sea: i32, x: i32, y: i32, z: i32) -> bool {
        if !Self::air_or_lava_ocean(reg, sea, x, y, z) {
            return false;
        }
        let b = reg.block(x, y - 1, z);
        !flag(b, F_AIR) && !CANNOT_PLACE_ON.contains(&BlockState(b as u32).name())
    }

    fn find_surface(reg: &Region, sea: i32, x: i32, mut y: i32, z: i32, mut i: i32) -> Option<i32> {
        while y > min_y(reg) + 1 && i > 0 {
            i -= 1;
            if Self::can_place_at(reg, sea, x, y, z) {
                return Some(y);
            }
            y -= 1;
        }
        None
    }

    fn find_air(reg: &Region, x: i32, mut y: i32, z: i32, mut i: i32) -> Option<i32> {
        while y < max_y(reg) && i > 0 {
            i -= 1;
            let b = reg.block(x, y, z);
            if CANNOT_PLACE_ON.contains(&BlockState(b as u32).name()) {
                return None;
            }
            if flag(b, F_AIR) {
                return Some(y);
            }
            y += 1;
        }
        None
    }

    fn column(
        reg: &mut Region,
        sea: i32,
        px: i32,
        py: i32,
        pz: i32,
        height: i32,
        reach: i32,
    ) -> bool {
        let basalt = st("basalt");
        let mut any = false;
        for dz in -reach..=reach {
            for dy in 0..1 {
                for dx in -reach..=reach {
                    let (x, y, z) = (px + dx, py + dy, pz + dz);
                    let d = dx.abs() + dz.abs();
                    if d > reach {
                        continue;
                    }
                    let start = if Self::air_or_lava_ocean(reg, sea, x, y, z) {
                        Self::find_surface(reg, sea, x, y, z, d)
                    } else {
                        Self::find_air(reg, x, y, z, d)
                    };
                    let Some(sy) = start else { continue };
                    let mut j = height - d / 2;
                    let mut m = sy;
                    while j >= 0 {
                        if Self::air_or_lava_ocean(reg, sea, x, m, z) {
                            reg.set(x, m, z, basalt);
                            any = true;
                            m += 1;
                        } else if is(reg, x, m, z, basalt) {
                            m += 1;
                        } else {
                            break;
                        }
                        j -= 1;
                    }
                }
            }
        }
        any
    }

    pub fn place(&self, reg: &mut Region, r: &mut R, x: i32, y: i32, z: i32) -> bool {
        let sea = reg.g.sea_level;
        if !Self::can_place_at(reg, sea, x, y, z) {
            return false;
        }
        let j = self.height.sample(r);
        let big = r.next_float() < 0.9;
        let k = j.min(if big { 5 } else { 8 });
        let l = if big { 50 } else { 15 };
        let mut any = false;
        let w = 2 * k + 1;
        for _ in 0..l {
            let px = x - k + r.next_int_bounded(w);
            let _py = r.next_int_bounded(1);
            let pz = z - k + r.next_int_bounded(w);
            let m = j - ((px - x).abs() + (pz - z).abs());
            if m < 0 {
                continue;
            }
            let reach = self.reach.sample(r);
            any |= Self::column(reg, sea, px, y, pz, m, reach);
        }
        any
    }
}

// ---- netherrack_replace_blobs ----

pub struct ReplaceBlobs {
    state: u16,
    target: &'static str,
    radius: IntProvider,
}

impl ReplaceBlobs {
    pub fn parse(c: &Value) -> Result<ReplaceBlobs> {
        Ok(ReplaceBlobs {
            state: state_from_json(&c["state"])?,
            target: BlockState(state_from_json(&c["target"])? as u32).name(),
            radius: IntProvider::parse(&c["radius"])?,
        })
    }

    pub fn place(&self, reg: &mut Region, r: &mut R, x: i32, y: i32, z: i32) -> bool {
        let mut yy = y.clamp(min_y(reg) + 1, max_y(reg));
        let mut found = false;
        while yy > min_y(reg) + 1 {
            if name_is(reg, x, yy, z, self.target) {
                found = true;
                break;
            }
            yy -= 1;
        }
        if !found {
            return false;
        }
        let i = self.radius.sample(r);
        let j = self.radius.sample(r);
        let k = self.radius.sample(r);
        let l = i.max(j).max(k);
        let mut any = false;
        for dy in -j..=j {
            for dz in -k..=k {
                for dx in -i..=i {
                    if dx.abs() + dy.abs() + dz.abs() > l {
                        continue;
                    }
                    if name_is(reg, x + dx, yy + dy, z + dz, self.target) {
                        reg.set(x + dx, yy + dy, z + dz, self.state);
                        any = true;
                    }
                }
            }
        }
        any
    }
}

pub fn parse_vegetation(f: &Features, c: &Value) -> Result<Vegetation> {
    Ok(Vegetation {
        provider: StateProvider::parse(&f.tags, &c["state_provider"])?,
        width: c["spread_width"].as_i64().unwrap_or(8) as i32,
        height: c["spread_height"].as_i64().unwrap_or(4) as i32,
    })
}

#[cfg(test)]
mod tests {
    use crate::{Dimension, Generator};
    use basalt_data::BlockState;

    #[test]
    fn nether_features_run() {
        let g = Generator::new_dim(12345, Dimension::Nether);
        assert!(!g
            .unsupported_features()
            .keys()
            .any(|k| k.contains("huge_fungus") || k.contains("count_on_every_layer")));
        // A crimson forest chunk (measured against vanilla seed 12345): stems, hats and vines appear.
        let c = g.generate_chunk(-3, 1);
        let mut n = std::collections::HashMap::new();
        for sec in &c.blocks {
            for &b in sec.iter() {
                *n.entry(BlockState(b as u32).name()).or_insert(0usize) += 1;
            }
        }
        assert!(n.get("crimson_stem").copied().unwrap_or(0) > 20);
        assert!(n.get("nether_wart_block").copied().unwrap_or(0) > 100);
        assert!(n.get("weeping_vines_plant").copied().unwrap_or(0) > 0);
    }
}

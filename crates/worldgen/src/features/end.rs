//! End features: obsidian spikes, small floating islands, chorus plants, return gateways and the
//! arrival platform. Shapes and random draw orders are fitted to vanilla 26.2 End worlds
//! (`tools/parity.sh --end`, NOTES.md "The End").

use super::R;
use crate::random::{JavaRandom, Rng};
use crate::region::{flag, Region, F_AIR};
use basalt_data::BlockState;

fn st(name: &str) -> u16 {
    crate::state(name).unwrap_or(0)
}

fn air(reg: &Region, x: i32, y: i32, z: i32) -> bool {
    flag(reg.block(x, y, z), F_AIR)
}

/// One obsidian spike around the main island.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spike {
    pub x: i32,
    pub z: i32,
    pub radius: i32,
    pub height: i32,
    pub guarded: bool,
}

/// The ten spikes of a world seed: on a circle of radius 42, sizes shuffled by a
/// 16-bit key drawn from the seed.
pub fn spikes(seed: i64) -> Vec<Spike> {
    let key = JavaRandom::new(seed).next_long() & 0xffff;
    let mut order: Vec<i32> = (0..10).collect();
    let mut r = JavaRandom::new(key);
    for i in (2..=order.len()).rev() {
        let j = r.next_int_bounded(i as i32) as usize;
        order.swap(i - 1, j);
    }
    (0..10)
        .map(|i| {
            let a = 2.0 * (-std::f64::consts::PI + std::f64::consts::PI / 10.0 * i as f64);
            let k = order[i];
            Spike {
                x: (42.0 * a.cos()).floor() as i32,
                z: (42.0 * a.sin()).floor() as i32,
                radius: 2 + k / 3,
                height: 76 + k * 3,
                guarded: k == 1 || k == 2,
            }
        })
        .collect()
}

/// Place the spikes whose centre lies in chunk (cx, cz).
pub fn end_spikes(reg: &mut Region, cx: i32, cz: i32) -> bool {
    let mut any = false;
    for s in spikes(reg.g.seed) {
        if s.x >> 4 != cx || s.z >> 4 != cz {
            continue;
        }
        any = true;
        place_spike(reg, &s);
    }
    any
}

fn place_spike(reg: &mut Region, s: &Spike) {
    let obsidian = st("obsidian");
    let r = s.radius;
    let lo = reg.g.dim.min_y();
    for y in lo..=s.height + 10 {
        for x in s.x - r..=s.x + r {
            for z in s.z - r..=s.z + r {
                let d = (x - s.x) * (x - s.x) + (z - s.z) * (z - s.z);
                if d <= r * r + 1 && y < s.height {
                    reg.set(x, y, z, obsidian);
                } else if y > 65 {
                    reg.set(x, y, z, 0);
                }
            }
        }
    }
    if s.guarded {
        // A 5x5x4 iron-bar cage on top (hollow), bars joined along the cage walls.
        for dx in -2i32..=2 {
            for dz in -2i32..=2 {
                for dy in 0..=3 {
                    let ex = dx.abs() == 2;
                    let ez = dz.abs() == 2;
                    let top = dy == 3;
                    if !(ex || ez || top) {
                        continue;
                    }
                    let ns = ex || top;
                    let we = ez || top;
                    let name = format!(
                        "iron_bars[east={},north={},south={},waterlogged=false,west={}]",
                        we && dx != 2,
                        ns && dz != -2,
                        ns && dz != 2,
                        we && dx != -2
                    );
                    reg.set(s.x + dx, s.height + dy, s.z + dz, st(&name));
                }
            }
        }
    }
    // The end crystal stands on bedrock at the top; its fire is lit by the crystal entity in
    // vanilla (saved worlds show fire there), placed here directly.
    reg.set(s.x, s.height, s.z, st("bedrock"));
    reg.set(
        s.x,
        s.height + 1,
        s.z,
        st("fire[age=0,east=false,north=false,south=false,up=false,west=false]"),
    );
}

/// Small floating island: stacked end-stone discs shrinking downwards.
pub fn end_island(reg: &mut Region, r: &mut R, x: i32, y: i32, z: i32) -> bool {
    let stone = st("end_stone");
    let mut f = r.next_int_bounded(3) as f32 + 4.0;
    let mut dy = 0;
    while f > 0.5 {
        let lo = (-f).floor() as i32;
        let hi = f.ceil() as i32;
        for dx in lo..=hi {
            for dz in lo..=hi {
                if ((dx * dx + dz * dz) as f32) <= (f + 1.0) * (f + 1.0) {
                    reg.set(x + dx, y + dy, z + dz, stone);
                }
            }
        }
        f -= r.next_int_bounded(2) as f32 + 0.5;
        dy -= 1;
    }
    true
}

/// End gateway with its bedrock frame (exit fixed by the config; the block entity is not
/// generated here).
pub fn end_gateway(reg: &mut Region, x: i32, y: i32, z: i32) -> bool {
    let bedrock = st("bedrock");
    let gateway = st("end_gateway");
    for py in y - 2..=y + 2 {
        for px in x - 1..=x + 1 {
            for pz in z - 1..=z + 1 {
                let cxm = px == x;
                let cym = py == y;
                let czm = pz == z;
                let rim = (py - y).abs() == 2;
                let s = if cxm && cym && czm {
                    gateway
                } else if cym {
                    0
                } else if (rim && cxm && czm) || ((cxm || czm) && !rim) {
                    // bedrock caps above/below the centre, a bedrock cross on the middle layers
                    bedrock
                } else {
                    0
                };
                reg.set(px, py, pz, s);
            }
        }
    }
    true
}

/// Arrival platform: 5x5 obsidian under a 5x5x3 air pocket.
pub fn end_platform(reg: &mut Region, x: i32, y: i32, z: i32) -> bool {
    let obsidian = st("obsidian");
    for dx in -2..=2 {
        for dz in -2..=2 {
            for dy in -1..=2 {
                let s = if dy == -1 { obsidian } else { 0 };
                if reg.block(x + dx, y + dy, z + dz) != s {
                    reg.set(x + dx, y + dy, z + dz, s);
                }
            }
        }
    }
    true
}

// ---------------------------------------------------------------- chorus plants

fn is_chorus(s: u16) -> bool {
    matches!(
        BlockState(s as u32).name(),
        "chorus_plant" | "chorus_flower"
    )
}

/// Chorus plant state at (x, y, z) joined to its current neighbours.
fn plant_state(reg: &Region, x: i32, y: i32, z: i32) -> u16 {
    let below = reg.block(x, y - 1, z);
    let down = is_chorus(below) || BlockState(below as u32).name() == "end_stone";
    let c = |dx: i32, dy: i32, dz: i32| is_chorus(reg.block(x + dx, y + dy, z + dz));
    st(&format!(
        "chorus_plant[down={},east={},north={},south={},up={},west={}]",
        down,
        c(1, 0, 0),
        c(0, 0, -1),
        c(0, 0, 1),
        c(0, 1, 0),
        c(-1, 0, 0)
    ))
}

/// Horizontal directions in the game's order (north, east, south, west) as (dx, dz).
const HORIZ: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];

fn neighbours_empty(reg: &Region, x: i32, y: i32, z: i32, except: Option<(i32, i32)>) -> bool {
    HORIZ
        .iter()
        .filter(|d| Some(**d) != except)
        .all(|(dx, dz)| air(reg, x + dx, y, z + dz))
}

pub fn chorus_plant(reg: &mut Region, r: &mut R, x: i32, y: i32, z: i32) -> bool {
    if !air(reg, x, y, z) || BlockState(reg.block(x, y - 1, z) as u32).name() != "end_stone" {
        return false;
    }
    let s = plant_state(reg, x, y, z);
    reg.set(x, y, z, s);
    grow_branch(reg, r, (x, y, z), (x, z), 8, 0);
    true
}

fn grow_branch(
    reg: &mut Region,
    r: &mut R,
    base: (i32, i32, i32),
    start: (i32, i32),
    reach: i32,
    depth: i32,
) {
    let (bx, by, bz) = base;
    let mut len = r.next_int_bounded(4) + 1;
    if depth == 0 {
        len += 1;
    }
    for j in 0..len {
        let y = by + j + 1;
        if !neighbours_empty(reg, bx, y, bz, None) {
            return;
        }
        let s = plant_state(reg, bx, y, bz);
        reg.set(bx, y, bz, s);
        let s = plant_state(reg, bx, y - 1, bz);
        reg.set(bx, y - 1, bz, s);
    }
    let mut branched = false;
    if depth < 4 {
        let mut n = r.next_int_bounded(4);
        if depth == 0 {
            n += 1;
        }
        for _ in 0..n {
            let d = HORIZ[r.next_int_bounded(4) as usize];
            let (qx, qy, qz) = (bx + d.0, by + len, bz + d.1);
            if (qx - start.0).abs() < reach
                && (qz - start.1).abs() < reach
                && air(reg, qx, qy, qz)
                && air(reg, qx, qy - 1, qz)
                && neighbours_empty(reg, qx, qy, qz, Some((-d.0, -d.1)))
            {
                branched = true;
                let s = plant_state(reg, qx, qy, qz);
                reg.set(qx, qy, qz, s);
                let (ox, oz) = (qx - d.0, qz - d.1);
                let s = plant_state(reg, ox, qy, oz);
                reg.set(ox, qy, oz, s);
                grow_branch(reg, r, (qx, qy, qz), start, reach, depth + 1);
            }
        }
    }
    if !branched {
        reg.set(bx, by + len, bz, st("chorus_flower[age=5]"));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn spikes_on_circle() {
        let s = super::spikes(12345);
        assert_eq!(s.len(), 10);
        let mut h: Vec<i32> = s.iter().map(|s| s.height).collect();
        h.sort();
        assert_eq!(h, (0..10).map(|k| 76 + 3 * k).collect::<Vec<_>>());
        assert_eq!(s.iter().filter(|s| s.guarded).count(), 2);
    }
}

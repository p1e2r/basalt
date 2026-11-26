//! Buckets (use_item): an empty bucket picks up the first fluid source (or waterlogged block's water)
//! along the look ray, a water / lava bucket empties against the first block the ray hits.
//! Powder snow buckets place like a block (use_item_on, game.rs) and pick up via the ray here.
//!
//! The ray: from the eye (1.62 standing / 1.27 sneaking) along yaw/pitch from the use_item packet,
//! 4.5 blocks (block_interaction_range). Every non-air, non-fluid block counts as a full cube
//! (vanilla uses outline shapes: thin plants are hit slightly differently). There is no fluid
//! flow yet (M6): placed water stays a single source.
//! Rules checked against vanilla 26.2 with scenario buckets (2026-11-26).
use super::*;
use basalt_data::BlockState;

fn fluid_of(s: BlockState) -> Option<(&'static str, bool)> {
    match s.name() {
        "water" | "lava" => Some((s.name(), s.get("level") == Some("0"))),
        _ => None,
    }
}

pub(super) struct RayHit {
    /// the block (or fluid source) hit
    pub b: [i32; 3],
    /// face entered through (index into FACE)
    pub face: usize,
}

impl Game {
    /// DDA through the voxel grid. `sources`: stop at fluid sources (empty bucket); otherwise fluids
    /// are passed through.
    pub(super) fn ray_block(
        &self,
        eye: [f64; 3],
        dir: [f64; 3],
        reach: f64,
        sources: bool,
    ) -> Option<RayHit> {
        let mut c = [
            eye[0].floor() as i32,
            eye[1].floor() as i32,
            eye[2].floor() as i32,
        ];
        let step: [i32; 3] = std::array::from_fn(|i| if dir[i] > 0.0 { 1 } else { -1 });
        let mut tmax = [0f64; 3];
        let mut tdelta = [0f64; 3];
        for i in 0..3 {
            if dir[i].abs() < 1e-12 {
                tmax[i] = f64::INFINITY;
                tdelta[i] = f64::INFINITY;
            } else {
                let next = if dir[i] > 0.0 {
                    c[i] as f64 + 1.0
                } else {
                    c[i] as f64
                };
                tmax[i] = (next - eye[i]) / dir[i];
                tdelta[i] = 1.0 / dir[i].abs();
            }
        }
        let mut boxes = Vec::new();
        for _ in 0..64 {
            let s = BlockState(self.world.get_block(c[0], c[1], c[2]));
            if !s.is_air() {
                self.ray_boxes(s, c, sources, &mut boxes);
                let mut best: Option<(f64, usize)> = None;
                for bx in &boxes {
                    let lo = [
                        c[0] as f64 + bx[0],
                        c[1] as f64 + bx[1],
                        c[2] as f64 + bx[2],
                    ];
                    let hi = [
                        c[0] as f64 + bx[3],
                        c[1] as f64 + bx[4],
                        c[2] as f64 + bx[5],
                    ];
                    if let Some(h) = ray_box(eye, dir, lo, hi) {
                        if h.0 <= reach && best.is_none_or(|b| h.0 < b.0) {
                            best = Some(h);
                        }
                    }
                }
                if let Some((_, face)) = best {
                    return Some(RayHit { b: c, face });
                }
            }
            let ax = if tmax[0] < tmax[1] && tmax[0] < tmax[2] {
                0
            } else if tmax[1] < tmax[2] {
                1
            } else {
                2
            };
            if tmax[ax] > reach {
                return None;
            }
            c[ax] += step[ax];
            tmax[ax] += tdelta[ax];
        }
        None
    }

    /// Boxes the look ray can hit in a cell: outline shapes approximated by the collision boxes,
    /// snow layers by their height, other non-colliding blocks (plants, torches...) by a
    /// 2/16-inset box 13/16 high; fluid sources (when `sources`) 8/9 high (full under fluid).
    fn ray_boxes(&self, s: BlockState, c: [i32; 3], sources: bool, out: &mut Vec<[f64; 6]>) {
        out.clear();
        if let Some((f, src)) = fluid_of(s) {
            if sources && src {
                let above = BlockState(self.world.get_block(c[0], c[1] + 1, c[2]));
                let full = fluid_of(above).is_some_and(|(g, _)| g == f);
                out.push([0.0, 0.0, 0.0, 1.0, if full { 1.0 } else { 8.0 / 9.0 }, 1.0]);
            }
            return;
        }
        if s.name() == "bubble_column" {
            return;
        }
        if sources && s.get("waterlogged") == Some("true") {
            // the contained water is hit before the block (fill_waterlogged)
            out.push([0.0, 0.0, 0.0, 1.0, 8.0 / 9.0, 1.0]);
            return;
        }
        if s.name() == "snow" {
            let l: f64 = s.get("layers").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            out.push([0.0, 0.0, 0.0, 1.0, l / 8.0, 1.0]);
            return;
        }
        super::mob::block_boxes(s.0, out);
        for b in out.iter_mut() {
            b[4] = b[4].min(1.0);
        }
        if out.is_empty() {
            out.push([0.125, 0.0, 0.125, 0.875, 13.0 / 16.0, 0.875]);
        }
    }

    /// Blocks an emptied bucket replaces in place (a snow layer is not one: the fluid goes on top,
    /// measured empty_water_snow_surface).
    fn fluid_replaces(&self, s: BlockState) -> bool {
        self.replaceable_for(s) && s.name() != "snow"
    }

    /// use_item with a bucket. True when handled (the item is a bucket).
    pub(super) fn try_bucket(&mut self, eid: i32, hand: i32, yaw: f32, pitch: f32) -> bool {
        let Some(p) = self.players.get_mut(&eid) else {
            return false;
        };
        let slot = if hand == 0 { 36 + p.held } else { 45 };
        let st = p.inv[slot];
        if st.count <= 0 {
            return false;
        }
        let name = reg::ITEM.get(st.item as usize).copied().unwrap_or("air");
        if !matches!(name, "bucket" | "water_bucket" | "lava_bucket") {
            return false;
        }
        if p.dead || p.gamemode == 3 {
            return true;
        }
        p.yaw = yaw;
        p.pitch = pitch;
        let eye = [
            p.pos[0],
            p.pos[1] + if p.sneaking { 1.27 } else { 1.62 },
            p.pos[2],
        ];
        let (yr, pr) = ((yaw as f64).to_radians(), (pitch as f64).to_radians());
        let dir = [-yr.sin() * pr.cos(), -pr.sin(), yr.cos() * pr.cos()];
        let (gm, adventure) = (p.gamemode, p.gamemode == 2);
        if adventure {
            return true;
        }
        let Some(hit) = self.ray_block(eye, dir, 4.5, name == "bucket") else {
            return true;
        };
        let b = hit.b;
        let s = BlockState(self.world.get_block(b[0], b[1], b[2]));
        let center = [b[0] as f64 + 0.5, b[1] as f64 + 0.5, b[2] as f64 + 0.5];
        if name == "bucket" {
            let filled = match fluid_of(s) {
                Some(("water", true)) => {
                    self.set_block(b[0], b[1], b[2], 0);
                    "water_bucket"
                }
                Some(("lava", true)) => {
                    self.set_block(b[0], b[1], b[2], 0);
                    "lava_bucket"
                }
                _ if s.get("waterlogged") == Some("true") => {
                    let ns = s.with("waterlogged", "false").map(|s| s.0).unwrap_or(s.0);
                    self.set_block(b[0], b[1], b[2], ns);
                    "water_bucket"
                }
                _ if s.name() == "powder_snow" => {
                    self.set_block(b[0], b[1], b[2], 0);
                    // break particles for everyone, the picking player included (measured)
                    self.level_event_data(2001, b, s.0 as i32, None);
                    "powder_snow_bucket"
                }
                _ => return true,
            };
            self.update_neighbors(b, 0);
            let sound = match filled {
                "lava_bucket" => "item.bucket.fill_lava",
                "powder_snow_bucket" => "item.bucket.fill_powder_snow",
                _ => "item.bucket.fill",
            };
            self.play_sound_except(sound, 4, center, 1.0, 1.0, Some(eid));
            self.fill_held(eid, slot, filled, gm == 1);
            return true;
        }
        // water / lava bucket: into a waterloggable block, a replaceable block, or the cell in front
        let fluid = if name == "water_bucket" {
            "water"
        } else {
            "lava"
        };
        let target = if fluid == "water" && s.get("waterlogged") == Some("false") {
            Some((b, true))
        } else if self.fluid_replaces(s) {
            Some((b, false))
        } else {
            let t = [
                b[0] + FACE[hit.face][0],
                b[1] + FACE[hit.face][1],
                b[2] + FACE[hit.face][2],
            ];
            let ts = BlockState(self.world.get_block(t[0], t[1], t[2]));
            if fluid == "water" && ts.get("waterlogged") == Some("false") {
                Some((t, true))
            } else if ts.is_air() || self.fluid_replaces(ts) || fluid_of(ts).is_some() {
                Some((t, false))
            } else {
                None
            }
        };
        let Some((t, log)) = target else {
            return true;
        };
        if !self.world.layout.contains_y(t[1]) {
            return true;
        }
        let tc = [t[0] as f64 + 0.5, t[1] as f64 + 0.5, t[2] as f64 + 0.5];
        if fluid == "water" && self.cur == dim::NETHER {
            // evaporates: hiss + smoke for everyone, the bucket is still emptied
            self.play_sound("block.fire.extinguish", 4, tc, 0.5, 2.6);
        } else if log {
            let ts = BlockState(self.world.get_block(t[0], t[1], t[2]));
            let ns = ts.with("waterlogged", "true").map(|s| s.0).unwrap_or(ts.0);
            self.set_block(t[0], t[1], t[2], ns);
        } else {
            let ts = BlockState(self.world.get_block(t[0], t[1], t[2]));
            if !ts.is_air() && fluid_of(ts).is_none() {
                // a replaceable plant is destroyed with its drops
                let drops = self.tool_less_drops(ts, t);
                self.set_block(t[0], t[1], t[2], 0);
                self.drop_at_block(t, drops);
            }
            let src = BlockState::from_name(fluid).map(|s| s.0).unwrap_or(0);
            self.set_block(t[0], t[1], t[2], src);
            self.update_neighbors(t, 0);
            let sound = if fluid == "lava" {
                "item.bucket.empty_lava"
            } else {
                "item.bucket.empty"
            };
            self.play_sound_except(sound, 4, tc, 1.0, 1.0, Some(eid));
        }
        if gm != 1 {
            if let Some(id) = basalt_data::registry_id(reg::ITEM, "bucket") {
                let p = self.players.get_mut(&eid).unwrap();
                p.inv[slot] = Stack {
                    item: id,
                    count: 1,
                    ..Stack::default()
                };
                Self::send_window(p);
            }
        }
        true
    }

    /// An empty bucket in `slot` became `filled`: survival swaps a single bucket, or takes one off
    /// the stack and adds the filled one to the inventory (dropped when full); creative keeps the
    /// bucket and adds the filled one only if the inventory has none.
    pub(super) fn fill_held(&mut self, eid: i32, slot: usize, filled: &str, creative: bool) {
        let Some(id) = basalt_data::registry_id(reg::ITEM, filled) else {
            return;
        };
        let one = Stack {
            item: id,
            count: 1,
            ..Stack::default()
        };
        let Some(p) = self.players.get_mut(&eid) else {
            return;
        };
        if creative {
            if !p.inv.iter().any(|s| s.count > 0 && s.item == id) {
                self.give_stack(eid, one);
            }
            return;
        }
        if p.inv[slot].count <= 1 {
            p.inv[slot] = one;
            Self::send_window(p);
            return;
        }
        p.inv[slot].count -= 1;
        Self::send_window(p);
        if self.give_stack(eid, one) > 0 {
            if let Some(p) = self.players.get(&eid) {
                let pos = [p.pos[0], p.pos[1] + 1.3, p.pos[2]];
                self.spawn_item(pos, [0.0, 0.1, 0.0], one, 40);
            }
        }
    }
}

/// Slab-method ray / box intersection: (entry distance, entry face) when the ray starts outside.
fn ray_box(o: [f64; 3], d: [f64; 3], lo: [f64; 3], hi: [f64; 3]) -> Option<(f64, usize)> {
    let (mut tmin, mut tmax) = (0.0f64, f64::INFINITY);
    let mut face = 1usize;
    for i in 0..3 {
        if d[i].abs() < 1e-12 {
            if o[i] < lo[i] || o[i] > hi[i] {
                return None;
            }
            continue;
        }
        let (a, b) = ((lo[i] - o[i]) / d[i], (hi[i] - o[i]) / d[i]);
        let (near, far) = if a < b { (a, b) } else { (b, a) };
        if near > tmin {
            tmin = near;
            // entered through the min side when moving +, the max side when moving -
            face = match (i, d[i] > 0.0) {
                (0, true) => 4,
                (0, false) => 5,
                (1, true) => 0,
                (1, false) => 1,
                (2, true) => 2,
                _ => 3,
            };
        }
        tmax = tmax.min(far);
        if tmin > tmax {
            return None;
        }
    }
    Some((tmin, face))
}

#[cfg(test)]
mod tests {
    use super::ray_box;

    #[test]
    fn ray_box_faces() {
        let h = ray_box([0.5, 3.0, 0.5], [0.0, -1.0, 0.0], [0.0; 3], [1.0, 0.5, 1.0]).unwrap();
        assert!((h.0 - 2.5).abs() < 1e-9 && h.1 == 1);
        let h = ray_box(
            [0.5, 0.25, -2.0],
            [0.0, 0.0, 1.0],
            [0.0; 3],
            [1.0, 0.5, 1.0],
        )
        .unwrap();
        assert!((h.0 - 2.0).abs() < 1e-9 && h.1 == 2);
        assert!(ray_box(
            [0.5, 0.75, -2.0],
            [0.0, 0.0, 1.0],
            [0.0; 3],
            [1.0, 0.5, 1.0]
        )
        .is_none());
    }
}

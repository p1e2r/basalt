//! A* pathfinding on the block grid for ground mobs.
//!
//! A node is a block position where a mob's feet can be: the block below is solid (or the node is
//! in water, where mobs swim), and `height` blocks starting at the node are passable. Moves: the 8
//! horizontal neighbours (diagonals only when both orthogonal neighbours are open), one block up
//! (a jump, needing head room above the current node) and drops of up to `max_drop` blocks.
//! Dangerous blocks (lava, fire, cactus, magma, …) are never entered or stood on.
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cell { Open, Solid, Water, Danger }

pub trait Grid { fn cell(&self, x: i32, y: i32, z: i32) -> Cell; }

pub struct Params { pub height: i32, pub max_drop: i32, pub max_nodes: usize, pub can_swim: bool }

impl Default for Params { fn default() -> Self { Params { height: 2, max_drop: 3, max_nodes: 600, can_swim: true } } }

#[derive(PartialEq)]
struct Open { f: f32, g: f32, p: [i32; 3] }
impl Eq for Open {}
impl Ord for Open { fn cmp(&self, o: &Self) -> Ordering { o.f.partial_cmp(&self.f).unwrap_or(Ordering::Equal).then_with(|| self.g.partial_cmp(&o.g).unwrap_or(Ordering::Equal)) } }
impl PartialOrd for Open { fn partial_cmp(&self, o: &Self) -> Option<Ordering> { Some(self.cmp(o)) } }

fn passable(c: Cell, swim: bool) -> bool { c == Cell::Open || (swim && c == Cell::Water) }

/// Can a mob stand with its feet at `p`?
pub fn standable(g: &impl Grid, p: [i32; 3], pr: &Params) -> bool {
    let feet = g.cell(p[0], p[1], p[2]);
    if !passable(feet, pr.can_swim) { return false; }
    for dy in 1..pr.height { if !passable(g.cell(p[0], p[1] + dy, p[2]), pr.can_swim) { return false; } }
    let below = g.cell(p[0], p[1] - 1, p[2]);
    below == Cell::Solid || (feet == Cell::Water && pr.can_swim)
}

/// Where a mob at `p` (feet possibly mid-air) would land: drop up to max_drop blocks.
fn land(g: &impl Grid, p: [i32; 3], pr: &Params) -> Option<[i32; 3]> {
    let mut q = p;
    for _ in 0..=pr.max_drop {
        if standable(g, q, pr) { return Some(q); }
        if !passable(g.cell(q[0], q[1] - 1, q[2]), pr.can_swim) { return None; }
        q[1] -= 1;
    }
    None
}

fn h(a: [i32; 3], b: [i32; 3]) -> f32 {
    let d = [(a[0] - b[0]) as f32, (a[1] - b[1]) as f32, (a[2] - b[2]) as f32];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// Neighbouring nodes reachable from `p` with their move cost.
fn neighbours(g: &impl Grid, p: [i32; 3], pr: &Params, out: &mut Vec<([i32; 3], f32)>) {
    out.clear();
    let open_column = |x: i32, y: i32, z: i32, n: i32| (0..n).all(|dy| passable(g.cell(x, y + dy, z), pr.can_swim));
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
        let diag = dx != 0 && dz != 0;
        if diag && !(open_column(p[0] + dx, p[1], p[2], pr.height) && open_column(p[0], p[1], p[2] + dz, pr.height)) { continue; }
        let q = [p[0] + dx, p[1], p[2] + dz];
        let base = if diag { std::f32::consts::SQRT_2 } else { 1.0 };
        if open_column(q[0], q[1], q[2], pr.height) {
            if let Some(l) = land(g, q, pr) {
                let water = g.cell(l[0], l[1], l[2]) == Cell::Water;
                out.push((l, base + (p[1] - l[1]) as f32 * 0.5 + if water { 2.0 } else { 0.0 }));
            }
        } else if !diag {
            // jump up one block: need head room above us and an open column above the obstacle
            let up = [q[0], q[1] + 1, q[2]];
            if passable(g.cell(p[0], p[1] + pr.height, p[2]), pr.can_swim) && standable(g, up, pr) { out.push((up, base + 1.0)); }
        }
    }
}

/// Find a path from `start` to within `reach` blocks of `goal`. The returned list excludes the start.
pub fn find(g: &impl Grid, start: [i32; 3], goal: [i32; 3], reach: f32, pr: &Params) -> Option<Vec<[i32; 3]>> {
    let mut open = BinaryHeap::new();
    let mut came: HashMap<[i32; 3], [i32; 3]> = HashMap::new();
    let mut cost: HashMap<[i32; 3], f32> = HashMap::new();
    cost.insert(start, 0.0);
    open.push(Open { f: h(start, goal), g: 0.0, p: start });
    let mut best = (h(start, goal), start);
    let mut nb = Vec::new();
    let mut expanded = 0;
    while let Some(Open { g: gc, p, .. }) = open.pop() {
        if gc > *cost.get(&p).unwrap_or(&f32::MAX) + 1e-4 { continue; }
        let d = h(p, goal);
        if d < best.0 { best = (d, p); }
        if d <= reach { return Some(rebuild(&came, start, p)); }
        expanded += 1;
        if expanded > pr.max_nodes { break; }
        neighbours(g, p, pr, &mut nb);
        for &(q, c) in &nb {
            if g.cell(q[0], q[1] - 1, q[2]) == Cell::Danger { continue; }
            let ng = gc + c;
            if ng < *cost.get(&q).unwrap_or(&f32::MAX) {
                cost.insert(q, ng); came.insert(q, p);
                open.push(Open { f: ng + h(q, goal), g: ng, p: q });
            }
        }
    }
    // goal unreachable within budget: go to the closest point found, if it is progress
    if best.1 != start && best.0 + 1.0 < h(start, goal) { Some(rebuild(&came, start, best.1)) } else { None }
}

fn rebuild(came: &HashMap<[i32; 3], [i32; 3]>, start: [i32; 3], mut p: [i32; 3]) -> Vec<[i32; 3]> {
    let mut v = vec![p];
    while let Some(&q) = came.get(&p) { if q == start { break; } v.push(q); p = q; }
    v.reverse();
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Flat floor at y=0 (solid at y<=0) plus overrides.
    struct Map(HashMap<[i32; 3], Cell>);
    impl Grid for Map {
        fn cell(&self, x: i32, y: i32, z: i32) -> Cell { *self.0.get(&[x, y, z]).unwrap_or(&if y <= 0 { Cell::Solid } else { Cell::Open }) }
    }
    fn map() -> Map { Map(HashMap::new()) }

    #[test]
    fn straight_line() {
        let m = map();
        let p = find(&m, [0, 1, 0], [5, 1, 0], 0.5, &Params::default()).unwrap();
        assert_eq!(p.last(), Some(&[5, 1, 0]));
        assert_eq!(p.len(), 5);
    }

    #[test]
    fn around_a_wall() {
        let mut m = map();
        for z in -3..=3 { for y in 1..=3 { m.0.insert([2, y, z], Cell::Solid); } }
        let p = find(&m, [0, 1, 0], [4, 1, 0], 0.5, &Params::default()).unwrap();
        assert_eq!(p.last(), Some(&[4, 1, 0]));
        assert!(p.iter().all(|q| q[0] != 2 || q[2].abs() > 3), "{p:?}");
        // diagonal moves never cut wall corners
        for w in p.windows(2) { let (a, b) = (w[0], w[1]); if a[0] != b[0] && a[2] != b[2] { assert!(m.cell(b[0], 1, a[2]) != Cell::Solid && m.cell(a[0], 1, b[2]) != Cell::Solid); } }
    }

    #[test]
    fn steps_up_one_block_not_two() {
        let mut m = map();
        // a one-high step at x=2 for all z: walkable by jumping
        for z in -20..=20 { m.0.insert([2, 1, z], Cell::Solid); }
        let p = find(&m, [0, 1, 0], [4, 2, 0], 0.5, &Params::default()).unwrap();
        assert!(p.contains(&[2, 2, 0]) || p.iter().any(|q| q[0] == 2 && q[1] == 2));
        // a two-high wall is not climbable
        let mut m = map();
        for z in -20..=20 { m.0.insert([2, 1, z], Cell::Solid); m.0.insert([2, 2, z], Cell::Solid); m.0.insert([2, 3, z], Cell::Solid); }
        let p = find(&m, [0, 1, 0], [4, 1, 0], 0.5, &Params { max_nodes: 300, ..Params::default() });
        assert!(p.map(|v| v.last() != Some(&[4, 1, 0])).unwrap_or(true));
    }

    #[test]
    fn avoids_cliffs_deeper_than_max_drop() {
        let mut m = map();
        // a pit 5 deep between x=1..=3; the only way to x=4 is through it
        for x in 1..=3 { for z in -1000..=1000 { for y in -4..=0 { m.0.insert([x, y, z], Cell::Open); } } }
        assert!(find(&m, [0, 1, 0], [4, 1, 0], 0.5, &Params { max_nodes: 2000, ..Params::default() }).map(|p| p.last() != Some(&[4, 1, 0])).unwrap_or(true));
        // 3 deep is fine to drop into (but can't climb out), 2-deep pit with steps is crossable
        let mut m = map();
        for z in -30..=30 { m.0.insert([1, 0, z], Cell::Open); }
        let p = find(&m, [0, 1, 0], [3, 1, 0], 0.5, &Params::default()).unwrap();
        assert_eq!(p.last(), Some(&[3, 1, 0]));
    }

    #[test]
    fn avoids_lava() {
        let mut m = map();
        // lava floor on x=2 except a gap at z=5
        for z in -10..=10 { if z != 5 { m.0.insert([2, 0, z], Cell::Danger); } }
        let p = find(&m, [0, 1, 0], [4, 1, 0], 0.5, &Params::default()).unwrap();
        assert_eq!(p.last(), Some(&[4, 1, 0]));
        assert!(p.iter().all(|q| q[0] != 2 || q[2] == 5), "{p:?}");
    }

    #[test]
    fn unreachable_gives_partial_progress() {
        let mut m = map();
        // goal enclosed in a box
        for x in 9..=11 { for z in -1..=1 { for y in 1..=4 { if (x, z) != (10, 0) { m.0.insert([x, y, z], Cell::Solid); } } } }
        let p = find(&m, [0, 1, 0], [10, 1, 0], 0.5, &Params { max_nodes: 400, ..Params::default() }).unwrap();
        let end = *p.last().unwrap();
        assert!(end[0] >= 7, "{end:?}");
    }
}

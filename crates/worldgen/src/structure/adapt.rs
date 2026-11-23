//! Structure terrain adaptation (`terrain_adaptation` in worldgen/structure/*.json): a density
//! contribution added to the noise-stage final density around the pieces of nearby structure
//! starts. The shape was fitted empirically (NOTES.md "Terrain adaptation"): vanilla worlds
//! generated with and without structures by the official server jar, compared with Basalt's own
//! pre-adaptation density per block (`structures adapt` in examples/structures.rs).

use super::template::BBox;
use super::{Adaptation, PieceKind, Start};
use crate::Generator;

/// One piece box contributing to a chunk.
#[derive(Clone, Debug)]
pub struct BeardPiece {
    pub bb: BBox,
    pub kind: Adaptation,
    /// The piece's ground level offset (jigsaw pieces: ground_level_delta), 0 otherwise.
    pub ground_delta: i32,
    /// Rigid jigsaw pieces adapt terrain; terrain-matching ones (paths) only via junctions.
    pub rigid: bool,
}

/// A jigsaw junction (connection of a rigid piece to a terrain-matching one).
#[derive(Clone, Copy, Debug)]
pub struct BeardJunction {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub kind: Adaptation,
}

/// Terrain adaptation of one chunk: every piece/junction within reach.
#[derive(Clone, Debug, Default)]
pub struct Beard {
    pub pieces: Vec<BeardPiece>,
    pub junctions: Vec<BeardJunction>,
}

/// Horizontal reach of the contribution outside a piece box / around a junction (blocks).
pub const REACH: i32 = 12;

// Fitted constants (seed 12345; plains, savanna and desert villages + a pillager outpost for the
// thin beard, the ancient city at chunk 11,-11 for the box beard; 2026-11-24, `structures adapt`
// + logistic / error-count fits of "vanilla solid <=> Basalt density + contribution > 0").
/// beard_thin, rigid piece: amplitude and Gaussian width (r^2 scale).
const THIN_AMP: f64 = 0.53;
const THIN_SIG2: f64 = 16.4;
/// Every junction entry of every piece (both ends of a connection are listed).
const JUNCTION_AMP: f64 = 0.26;
const JUNCTION_SIG2: f64 = 16.0;
/// beard_box, at or above the piece floor.
const BOX_AMP: f64 = 0.36;
const BOX_SIG2: f64 = 16.0;

/// Odd kernel of the thin beard: fills below `ref_y` and removes above it, `dyc` = y + 0.5 - ref.
fn thin(h2: f64, dyc: f64, amp: f64, sig2: f64) -> f64 {
    let r2 = h2 + dyc * dyc;
    let r = r2.sqrt().max(1e-6);
    -amp * dyc / r * (-r2 / sig2).exp()
}

/// One contributing item as seen from a block column.
#[derive(Clone, Copy, Debug)]
enum ColItem {
    /// Thin kernel around a reference y (piece floor or junction).
    Thin {
        h2: f64,
        ref_y: i32,
        amp: f64,
        sig2: f64,
    },
    /// Box kernel: piece floor and top.
    Box { h2: f64, min_y: i32, max_y: i32 },
}

/// The items within reach of one block column.
#[derive(Clone, Debug, Default)]
pub struct Column {
    items: Vec<ColItem>,
    pub min_y: i32,
    pub max_y: i32,
}

impl Column {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn at(&self, y: i32) -> f64 {
        if y < self.min_y || y > self.max_y {
            return 0.0;
        }
        let mut d = 0.0;
        for it in &self.items {
            match *it {
                ColItem::Thin {
                    h2,
                    ref_y,
                    amp,
                    sig2,
                } => {
                    let dyc = (y - ref_y) as f64 + 0.5;
                    if dyc.abs() <= REACH as f64 {
                        d += thin(h2, dyc, amp, sig2);
                    }
                }
                ColItem::Box { h2, min_y, max_y } => {
                    if y >= min_y && y <= max_y + REACH {
                        let dy = (y - max_y).max(0) as f64;
                        d -= BOX_AMP * (-(h2 + dy * dy) / BOX_SIG2).exp();
                    }
                }
            }
        }
        d
    }
}

fn outside(v: i32, lo: i32, hi: i32) -> i32 {
    (lo - v).max(v - hi).max(0)
}

impl Beard {
    pub fn is_empty(&self) -> bool {
        self.pieces.is_empty() && self.junctions.is_empty()
    }

    /// The items reaching block column (x, z).
    pub fn column(&self, x: i32, z: i32) -> Column {
        let mut c = Column {
            items: Vec::new(),
            min_y: i32::MAX,
            max_y: i32::MIN,
        };
        for p in &self.pieces {
            let dx = outside(x, p.bb.min[0], p.bb.max[0]);
            let dz = outside(z, p.bb.min[2], p.bb.max[2]);
            if dx > REACH || dz > REACH {
                continue;
            }
            let h2 = (dx * dx + dz * dz) as f64;
            match p.kind {
                Adaptation::BeardThin => {
                    let ref_y = p.bb.min[1] + p.ground_delta;
                    c.items.push(ColItem::Thin {
                        h2,
                        ref_y,
                        amp: THIN_AMP,
                        sig2: THIN_SIG2,
                    });
                    c.min_y = c.min_y.min(ref_y - REACH);
                    c.max_y = c.max_y.max(ref_y + REACH);
                }
                Adaptation::BeardBox => {
                    c.items.push(ColItem::Box {
                        h2,
                        min_y: p.bb.min[1],
                        max_y: p.bb.max[1],
                    });
                    c.min_y = c.min_y.min(p.bb.min[1]);
                    c.max_y = c.max_y.max(p.bb.max[1] + REACH);
                }
                _ => {}
            }
        }
        for j in &self.junctions {
            let (dx, dz) = (x - j.x, z - j.z);
            if dx.abs() > REACH || dz.abs() > REACH || j.kind != Adaptation::BeardThin {
                continue;
            }
            let h2 = (dx * dx + dz * dz) as f64;
            c.items.push(ColItem::Thin {
                h2,
                ref_y: j.y,
                amp: JUNCTION_AMP,
                sig2: JUNCTION_SIG2,
            });
            c.min_y = c.min_y.min(j.y - REACH);
            c.max_y = c.max_y.max(j.y + REACH);
        }
        c
    }

    /// Density contribution at block (x, y, z).
    pub fn at(&self, x: i32, y: i32, z: i32) -> f64 {
        self.column(x, z).at(y)
    }
}

impl Generator {
    /// Terrain adaptation contributions reaching chunk (cx, cz), None if there are none.
    pub fn beard(&self, cx: i32, cz: i32) -> Option<Beard> {
        let s = self.structures.as_ref().filter(|s| s.beard)?;
        let clip = BBox {
            min: [cx * 16 - REACH, -64, cz * 16 - REACH],
            max: [cx * 16 + 15 + REACH, 319, cz * 16 + 15 + REACH],
        };
        let mut b = Beard::default();
        for v in self.structure_refs_reach(cx, cz, REACH) {
            for st in v.iter() {
                let kind = s.defs[st.def].adaptation;
                if kind == Adaptation::None || st.pieces.is_empty() {
                    continue;
                }
                collect(st, kind, &clip, &mut b);
            }
        }
        (!b.is_empty()).then_some(b)
    }
}

fn collect(st: &Start, kind: Adaptation, clip: &BBox, b: &mut Beard) {
    for p in &st.pieces {
        let (rigid, gd, junctions) = match &p.kind {
            PieceKind::Pool {
                element,
                ground_delta,
                junctions,
                ..
            } => (element.rigid(), *ground_delta, junctions.as_slice()),
            _ => (true, 0, &[][..]),
        };
        if rigid && p.bb.intersects(clip) {
            b.pieces.push(BeardPiece {
                bb: p.bb,
                kind,
                ground_delta: gd,
                rigid,
            });
        }
        for j in junctions {
            if clip.contains([j.x, j.y.clamp(clip.min[1], clip.max[1]), j.z]) {
                b.junctions.push(BeardJunction {
                    x: j.x,
                    y: j.y,
                    z: j.z,
                    kind,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(kind: Adaptation) -> Beard {
        Beard {
            pieces: vec![BeardPiece {
                bb: BBox::new([0, 70, 0], [8, 76, 8]),
                kind,
                ground_delta: 1,
                rigid: true,
            }],
            junctions: Vec::new(),
        }
    }

    #[test]
    fn thin_fills_below_and_clears_above_the_floor() {
        let b = piece(Adaptation::BeardThin);
        // floor reference = min y + ground delta = 71
        assert!(b.at(4, 70, 4) > 0.4 && b.at(4, 71, 4) < -0.4);
        assert!(b.at(10, 69, 4) > 0.0 && b.at(10, 73, 4) < 0.0);
        // odd around y = 70.5, fades out with distance (measured: ~0.2 one block beside the box)
        assert!((b.at(9, 70, 4) + b.at(9, 71, 4)).abs() < 1e-9);
        assert!((0.15..0.25).contains(&b.at(9, 70, 4)));
        assert!(b.at(4 + 8 + REACH, 70, 4) == 0.0 && b.at(4, 70 + 13, 4) == 0.0);
    }

    #[test]
    fn box_clears_the_box_and_above_but_not_below() {
        let b = piece(Adaptation::BeardBox);
        assert!(b.at(4, 73, 4) < -0.3 && b.at(4, 69, 4) == 0.0);
        assert!(b.at(4, 77 + 6, 4) < 0.0 && b.at(4, 77 + 6, 4) > -0.05);
        assert!(b.at(14, 73, 4) > -0.05);
    }
}

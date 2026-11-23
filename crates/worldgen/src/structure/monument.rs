//! Ocean monuments (procedural in vanilla). The start (one saved piece `omb`, its box and `O`) is
//! measured exactly (mod.rs); the building is placed from a measured majority layout
//! (monument_layout.rs, NOTES.md "Ocean monuments"): the outer shell is the same in every vanilla
//! monument, the rooms inside vary (vanilla's room graph is not reproduced).

use super::monument_layout::{LAYOUTS, PALETTE};
use super::template::BBox;
use basalt_data::BlockState;
use std::sync::OnceLock;

/// Palette index of a layout char: '!'.. without '"', '\'', '\\' and the digits (run lengths).
fn pal_index(c: u8) -> usize {
    (0x21u8..0x7f)
        .filter(|ch| !matches!(ch, b'"' | b'\'' | b'\\') && !ch.is_ascii_digit())
        .position(|ch| ch == c)
        .unwrap_or(0)
}

struct Parsed {
    d: i32,
    cells: Vec<(i32, i32, i32, u16)>,
}

fn layout() -> &'static Parsed {
    static L: OnceLock<Parsed> = OnceLock::new();
    L.get_or_init(|| {
        let pal: Vec<u16> = PALETTE
            .iter()
            .map(|n| match *n {
                "R" | "." => BlockState::parse("air").map(|b| b.0 as u16).unwrap_or(0),
                n => BlockState::parse(n).map(|b| b.0 as u16).unwrap_or(0),
            })
            .collect();
        let (_, _w, _h, d, layers) = LAYOUTS[0];
        let mut cells = Vec::new();
        for (y, layer) in layers.iter().enumerate() {
            for (z, row) in layer.split('/').enumerate() {
                let mut x = 0i32;
                let mut n = 0i32;
                for c in row.bytes() {
                    if c.is_ascii_digit() {
                        n = n * 10 + (c - b'0') as i32;
                        continue;
                    }
                    let k = n.max(1);
                    n = 0;
                    if c != b' ' {
                        let s = pal[pal_index(c)];
                        for i in 0..k {
                            cells.push((x + i, y as i32, z as i32, s));
                        }
                    }
                    x += k;
                }
            }
        }
        Parsed { d, cells }
    })
}

/// Place the monument building (box `bb`, orientation `o`) into the region (clipped to `clip`).
pub fn place(
    reg: &mut crate::region::Region,
    clip: BBox,
    bb: BBox,
    o: u8,
    _r: &mut crate::features::R,
) {
    let l = layout();
    for &(x, y, z, s) in &l.cells {
        let (bx, bz) = super::temple::to_box(o, x, z, l.d);
        let w = [bb.min[0] + bx, bb.min[1] + y, bb.min[2] + bz];
        if clip.contains(w) {
            reg.set(w[0], w[1], w[2], s);
        }
    }
}

//! Sky light and block light: BFS propagation over a 3x3 window of chunks.
//!
//! Rules (approximating vanilla): light entering a block loses `max(1, opacity)` levels;
//! fully opaque blocks (opacity 15) receive no light (but emit their own); sky light at
//! level 15 travels straight down through opacity-0 blocks without loss. Everything above
//! the build height is sky light 15. Unloaded chunks are treated as walls: light neither
//! enters nor leaves them (it is filled in when they load and get lit).
//!
//! Since light travels at most 15 blocks, any single change (or the full lighting of one
//! chunk) only affects the chunk itself and its 8 neighbours, so the engine works on a
//! window of those 9 chunks temporarily taken out of the world's map.

use crate::chunk::{Chunk, LightArray};
use crate::props::props;
use crate::{HEIGHT, MIN_Y};
use std::collections::VecDeque;

const MAX_Y: i32 = MIN_Y + HEIGHT; // exclusive

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LightKind { Sky, Block }

const DIRS: [(i32, i32, i32); 6] = [(0, -1, 0), (0, 1, 0), (-1, 0, 0), (1, 0, 0), (0, 0, -1), (0, 0, 1)];

/// A 3x3 grid of chunks centred on (cx, cz). Index `(dz+1)*3 + (dx+1)`.
pub struct Window {
    pub cx: i32,
    pub cz: i32,
    pub chunks: [Option<Chunk>; 9],
    changed: [bool; 9],
}

impl Window {
    pub fn new(cx: i32, cz: i32, chunks: [Option<Chunk>; 9]) -> Window {
        Window { cx, cz, chunks, changed: [false; 9] }
    }

    /// Chunk coords whose light changed.
    pub fn changed(&self) -> Vec<(i32, i32)> {
        (0..9).filter(|i| self.changed[*i]).map(|i| (self.cx + (i % 3) as i32 - 1, self.cz + (i / 3) as i32 - 1)).collect()
    }

    #[inline]
    fn slot(&self, x: i32, z: i32) -> Option<usize> {
        let dx = (x >> 4) - self.cx + 1;
        let dz = (z >> 4) - self.cz + 1;
        if !(0..3).contains(&dx) || !(0..3).contains(&dz) { return None; }
        let i = (dz * 3 + dx) as usize;
        if self.chunks[i].is_some() { Some(i) } else { None }
    }

    #[inline]
    fn chunk(&self, i: usize) -> &Chunk { self.chunks[i].as_ref().unwrap() }

    #[inline]
    fn state(&self, i: usize, x: i32, y: i32, z: i32) -> u32 {
        self.chunk(i).get_block((x & 15) as usize, y, (z & 15) as usize)
    }

    #[inline]
    fn arrays(&self, i: usize, k: LightKind) -> &Vec<LightArray> {
        let c = self.chunk(i);
        match k { LightKind::Sky => &c.sky_light, LightKind::Block => &c.block_light }
    }

    #[inline]
    fn get(&self, i: usize, k: LightKind, x: i32, y: i32, z: i32) -> u8 {
        match Chunk::light_index((x & 15) as usize, y, (z & 15) as usize) {
            Some((s, j)) => self.arrays(i, k)[s].get(j),
            None => 0,
        }
    }

    #[inline]
    fn set(&mut self, i: usize, k: LightKind, x: i32, y: i32, z: i32, v: u8) {
        if let Some((s, j)) = Chunk::light_index((x & 15) as usize, y, (z & 15) as usize) {
            let c = self.chunks[i].as_mut().unwrap();
            let a = match k { LightKind::Sky => &mut c.sky_light[s], LightKind::Block => &mut c.block_light[s] };
            if a.get(j) != v {
                a.set(j, v);
                c.dirty = true;
                self.changed[i] = true;
            }
        }
    }

    /// Light level `level` arriving at a block with `opacity` while moving in `dir`.
    #[inline]
    fn attenuate(k: LightKind, level: u8, dir: usize, opacity: u8) -> u8 {
        if opacity >= 15 { return 0; }
        if k == LightKind::Sky && level == 15 && dir == 0 && opacity == 0 { return 15; }
        level.saturating_sub(opacity.max(1))
    }

    /// Light a block receives from directly above the build limit.
    #[inline]
    fn from_sky(opacity: u8) -> u8 { Self::attenuate(LightKind::Sky, 15, 0, opacity) }

    /// Standard increase BFS: every queued cell spreads its current level to neighbours.
    pub fn propagate(&mut self, k: LightKind, queue: &mut VecDeque<(i32, i32, i32)>) {
        while let Some((x, y, z)) = queue.pop_front() {
            let Some(i) = self.slot(x, z) else { continue };
            let l = self.get(i, k, x, y, z);
            if l <= 1 { continue; }
            for (d, (dx, dy, dz)) in DIRS.iter().enumerate() {
                let (nx, ny, nz) = (x + dx, y + dy, z + dz);
                if !(MIN_Y..MAX_Y).contains(&ny) { continue; }
                let Some(ni) = self.slot(nx, nz) else { continue };
                let op = props(self.state(ni, nx, ny, nz)).opacity;
                let nl = Self::attenuate(k, l, d, op);
                if nl > self.get(ni, k, nx, ny, nz) {
                    self.set(ni, k, nx, ny, nz, nl);
                    queue.push_back((nx, ny, nz));
                }
            }
        }
    }

    /// Decrease BFS from cells whose light may have dropped. `seeds` holds (pos, old level)
    /// for cells already reset (to 0 or their own emission). Brighter neighbours that may
    /// refill the hole are pushed onto `refill`.
    fn remove(&mut self, k: LightKind, mut seeds: VecDeque<(i32, i32, i32, u8)>, refill: &mut VecDeque<(i32, i32, i32)>) {
        while let Some((x, y, z, old)) = seeds.pop_front() {
            for (d, (dx, dy, dz)) in DIRS.iter().enumerate() {
                let (nx, ny, nz) = (x + dx, y + dy, z + dz);
                if !(MIN_Y..MAX_Y).contains(&ny) { continue; }
                let Some(ni) = self.slot(nx, nz) else { continue };
                let l = self.get(ni, k, nx, ny, nz);
                if l == 0 { continue; }
                let from_us = l < old || (k == LightKind::Sky && d == 0 && old == 15 && l == 15);
                if from_us {
                    let st = self.state(ni, nx, ny, nz);
                    let emit = if k == LightKind::Block { props(st).emission } else { 0 };
                    let base = if k == LightKind::Sky && ny == MAX_Y - 1 { Self::from_sky(props(st).opacity) } else { emit };
                    self.set(ni, k, nx, ny, nz, base);
                    if base > 0 { refill.push_back((nx, ny, nz)); }
                    seeds.push_back((nx, ny, nz, l));
                } else {
                    refill.push_back((nx, ny, nz));
                }
            }
        }
    }

    /// Re-light after the block at (x, y, z) changed (the chunk already holds the new state).
    pub fn block_changed(&mut self, x: i32, y: i32, z: i32) {
        let Some(i) = self.slot(x, z) else { return };
        if !(MIN_Y..MAX_Y).contains(&y) { return; }
        let p = props(self.state(i, x, y, z));
        for k in [LightKind::Sky, LightKind::Block] {
            let old = self.get(i, k, x, y, z);
            let base = match k {
                LightKind::Block => p.emission,
                LightKind::Sky => if y == MAX_Y - 1 { Self::from_sky(p.opacity) } else { 0 },
            };
            self.set(i, k, x, y, z, base);
            let mut refill = VecDeque::new();
            if base > 0 { refill.push_back((x, y, z)); }
            let mut seeds = VecDeque::new();
            seeds.push_back((x, y, z, old));
            self.remove(k, seeds, &mut refill);
            // Neighbours may now shine into this (possibly more transparent) block.
            for (dx, dy, dz) in DIRS {
                let ny = y + dy;
                if (MIN_Y..MAX_Y).contains(&ny) { refill.push_back((x + dx, ny, z + dz)); }
            }
            self.propagate(k, &mut refill);
        }
    }

    /// Compute light for the centre chunk from scratch, pulling light in from (and pushing
    /// it out to) loaded neighbours.
    pub fn light_center(&mut self) {
        let ci = 4;
        {
            let c = self.chunks[ci].as_mut().expect("centre chunk");
            c.clear_light();
            c.lit = true;
        }
        self.changed[ci] = true;
        let (bx, bz) = (self.cx * 16, self.cz * 16);

        // --- sky ---
        // Highest light-relevant y around: everything above is open sky in all 9 chunks.
        let mut top = MIN_Y;
        for c in self.chunks.iter().flatten() {
            top = top.max(c.heightmaps.world_surface.iter().copied().max().unwrap_or(0) as i32 + MIN_Y);
        }
        let mut q = VecDeque::new();
        for lz in 0..16 {
            for lx in 0..16 {
                let (x, z) = (bx + lx, bz + lz);
                let mut y = MAX_Y - 1;
                let mut level = 15u8;
                while y >= MIN_Y {
                    let op = props(self.state(ci, x, y, z)).opacity;
                    let nl = Self::attenuate(LightKind::Sky, level, 0, op);
                    if nl == 0 { break; }
                    self.set(ci, LightKind::Sky, x, y, z, nl);
                    if y <= top + 1 || nl < 15 { q.push_back((x, y, z)); }
                    if nl < 15 { break; } // the BFS takes it from here
                    level = nl;
                    y -= 1;
                }
            }
        }
        self.seed_borders(LightKind::Sky, &mut q);
        self.propagate(LightKind::Sky, &mut q);

        // --- block ---
        let mut q = VecDeque::new();
        let c = self.chunk(ci);
        let mut sources = Vec::new();
        for (si, s) in c.sections.iter().enumerate() {
            if !s.blocks.palette().iter().any(|v| props(*v).emission > 0) { continue; }
            for (j, v) in s.blocks.values().enumerate() {
                let e = props(v).emission;
                if e > 0 {
                    let (lx, ly, lz) = ((j & 15) as i32, (j >> 8) as i32, ((j >> 4) & 15) as i32);
                    sources.push((bx + lx, MIN_Y + si as i32 * 16 + ly, bz + lz, e));
                }
            }
        }
        for (x, y, z, e) in sources {
            self.set(ci, LightKind::Block, x, y, z, e);
            q.push_back((x, y, z));
        }
        self.seed_borders(LightKind::Block, &mut q);
        self.propagate(LightKind::Block, &mut q);
    }

    /// Queue lit cells of the four side neighbours that touch the centre chunk.
    fn seed_borders(&self, k: LightKind, q: &mut VecDeque<(i32, i32, i32)>) {
        let (bx, bz) = (self.cx * 16, self.cz * 16);
        for t in 0..16 {
            for (x, z) in [(bx - 1, bz + t), (bx + 16, bz + t), (bx + t, bz - 1), (bx + t, bz + 16)] {
                let Some(i) = self.slot(x, z) else { continue };
                for y in MIN_Y..MAX_Y {
                    if self.get(i, k, x, y, z) > 1 { q.push_back((x, y, z)); }
                }
            }
        }
    }
}

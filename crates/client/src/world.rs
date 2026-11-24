//! Client-side world storage: chunks decoded from level_chunk_with_light.
use anyhow::{bail, Result};
use basalt_protocol::Reader;
use std::collections::HashMap;

pub struct Section {
    /// None = uniform section (`single`).
    pub blocks: Option<Box<[u16; 4096]>>,
    pub single: u16,
    pub biomes: [u8; 64],
}

impl Section {
    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> u16 {
        match &self.blocks {
            Some(b) => b[y << 8 | z << 4 | x],
            None => self.single,
        }
    }
    pub fn set(&mut self, x: usize, y: usize, z: usize, s: u16) {
        if self.blocks.is_none() {
            if self.single == s {
                return;
            }
            self.blocks = Some(Box::new([self.single; 4096]));
        }
        self.blocks.as_mut().unwrap()[y << 8 | z << 4 | x] = s;
    }
    pub fn is_empty(&self) -> bool {
        self.blocks.is_none() && self.single == 0
    }
}

pub struct Chunk {
    pub sections: Vec<Section>,
    /// Light arrays indexed by section+1 (one extra below and above the world).
    pub sky: Vec<Option<Box<[u8; 2048]>>>,
    pub block: Vec<Option<Box<[u8; 2048]>>>,
    /// Sections at or above this light index with no array are fully sky-lit.
    pub sky_open_from: usize,
}

fn read_container(
    r: &mut Reader,
    entries: usize,
    max_indirect: u8,
    min_bits: u8,
) -> Result<(Option<Vec<u32>>, u32)> {
    let bpe = r.u8()?;
    if bpe == 0 {
        return Ok((None, r.varint()? as u32));
    }
    let palette: Option<Vec<u32>> = if bpe <= max_indirect {
        let n = r.varint()?;
        if !(0..=4096).contains(&n) {
            bail!("bad palette size {n}")
        }
        let mut p = Vec::with_capacity(n as usize);
        for _ in 0..n {
            p.push(r.varint()? as u32);
        }
        Some(p)
    } else {
        None
    };
    let bits = bpe.max(min_bits) as usize;
    if bits > 32 {
        bail!("bad bits per entry {bits}")
    }
    let per = 64 / bits;
    let longs = entries.div_ceil(per);
    let raw = r.take(longs * 8)?;
    let mask = (1u64 << bits) - 1;
    let mut out = Vec::with_capacity(entries);
    'outer: for l in 0..longs {
        let mut v = u64::from_be_bytes(raw[l * 8..l * 8 + 8].try_into().unwrap());
        for _ in 0..per {
            let idx = (v & mask) as u32;
            v >>= bits;
            out.push(match &palette {
                Some(p) => *p.get(idx as usize).unwrap_or(&0),
                None => idx,
            });
            if out.len() == entries {
                break 'outer;
            }
        }
    }
    Ok((Some(out), 0))
}

fn read_bitset(r: &mut Reader) -> Result<u64> {
    let n = r.varint()?;
    let mut v = 0u64;
    for i in 0..n {
        let l = r.i64()? as u64;
        if i == 0 {
            v = l;
        }
    }
    Ok(v)
}

impl Chunk {
    /// Decode the body of level_chunk_with_light after the chunk coordinates.
    pub fn decode(r: &mut Reader, nsections: usize) -> Result<Chunk> {
        let nh = r.varint()?;
        for _ in 0..nh {
            let _t = r.varint()?;
            let n = r.varint()?;
            r.take(n as usize * 8)?;
        }
        let size = r.varint()? as usize;
        let data = r.take(size)?;
        let mut d = Reader::new(data);
        let mut sections = Vec::with_capacity(nsections);
        for _ in 0..nsections {
            let _non_air = d.i16()?;
            let _fluids = d.i16()?;
            let (blocks, single) = read_container(&mut d, 4096, 8, 4)?;
            let (biomes, bsingle) = read_container(&mut d, 64, 3, 1)?;
            let blocks = blocks.map(|v| {
                let mut b = Box::new([0u16; 4096]);
                for (i, s) in v.iter().enumerate() {
                    b[i] = *s as u16;
                }
                b
            });
            let mut bi = [bsingle as u8; 64];
            if let Some(v) = biomes {
                for (i, s) in v.iter().enumerate() {
                    bi[i] = *s as u8;
                }
            }
            sections.push(Section {
                blocks,
                single: single as u16,
                biomes: bi,
            });
        }
        // block entities (skipped)
        let nbe = r.varint()?;
        for _ in 0..nbe {
            r.u8()?;
            r.i16()?;
            r.varint()?;
            r.nbt()?;
        }
        let (sky, block, sky_open_from) = Self::decode_light(r, nsections)?;
        Ok(Chunk {
            sections,
            sky,
            block,
            sky_open_from,
        })
    }

    #[allow(clippy::type_complexity)]
    pub fn decode_light(
        r: &mut Reader,
        nsections: usize,
    ) -> Result<(
        Vec<Option<Box<[u8; 2048]>>>,
        Vec<Option<Box<[u8; 2048]>>>,
        usize,
    )> {
        let sky_mask = read_bitset(r)?;
        let block_mask = read_bitset(r)?;
        let empty_sky = read_bitset(r)?;
        let _empty_block = read_bitset(r)?;
        let n = nsections + 2;
        let mut read_arrays = |mask: u64| -> Result<Vec<Option<Box<[u8; 2048]>>>> {
            let cnt = r.varint()?;
            let mut out: Vec<Option<Box<[u8; 2048]>>> = (0..n).map(|_| None).collect();
            let mut bits = (0..n).filter(|i| mask >> i & 1 == 1);
            for _ in 0..cnt {
                let len = r.varint()? as usize;
                let raw = r.take(len)?;
                if let Some(i) = bits.next() {
                    if len == 2048 {
                        out[i] = Some(Box::new(raw.try_into().unwrap()));
                    }
                }
            }
            Ok(out)
        };
        let sky = read_arrays(sky_mask)?;
        let block = read_arrays(block_mask)?;
        let top = (0..n)
            .rev()
            .find(|i| (sky_mask | empty_sky) >> i & 1 == 1)
            .map(|i| i + 1)
            .unwrap_or(0);
        Ok((sky, block, top))
    }

    #[inline]
    pub fn sky_light(&self, x: usize, sy: i32, y: usize, z: usize) -> u8 {
        let li = (sy + 1) as usize;
        if li >= self.sky.len() {
            return 15;
        }
        match &self.sky[li] {
            Some(a) => {
                let i = y << 8 | z << 4 | x;
                (a[i >> 1] >> ((i & 1) * 4)) & 15
            }
            None => {
                if li >= self.sky_open_from {
                    15
                } else {
                    0
                }
            }
        }
    }
    #[inline]
    pub fn block_light(&self, x: usize, sy: i32, y: usize, z: usize) -> u8 {
        let li = (sy + 1) as usize;
        if li >= self.block.len() {
            return 0;
        }
        match &self.block[li] {
            Some(a) => {
                let i = y << 8 | z << 4 | x;
                (a[i >> 1] >> ((i & 1) * 4)) & 15
            }
            None => 0,
        }
    }
}

pub struct World {
    pub chunks: HashMap<(i32, i32), Chunk>,
    pub min_y: i32,
    pub height: i32,
}

impl World {
    pub fn new(min_y: i32, height: i32) -> Self {
        World {
            chunks: HashMap::new(),
            min_y,
            height,
        }
    }
    pub fn nsections(&self) -> usize {
        (self.height / 16) as usize
    }
    #[inline]
    pub fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        if y < self.min_y || y >= self.min_y + self.height {
            return 0;
        }
        match self.chunks.get(&(x >> 4, z >> 4)) {
            Some(c) => c.sections[((y - self.min_y) >> 4) as usize].get(
                (x & 15) as usize,
                (y & 15) as usize,
                (z & 15) as usize,
            ),
            None => 0,
        }
    }
    pub fn loaded(&self, x: i32, z: i32) -> bool {
        self.chunks.contains_key(&(x >> 4, z >> 4))
    }
    pub fn set(&mut self, x: i32, y: i32, z: i32, s: u16) -> bool {
        if y < self.min_y || y >= self.min_y + self.height {
            return false;
        }
        let min_y = self.min_y;
        match self.chunks.get_mut(&(x >> 4, z >> 4)) {
            Some(c) => {
                c.sections[((y - min_y) >> 4) as usize].set(
                    (x & 15) as usize,
                    (y & 15) as usize,
                    (z & 15) as usize,
                    s,
                );
                true
            }
            None => false,
        }
    }
    /// (sky, block) light at a position.
    pub fn light(&self, x: i32, y: i32, z: i32) -> (u8, u8) {
        if y >= self.min_y + self.height {
            return (15, 0);
        }
        if y < self.min_y {
            return (0, 0);
        }
        match self.chunks.get(&(x >> 4, z >> 4)) {
            Some(c) => {
                let sy = (y - self.min_y) >> 4;
                let (lx, ly, lz) = ((x & 15) as usize, (y & 15) as usize, (z & 15) as usize);
                (c.sky_light(lx, sy, ly, lz), c.block_light(lx, sy, ly, lz))
            }
            None => (15, 0),
        }
    }
    pub fn biome(&self, x: i32, y: i32, z: i32) -> u8 {
        let y = y.clamp(self.min_y, self.min_y + self.height - 1);
        match self.chunks.get(&(x >> 4, z >> 4)) {
            Some(c) => {
                c.sections[((y - self.min_y) >> 4) as usize].biomes
                    [(((y & 15) >> 2) << 4 | ((z & 15) >> 2) << 2 | ((x & 15) >> 2)) as usize]
            }
            None => 0,
        }
    }
}

//! Paletted containers: the compact storage for a section's 4096 block states or 64 biomes.
//!
//! In memory the container is always "palette + packed indices" (or a single value), with
//! the index width growing as the palette grows. The network and disk encodings are
//! produced on demand from a compacted copy, so stale palette entries never leak out.
//!
//! Packing (both wire and disk): `64 / bits` entries per long, entries never span longs,
//! lowest bits first, `ceil(n / per)` longs.

use anyhow::{bail, Result};
use basalt_protocol::Reader;

/// Which kind of data a container holds; determines size and encoding tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// 16x16x16 block states, index `y<<8 | z<<4 | x`.
    Blocks,
    /// 4x4x4 biomes, index `y<<4 | z<<2 | x`.
    Biomes,
}

impl Kind {
    pub const fn len(self) -> usize {
        match self {
            Kind::Blocks => 4096,
            Kind::Biomes => 64,
        }
    }
    pub const fn is_empty(self) -> bool {
        false
    }
    /// Smallest index width used for indirect (palette) storage.
    pub const fn min_bits(self) -> u8 {
        match self {
            Kind::Blocks => 4,
            Kind::Biomes => 1,
        }
    }
    /// Largest index width that still uses a palette on the wire.
    pub const fn max_indirect_bits(self) -> u8 {
        match self {
            Kind::Blocks => 8,
            Kind::Biomes => 3,
        }
    }
    /// Entry width for the direct (global id) wire encoding.
    pub fn direct_bits(self) -> u8 {
        match self {
            Kind::Blocks => ceil_log2(basalt_data::blocks::STATE_COUNT),
            Kind::Biomes => ceil_log2(crate::BIOMES.len()),
        }
    }
    /// Number of valid global ids.
    pub fn registry_size(self) -> usize {
        match self {
            Kind::Blocks => basalt_data::blocks::STATE_COUNT,
            Kind::Biomes => crate::BIOMES.len(),
        }
    }
    /// Index width used on disk for a palette of `n` entries (vanilla never uses direct on disk).
    pub fn disk_bits(self, n: usize) -> u8 {
        if n <= 1 {
            0
        } else {
            match self {
                Kind::Blocks => ceil_log2(n).max(4),
                Kind::Biomes => ceil_log2(n),
            }
        }
    }
}

/// Number of bits needed to store values `0..n`.
pub fn ceil_log2(n: usize) -> u8 {
    if n <= 1 {
        0
    } else {
        (usize::BITS - (n - 1).leading_zeros()) as u8
    }
}

/// Number of longs needed to pack `n` entries of `bits` width.
pub fn packed_len(n: usize, bits: u8) -> usize {
    if bits == 0 {
        0
    } else {
        n.div_ceil(64 / bits as usize)
    }
}

/// Pack values of `bits` width (non-spanning, lowest bits first).
pub fn pack(values: impl ExactSizeIterator<Item = u32>, bits: u8) -> Vec<u64> {
    let per = 64 / bits as usize;
    let mut out = vec![0u64; values.len().div_ceil(per)];
    for (i, v) in values.enumerate() {
        out[i / per] |= (v as u64) << ((i % per) * bits as usize);
    }
    out
}

/// Unpack `n` values of `bits` width. `data` must hold at least `packed_len(n, bits)` longs.
pub fn unpack(data: &[u64], bits: u8, n: usize) -> Vec<u32> {
    let per = 64 / bits as usize;
    let mask = if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    };
    (0..n)
        .map(|i| ((data[i / per] >> ((i % per) * bits as usize)) & mask) as u32)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PalettedContainer {
    kind: Kind,
    /// Distinct values referenced by `data` (may contain stale entries after `set`).
    palette: Vec<u32>,
    /// Index width; 0 means single-valued (`palette[0]`, `data` empty).
    bits: u8,
    data: Vec<u64>,
}

impl PalettedContainer {
    pub fn filled(kind: Kind, v: u32) -> Self {
        PalettedContainer {
            kind,
            palette: vec![v],
            bits: 0,
            data: Vec::new(),
        }
    }
    pub fn blocks_filled(state: u32) -> Self {
        Self::filled(Kind::Blocks, state)
    }
    pub fn biomes_filled(id: u32) -> Self {
        Self::filled(Kind::Biomes, id)
    }

    /// Build from a full slice of values (`values.len()` must equal `kind.len()`).
    pub fn from_values(kind: Kind, values: &[u32]) -> Self {
        assert_eq!(
            values.len(),
            kind.len(),
            "wrong number of values for {kind:?}"
        );
        let mut palette: Vec<u32> = Vec::new();
        let mut map: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        let idx: Vec<u32> = values
            .iter()
            .map(|v| {
                *map.entry(*v).or_insert_with(|| {
                    palette.push(*v);
                    palette.len() as u32 - 1
                })
            })
            .collect();
        Self::from_parts(kind, palette, &idx)
    }

    fn from_parts(kind: Kind, palette: Vec<u32>, idx: &[u32]) -> Self {
        if palette.len() <= 1 {
            return Self::filled(kind, palette.first().copied().unwrap_or(0));
        }
        let bits = ceil_log2(palette.len()).max(kind.min_bits());
        PalettedContainer {
            kind,
            palette,
            bits,
            data: pack(idx.iter().copied(), bits),
        }
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }
    pub fn len(&self) -> usize {
        self.kind.len()
    }
    pub fn is_empty(&self) -> bool {
        false
    }

    /// The palette (may include values no longer present after `set`).
    pub fn palette(&self) -> &[u32] {
        &self.palette
    }

    /// Single value if the container is uniform in its current representation.
    pub fn single(&self) -> Option<u32> {
        if self.bits == 0 {
            Some(self.palette[0])
        } else {
            None
        }
    }

    #[inline]
    fn index_at(&self, i: usize) -> usize {
        let bits = self.bits as usize;
        let per = 64 / bits;
        ((self.data[i / per] >> ((i % per) * bits)) & ((1u64 << bits) - 1)) as usize
    }

    #[inline]
    pub fn get(&self, i: usize) -> u32 {
        if self.bits == 0 {
            return self.palette[0];
        }
        self.palette[self.index_at(i)]
    }

    /// Set entry `i` to `v`, returning the previous value.
    pub fn set(&mut self, i: usize, v: u32) -> u32 {
        assert!(i < self.kind.len());
        let old = self.get(i);
        if old == v {
            return old;
        }
        let pi = match self.palette.iter().position(|p| *p == v) {
            Some(p) => p,
            None => {
                if self.palette.len() >= self.kind.len() {
                    // Too many stale entries: drop them before growing further.
                    self.compact();
                }
                self.palette.push(v);
                let need = ceil_log2(self.palette.len());
                if need > self.bits {
                    self.resize(need.max(self.kind.min_bits()));
                }
                self.palette.len() - 1
            }
        };
        let bits = self.bits as usize;
        let per = 64 / bits;
        let shift = (i % per) * bits;
        let mask = ((1u64 << bits) - 1) << shift;
        let l = &mut self.data[i / per];
        *l = (*l & !mask) | ((pi as u64) << shift);
        old
    }

    fn resize(&mut self, new_bits: u8) {
        let n = self.kind.len();
        let idx: Vec<u32> = if self.bits == 0 {
            vec![0; n]
        } else {
            (0..n).map(|i| self.index_at(i) as u32).collect()
        };
        self.bits = new_bits;
        self.data = pack(idx.into_iter(), new_bits);
    }

    /// Iterate all values in index order.
    pub fn values(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.kind.len()).map(move |i| self.get(i))
    }

    /// Palette of values actually used (first-occurrence order) and per-entry indices into it.
    pub fn compacted(&self) -> (Vec<u32>, Vec<u32>) {
        if self.bits == 0 {
            return (vec![self.palette[0]], vec![0; self.kind.len()]);
        }
        let mut remap = vec![u32::MAX; self.palette.len()];
        let mut pal = Vec::new();
        let idx = (0..self.kind.len())
            .map(|i| {
                let p = self.index_at(i);
                if remap[p] == u32::MAX {
                    remap[p] = pal.len() as u32;
                    pal.push(self.palette[p]);
                }
                remap[p]
            })
            .collect();
        (pal, idx)
    }

    /// Re-pack with only the used palette entries.
    pub fn compact(&mut self) {
        let (pal, idx) = self.compacted();
        *self = Self::from_parts(self.kind, pal, &idx);
    }

    /// Vanilla network encoding: `u8 bpe`; bpe 0 → varint value; indirect → varint palette
    /// length + varint ids; direct → global ids; then the packed longs with no length prefix.
    pub fn write_network(&self, w: &mut Vec<u8>) {
        let (pal, idx) = self.compacted();
        if pal.len() == 1 {
            w.push(0);
            write_varint(w, pal[0] as i32);
            return;
        }
        let need = ceil_log2(pal.len());
        let longs = if need <= self.kind.max_indirect_bits() {
            let bits = need.max(self.kind.min_bits());
            w.push(bits);
            write_varint(w, pal.len() as i32);
            for p in &pal {
                write_varint(w, *p as i32);
            }
            pack(idx.into_iter(), bits)
        } else {
            let bits = self.kind.direct_bits();
            w.push(bits);
            pack(idx.into_iter().map(|i| pal[i as usize]), bits)
        };
        for l in longs {
            w.extend_from_slice(&l.to_be_bytes());
        }
    }

    /// Decode the network encoding (`blocks` selects 4096 block states vs 64 biomes).
    pub fn read_network(r: &mut Reader, blocks: bool) -> Result<Self> {
        let kind = if blocks { Kind::Blocks } else { Kind::Biomes };
        let bpe = r.u8()?;
        if bpe == 0 {
            let v = r.varint()?;
            if v < 0 || v as usize >= kind.registry_size() {
                bail!("paletted container: bad single value {v}")
            }
            return Ok(Self::filled(kind, v as u32));
        }
        let palette = if bpe <= kind.max_indirect_bits() {
            let n = r.varint()?;
            if n < 1 || n as usize > kind.len() {
                bail!("paletted container: bad palette size {n}")
            }
            let mut p = Vec::with_capacity(n as usize);
            for _ in 0..n {
                let v = r.varint()?;
                if v < 0 || v as usize >= kind.registry_size() {
                    bail!("paletted container: bad palette entry {v}")
                }
                p.push(v as u32);
            }
            Some(p)
        } else {
            None
        };
        let bits = if palette.is_some() {
            bpe.max(kind.min_bits())
        } else {
            bpe
        };
        if bits > 32 {
            bail!("paletted container: bad bits per entry {bits}")
        }
        let n = packed_len(kind.len(), bits);
        let raw = r.take(n * 8)?;
        let data: Vec<u64> = raw
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| u64::from_be_bytes(*c))
            .collect();
        let vals = unpack(&data, bits, kind.len());
        match palette {
            Some(p) => {
                if let Some(bad) = vals.iter().find(|v| **v as usize >= p.len()) {
                    bail!("paletted container: index {bad} out of palette")
                }
                let vals: Vec<u32> = vals.iter().map(|v| p[*v as usize]).collect();
                Ok(Self::from_values(kind, &vals))
            }
            None => {
                if let Some(bad) = vals.iter().find(|v| **v as usize >= kind.registry_size()) {
                    bail!("paletted container: bad direct id {bad}")
                }
                Ok(Self::from_values(kind, &vals))
            }
        }
    }

    /// Disk form: (palette of used values, packed indices — empty when single-valued).
    pub fn to_disk(&self) -> (Vec<u32>, Vec<i64>) {
        let (pal, idx) = self.compacted();
        let bits = self.kind.disk_bits(pal.len());
        let data = if bits == 0 {
            Vec::new()
        } else {
            pack(idx.into_iter(), bits)
                .into_iter()
                .map(|l| l as i64)
                .collect()
        };
        (pal, data)
    }

    /// Inverse of `to_disk`.
    pub fn from_disk(kind: Kind, palette: Vec<u32>, data: &[i64]) -> Result<Self> {
        if palette.is_empty() {
            bail!("empty palette")
        }
        if palette.len() == 1 {
            return Ok(Self::filled(kind, palette[0]));
        }
        let mut bits = kind.disk_bits(palette.len());
        if data.len() != packed_len(kind.len(), bits) {
            // Tolerate writers that used a different (wider) index width.
            match (1..=32u8).find(|b| *b >= bits && packed_len(kind.len(), *b) == data.len()) {
                Some(b) => bits = b,
                None => bail!(
                    "data length {} doesn't match palette of {}",
                    data.len(),
                    palette.len()
                ),
            }
        }
        let data: Vec<u64> = data.iter().map(|l| *l as u64).collect();
        let idx = unpack(&data, bits, kind.len());
        if idx.iter().any(|i| *i as usize >= palette.len()) {
            bail!("palette index out of range")
        }
        let vals: Vec<u32> = idx.iter().map(|i| palette[*i as usize]).collect();
        Ok(Self::from_values(kind, &vals))
    }
}

pub(crate) fn write_varint(w: &mut Vec<u8>, v: i32) {
    let mut v = v as u32;
    loop {
        if v & !0x7F == 0 {
            w.push(v as u8);
            return;
        }
        w.push((v & 0x7F | 0x80) as u8);
        v >>= 7;
    }
}

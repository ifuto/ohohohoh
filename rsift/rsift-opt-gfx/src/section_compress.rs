//! Hybrid section storage: Single / RLE (hot) / LZ4 (cold distant columns).
//! Builds on [`crate::section_rle::RleSection`] + `lz4_flex` (fastest pure-Rust LZ4).

use crate::binary_greedy_meshing::{SectionPalette, SECTION_SIZE};
use crate::section_rle::RleSection;
use lz4_flex::{compress_prepend_size, decompress_size_prepended};

const VOL: usize = SECTION_SIZE * SECTION_SIZE * SECTION_SIZE;

/// Memory-efficient section payload.
#[derive(Debug, Clone)]
pub enum CompactSection {
    Single(u16),
    Rle(RleSection),
    Lz4(Vec<u8>),
}

impl CompactSection {
    /// Hot-path encode: prefer single-value, else RLE (vanilla-like density).
    pub fn encode_hot(palette: &SectionPalette) -> Self {
        let first = palette[0];
        if palette.iter().all(|&b| b == first) {
            return Self::Single(first);
        }
        let rle = RleSection::encode(palette);
        // If RLE is worse than raw, fall back to LZ4 of dense.
        if rle.runs.len() * 4 >= VOL {
            return Self::encode_cold(palette);
        }
        Self::Rle(rle)
    }

    /// Cold-path encode for columns outside the mesh window.
    pub fn encode_cold(palette: &SectionPalette) -> Self {
        let first = palette[0];
        if palette.iter().all(|&b| b == first) {
            return Self::Single(first);
        }
        let bytes: &[u8] = bytemuck::cast_slice(palette.as_slice());
        Self::Lz4(compress_prepend_size(bytes))
    }

    pub fn decode(&self) -> SectionPalette {
        match self {
            Self::Single(id) => [*id; VOL],
            Self::Rle(r) => r.decode(),
            Self::Lz4(buf) => {
                let raw = decompress_size_prepended(buf).unwrap_or_default();
                let mut out = [0u16; VOL];
                let words: &[u16] = bytemuck::try_cast_slice(&raw).unwrap_or(&[]);
                let n = words.len().min(VOL);
                out[..n].copy_from_slice(&words[..n]);
                out
            }
        }
    }

    pub fn stored_bytes(&self) -> usize {
        match self {
            Self::Single(_) => 2,
            Self::Rle(r) => 2 + r.runs.len() * 4,
            Self::Lz4(b) => b.len(),
        }
    }

    pub fn is_air(&self) -> bool {
        match self {
            Self::Single(0) => true,
            Self::Single(_) => false,
            Self::Rle(r) => r.is_empty(),
            Self::Lz4(_) => false,
        }
    }

    /// Re-encode for cold storage after leaving the mesh radius.
    pub fn to_cold(&self) -> Self {
        match self {
            Self::Single(_) => self.clone(),
            Self::Lz4(_) => self.clone(),
            Self::Rle(_) => Self::encode_cold(&self.decode()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary_greedy_meshing::idx;

    #[test]
    fn hot_rle_roundtrip() {
        let mut p = [0u16; VOL];
        p[idx(1, 2, 3)] = 9;
        let c = CompactSection::encode_hot(&p);
        assert_eq!(c.decode()[idx(1, 2, 3)], 9);
        assert!(c.stored_bytes() < VOL * 2);
    }

    #[test]
    fn cold_lz4_roundtrip() {
        let mut p = [0u16; VOL];
        for i in 0..VOL {
            p[i] = (i % 40) as u16;
        }
        let c = CompactSection::encode_cold(&p);
        assert_eq!(c.decode(), p);
    }
}

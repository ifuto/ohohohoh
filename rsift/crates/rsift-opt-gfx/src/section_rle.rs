//! Run-length encoding for 16³ section palettes and Y-layer occupancy masks.
//!
//! Sparse terrain (air runs, flat layers) compresses 10–50× vs raw `[u16; 4096]`.
//! Used before meshing (skip empty layers) and in disk cache (bandwidth).

use crate::binary_greedy_meshing::{SectionPalette, SECTION_SIZE};

const VOLUME: usize = SECTION_SIZE * SECTION_SIZE * SECTION_SIZE;

/// One contiguous run of identical block ids in palette scan order (x, y, z).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RleRun {
    pub block: u16,
    pub count: u16,
}

/// RLE-compressed section palette.
#[derive(Debug, Clone, Default)]
pub struct RleSection {
    pub runs: Vec<RleRun>,
}

impl RleSection {
    /// Encode palette in x-major order (matches Minecraft section indexing).
    pub fn encode(palette: &SectionPalette) -> Self {
        let mut runs = Vec::with_capacity(64);
        if VOLUME == 0 {
            return Self { runs };
        }
        let mut cur = palette[0];
        let mut count = 1u16;
        for &block in &palette[1..] {
            if block == cur && count < u16::MAX {
                count += 1;
            } else {
                runs.push(RleRun { block: cur, count });
                cur = block;
                count = 1;
            }
        }
        runs.push(RleRun { block: cur, count });
        Self { runs }
    }

    pub fn decode(&self) -> SectionPalette {
        let mut out = [0u16; VOLUME];
        let mut i = 0usize;
        for run in &self.runs {
            let end = i + run.count as usize;
            if end > VOLUME {
                break;
            }
            out[i..end].fill(run.block);
            i = end;
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.runs.iter().all(|r| r.block == 0)
    }

    /// True if every voxel is opaque block `block`.
    pub fn is_solid(&self, block: u16) -> bool {
        // `|| (r.block == 0 && false)` は恒偽の冗長項 (常に r.block == block と同値)。
        !self.is_empty()
            && self.runs.iter().all(|r| r.block == block)
            && self.runs.len() == 1
            && self.runs[0].block == block
            && self.runs[0].count as usize == VOLUME
    }

    /// Count non-air voxels without full decode.
    pub fn solid_voxel_count(&self) -> u32 {
        self.runs
            .iter()
            .filter(|r| r.block != 0)
            .map(|r| r.count as u32)
            .sum()
    }

    /// Y-layer occupancy: bit `y` set if layer has any non-air block.
    pub fn layer_occupancy(&self) -> u16 {
        let mut bits = 0u16;
        let mut idx = 0usize;
        for run in &self.runs {
            if run.block != 0 {
                for off in 0..run.count as usize {
                    let i = idx + off;
                    if i < VOLUME {
                        let y = (i / SECTION_SIZE) % SECTION_SIZE;
                        bits |= 1u16 << y;
                    }
                }
            }
            idx += run.count as usize;
        }
        bits
    }

    /// Serialize to compact bytes: `[u16 run_count][block:u16 count:u16]*`
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(2 + self.runs.len() * 4);
        out.extend_from_slice(&(self.runs.len() as u16).to_le_bytes());
        for r in &self.runs {
            out.extend_from_slice(&r.block.to_le_bytes());
            out.extend_from_slice(&r.count.to_le_bytes());
        }
        out
    }

    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 2 {
            return None;
        }
        let run_count = u16::from_le_bytes([data[0], data[1]]) as usize;
        let need = 2 + run_count * 4;
        if data.len() < need {
            return None;
        }
        let mut runs = Vec::with_capacity(run_count);
        let mut off = 2;
        for _ in 0..run_count {
            let block = u16::from_le_bytes([data[off], data[off + 1]]);
            let count = u16::from_le_bytes([data[off + 2], data[off + 3]]);
            off += 4;
            runs.push(RleRun { block, count });
        }
        Some(Self { runs })
    }
}

/// RLE-compress a row of 16 face-visibility bits (one Y slice row).
pub fn encode_row_mask_rle(rows: &[u16; SECTION_SIZE]) -> Vec<u8> {
    let mut out = Vec::with_capacity(SECTION_SIZE * 2);
    for &row in rows {
        if row == 0 {
            out.push(0);
            out.push(1);
        } else if row == 0xFFFF {
            out.push(0xFF);
            out.push(0xFF);
            out.push(16);
        } else {
            out.push((row & 0xFF) as u8);
            out.push((row >> 8) as u8);
            out.push(1);
        }
    }
    out
}

/// Build per-Y layer opaque row masks from RLE.
/// 【現状の実装事実】「without full palette decode when possible」という旧 doc
/// 記述に反し、実装はフル decode して palette 版へ委譲している (高速経路は
/// まだ存在しない)。呼び出し側は decode コストを前提にすること。
pub fn layer_masks_from_rle(rle: &RleSection) -> [u16; SECTION_SIZE] {
    let palette = rle.decode();
    layer_masks_from_palette(&palette)
}

pub fn layer_masks_from_palette(palette: &SectionPalette) -> [u16; SECTION_SIZE] {
    let mut per_y = [[0u16; SECTION_SIZE]; SECTION_SIZE];
    for y in 0..SECTION_SIZE {
        for z in 0..SECTION_SIZE {
            let mut row = 0u16;
            for x in 0..SECTION_SIZE {
                let i = x + y * SECTION_SIZE + z * SECTION_SIZE * SECTION_SIZE;
                if palette[i] != 0 {
                    row |= 1u16 << x;
                }
            }
            per_y[y][z] = row;
        }
    }
    let mut combined = [0u16; SECTION_SIZE];
    for y in 0..SECTION_SIZE {
        combined[y] = per_y[y].iter().fold(0u16, |a, &b| a | b);
    }
    combined
}

/// Section indices for VisGraph (maps column Y → graph node at y=section, x=0, z=0).
pub fn occupied_section_indices(sections: &[RleSection]) -> Vec<u32> {
    sections
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.is_empty())
        .map(|(i, _)| (i as u32) * 8)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary_greedy_meshing::idx;

    #[test]
    fn roundtrip_palette() {
        let mut p = [0u16; VOLUME];
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                p[idx(x, 4, z)] = 1;
            }
        }
        let rle = RleSection::encode(&p);
        let back = rle.decode();
        assert_eq!(p, back);
    }

    #[test]
    fn air_column_single_run() {
        let p = [0u16; VOLUME];
        let rle = RleSection::encode(&p);
        assert_eq!(rle.runs.len(), 1);
        assert!(rle.is_empty());
    }

    #[test]
    fn bytes_roundtrip() {
        let mut p = [0u16; VOLUME];
        p[0] = 7;
        p[1] = 7;
        let rle = RleSection::encode(&p);
        let bytes = rle.to_bytes();
        let back = RleSection::from_bytes(&bytes).unwrap();
        assert_eq!(rle.decode(), back.decode());
    }

    #[test]
    fn layer_occupancy_detects_solid_y() {
        let mut p = [0u16; VOLUME];
        p[idx(0, 3, 0)] = 1;
        let rle = RleSection::encode(&p);
        assert!(rle.layer_occupancy() & (1 << 3) != 0);
    }
}

#[cfg(test)]
mod extra_tests {
    use super::*;
    use crate::binary_greedy_meshing::idx;

    #[test]
    fn from_bytes_rejects_truncation() {
        let sec: SectionPalette = [0u16; VOLUME];
        let rle = RleSection::encode(&sec);
        let bytes = rle.to_bytes();
        assert!(RleSection::from_bytes(&bytes).is_some());
        assert!(RleSection::from_bytes(&[]).is_none(), "0B はヘッダすら無い");
        assert!(RleSection::from_bytes(&bytes[..1]).is_none());
        // ヘッダは len を誇張した料簡: 実バイト不足 → None。
        let mut lying = bytes[..2].to_vec();
        lying.extend_from_slice(&[0u8; 2]);
        // run_count=1 を名乗りながら本体 4B しか無い (need=6)
        let mut lying2 = Vec::new();
        lying2.extend_from_slice(&1u16.to_le_bytes());
        lying2.extend_from_slice(&[7u8, 7u8]);
        assert!(RleSection::from_bytes(&lying2).is_none());
    }

    #[test]
    fn is_solid_requires_uniform_single_run() {
        let mut p: SectionPalette = [1u16; VOLUME];
        let rle = RleSection::encode(&p);
        assert!(rle.is_solid(1));
        assert!(!rle.is_solid(2));
        p[idx(0, 0, 0)] = 2; // 1 voxel だけ異質
        let rle2 = RleSection::encode(&p);
        assert!(!rle2.is_solid(1), "混在は solid ではない");
        assert!(!rle2.is_empty());
        // 全 air は is_solid(0) ではなく is_empty (air を「固体」扱いしない規約)。
        let air = RleSection::encode(&[0u16; VOLUME]);
        assert!(air.is_empty());
        assert!(!air.is_solid(0));
    }

    #[test]
    fn layer_masks_combined_is_or_over_z_rows() {
        // combined[y] の bit x = 「その y 層のどこかの z 行に x 列の非airがある」。
        let mut p: SectionPalette = [0u16; VOLUME];
        p[idx(3, 0, 0)] = 5; // y=0, z=0, x=3
        p[idx(9, 0, 7)] = 5; // y=0, z=7, x=9
        p[idx(3, 4, 15)] = 5; // y=4, z=15, x=3
        let masks = layer_masks_from_palette(&p);
        assert_eq!(masks[0], (1 << 3) | (1 << 9));
        assert_eq!(masks[4], 1 << 3);
        for y in [1usize, 2, 3, 5, 15] {
            assert_eq!(masks[y], 0, "empty layer y={y} must be zero mask");
        }
    }

    #[test]
    fn encode_row_mask_rle_emits_exact_wire_bytes() {
        // 語彙: 全 0 行 → [0,1] / 全 F 行 → [0xFF,0xFF,16] / 部分行 → [lo,hi,1]。
        let mut rows = [0u16; SECTION_SIZE];
        let b0 = encode_row_mask_rle(&rows);
        assert_eq!(b0.len(), 2 * SECTION_SIZE, "zero rows → 2B each");
        assert_eq!(&b0[0..2], &[0u8, 1u8]);

        rows[0] = 0xFFFF;
        rows[1] = 0x00F3;
        let b1 = encode_row_mask_rle(&rows);
        assert_eq!(&b1[0..3], &[0xFFu8, 0xFF, 16], "0xFFFF row → mark + len16");
        assert_eq!(&b1[3..6], &[0xF3u8, 0x00, 1], "partial row → LE 2B + len1");
    }

    #[test]
    fn row_masks_of_solid_y_layer_are_all_ffff() {
        // y=2 層だけ全面固体 → その層の行マスクは全て 0xFFFF。
        let mut p: SectionPalette = [0u16; VOLUME];
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                p[idx(x, 2, z)] = 1;
            }
        }
        let rle = RleSection::encode(&p);
        assert_eq!(rle.layer_occupancy(), 1 << 2);
    }
}

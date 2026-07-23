//! Run-length encoding for 16³ section palettes and Y-layer occupancy masks.
//!
//! Sparse terrain (air runs, flat layers) compresses vs raw `[u16; 4096]`
//! (全空気は 8192B→6B で ~1365×、層状地形で数十×。**最悪ケース** (全ボクセル
//! 交互違い) は 4B/run × 4096 + 2B = 16386B で **~2× 膨張** — wave 61 で
//! 誇大の無い範囲を明記)。
//! Used before meshing (skip empty layers) and in disk cache (bandwidth).
//!
//! ## wave 61 BK 監査で撤去 (消費ゼロ実測、根拠を記録)
//! - `encode_row_mask_rle`: workspace 全域に呼出・デコーダ共に皆無の
//!   write-only wire 語彙だった (BA-2 と同根拠で撤去)。
//! - `RleSection::layer_occupancy`: 同じく消費者ゼロ (meshing の層 skip は
//!   palette 版 `layer_masks_from_palette` が担う)。

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
    ///
    /// count の u16 安全性: 1 run の最大長は VOLUME=4096 < u16::MAX なので
    /// `count < u16::MAX` の分岐は到達不能の防御 (wave 61 監査で証明・明記)。
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
        // **契約 (wave 61 BK-1 で fail-loud 化)**: runs の count 総和は必ず
        // VOLUME (=4096)。旧実装は超過時に `break` で静寂切捨て・不足時は
        // 末尾を 0 (空気) のまま返し、破損 wire/手組み RLE が**空気ボクセル
        // を静寂注入**し得た。encode() 経由の RLE は Σ=VOLUME が不変条件
        // (ループが palette 全 4096 要素を走査して push される count の
        // 総和は定義通り 4096)。
        let total: usize = self.runs.iter().map(|r| r.count as usize).sum();
        assert!(
            total == VOLUME,
            "RleSection::decode 契約違反: count 総和 {total} != VOLUME {VOLUME} (破損/手組み RLE の静寂 air 化を拒否)"
        );
        let mut out = [0u16; VOLUME];
        let mut i = 0usize;
        for run in &self.runs {
            let end = i + run.count as usize;
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

    /// **厳格 wire 復元 (wave 61 BK-2 で強化)**:
    /// (a) バイト長はヘッダの run_count と**完全一致** (末尾ゴミも拒否)、
    /// (b) count 総和は VOLUME (=4096) に一致。旧実装は (b) を検査せず
    /// 破損 wire を受理し、decode 時の静寂 air 注入に繋げていた (BK-1)。
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 2 {
            return None;
        }
        let run_count = u16::from_le_bytes([data[0], data[1]]) as usize;
        let need = 2 + run_count * 4;
        if data.len() != need {
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
        let total: usize = runs.iter().map(|r| r.count as usize).sum();
        if total != VOLUME {
            return None; // Σ≠4096 は破損 wire (BK-1 の静寂 air 注入源)
        }
        Some(Self { runs })
    }
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

    /// wave 61 BK-1: 超過 run を持つ手組み RLE の decode は fail-loud
    /// (旧実装は末尾を断ち切って静寂受理していた)。
    #[test]
    #[should_panic(expected = "RleSection::decode 契約違反")]
    fn decode_rejects_overrun_runs() {
        let bad = RleSection {
            runs: vec![
                RleRun {
                    block: 1,
                    count: VOLUME as u16,
                },
                RleRun { block: 1, count: 1 },
            ],
        };
        let _ = bad.decode();
    }

    /// wave 61 BK-1: 不足 (Σ<4096) の手組み RLE も fail-loud
    /// (旧実装は末尾を空気のまま静寂返却していた)。
    #[test]
    #[should_panic(expected = "RleSection::decode 契約違反")]
    fn decode_rejects_underrun_runs() {
        let bad = RleSection {
            runs: vec![RleRun {
                block: 1,
                count: (VOLUME - 1) as u16,
            }],
        };
        let _ = bad.decode();
    }

    /// wave 61: encode の不変条件 (Σcount == VOLUME、最大 run ≤4096) を
    /// 一様パレットでピン — count: u16 の余地証明。
    #[test]
    fn uniform_palette_single_max_run() {
        let p = [5u16; VOLUME];
        let rle = RleSection::encode(&p);
        assert_eq!(rle.runs.len(), 1);
        assert_eq!(
            (rle.runs[0].block, rle.runs[0].count),
            (5, VOLUME as u16),
            "1 run = 4096 (< u16::MAX = 65535 で安全)"
        );
        let total: usize = rle.runs.iter().map(|r| r.count as usize).sum();
        assert_eq!(total, VOLUME, "encode 不変条件 Σ=VOLUME");
    }

    /// wave 61: occupied_section_indices の ×8 語彙ピン
    /// (VisGraph ノード ID 名前空間との結合語彙 — 値自体は
    /// frame_reuse 内で等価比較のみに消費される閉じた語彙)。
    #[test]
    fn occupied_section_indices_vocabulary() {
        let air = RleSection::encode(&[0u16; VOLUME]);
        let mut solid = [0u16; VOLUME];
        solid[0] = 1;
        let solid = RleSection::encode(&solid);
        assert_eq!(
            occupied_section_indices(&[solid.clone(), air.clone(), solid]),
            vec![0, 16]
        );
        assert!(occupied_section_indices(&[air]).is_empty());
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

    /// wave 61 BK-2: 厳格復元の新規則 — (a) 末尾ゴミ拒否、(b) Σcount≠VOLUME 拒否。
    #[test]
    fn from_bytes_rejects_trailing_garbage_and_bad_sum() {
        let sec: SectionPalette = [3u16; VOLUME];
        let bytes = RleSection::encode(&sec).to_bytes(); // 1 run (3, 4096)
                                                         // 末尾に 1B 追加 → 厳格一致で拒否
        let mut padded = bytes.clone();
        padded.push(0);
        assert!(RleSection::from_bytes(&padded).is_none(), "末尾ゴミは拒否");
        // count を 4096 → 4095 に改竄 (Σ 不足) → 拒否
        // (wire 配置: [run_count:u16][block:u16][count:u16] の LE 2B = index 4,5)
        let mut tampered = bytes.clone();
        tampered[4] = 0xFF;
        tampered[5] = 0x0F; // count = 0x0FFF = 4095
        assert!(
            RleSection::from_bytes(&tampered).is_none(),
            "Σ=4095 (<4096) は拒否"
        );
        // count を超過側に改竄 (2 run 構成で Σ=4097) → 拒否
        let mut over = Vec::new();
        over.extend_from_slice(&2u16.to_le_bytes());
        over.extend_from_slice(&3u16.to_le_bytes());
        over.extend_from_slice(&(VOLUME as u16).to_le_bytes());
        over.extend_from_slice(&3u16.to_le_bytes());
        over.extend_from_slice(&1u16.to_le_bytes());
        assert!(RleSection::from_bytes(&over).is_none(), "Σ>4096 は拒否");
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
}

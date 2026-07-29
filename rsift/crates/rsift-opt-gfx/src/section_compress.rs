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
        // 【wave 177 FW-2 truth 化】旧記述「If RLE is worse than raw」は不正確:
        // RLE stored bytes = 2 + 4·runs、raw = 2·VOL = 8192B。本条件
        // runs·4 ≥ VOL (=4096) は RLE bytes ≥ 4098 (raw の約半分) の時点で
        // LZ4 へ**保守的に早期フォールバック**するもので、真の break-even
        // (RLE ≥ raw ⇔ runs ≥ 2048) ではない。挙動は維持 (ヒューリスティック
        // 設計は据え置き、記述のみ真実へ)。
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

    /// Air 判定。【wave 177 FW-1 実消費配線】world_column_store::column_for_mesh
    /// が非 air のみ decode する最適化に使用中。truth 補題: is_air()=true ⟹
    /// decode()≡[0;VOL] (Single(0) 定義・Rle is_empty()=全 run block 0)。
    /// Lz4 ⇒ 非 air は encode 不変条件 (encode_hot/encode_cold は一様を必ず
    /// Single に逃がす) 依存 — 破損 LZ4 の decode は unwrap_or_default で
    /// 全 0 を生成し得るが、それは decode 側の静寂空気化であり本判定とは別経路。
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

    /// 【wave 177 FW】variant 選択の厳密 pin (rq fw_section 機械導出):
    /// 一様→Single・1 差分 (idx(1,2,3)=801) → Rle runs=3・stored_bytes=14、
    /// fallback 境界 runs=1022 (1022*4=4088<4096) → Rle 維持・
    /// runs=1024 (1024*4=4096>=4096) → Lz4。green-today。
    #[test]
    fn fw_encode_variant_selection_exact() {
        // 一様 → Single + stored_bytes = 2
        let single = CompactSection::encode_hot(&[7u16; VOL]);
        assert!(matches!(single, CompactSection::Single(7)));
        assert_eq!(single.stored_bytes(), 2);
        // 1 値差分 → Rle: runs=[{0,801},{9,1},{0,3294}] (idx(1,2,3)=801)
        let mut p = [0u16; VOL];
        p[idx(1, 2, 3)] = 9;
        let c = CompactSection::encode_hot(&p);
        let r = match &c {
            CompactSection::Rle(r) => r,
            _ => panic!("expect Rle"),
        };
        assert_eq!(r.runs.len(), 3);
        assert_eq!((r.runs[0].block, r.runs[0].count), (0, 801));
        assert_eq!((r.runs[1].block, r.runs[1].count), (9, 1));
        assert_eq!((r.runs[2].block, r.runs[2].count), (0, (VOL - 802) as u16));
        assert_eq!(c.stored_bytes(), 2 + 3 * 4);
        assert_eq!(c.decode(), p);
        // 境界マイナス側: 交互 1022 ワード + 残り 1 → runs=1022 (Σ=1021+3075=4096)
        let mut near = [0u16; VOL];
        for (i, v) in near.iter_mut().enumerate() {
            *v = if i < 1022 { (i % 2) as u16 } else { 1 };
        }
        assert!(
            matches!(CompactSection::encode_hot(&near), CompactSection::Rle(_)),
            "runs=1022 (4088<4096) は Rle 維持"
        );
        // 境界ちょうど: 交互 1024 ワード → runs=1024 (1024*4=4096>=4096) → Lz4
        let mut at = [0u16; VOL];
        for (i, v) in at.iter_mut().enumerate() {
            *v = if i < 1024 { (i % 2) as u16 } else { 1 };
        }
        let c3 = CompactSection::encode_hot(&at);
        assert!(
            matches!(c3, CompactSection::Lz4(_)),
            "runs=1024 は Lz4 fallback"
        );
        assert_eq!(c3.decode(), at);
    }

    /// 【wave 177 FW-1 数学 pin】is_air()=true ⟹ decode()≡[0;VOL] —
    /// column_for_mesh の decode skip 正当性補題 (全構築 variant で pin、
    /// Lz4 は encode 不変条件により必ず非空気)。green-today。
    #[test]
    fn fw_is_air_truth_lemma() {
        assert!(CompactSection::Single(0).is_air());
        assert_eq!(CompactSection::Single(0).decode(), [0u16; VOL]);
        assert!(!CompactSection::Single(7).is_air());
        let air = CompactSection::Rle(RleSection::encode(&[0u16; VOL]));
        assert!(air.is_air());
        assert_eq!(air.decode(), [0u16; VOL]);
        let mut p = [0u16; VOL];
        p[idx(1, 2, 3)] = 9;
        assert!(!CompactSection::Rle(RleSection::encode(&p)).is_air());
        let mut hi = [0u16; VOL];
        for (i, v) in hi.iter_mut().enumerate() {
            *v = (i % 40) as u16;
        }
        let lz = CompactSection::encode_cold(&hi);
        assert!(matches!(lz, CompactSection::Lz4(_)));
        assert!(!lz.is_air(), "encode 経路 Lz4 は必ず非空気 (一様は Single)");
        assert_eq!(lz.decode(), hi);
    }

    /// 【wave 177 FW】破損 LZ4 decode の現行契約 pin (adversarial 変異 C
    /// 非検出捕捉 = FW-3): 不健全バイト列は unwrap_or_default で空 → 全 0 を
    /// 静寂返却する (panic しない防御設計 — Lz4 wire は to_cold メモリ内
    /// encode 経由のみで破損 wire 到達不能、wave 61 BK fail-loud の対象外)。
    /// unwrap 化変異で本テストが RED になり検出可能。
    #[test]
    fn fw_lz4_decode_corrupt_contract() {
        // 空バッファ → Err → 全 0
        let empty = CompactSection::Lz4(vec![]);
        assert_eq!(empty.decode(), [0u16; VOL]);
        // 巨大 size prefix の欺瞞ワイヤ → Err → 全 0 (panic しない)
        let mut lying = Vec::new();
        lying.extend_from_slice(&(1u32 << 30).to_le_bytes());
        lying.extend_from_slice(&[0xAA; 16]);
        let c = CompactSection::Lz4(lying);
        assert_eq!(c.decode(), [0u16; VOL]);
        // 部分有効: 正当な圧縮の切詰め (cut) も Err → 全 0
        let mut hi = [0u16; VOL];
        for (i, v) in hi.iter_mut().enumerate() {
            *v = (i % 40) as u16;
        }
        let good = match CompactSection::encode_cold(&hi) {
            CompactSection::Lz4(b) => b,
            _ => unreachable!(),
        };
        let cut = CompactSection::Lz4(good[..good.len().saturating_sub(8)].to_vec());
        assert_eq!(cut.decode(), [0u16; VOL]);
    }

    /// 【wave 177 FW】to_cold 遷移の形 pin: Single/Lz4 は clone、Rle は
    /// encode_cold 再構成で Lz4 化、decode roundtrip 保存。green-today。
    #[test]
    fn fw_to_cold_forms() {
        assert!(matches!(
            CompactSection::Single(3).to_cold(),
            CompactSection::Single(3)
        ));
        let mut hi = [0u16; VOL];
        for (i, v) in hi.iter_mut().enumerate() {
            *v = (i % 40) as u16;
        }
        let cold = CompactSection::encode_cold(&hi).to_cold();
        assert!(matches!(cold, CompactSection::Lz4(_)));
        assert_eq!(cold.decode(), hi);
        let mut p = [0u16; VOL];
        p[idx(1, 2, 3)] = 9;
        let rle = CompactSection::encode_hot(&p);
        assert!(matches!(rle, CompactSection::Rle(_)));
        let cold2 = rle.to_cold();
        assert!(
            matches!(cold2, CompactSection::Lz4(_)),
            "Rle→cold は LZ4 化"
        );
        assert_eq!(cold2.decode(), p);
        assert!(matches!(
            CompactSection::encode_hot(&[0u16; VOL]).to_cold(),
            CompactSection::Single(0)
        ));
    }
}

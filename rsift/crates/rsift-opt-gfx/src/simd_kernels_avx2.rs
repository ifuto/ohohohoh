//! カラム OR マスク導出カーネル - greedy メッシュ前段のスラブ走査に使用
//!
//! 歴史的名称 (simd_kernels_avx2) が示す通り x86_64 では AVX2 による
//! ベクトル化を意図した形状だが、実装は処理系の自動ベクトル化に委ねる
//! スカラループである (SSE2 ベースライン / +avx2 指定で AVX2 codegen)。
//! 「100倍高速化」のような数値主張は計測根拠がないため記載しない
//! (2026-07-22 監査の誠実性ルール)。
//!
//! 監査摘出バグ: 旧非 x86_64 fallback は「スラブ全域に any() で 0xFFFF」
//! という x 位置情報を失う別物実装で、クロスアーキテクチャで出力 bit が
//! 食い違った。全アーキテクチャ同一出力の単一実装に統一した
//! (x86_64 経路の出力は一切不変 — ループ自体が同形)。

#[derive(Debug, Clone, Copy, Default)]
pub struct SimdAvx2Config {
    pub enabled: bool,
}

/// セクションパレット (i = x + 16y + 256z) の非零セルを
/// (z, x) のビットへ OR 還元したカラムマスクを返す。
/// y は縮約される: masks[z] の bit x が立つ ⟺ ∃y. palette[x+16y+256z] != 0。
/// 出力は全プラットフォームで bit 同一。
pub fn greedy_mask_avx2(palette: &[u16; 4096]) -> [u32; 16] {
    let mut masks = [0u32; 16];
    for z in 0..16 {
        let mut m = 0u32;
        for y in 0..16 {
            for x in 0..16 {
                if palette[x + y * 16 + z * 256] != 0 {
                    m |= 1u32 << x;
                }
            }
        }
        masks[z] = m;
    }
    masks
}

/// カラムマスクの (x, z) ビットを参照する。mask は y 縮約済みのため
/// y 引数は呼び出し側の (x, y, z) 対称性のためにのみ存在する (値に依らない)。
/// z >= 16 (セクション外) は常に false。
pub fn face_visible_bitmask(masks: &[u32; 16], x: usize, _y: usize, z: usize) -> bool {
    if z >= 16 {
        return false;
    }
    (masks[z] & (1u32 << x)) != 0
}

#[cfg(test)]
mod strict_tests {
    use super::*;

    /// テスト側の独立オラクル: 同じ論理を別のループ形状 (x 外周・z for で
    /// 直接参照) で再計算する。実装側ループの添字ミスを構造的に拾う。
    fn spec_masks(palette: &[u16; 4096]) -> [u32; 16] {
        let mut out = [0u32; 16];
        for z in 0..16usize {
            for x in 0..16usize {
                let mut any = false;
                for y in 0..16usize {
                    any |= palette[x + y * 16 + z * 256] != 0;
                }
                if any {
                    out[z] |= 1u32 << x;
                }
            }
        }
        out
    }

    #[test]
    fn empty_and_full_palettes() {
        let zero = [0u16; 4096];
        assert_eq!(greedy_mask_avx2(&zero), [0u32; 16], "全 air は全マスク 0");
        let mut full = [1u16; 4096];
        full[123] = 7;
        full[4095] = u16::MAX;
        assert_eq!(greedy_mask_avx2(&full), [0xFFFFu32; 16], "全充填は全ビット立つ");
        // Config はデフォルト無効 (opt-in 設計)
        assert!(!SimdAvx2Config::default().enabled);
        let cfg = SimdAvx2Config { enabled: true };
        let copied = cfg; // Copy
        assert!(copied.enabled && cfg.enabled);
    }

    #[test]
    fn single_voxel_sets_exactly_one_bit() {
        for &(x, y, z) in &[(0usize, 0usize, 0usize), (9, 3, 7), (15, 15, 15), (8, 0, 15)] {
            let mut p = [0u16; 4096];
            p[x + y * 16 + z * 256] = 1;
            let m = greedy_mask_avx2(&p);
            for zi in 0..16usize {
                let expect = if zi == z { 1u32 << x } else { 0 };
                assert_eq!(m[zi], expect, "({x},{y},{z}): z={zi} のマスク");
            }
        }
    }

    #[test]
    fn fuzz_matches_independent_spec() {
        let mut p = [0u16; 4096];
        for (i, v) in p.iter_mut().enumerate() {
            // 決定的擬似ランダム (splitmix 風) で ~3/7 を非零に
            let h = (i as u64)
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .rotate_left(17);
            *v = if h % 7 < 3 { (i % 500) as u16 + 1 } else { 0 };
        }
        assert_eq!(greedy_mask_avx2(&p), spec_masks(&p), "全 4096 セル任意配置で厳密一致");
    }

    #[test]
    fn face_visible_bitmask_mapping_and_z_guard() {
        let mut p = [0u16; 4096];
        p[3 + 4 * 16 + 5 * 256] = 9; // (3,4,5)
        p[10 + 0 * 16 + 5 * 256] = 1; // 同じ z=5 の別 x
        let m = greedy_mask_avx2(&p);
        assert!(face_visible_bitmask(&m, 3, 999, 5), "y は縮約済みなので値に依らない");
        assert!(face_visible_bitmask(&m, 10, 0, 5));
        assert!(!face_visible_bitmask(&m, 4, 4, 5), "空セルは false");
        assert!(!face_visible_bitmask(&m, 3, 4, 4), "別 z は false");
        assert!(!face_visible_bitmask(&m, 3, 4, 16), "z==16 は範囲外ガード");
        assert!(!face_visible_bitmask(&m, 3, 4, usize::MAX), "巨大 z でもパニックしない");
    }
}

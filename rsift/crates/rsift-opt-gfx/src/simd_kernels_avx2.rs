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
/// 【wave 131 EE-1】x >= 16 も常に false: 旧実装は x 未ガードで
/// `1u32 << x` が **x >= 32 で debug パニック / release では静寂に
/// ビット巻付き (x % 32) → bit 0 立ちの mask に対し誤 true** を返し得た
/// (z ガードとの非対称)。x ∈ [16, 32) では旧結果も false (masks の
/// 実効ビットは 0..16 のみ) であり、本ガードの挙動変更域は x >= 32 の
/// み (debug panic/巻付き → 決定的 false。M-4/DU-5 系の堅牢化と同型)。
pub fn face_visible_bitmask(masks: &[u32; 16], x: usize, _y: usize, z: usize) -> bool {
    if z >= 16 || x >= 16 {
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
        assert_eq!(
            greedy_mask_avx2(&full),
            [0xFFFFu32; 16],
            "全充填は全ビット立つ"
        );
        // Config はデフォルト無効 (opt-in 設計)
        assert!(!SimdAvx2Config::default().enabled);
        let cfg = SimdAvx2Config { enabled: true };
        let copied = cfg; // Copy
        assert!(copied.enabled && cfg.enabled);
    }

    #[test]
    fn single_voxel_sets_exactly_one_bit() {
        for &(x, y, z) in &[
            (0usize, 0usize, 0usize),
            (9, 3, 7),
            (15, 15, 15),
            (8, 0, 15),
        ] {
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
        assert_eq!(
            greedy_mask_avx2(&p),
            spec_masks(&p),
            "全 4096 セル任意配置で厳密一致"
        );
    }

    #[test]
    fn face_visible_bitmask_mapping_and_z_guard() {
        let mut p = [0u16; 4096];
        p[3 + 4 * 16 + 5 * 256] = 9; // (3,4,5)
        p[10 + 0 * 16 + 5 * 256] = 1; // 同じ z=5 の別 x
        let m = greedy_mask_avx2(&p);
        assert!(
            face_visible_bitmask(&m, 3, 999, 5),
            "y は縮約済みなので値に依らない"
        );
        assert!(face_visible_bitmask(&m, 10, 0, 5));
        assert!(!face_visible_bitmask(&m, 4, 4, 5), "空セルは false");
        assert!(!face_visible_bitmask(&m, 3, 4, 4), "別 z は false");
        assert!(!face_visible_bitmask(&m, 3, 4, 16), "z==16 は範囲外ガード");
        assert!(
            !face_visible_bitmask(&m, 3, 4, usize::MAX),
            "巨大 z でもパニックしない"
        );
    }

    #[test]
    fn face_visible_bitmask_x_guard_and_wraparound_regression() {
        // EE-1: x >= 16 は常に false (z ガードとの対称性)。
        // 旧実装は x >= 32 で debug panic / release 静寂巻付き — 本 pin の
        // x=32/64 ケースは旧実装で panic (=回帰検出)。masks の実効ビット
        // 0..16 域からの逸脱は 16..32 で旧実装と結果一致 (false) も確認。
        let mut p = [0u16; 4096];
        p[0 + 0 * 16 + 5 * 256] = 1; // (0,0,5): bit0 が立つ mask
        let m = greedy_mask_avx2(&p);
        for x in [16usize, 17, 31] {
            assert!(
                !face_visible_bitmask(&m, x, 0, 5),
                "x={x}: セクション外は false"
            );
        }
        for x in [32usize, 33, 64, usize::MAX] {
            assert!(
                !face_visible_bitmask(&m, x, 0, 5),
                "x={x}: 旧リリース巻付き域 (bit0 ⟹ 誤 true) も panic せず false"
            );
        }
        assert!(face_visible_bitmask(&m, 0, 0, 5), "有効域は不変");
    }
}

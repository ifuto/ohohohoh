//! Morton Order (Z-curve) - 隣接アクセス局所性向上
//!
//! 3D は BMI2 PDEP/PEXT のランタイム動的検出 (`morton_encode_3d_fast` /
//! `morton_decode_3d_fast`、SWAR との全入力 bitwise 一致を差分ファズ済) と
//! ポータブル SWAR ビット拡張/収縮を提供する。**2D に BMI2 経路は存在しない**
//! (SWAR のみ — 32+32=64 bit で丁度埋まるため)。
//!
//! ## ドメイン契約 (DH-1: wave 108 で公表)
//! - 3D: **各成分 10 bit (0..=1023) 有効** — 超過上位 bit は**静寂切捨て**
//!   (`& 0x3FF`)。3×10=30 bit で u64 に収まる設計。呼出側は `morton_encode_3d`
//!   系の使用前に意図を明示 (例: `& 1023`) すること。
//! - 2D: 各成分 32 bit 全ドメイン (32+32=64 bit で丁度)。往復は全 u32 で恒等。
//! - 負座標: `i32 as u32` で wrap してから切捨てる (例: -1 → 1023 = Z 曲線の
//!   反対側高角)。負値の局所性保存を意図するなら呼出側で offset せよ。

/// 10 bit → 30 bit 3-splay。**入力は `& 0x3FF` で静寂切捨て** (DH-1 契約)。
#[inline(always)]
pub fn split_by_3(a: u32) -> u64 {
    // 10bit -> 30bitへ3bit毎に拡張
    let mut x = a as u64 & 0x3FF;
    x = (x | x << 16) & 0x030000FF;
    x = (x | x << 8) & 0x0300F00F;
    x = (x | x << 4) & 0x030C30C3;
    x = (x | x << 2) & 0x09249249;
    x
}

/// 3-splay の逆変換。出力は `& 0x3FF` (= split_by_3 の切捨てドメインと対称)。
#[inline(always)]
pub fn compact_by_3(mut x: u64) -> u32 {
    x &= 0x09249249;
    x = (x | (x >> 2)) & 0x030C30C3;
    x = (x | (x >> 4)) & 0x0300F00F;
    x = (x | (x >> 8)) & 0x030000FF;
    x = (x | (x >> 16)) & 0x3FF;
    x as u32
}

/// 32 bit → 64 bit 2-splay (全ドメイン、切捨てなし)。
#[inline(always)]
pub fn split_by_2(a: u32) -> u64 {
    let mut x = a as u64 & 0xFFFFFFFF;
    x = (x | (x << 16)) & 0x0000FFFF0000FFFF;
    x = (x | (x << 8))  & 0x00FF00FF00FF00FF;
    x = (x | (x << 4))  & 0x0F0F0F0F0F0F0F0F;
    x = (x | (x << 2))  & 0x3333333333333333;
    x = (x | (x << 1))  & 0x5555555555555555;
    x
}

#[inline(always)]
pub fn compact_by_2(mut x: u64) -> u32 {
    x &= 0x5555555555555555;
    x = (x | (x >> 1))  & 0x3333333333333333;
    x = (x | (x >> 2))  & 0x0F0F0F0F0F0F0F0F;
    x = (x | (x >> 4))  & 0x00FF00FF00FF00FF;
    x = (x | (x >> 8))  & 0x0000FFFF0000FFFF;
    x = (x | (x >> 16)) & 0xFFFFFFFF;
    x as u32
}

#[inline(always)]
pub fn morton_encode_3d(x: u32, y: u32, z: u32) -> u64 {
    split_by_3(x) | (split_by_3(y) << 1) | (split_by_3(z) << 2)
}

#[inline(always)]
pub fn morton_decode_3d(m: u64) -> [u32; 3] {
    [
        compact_by_3(m),
        compact_by_3(m >> 1),
        compact_by_3(m >> 2),
    ]
}

#[inline(always)]
pub fn morton_encode_2d(x: u32, y: u32) -> u64 {
    split_by_2(x) | (split_by_2(y) << 1)
}

#[inline(always)]
pub fn morton_decode_2d(m: u64) -> [u32; 2] {
    [
        compact_by_2(m),
        compact_by_2(m >> 1),
    ]
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "bmi2")]
unsafe fn morton_encode_3d_bmi2_impl(x: u32, y: u32, z: u32) -> u64 {
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::_pdep_u64;
    #[cfg(target_arch = "x86")]
    use std::arch::x86::_pdep_u32;

    #[cfg(target_arch = "x86_64")]
    {
        let mx = _pdep_u64(x as u64, 0x09249249);
        let my = _pdep_u64(y as u64, 0x09249249 << 1);
        let mz = _pdep_u64(z as u64, 0x09249249 << 2);
        mx | my | mz
    }
    #[cfg(target_arch = "x86")]
    {
        let mx = _pdep_u32(x, 0x09249249) as u64;
        let my = _pdep_u32(y, 0x09249249 << 1) as u64;
        let mz = _pdep_u32(z, 0x09249249 << 2) as u64;
        mx | my | mz
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "bmi2")]
unsafe fn morton_decode_3d_bmi2_impl(m: u64) -> [u32; 3] {
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::_pext_u64;
    #[cfg(target_arch = "x86")]
    use std::arch::x86::_pext_u32;

    #[cfg(target_arch = "x86_64")]
    {
        [
            _pext_u64(m, 0x09249249) as u32,
            _pext_u64(m, 0x09249249 << 1) as u32,
            _pext_u64(m, 0x09249249 << 2) as u32,
        ]
    }
    #[cfg(target_arch = "x86")]
    {
        [
            _pext_u32(m as u32, 0x09249249),
            _pext_u32(m as u32, 0x09249249 << 1),
            _pext_u32(m as u32, 0x09249249 << 2),
        ]
    }
}

/// Dynamic dispatching Morton 3D encode (uses PDEP when hardware supports BMI2).
/// ドメインは 10 bit/成分 (超過上位 bit 切捨て、DH-1) で `morton_encode_3d` と
/// 全入力 bitwise 一致 (差分ファズ証明)。ホットループ用に #[inline] 指定。
#[inline]
pub fn morton_encode_3d_fast(x: u32, y: u32, z: u32) -> u64 {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if std::is_x86_feature_detected!("bmi2") {
            return unsafe { morton_encode_3d_bmi2_impl(x, y, z) };
        }
    }
    morton_encode_3d(x, y, z)
}

/// Dynamic dispatching Morton 3D decode (uses PEXT when hardware supports BMI2)。
/// `morton_decode_3d` と全入力 bitwise 一致 (差分ファズ証明)。消費者ゼロの生存確認
/// (DH-5): 削除せずユーティリティとして保持。
#[inline]
pub fn morton_decode_3d_fast(m: u64) -> [u32; 3] {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if std::is_x86_feature_detected!("bmi2") {
            return unsafe { morton_decode_3d_bmi2_impl(m) };
        }
    }
    morton_decode_3d(m)
}

/// `morton_encode_3d_fast` への別名 (消費者ゼロの生存確認: 削除せず保持)。
/// 名称は誤解を招く (DH-3): x86_64 でも BMI2 非対応 CPU では実行時検出で SWAR
/// へ落ち、非 x86_64 では dispatch 自体が無い。厳密固定 dispatch が要る場合は
/// 直接 `morton_encode_3d_fast` を使うこと。
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn morton_encode_bmi2(x: u32, y: u32, z: u32) -> u64 {
    morton_encode_3d_fast(x, y, z)
}

/// 上アーキ向けフォールバック (dispatch なし、SWAR 固定)。
#[cfg(not(target_arch = "x86_64"))]
#[inline]
pub fn morton_encode_bmi2(x: u32, y: u32, z: u32) -> u64 {
    morton_encode_3d(x, y, z)
}

/// Compatibility tag struct for wiring and compile verification
/// (消費者ゼロ scaffold — 消費者追加方針で保持、機能本体ではない)。
pub struct MortonOrderTest;

/// Linear Z-order storage grid for cache-friendly chunk block lookups.
/// 座標は各軸 SIZE (2 の冪、≤ 1024) 未満 — 10 bit ドメイン内で encode は
/// 0..SIZE³ の全単射 (差分ファズで SIZE=16 全 4,096 セル検証)。消費者ゼロの
/// 生存確認: 削除せず保持。SIZE=1024 は T=u32 で 4 GiB 級の確保になる実用上の
/// 注意を明記 (実メモリ予算の消費者側責任)。
pub struct MortonGrid3D<T: Copy + Default, const SIZE: usize> {
    data: Vec<T>,
}

impl<T: Copy + Default, const SIZE: usize> MortonGrid3D<T, SIZE> {
    pub fn new() -> Self {
        assert!(SIZE <= 1024, "Grid dimension too large for 30-bit Morton limit");
        assert!(SIZE.is_power_of_two(), "SIZE must be a power of two");
        let total = SIZE * SIZE * SIZE;
        Self {
            data: vec![T::default(); total],
        }
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> Option<&T> {
        if x >= SIZE || y >= SIZE || z >= SIZE {
            return None;
        }
        let m = morton_encode_3d_fast(x as u32, y as u32, z as u32) as usize;
        self.data.get(m)
    }

    #[inline]
    pub fn set(&mut self, x: usize, y: usize, z: usize, value: T) -> bool {
        if x >= SIZE || y >= SIZE || z >= SIZE {
            return false;
        }
        let m = morton_encode_3d_fast(x as u32, y as u32, z as u32) as usize;
        if let Some(slot) = self.data.get_mut(m) {
            *slot = value;
            true
        } else {
            false
        }
    }

    pub fn as_slice(&self) -> &[T] {
        &self.data
    }
}

/// Z-order 局所性に並べ替える index 列を返す (安定 sort: 同一キーは入力順保持)。
/// 負座標は `& 1023` で wrap (例: -1 → 1023、DH-4 契約)。
/// 消費者ゼロの生存確認: 削除せず保持。
///
/// DH-5 効率化: キーを前計算して比較毎の再評価 (O(n log n) 回) を O(n) へ削減。
/// 安定性を含む新旧完全一致は差分ファズ (64 種ランダム入力、負座標衝突多発構成)
/// で証明済。
pub fn morton_sort_indices(positions: &[[i32; 3]]) -> Vec<usize> {
    let mut pairs: Vec<(u64, usize)> = positions
        .iter()
        .enumerate()
        .map(|(i, p)| {
            (
                morton_encode_3d_fast(p[0] as u32 & 1023, p[1] as u32 & 1023, p[2] as u32 & 1023),
                i,
            )
        })
        .collect();
    pairs.sort_by_key(|&(k, _)| k);
    pairs.into_iter().map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_3d() {
        for x in [0, 1, 15, 31, 63, 1023] {
            for y in [0, 3, 16, 100] {
                for z in [0, 7, 55, 999] {
                    let m = morton_encode_3d_fast(x, y, z);
                    let [dx, dy, dz] = morton_decode_3d_fast(m);
                    assert_eq!([x, y, z], [dx, dy, dz], "Failed 3D roundtrip at {}, {}, {}", x, y, z);
                }
            }
        }
    }

    #[test]
    fn test_encode_decode_2d() {
        for x in [0, 1, 100, 3000, 65535] {
            for y in [0, 5, 200, 12345] {
                let m = morton_encode_2d(x, y);
                let [dx, dy] = morton_decode_2d(m);
                assert_eq!([x, y], [dx, dy]);
            }
        }
    }

    #[test]
    fn test_morton_grid() {
        let mut grid = MortonGrid3D::<u32, 16>::new();
        grid.set(3, 5, 12, 999);
        assert_eq!(grid.get(3, 5, 12), Some(&999));
        assert_eq!(grid.get(0, 0, 0), Some(&0));
    }
}

#[cfg(test)]
mod strict_tests {
    use super::*;

    /// DH-1 契約ピン (adversarial (a) 標的): 3D encode は 10 bit 超を**静寂切捨て**。
    /// encode(x) の値は x の低 10 bit で完全に決まる (上位は観測に出ない)。
    #[test]
    fn domain_truncation_pin() {
        assert_eq!(morton_encode_3d(1024, 0, 0), morton_encode_3d(0, 0, 0));
        assert_eq!(
            morton_encode_3d(1025, 2048, 4099),
            morton_encode_3d_fast(1, 0, 3)
        );
        assert_eq!(split_by_3(u32::MAX), split_by_3(0x3FF));
        assert_eq!(compact_by_3(split_by_3(u32::MAX)), 0x3FF);
        // decode 出力も 10 bit に留まる (対称ドメイン)
        let [a, b, c] = morton_decode_3d(u64::MAX);
        assert!([a, b, c].iter().all(|&v| v <= 0x3FF));
    }

    /// 1D split/compact のドメイン内完全往復 (全 1,024 値) + 2D 32 bit 全ドメイン
    /// 往復エッジピン (差分ファズの縮小決定版)。
    #[test]
    fn one_d_and_two_d_roundtrip_sweeps() {
        for v in 0..1024u32 {
            assert_eq!(compact_by_3(split_by_3(v)), v);
        }
        for v in (0..u32::MAX).step_by(1 << 18) {
            assert_eq!(compact_by_2(split_by_2(v)), v, "split2 rt v={v}");
        }
        for (x, y) in [
            (0u32, 0),
            (1, u32::MAX),
            (u32::MAX, 1),
            (65_535, 65_536),
            (u32::MAX, u32::MAX),
        ] {
            assert_eq!(morton_decode_2d(morton_encode_2d(x, y)), [x, y]);
        }
    }

    /// DH-1 3D roundtrip は 10 bit ドメイン内で恒等 (fast/base 両経路)。
    #[test]
    fn three_d_inner_domain_roundtrip_grid() {
        for z in (0..1024u32).step_by(113) {
            for y in (0..1024u32).step_by(97) {
                for x in (0..1024u32).step_by(89) {
                    let m = morton_encode_3d(x, y, z);
                    assert_eq!(morton_decode_3d(m), [x, y, z]);
                    assert_eq!(m, morton_encode_3d_fast(x, y, z), "base!=fast");
                }
            }
        }
    }

    /// DH-4 契約ピン: 負座標は `& 1023` で wrap (例: -1 → 1023)。
    /// 局所性が「非負 10 bit 座標への事前 offset 正規化を消費者へ要求」する契約。
    #[test]
    fn negative_coord_wrap_contract_pin() {
        assert_eq!(((-1i32) as u32) & 1023, 1023);
        assert_eq!(
            morton_encode_3d((-1i32 as u32) & 1023, 0, 0),
            morton_encode_3d(1023, 0, 0)
        );
        // wrap alias は sort でも厳密再現 (adversarial 標的: マスク変更で RED)
        assert_eq!(morton_sort_indices(&[[-1, 0, 0], [1023, 0, 0]]), vec![0, 1]);
    }

    /// DH-7 相互等価ピン: 3 系統 Morton (本モジュール / `lbvh::morton3` /
    /// WGSL `cs_morton` (frame_worldgen で精密ミラー検証済)) の発散抑止。
    /// lbvh 側 part1by2 も 10 bit 切捨てを内包 → 全 u32 ドメインで bitwise 一致。
    #[test]
    fn lbvh_morton3_equivalence_pin() {
        let mut v: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            v ^= v >> 12;
            v ^= v << 25;
            v ^= v >> 27;
            v.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        for &x in [0u32, 1, 1023, 1024, 65535, u32::MAX].iter() {
            for &y in [0u32, 1023].iter() {
                assert_eq!(
                    crate::lbvh::morton3(x, y, x ^ y) as u64,
                    morton_encode_3d(x, y, x ^ y),
                    "lbvh!=module at edges ({x},{y})"
                );
            }
        }
        for z in (0..1024u32).step_by(29) {
            for y in (0..1024u32).step_by(31) {
                for x in (0..1024u32).step_by(37) {
                    assert_eq!(
                        crate::lbvh::morton3(x, y, z) as u64,
                        morton_encode_3d(x, y, z)
                    );
                }
            }
        }
        for _ in 0..200_000u32 {
            let (x, y, z) = (next() as u32, next() as u32, next() as u32);
            assert_eq!(
                crate::lbvh::morton3(x, y, z) as u64,
                morton_encode_3d(x, y, z),
                "rand wrap"
            );
        }
    }

    /// DH-5 安定性ピン (adversarial (b) 標的): 同一キー入力は入力順を保持。
    /// 1023 と -1 が同一キーに衝突する構成 (wrap 契約を衝突源として意図的に利用)。
    #[test]
    fn sort_stability_collision_pin() {
        let positions = [
            [5, 0, 0],    // key k5
            [1023, 0, 0], // key k_hi
            [-1, 0, 0],   // wrap → k_hi に衝突 (2 番目と同キー)
            [1, 0, 0],
            [5, 8, 0], // 5 系別 key
        ];
        let idx = morton_sort_indices(&positions);
        // [5,0,0](k5),[1,0,0],… 昇順。k_hi 衝突の (1番, 2番) は入力順。
        let pos_of = |i: usize| idx.iter().position(|&v| v == i).unwrap();
        assert!(
            pos_of(1) < pos_of(2),
            "同一キー (1023 系) は入力順保持が契約"
        );
        assert!(pos_of(3) < pos_of(0), "k1 < k5");
        assert!(pos_of(0) < pos_of(1), "k5 系 < k_hi 系");
        assert_eq!(idx.len(), 5);
    }

    /// Grid 契約: 2 の冪強制・範囲外 None/false・small SIZE の全単射往復。
    #[test]
    fn grid_contract_pin() {
        let mut g = MortonGrid3D::<u32, 4>::new();
        let mut codes = std::collections::HashSet::new();
        for z in 0..4usize {
            for y in 0..4 {
                for x in 0..4 {
                    let v = (x + y * 4 + z * 16) as u32;
                    assert!(g.set(x, y, z, v));
                    assert!(codes.insert(morton_encode_3d_fast(x as u32, y as u32, z as u32)));
                }
            }
        }
        assert_eq!(codes.len(), 64);
        assert_eq!(g.get(2, 3, 1), Some(&30)); // v = x + y*4 + z*16 = 2+12+16
        assert!(!g.set(4, 0, 0, 0) && g.get(0, 4, 0).is_none());
    }

    /// SIZE=非 2 の冪は assert fail-loud (doc 契約の機械執行)。
    #[test]
    #[should_panic(expected = "SIZE must be a power of two")]
    fn grid_rejects_non_power_of_two() {
        let _ = MortonGrid3D::<u32, 3>::new();
    }
}

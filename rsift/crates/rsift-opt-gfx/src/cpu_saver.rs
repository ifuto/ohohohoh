//! # CPU Overhead Reduction & Cache Optimization Engine (`cpu_saver`)
//!
//! 分岐除去設計の選択プリミティブ群 (`branchless_select_*`) と、
//! ボクセル歩進の符号/軸選択プリミティブ (`BranchlessVoxelStepper`)、
//! ソフトウェア・プリフェッチ (`CacheLinePrefetcher`) からなる低レベル API 面
//! (wave HQ で `LoopTiledVoxelScanner` は不可能証明削除 — 詳細は下記「消費者」節)。
//!
//! ## 誠実性メモ (監査 2026-07-26 DN-1)
//! 旧ヘッダの「分岐予測ミスを完全撲滅する」「4x4x4 L1 キャッシュライン最適化」
//! 「Branchless DDA ボクセル歩進」は強すぎる記述だった。実際には:
//! * 選択はマスク演算 (`(t & m) | (f & !m)`) で書かれており分岐を**ソース上は**
//!   含まないが、最終的な命令選択 (cmov 化されるか) はコンパイラ依存であり、
//!   「完全撲滅」は生成コードの性質を静的に保証できない。
//! * 歩進プリミティブは符号抽出 (`step_direction`) と同値時の軸優先選択
//!   (`advance_axis`) のみを提供し、tmax 更新を含む DDA 本体は
//!   `branchless_dda` 側 (監査済 DE) が担う。
//! * (旧第三弾 `LoopTiledVoxelScanner` のタイル走査は wave HQ で削除 —
//!   voxel=1B 前提が u16 palette で破綻、恩恵消費者なし。下記参照。)
//!
//! ## 消費者 (wave HQ 2026-08-03 §7 根治)
//! **旧 DN-5 (2026-07-26)** は「ワークスペース全体で直接呼出ゼロ・将来ホット
//! ループ向けプリミティブとして保持」としていたが、§7 (2026-07-26) は「消費者
//! ゼロのまま保持」を違反化したため本 wave で根治:
//! * `BranchlessVoxelStepper::{step_direction, advance_axis}`・`branchless_select_*`
//!   ・`CacheLinePrefetcher::prefetch_read` は**本来の意図された消費者**
//!   `branchless_dda` (本モジュール doc が「DDA 本体は branchless_dda 側が担う」と
//!   明記) へ真配線した (step 計算 / branchless_axis 委譲 / inv_dir の INF 選択 /
//!   DDA ループの先行プリフェッチ)。挙動は全て bit 同一・観測等価。
//! * `LoopTiledVoxelScanner::scan_tiled` は**不可能証明削除** (§7 option b): タイル
//!   前提は voxel=1B レイアウト依存だが実際の `SectionPalette` は `u16` (2B/voxel)
//!   で 64 voxel=128B=2 キャッシュライン ≒ 前提破綻。ホット 16³ 走査は順序依存
//!   (メッシュ → digest 拘束) か既にシーケンシャル自動ベクトル化 (`section_all_air`)
//!   で、タイリングが恩恵を与える反復ランダムアクセス走査は現エンジンに存在しない。
//!   恩恵を与える消費者が存在しないことの機械証明上で削除 (git 履歴に残置)。
//!
//! bytemuck 参照は構造体を持たず実使用が無かったため DN-1 で除去済み (警告 1 件根治)。

/// Branchless conditional select (`cond ? true_val : false_val`).
///
/// ## 契約 (監査 2026-07-26 DN-2)
/// mask = `-(cond as i32)` は `cond=true` で `0xFFFF_FFFF`、`false` で `0`。
/// `(true_val & mask) | (false_val & !mask)` は mask 全域のため両側の支持集合が
/// 交差せず、`|` は `+`/`^` と**完全等価** (DK-1 と同型の分離ビット性)。
/// 結果は `cond ? true_val : false_val` と**全入力で厳密一致** (説明のための
/// OR であって選択意味論は if/else と同じ)。パイプライン面の効果は
/// コンパイラの命令選択に委譲されるため本関数が保証するのは**値の同一性**。
#[inline(always)]
pub const fn branchless_select_u32(cond: bool, true_val: u32, false_val: u32) -> u32 {
    // cond == true => mask = !0u32 (0xFFFFFFFF), cond == false => mask = 0u32
    let mask = ((cond as i32).wrapping_neg()) as u32;
    (true_val & mask) | (false_val & !mask)
}

/// `branchless_select_u32` の i32 版。契約は DN-2 と同型 (マスクは i32 全域)。
#[inline(always)]
pub const fn branchless_select_i32(cond: bool, true_val: i32, false_val: i32) -> i32 {
    let mask = (cond as i32).wrapping_neg();
    (true_val & mask) | (false_val & !mask)
}

/// bit 選択による f32 版。
///
/// ## 契約 (監査 2026-07-26 DN-2)
/// `to_bits`/`from_bits` の往復は Rust の安定保証で**bit 厳密**、比較変換を
/// 経由しないため **NaN ペイロード・-0.0・±inf の bit パターンをそのまま保持**
/// する (signaling NaN でも quiet 化しない)。`cond ? true_val : false_val`
/// と to_bits 単位で厳密一致。
#[inline(always)]
pub fn branchless_select_f32(cond: bool, true_val: f32, false_val: f32) -> f32 {
    let t_bits = true_val.to_bits();
    let f_bits = false_val.to_bits();
    let res_bits = branchless_select_u32(cond, t_bits, f_bits);
    f32::from_bits(res_bits)
}

/// DDA 歩進の符号抽出・軸選択プリミティブ (DDA 本体は `branchless_dda`)。
pub struct BranchlessVoxelStepper;

impl BranchlessVoxelStepper {
    /// 符号の方向ステップ。
    ///
    /// ## 契約 (監査 2026-07-26 DN-3)
    /// `dir > 0` → 1、`dir < 0` → -1、それ以外 → 0。
    /// 境界扱い: `+0.0`/`-0.0` → 0 (IEEE では `-0.0 < 0.0` は false)、
    /// **NaN → 0** (両比較 false)、+inf → 1、-inf → -1。
    #[inline(always)]
    pub fn step_direction(dir: f32) -> i32 {
        let is_pos = dir > 0.0;
        let is_neg = dir < 0.0;
        branchless_select_i32(is_pos, 1, branchless_select_i32(is_neg, -1, 0))
    }

    /// 次に進む軸の選択 (tmax 最小値の軸、**同値優先 x → y → z**)。
    ///
    /// ## 契約 (監査 2026-07-26 DN-3)
    /// * 結果は **高々 1 軸の true でちょうどどれか 1 つが必ず true**
    ///   (異常値含む全入力で排他的に 1 軸が選ばれる):
    ///   `step_x = tmx<=tmy && tmx<=tmz`、`step_y = !step_x && tmy<=tmz`、
    ///   `step_z = !step_x && !step_y`。全 NaN では `step_z` (x/y 比較が全て
    ///   false で落ちる安全側)。同値タイ (tmx=tmy<tmz 等) は **x 優先**、
    ///   x が非最小のタイ (tmy=tmz<tmx) は **y 優先**。
    /// * 戻り値は (step_x, step_y, step_z) の順。
    #[inline(always)]
    pub fn advance_axis(tmx: f32, tmy: f32, tmz: f32) -> (bool, bool, bool) {
        let step_x = tmx <= tmy && tmx <= tmz;
        let step_y = !step_x && tmy <= tmz;
        let step_z = !step_x && !step_y;
        (step_x, step_y, step_z)
    }
}

/// Software Cache Line Prefetcher (`_mm_prefetch`).
pub struct CacheLinePrefetcher;

impl CacheLinePrefetcher {
    /// x86/x86_64 では `_mm_prefetch<T0>` を発行、その他アーキテクチャでは no-op。
    ///
    /// ## 契約 (監査 2026-07-26 DN-4)
    /// * プリフェッチは**セマンティクス非観測** (実行結果・値に影響しない)、
    ///   メモリレイテンシ隠蔽のみのヒント。null/無効アドレスでも x86 では
    ///   フォールトしない (アーキテクチャ保証)、本 API は参照生成を伴わない
    ///   raw ポインタ受けのため安全に呼べる。
    /// * 意味論が無いため adversarial 変体 (除去) は**検出不能=中性**。
    #[inline(always)]
    pub fn prefetch_read<T>(ptr: *const T) {
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        unsafe {
            #[cfg(target_arch = "x86")]
            use std::arch::x86::_mm_prefetch;
            #[cfg(target_arch = "x86")]
            use std::arch::x86::_MM_HINT_T0;
            #[cfg(target_arch = "x86_64")]
            use std::arch::x86_64::_mm_prefetch;
            #[cfg(target_arch = "x86_64")]
            use std::arch::x86_64::_MM_HINT_T0;

            _mm_prefetch::<_MM_HINT_T0>(ptr as *const i8);
        }
        #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
        {
            let _ = ptr;
        }
    }
}

/// ソフトウェア・プリフェッチ・インターフェースは [`CacheLinePrefetcher`] 参照。
/// (wave HQ: `LoopTiledVoxelScanner::scan_tiled` は不可能証明削除 — モジュール doc
/// 「消費者 (wave HQ)」節のとおり u16 palette でタイリング前提が破綻し恩恵消費者
/// なし。git 履歴に残置。)

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_branchless_select() {
        assert_eq!(branchless_select_u32(true, 123, 456), 123);
        assert_eq!(branchless_select_u32(false, 123, 456), 456);
        assert_eq!(BranchlessVoxelStepper::step_direction(15.0), 1);
        assert_eq!(BranchlessVoxelStepper::step_direction(-3.0), -1);
        assert_eq!(BranchlessVoxelStepper::step_direction(0.0), 0);
    }

    // ------------------------------------------------------ 監査 2026-07-26 DN追加分

    /// DN-2: select は if/else と**全ケースで厳密一致**し、bit パターン
    /// (NaN ペイロード/-0.0/±inf) を保持する。
    #[test]
    fn select_is_bit_exact_and_preserves_nan_payload() {
        // 決定的 xorshift で 200 組を if/else 参照と厳密照合。
        let mut s = 0x9E3779B97F4A7C15u64;
        let mut rng = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        for _ in 0..200 {
            let a = rng() as u32;
            let b = (rng() >> 32) as u32;
            let cond = rng() & 1 == 1;
            assert_eq!(
                branchless_select_u32(cond, a, b),
                if cond { a } else { b },
                "u32 select 不一致"
            );
            let (ai, bi) = (a as i32, b as i32);
            assert_eq!(
                branchless_select_i32(cond, ai, bi),
                if cond { ai } else { bi },
                "i32 select 不一致"
            );
            let (af, bf) = (f32::from_bits(a), f32::from_bits(b));
            assert_eq!(
                branchless_select_f32(cond, af, bf).to_bits(),
                (if cond { af } else { bf }).to_bits(),
                "f32 select bit 不一致"
            );
        }
        // 厳密 pin: NaN ペイロード保持 (quiet 化しない)。
        let nan_payload = f32::from_bits(0x7FC0_0001);
        assert_eq!(
            branchless_select_f32(true, nan_payload, 1.0).to_bits(),
            0x7FC0_0001,
            "NaN ペイロード bit 保持"
        );
        // -0.0 の bit 保持 (0.0 へ正規化しない)。
        assert_eq!(
            branchless_select_f32(false, 1.0, -0.0).to_bits(),
            0x8000_0000,
            "-0.0 bit 保持"
        );
        // ±inf もそのまま。
        assert_eq!(
            branchless_select_f32(true, f32::INFINITY, 0.0),
            f32::INFINITY
        );
        assert_eq!(
            branchless_select_f32(false, 0.0, f32::NEG_INFINITY),
            f32::NEG_INFINITY
        );
    }

    /// DN-3: step_direction の境界契約厳密 pin (NaN→0、-0.0→0、±inf→±1)。
    #[test]
    fn step_direction_boundary_contract() {
        use BranchlessVoxelStepper as S;
        assert_eq!(S::step_direction(3.25), 1);
        assert_eq!(S::step_direction(f32::MIN_POSITIVE), 1, "正の最小正規数");
        assert_eq!(S::step_direction(-0.5), -1);
        assert_eq!(S::step_direction(-f32::INFINITY), -1);
        assert_eq!(S::step_direction(f32::INFINITY), 1);
        assert_eq!(S::step_direction(0.0), 0);
        assert_eq!(
            S::step_direction(-0.0),
            0,
            "-0.0 は IEEE で -0.0 < 0.0 が false のため 0"
        );
        assert_eq!(
            S::step_direction(f32::NAN),
            0,
            "NaN は両比較 false のため 0 (契約 DN-3)"
        );
    }

    /// DN-3: advance_axis は 343 網羅で**ちょうど 1 軸**が true となり、
    /// 同値タイ優先 (x>y>z) と NaN フォールバックが契約通り。
    #[test]
    fn advance_axis_exactly_one_and_tie_priority() {
        use BranchlessVoxelStepper as S;
        let vals = [-1.0f32, 0.0, -0.0, 1.0, 2.0, f32::NAN, f32::INFINITY];
        for &x in &vals {
            for &y in &vals {
                for &z in &vals {
                    let (sx, sy, sz) = S::advance_axis(x, y, z);
                    assert_eq!(
                        (sx as u8) + (sy as u8) + (sz as u8),
                        1,
                        "ちょうど 1 軸が true: ({x}, {y}, {z})"
                    );
                }
            }
        }
        // 同値タイ → x 優先。
        assert_eq!(S::advance_axis(1.0, 1.0, 2.0), (true, false, false));
        // 3 軸同一 → x 優先。
        assert_eq!(S::advance_axis(2.0, 2.0, 2.0), (true, false, false));
        // x 非最小・y=z タイ → y 優先。
        assert_eq!(S::advance_axis(3.0, 1.0, 1.0), (false, true, false));
        // x 最小・y=z タイも x 優先。
        assert_eq!(S::advance_axis(1.0, 2.0, 2.0), (true, false, false));
        // 全 NaN → z (x/y 比較全 false で落ちる安全側)。
        assert_eq!(
            S::advance_axis(f32::NAN, f32::NAN, f32::NAN),
            (false, false, true)
        );
        // ±inf: 単一有限最小 → その軸。
        assert_eq!(
            S::advance_axis(f32::INFINITY, f32::INFINITY, 1.0),
            (false, false, true)
        );
    }

    /// DN-4: prefetch は null/有効ポインタともに決してフォールトしない。
    #[test]
    fn prefetch_is_fault_free_smoke() {
        let v = [1u8, 2, 3, 4];
        CacheLinePrefetcher::prefetch_read(v.as_ptr());
        CacheLinePrefetcher::prefetch_read(std::ptr::null::<u8>());
        CacheLinePrefetcher::prefetch_read(0xCAFE as *const u64);
    }
}

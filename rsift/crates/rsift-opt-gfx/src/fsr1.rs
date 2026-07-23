//! FSR 1.0 — EASU (Edge-Aware Spatial Upsampling) + RCAS (Robust CAS) の CPU 参照実装。
//!
//! EASU は周辺 4 テクセルの輝度勾配からエッジを検出し、エッジに直交する方向の
//! サンプリング位置を中央へ寄せて（シャープにして）アップサンプリングする。
//! RCAS はその後ろのコントラスト適応シャープ化パス。
//!
//! ## 3連鎖ミラーの勾配チャンネル規約
//! エッジ検出の勾配は **R チャンネルのみ** で計算する。出荷 GPU シェーダ
//! (`shaders/fsr1.wgsl` fsr_easu) がこの規則であり、GPU/CPU 厳密一致のため
//! 本実装と精密ミラー `frame_reference::fsr1_reference` もこれに合わせる
//! (過去このモジュールだけ luma 加重で計算していて 3連鎖が分岐していた
//! — 2026-07-22 wave 17 監査で R に統一)。

//! ## 勾配チャンネル規約と mix 演算順 (3連鎖)
//! エッジ検出の勾配は **R チャンネルのみ** で計算する (出荷 GPU シェーダ規約、
//! wave 17 で統一)。バイリニア合成部の WGSL `mix(a, b, t)` は W3C WGSL
//! 2026-07 CRD 上「linear blend (e.g. `a * (T(1) - t) + b * t`)」と例示定義で
//! あり、精度規定も `x * (1.0 - z) + y * z` からの継承に留まる — gpuweb#3260
//! によりバックエンド実装は差分形 `a + (b - a) * t` も許容される (Vulkan CTS
//! 同等物)。本モジュールと精密ミラー `frame_reference::fsr1_reference` は
//! 差分形を採用するため、GPU 出力とはバイリニア部で ±1-2 ulp の規格上
//! 許容ドリフトがあり得る (u8 量子化後の ±1LSB 統計許容内、
//! frame_reference の doc と同一結論)。勾配・エッジ寄せ・RCAS 部は
//! 全実装で演算順が厳密一致する。

/// EASU の 4-tap エッジ感知再構成。
/// `nw/ne/sw/se` は低解像度グリッドの 2x2 ブロック
/// (`nw`=左上...`se`=右下、y 下向き; WGSL の p00/p10/p01/p11 に対応)。
/// `fx,fy` は低解像度グリッド内の小数位置(0..1)。
///
/// wave 64 BN-2: 旧シグネチャの第 1 引数 `c` (中心画素) は WGSL が
/// 2x2 ブロック 4 サンプルのみを参照する規約と無関係な**死引数**であり、
/// 呼出側に中心画素が必要との誤認を与えていたため撤去 (数学的影響ゼロ)。
///
/// 勾配は R チャンネルのみで、演算順は `shaders/fsr1.wgsl` の
/// `gx = abs((p10.r + p11.r) - (p00.r + p01.r))` /
/// `gy = abs((p00.r + p10.r) - (p01.r + p11.r))` と同一
/// (p00↔nw, p10↔ne, p01↔sw, p11↔se)。
pub fn easu_reconstruct(
    nw: [f32; 3],
    ne: [f32; 3],
    sw: [f32; 3],
    se: [f32; 3],
    fx: f32,
    fy: f32,
) -> [f32; 3] {
    // 水平・垂直の勾配 (R チャンネルのみ — WGSL/mirror 3連鎖規約)。
    let gx = ((ne[0] + se[0]) - (nw[0] + sw[0])).abs();
    let gy = ((nw[0] + ne[0]) - (sw[0] + se[0])).abs();
    // エッジ強度 (0..~1)
    let ex = gx / (gx + 0.5);
    let ey = gy / (gy + 0.5);
    // エッジ沿いではサンプル位置を中心(0.5)に寄せてシャープ化
    let fx2 = fx + (0.5 - fx) * ex;
    let fy2 = fy + (0.5 - fy) * ey;
    let mut out = [0.0f32; 3];
    for i in 0..3 {
        let top = nw[i] + (ne[i] - nw[i]) * fx2;
        let bot = sw[i] + (se[i] - sw[i]) * fx2;
        out[i] = top + (bot - top) * fy2;
    }
    out
}

/// RCAS (Robust Contrast Adaptive Sharpening) — 簡約実装。
/// ラプラシアンに基づきコントラスト適応でシャープ化。
///
/// 符号規約は 3連鎖で統一: `out = p - lap * sharpness` (`lap` = 近傍平均 − 中心)。
/// `p + lap * sharpness` は中心が近傍平均に近づく「ぼかし」であり、
/// `cas.rs::cas_sample` (FidelityFX CAS 準拠) とも整合しない。
/// `shaders/fsr1.wgsl::fsr_rcas` と `frame_reference::fsr1_reference` も
/// この符号で一致している (wave 17 でぼかし符号から修正)。
pub fn rcas(
    p: [f32; 3],
    n: [f32; 3],
    s: [f32; 3],
    e: [f32; 3],
    w: [f32; 3],
    sharpness: f32,
) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for i in 0..3 {
        let lap = ((n[i] + s[i] + e[i] + w[i]) * 0.25) - p[i];
        // CAS は近傍平均から「遠ざける」方向がシャープ化 (加算だとブラーになる)。
        out[i] = (p[i] - lap * sharpness).clamp(0.0, 1.0);
    }
    out
}

pub struct Fsr1 {
    /// RCAS (シャープ化) パスの強度。EASU (再構成) は強度パラメータを
    /// 持たない (WGSL 規約) ため `reconstruct` の出力には影響しない。
    /// `sharpen` / GPU パス `frame_fsr1::GpuFsr1Pass` の uniform が消費する。
    pub sharpness: f32,
}
impl Default for Fsr1 {
    fn default() -> Self {
        Self { sharpness: 0.2 }
    }
}
impl Fsr1 {
    pub fn reconstruct(
        &self,
        nw: [f32; 3],
        ne: [f32; 3],
        sw: [f32; 3],
        se: [f32; 3],
        fx: f32,
        fy: f32,
    ) -> [f32; 3] {
        easu_reconstruct(nw, ne, sw, se, fx, fy)
    }

    /// RCAS パスを `self.sharpness` で適用する。
    /// wave 64 BN-3a: 旧来 `sharpness` は GPU パス (`GpuFsr1Pass`) 側が別系統で
    /// 保持するのみで CPU 構造体では**どのメソッドからも消費されない死に状態**
    /// だった (`Fsr1 { sharpness: 0.2 }` と初期化しても CPU 経路の出力に
    /// 影響しない doc 嘘)。消費者追加方針に従い本メソッドを配線 — CPU でも
    /// EASU + RCAS の完全 2 パス構成が API 上で成立する。
    pub fn sharpen(
        &self,
        p: [f32; 3],
        n: [f32; 3],
        s: [f32; 3],
        e: [f32; 3],
        w: [f32; 3],
    ) -> [f32; 3] {
        rcas(p, n, s, e, w, self.sharpness)
    }

    pub fn wgsl_source(&self) -> &'static str {
        FSR1_WGSL
    }
}

pub const FSR1_WGSL: &str = include_str!("../shaders/fsr1.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_region_returns_input() {
        let same = [0.5; 3];
        let out = easu_reconstruct(same, same, same, same, 0.3, 0.7);
        for i in 0..3 {
            assert!((out[i] - 0.5).abs() < 1e-5, "channel {}", i);
        }
    }

    #[test]
    fn edge_is_preserved_sharp() {
        // 水平エッジ: 左半明、右半暗
        let nw = [1.0, 1.0, 1.0];
        let ne = [0.0, 0.0, 0.0];
        let sw = [1.0, 1.0, 1.0];
        let se = [0.0, 0.0, 0.0];
        // fx が中央付近なら、エッジ強度により 0.5 に近い値になる（過剰ブレンドせず）
        let out = easu_reconstruct(nw, ne, sw, se, 0.5, 0.5);
        assert!(out[0] > 0.3 && out[0] < 0.7, "edge pixel should stay mid: {}", out[0]);
    }

    #[test]
    fn rcas_increases_contrast_on_edge() {
        // 明側の画素が暗い近傍からさらに明へ押し出されるか (CAS の伸長方向)。
        // 明暗対称の近傍だとラプラシアンが 0 に相殺され何も起きないので、
        // 方向が一意に決まる非対称配置で検証する。
        let p = [0.8; 3];
        let dark = [0.2; 3];
        let out = rcas(p, dark, dark, dark, dark, 0.5);
        assert!(out[0] > 0.8, "rcas should push toward bright side: {}", out[0]);
    }

    #[test]
    fn rcas_flat_is_identity() {
        let p = [0.4; 3];
        let out = rcas(p, p, p, p, p, 0.5);
        for i in 0..3 {
            assert!((out[i] - 0.4).abs() < 1e-6);
        }
    }

    /// 勾配は R チャンネルのみで検出する (WGSL/mirror 3連鎖規約)。
    /// R が平坦で G/B だけ変化する近傍では勾配ゼロ → fx そのままの
    /// 純バイリニアに帰着する (旧 luma 版では G/B 変化に反応してしまう)。
    #[test]
    fn easu_gradient_uses_r_channel_only() {
        // R は全tapで 0.5 に固定、G/B だけが左明右暗。
        let nw = [0.5, 1.0, 1.0];
        let ne = [0.5, 0.0, 0.0];
        let sw = [0.5, 1.0, 1.0];
        let se = [0.5, 0.0, 0.0];
        let fx = 0.25;
        let fy = 0.75;
        let out = easu_reconstruct(nw, ne, sw, se, fx, fy);
        // 勾配ゼロ扱い → fx2=fx, fy2=fy の純バイリニアを独立手計算。
        for ch in 0..3 {
            let top = nw[ch] + (ne[ch] - nw[ch]) * fx;
            let bot = sw[ch] + (se[ch] - sw[ch]) * fx;
            let expect = top + (bot - top) * fy;
            assert_eq!(out[ch].to_bits(), expect.to_bits(), "ch {ch}");
        }
    }

    /// R チャンネルにだけ勾配がある場合はエッジ寄せが発動する (G/B 差は無視と
    /// 組み合わせて、R 専用であることの双方向証明)。
    #[test]
    fn easu_r_channel_gradient_triggers_edge_steering() {
        // R は左明右暗 (G/B は全tap同値)。
        let nw = [1.0, 0.4, 0.4];
        let ne = [0.0, 0.4, 0.4];
        let sw = [1.0, 0.4, 0.4];
        let se = [0.0, 0.4, 0.4];
        let fx = 0.25;
        let out = easu_reconstruct(nw, ne, sw, se, fx, 0.5);
        // gx = |(0+0)-(1+1)| = 2 → ex = 2/2.5 = 0.8 → fx2 = 0.25+0.25*0.8 = 0.45。
        // 勾配が無ければ fx2=0.25。寄せ方向への移動を厳密値で固定。
        let expect_fx2 = fx + (0.5 - fx) * (2.0f32 / 2.5);
        let expect_top = nw[0] + (ne[0] - nw[0]) * expect_fx2;
        let expect_bot = sw[0] + (se[0] - sw[0]) * expect_fx2;
        let expect = expect_top + (expect_bot - expect_top) * 0.5;
        assert_eq!(out[0].to_bits(), expect.to_bits());
    }

    /// wave 64 BN-3: EASU 全経路 (勾配 → エッジ応答 → 位置寄せ → 3ch バイリニア)
    /// の厳密ビット固定。期待値はモジュール固定演算順を f32 エミュレーション
    /// (Python struct 往復で各 IEEE 単精度演算を手続き的に丸め) から独立導出
    /// (W-3 教訓: 直感値禁止)。加減乗除のみで構成されるため全プラットフォーム
    /// で決定的 (除算は IEEE 754 で correctly rounded)。
    #[test]
    fn easu_exact_bits_canonical() {
        let nw = [0.25, 0.5, 0.75];
        let ne = [0.5, 0.5, 0.5];
        let sw = [0.75, 0.5, 0.5];
        let se = [1.0, 0.5, 0.5];
        // gx = |(0.5+1.0)-(0.25+0.75)| = 0.5 → ex = 0.5/1.0 = 0.5
        // gy = |(0.25+0.5)-(0.75+1.0)| = 1.0 → ey = 1/1.5 = 0x3f2aaaab
        // fx2 = 0.2+0.3·0.5 = 0.35、fy2 = 0.3+0.2·(1/1.5)
        let out = easu_reconstruct(nw, ne, sw, se, 0.2, 0.3);
        assert_eq!(
            [out[0].to_bits(), out[1].to_bits(), out[2].to_bits()],
            [0x3f0dddde, 0x3f000000, 0x3f1792c6],
            "EASU canonical bit drift: out={out:?}"
        );
    }

    /// wave 64 BN-3: RCAS のクランプ境界両方向の厳密固定。
    /// 上方向: p=0.8, 近傍=0.2, sharpness=0.5 → v=1.1 を 1.0 丁度に。
    /// 下方向: p=0.2, 近傍=0.8, sharpness=0.5 → v=-0.1 を +0.0 丁度に。
    #[test]
    fn rcas_clamp_bounds_are_exact() {
        let dark = [0.2; 3];
        let out = rcas([0.8; 3], dark, dark, dark, dark, 0.5);
        for v in out {
            assert_eq!(v.to_bits(), 0x3f800000, "overshoot must clamp to 1.0");
        }
        let bright = [0.8; 3];
        let out = rcas([0.2; 3], bright, bright, bright, bright, 0.5);
        for v in out {
            assert_eq!(v.to_bits(), 0, "undershoot must clamp to +0.0");
        }
    }

    /// wave 64 BN-3: RCAS 非クランプ域の厳密ビット固定 + チャンネル独立
    /// (クロスチャンネル混入があれば決定的に不一致となる値配置)。
    /// 非クランプ: p=0.8, 近傍=0.2, sharpness=0.25 → f32 エミュレーション導出。
    #[test]
    fn rcas_exact_bits_non_clamped_and_channel_independent() {
        let dark = [0.2; 3];
        let out = rcas([0.8; 3], dark, dark, dark, dark, 0.25);
        for v in out {
            assert_eq!(v.to_bits(), 0x3f733334, "non-clamped RCAS bit drift");
        }
        // チャンネル毎に異なる近傍配置。ch2 はクランプ、ch0/ch1 は非クランプで
        // 共存 (独立導出: ch0 谷深め方向 0.025、ch1 0.575 に相当)。
        let out = rcas(
            [0.1, 0.5, 0.9],
            [0.4, 0.1, 0.2],
            [0.3, 0.2, 0.1],
            [0.2, 0.3, 0.05],
            [0.5, 0.4, 0.15],
            0.3,
        );
        assert_eq!(
            [out[0].to_bits(), out[1].to_bits(), out[2].to_bits()],
            [0x3cccccc8, 0x3f133333, 0x3f800000],
            "per-channel RCAS must not bleed across channels"
        );
    }

    /// wave 64 BN-3a: `Fsr1::sharpen` が構造体の sharpness を実消費すること
    /// (旧来どのメソッドからも消費されなかった死に状態の解消を機械固定)。
    /// sharpness=0 は厳密恒等、0≠0 のとき rcas とビット一致。
    #[test]
    fn sharpen_consumes_struct_sharpness() {
        let p = [0.8; 3];
        let dark = [0.2; 3];
        let identity = Fsr1 { sharpness: 0.0 }.sharpen(p, dark, dark, dark, dark);
        for (i, v) in identity.iter().enumerate() {
            assert_eq!(v.to_bits(), p[i].to_bits(), "sharpness 0 must be identity");
        }
        let s = 0.5f32;
        let via_method = Fsr1 { sharpness: s }.sharpen(p, dark, dark, dark, dark);
        let via_fn = rcas(p, dark, dark, dark, dark, s);
        for i in 0..3 {
            assert_eq!(
                via_method[i].to_bits(),
                via_fn[i].to_bits(),
                "sharpen must delegate to rcas with self.sharpness"
            );
        }
        assert_ne!(
            via_method[0].to_bits(),
            p[0].to_bits(),
            "non-zero sharpness must take effect (dead-state regression)"
        );
    }

    /// wave 64 BN-3a: CPU 構造体既定値と GPU パス既定 uniform のドリフト固定。
    /// frame_fsr1::DEFAULT_SHARPNESS は「full_graph_wiring の Fsr1{0.2} と
    /// 同値」と doc される二重真実源であり、Fsr1::default() を単一真実に
    /// 集約した上で両者の一致を機械保証する。
    #[test]
    fn default_sharpness_matches_gpu_pass_default() {
        assert_eq!(
            Fsr1::default().sharpness.to_bits(),
            crate::frame_fsr1::DEFAULT_SHARPNESS.to_bits(),
            "CPU/GPU default sharpness drift"
        );
    }
}

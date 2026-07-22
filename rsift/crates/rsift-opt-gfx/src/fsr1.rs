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

/// EASU の 4-tap エッジ感知再構成。
/// `c` 中心、`nw/ne/sw/se` は 4 近傍 (`nw`=左上...`se`=右下、y 下向き)。
/// `fx,fy` は低解像度グリッド内の小数位置(0..1)。
///
/// 勾配は R チャンネルのみで、演算順は `shaders/fsr1.wgsl` の
/// `gx = abs((p10.r + p11.r) - (p00.r + p01.r))` /
/// `gy = abs((p00.r + p10.r) - (p01.r + p11.r))` と同一
/// (p00↔nw, p10↔ne, p01↔sw, p11↔se)。
pub fn easu_reconstruct(
    _c: [f32; 3],
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
        c: [f32; 3],
        nw: [f32; 3],
        ne: [f32; 3],
        sw: [f32; 3],
        se: [f32; 3],
        fx: f32,
        fy: f32,
    ) -> [f32; 3] {
        easu_reconstruct(c, nw, ne, sw, se, fx, fy)
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
        let c = [0.5; 3];
        let same = [0.5; 3];
        let out = easu_reconstruct(c, same, same, same, same, 0.3, 0.7);
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
        let out = easu_reconstruct([0.5; 3], nw, ne, sw, se, 0.5, 0.5);
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
        let out = easu_reconstruct([0.5; 3], nw, ne, sw, se, fx, fy);
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
        let out = easu_reconstruct([0.5; 3], nw, ne, sw, se, fx, 0.5);
        // gx = |(0+0)-(1+1)| = 2 → ex = 2/2.5 = 0.8 → fx2 = 0.25+0.25*0.8 = 0.45。
        // 勾配が無ければ fx2=0.25。寄せ方向への移動を厳密値で固定。
        let expect_fx2 = fx + (0.5 - fx) * (2.0f32 / 2.5);
        let expect_top = nw[0] + (ne[0] - nw[0]) * expect_fx2;
        let expect_bot = sw[0] + (se[0] - sw[0]) * expect_fx2;
        let expect = expect_top + (expect_bot - expect_top) * 0.5;
        assert_eq!(out[0].to_bits(), expect.to_bits());
    }
}

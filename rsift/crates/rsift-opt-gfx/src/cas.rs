//! AMD FidelityFX Contrast Adaptive Sharpening (CAS) for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): 公式リファレンス (GPUOpen-Effects/FidelityFX-CAS
//! `ffx-cas/ffx_cas.h` の `CasFilter` no-scaling 5 タップ版、高品質パス
//! `CAS_SLOW` = チャネル毎ウェイト) を忠実に実装する。核は
//!   mn/mx = cross 近傍 + 中心の soft min/max
//!   amp   = sqrt(sat(min(mn, 1 - mx) / mx))
//!   w     = amp * peak        (peak = -1/lerp(8, 5, sharpness) ∈ [-1/8, -1/5])
//!   out   = sat((c + Σcross·w) / (1 + 4w))
//! で、負ローブカーネルが中心画素を局所平均から「遠ざける」(=真正のシャープ化)。
//! 旧実装は `out = c·(1-a) + avg·a` の平均混合で局所コントラストを下げる
//! (ぼかす) 向きであり、CAS ではなかった (2026-07-21 監査で検出・本式へ修正)。
//! 鮮鋭化は TAA/upscale で失われる高周波を回復し、解像度は一切落とさない。

use std::ops::{Add, Mul, Sub};

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    pub fn clamp(self, lo: f32, hi: f32) -> Vec3 {
        Vec3::new(self.x.clamp(lo, hi), self.y.clamp(lo, hi), self.z.clamp(lo, hi))
    }
}
impl Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f32) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}
impl Vec4 {
    pub fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }
}
impl Add for Vec4 {
    type Output = Vec4;
    fn add(self, o: Vec4) -> Vec4 {
        Vec4::new(self.x + o.x, self.y + o.y, self.z + o.z, self.w + o.w)
    }
}
impl Sub for Vec4 {
    type Output = Vec4;
    fn sub(self, o: Vec4) -> Vec4 {
        Vec4::new(self.x - o.x, self.y - o.y, self.z - o.z, self.w - o.w)
    }
}
impl Mul<f32> for Vec4 {
    type Output = Vec4;
    fn mul(self, s: f32) -> Vec4 {
        Vec4::new(self.x * s, self.y * s, self.z * s, self.w * s)
    }
}

/// Rec.709 luma.
pub fn luma(c: Vec3) -> f32 {
    0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z
}

/// 公式 `CasSetup` の sharpness→peak 変換 `sharp = -rcp(lerp(8.0, 5.0, sat(sharpness)))`
/// をそのまま写したもの (ffx_cas.h `CasSetup`: const1.x)。sharpness=0 で -1/8
/// (低リンギングの既定)、1 で -1/5 (最大鮮鋭化)。0.0 でも恒等にはならない点に注意
/// (公式の既定動作。恒等が欲しい経路では pass 自体を skip すること)。
pub fn cas_peak(sharpness: f32) -> f32 {
    let s = sharpness.clamp(0.0, 1.0);
    -1.0 / (8.0 + (5.0 - 8.0) * s)
}

/// 0 除算 NaN ガード用の下限。公式はハードウェアの近似 rcp 挙動に依存して
/// mx=0 (5 タップ全て真っ黒) を暗黙に捌いているが、IEEE 厳密な `1.0 / mx` は
/// +inf となり `min(0, 1) * inf = NaN` になる。CPU/WGSL 双方で同一の
/// `max(mx, MX_FLOOR)` を掛けて決定的に潰す。mx ≤ MX_FLOOR は mn == mx で
/// フィルタが恒等に退化する領域なので、画への影響は無い。
pub const MX_FLOOR: f32 = 1.0e-30;

/// 公式 CAS (no-scaling cross 5 タップ、`CAS_SLOW` 高品質パス) を 1 画素に適用。
/// `center` と 4 近傍 (n/s/e/w) は [0,1] リニア色 (CAS の入力契約)。
/// `sharpness` in [0,1]。`frame_postfx::cas_run_cpu` / `shaders/cas.wgsl` と
/// 演算順が一致するよう保つこと (精密ミラー連鎖)。
pub fn cas_sample(center: Vec3, n: Vec3, s: Vec3, e: Vec3, w: Vec3, sharpness: f32) -> Vec3 {
    // soft min/max: cross (b,d,f,h) + center e。公式は中心を含める
    // (旧実装は中心を除外しており、中心が極値の画素で局所コントラストの
    // 推定が狂っていた)。
    let mn = Vec3::new(
        n.x.min(s.x).min(e.x.min(w.x)).min(center.x),
        n.y.min(s.y).min(e.y.min(w.y)).min(center.y),
        n.z.min(s.z).min(e.z.min(w.z)).min(center.z),
    );
    let mx = Vec3::new(
        n.x.max(s.x).max(e.x.max(w.x)).max(center.x),
        n.y.max(s.y).max(e.y.max(w.y)).max(center.y),
        n.z.max(s.z).max(e.z.max(w.z)).max(center.z),
    );
    let peak = cas_peak(sharpness);
    let mut out = Vec3::new(0.0, 0.0, 0.0);
    for ch in 0..3 {
        // amp = sqrt(sat(min(mn, 1-mx) * rcp(mx)))
        let rcp_m = 1.0 / mx[ch].max(MX_FLOOR);
        let amp = (mn[ch].min(1.0 - mx[ch]) * rcp_m).clamp(0.0, 1.0).sqrt();
        // 負ローブカーネル (w < 0): out = sat((c + (n+s+e+w)·w) / (1+4w))
        let wg = amp * peak;
        let rcp_w = 1.0 / (1.0 + 4.0 * wg);
        out[ch] = ((center[ch] + (n[ch] + s[ch] + e[ch] + w[ch]) * wg) * rcp_w).clamp(0.0, 1.0);
    }
    out
}

// Helper index accessor for Vec3 (used above).
impl std::ops::Index<usize> for Vec3 {
    type Output = f32;
    fn index(&self, i: usize) -> &f32 {
        match i {
            0 => &self.x,
            1 => &self.y,
            _ => &self.z,
        }
    }
}
impl std::ops::IndexMut<usize> for Vec3 {
    fn index_mut(&mut self, i: usize) -> &mut f32 {
        match i {
            0 => &mut self.x,
            1 => &mut self.y,
            _ => &mut self.z,
        }
    }
}

pub fn wgsl_source() -> &'static str {
    CAS_WGSL
}

pub const CAS_WGSL: &str = include_str!("../shaders/cas.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32) -> Vec3 {
        Vec3::new(x, x, x)
    }

    #[test]
    fn flat_region_unchanged() {
        let c = v(0.5);
        for s in [0.0f32, 0.5, 1.0] {
            let out = cas_sample(c, c, c, c, c, s);
            assert!(
                (out.x - 0.5).abs() < 1e-6,
                "flat stays flat (sharpness {s}): {:?}",
                out
            );
        }
    }

    #[test]
    fn edge_center_is_pushed_away_from_mean() {
        // 暗い中心 + 明るい cross 近傍 → 公式 CAS は負ローブで中心を
        // 局所平均から遠ざける (= より暗くする = 真の鮮鋭化)。
        // 旧実装 (平均混合) は逆に明るくしていた (= ぼかし) ため、
        // このテストの方向 assert が新旧を判別する決定打になる。
        let c = v(0.1);
        let n = v(0.9);
        let out = cas_sample(c, n, n, n, n, 0.7);
        assert!(out.x < 0.1, "centre must be pushed darker, got {}", out.x);
        assert!(out.x >= 0.0, "LDR 下限で sat");
    }

    #[test]
    fn matches_gpuopen_reference_formula() {
        // ffx_cas.h `CasFilter` (noScaling, CAS_SLOW = チャネル毎ウェイト)
        // の式をテスト内で独立に再構成し、cas_sample と一致するかを検査
        // (ミラー式の回 regress 検知用)。
        let c = Vec3::new(0.3, 0.6, 0.9);
        let n = Vec3::new(0.2, 0.7, 0.4);
        let s = Vec3::new(0.8, 0.5, 0.1);
        let e = Vec3::new(0.4, 0.3, 0.2);
        let w = Vec3::new(0.9, 0.8, 0.7);
        let sharpness = 0.85f32;
        let peak = -(1.0 / (8.0 - 3.0 * sharpness));
        let mut expect = [0.0f32; 3];
        for ch in 0..3 {
            let mn = n[ch].min(s[ch]).min(e[ch].min(w[ch])).min(c[ch]);
            let mx = n[ch].max(s[ch]).max(e[ch].max(w[ch])).max(c[ch]);
            let a = (mn.min(1.0 - mx) * (1.0 / mx)).clamp(0.0, 1.0).sqrt();
            let wg = a * peak;
            expect[ch] = ((c[ch] + (n[ch] + s[ch] + e[ch] + w[ch]) * wg)
                * (1.0 / (1.0 + 4.0 * wg)))
            .clamp(0.0, 1.0);
        }
        let out = cas_sample(c, n, s, e, w, sharpness);
        for ch in 0..3 {
            assert!(
                (out[ch] - expect[ch]).abs() < 1e-7,
                "ch{ch} out={} expect={}",
                out[ch],
                expect[ch]
            );
        }
    }

    #[test]
    fn pure_black_cross_is_finite_black() {
        // mx=0 (5 タップ全て 0): 公式は近似 rcp の HW 挙動依存。MX_FLOOR
        // ガードにより NaN を出さず 0 を返す。
        let z = v(0.0);
        let out = cas_sample(z, z, z, z, z, 1.0);
        assert!(
            !out.x.is_nan() && out.x == 0.0,
            "pure black stays finite black: {:?}",
            out
        );
    }

    #[test]
    fn extreme_center_is_not_sharpened_further() {
        // 中心が極値 (mn か mx と一致) のとき公式は amp を 0 に潰し、
        // クリッピング (リンギング) を防ぐ。中心 0.0 + 近傍 1.0 → 出力 0.0。
        let c = v(0.0);
        let n = v(1.0);
        let out = cas_sample(c, n, n, n, n, 1.0);
        assert!(
            (out.x - 0.0).abs() < 1e-7,
            "extreme centre must be protected from ringing, got {}",
            out.x
        );
    }

    #[test]
    fn output_stays_in_unit_range() {
        // 擬似乱択でも NaN 無し・[0,1] 内 (入力は CAS 契約通り [0,1] リニア)。
        for i in 0..64u32 {
            let f = |k: u32| ((i * 37 + k * 101) % 257) as f32 / 256.0;
            let c = Vec3::new(f(0), f(1), f(2));
            let n = Vec3::new(f(3), f(4), f(5));
            let s = Vec3::new(f(6), f(7), f(8));
            let e = Vec3::new(f(9), f(10), f(11));
            let w = Vec3::new(f(12), f(13), f(14));
            let out = cas_sample(c, n, s, e, w, (i % 10) as f32 / 9.0);
            for ch in 0..3 {
                assert!(
                    !out[ch].is_nan() && (0.0..=1.0).contains(&out[ch]),
                    "i={i} ch{ch}={}",
                    out[ch]
                );
            }
        }
    }
}

//! TAA neighbourhood clamping in YCoCg space for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): RGB↔YCoCg conversion and the Jimenez-style variance
//! clip (`mu ± gamma*sigma` per channel). Clamping history to the current
//! pixel's local colour variance is what kills TAA ghosting without blurring —
//! a *quality* improvement, no resolution change, safe on integrated GPUs.

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

/// RGB → YCoCg. Y is luma, Co/Cg are chroma.
pub fn rgb_to_ycocg(c: Vec3) -> Vec3 {
    Vec3::new(
        0.25 * c.x + 0.5 * c.y + 0.25 * c.z,
        0.5 * c.x - 0.5 * c.z,
        -0.25 * c.x + 0.5 * c.y - 0.25 * c.z,
    )
}

/// YCoCg → RGB.
///
/// 変換対は代数的に逆変換 (係数が 2 の冪) だが、f32 での往復は **bit 厳密では
/// ない** (中間和の丸めで最大数 ulp ずれる — 例: (0.2,0.6,0.9) の往復で
/// G は完全一致・R/B は 2-3 ulp 差)。順変換自体は入力に対し完全決定的であり、
/// `ycocg_transform_exact_bits` テストが往復値をビットピンしている。
pub fn ycocg_to_rgb(c: Vec3) -> Vec3 {
    Vec3::new(c.x + c.y - c.z, c.x + c.z, c.x - c.y - c.z)
}

/// Variance clip: pull `color` into `mu ± gamma*sigma` per channel.
///
/// 契約 (2026-07-22 wave 24 監査で明文化): `gamma` は有限かつ非負。
/// gamma < 0 なら lo > hi、NaN なら境界自体が NaN となり、どちらも
/// `f32::clamp` 内部の assert で **意味不明な std メッセージのまま panic**
/// するため、ここで契約違反を明確に fail-loud する (σ は非負前提 — 呼び出し側
/// `mean_sigma` は sqrt 済み分散を返すので構造的に保証される)。
/// 現行 caller (full_graph_wiring) は gamma=1.25 固定で契約内。
/// 注: gpu/cpu 両側一致 (shaders/taa_ycocg.wgsl の clamp_to_variance と
/// 演算順一致) が保てるのはこの契約内のみ。
pub fn clamp_to_variance(color: Vec3, mu: Vec3, sigma: Vec3, gamma: f32) -> Vec3 {
    assert!(
        gamma.is_finite() && gamma >= 0.0,
        "clamp_to_variance: gamma must be finite and non-negative (got {gamma})"
    );
    let lo = mu - sigma * gamma;
    let hi = mu + sigma * gamma;
    Vec3::new(
        color.x.clamp(lo.x, hi.x),
        color.y.clamp(lo.y, hi.y),
        color.z.clamp(lo.z, hi.z),
    )
}

pub fn wgsl_source() -> &'static str {
    TAA_YCOCG_WGSL
}

pub const TAA_YCOCG_WGSL: &str = include_str!("../shaders/taa_ycocg.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_rgb_ycocg() {
        let c = Vec3::new(0.2, 0.6, 0.9);
        let r = ycocg_to_rgb(rgb_to_ycocg(c));
        assert!((r.x - c.x).abs() < 1e-6 && (r.y - c.y).abs() < 1e-6 && (r.z - c.z).abs() < 1e-6);
    }
    #[test]
    fn clamp_near_mean_is_unchanged() {
        let mu = Vec3::new(0.5, 0.5, 0.5);
        let c = clamp_to_variance(mu, mu, Vec3::new(0.1, 0.1, 0.1), 1.0);
        assert!((c.x - 0.5).abs() < 1e-6);
    }
    #[test]
    fn outlier_is_pulled_toward_mean() {
        let mu = Vec3::new(0.5, 0.5, 0.5);
        let sigma = Vec3::new(0.1, 0.1, 0.1);
        let out = Vec3::new(0.95, 0.5, 0.5);
        let c = clamp_to_variance(out, mu, sigma, 1.0);
        assert!(c.x < 0.95 && c.x > 0.5, "clamped x = {}", c.x);
    }

    /// wave 24-1: 順変換・往復の厳密ビット値 (f32 単一回丸め規則から exact
    /// rational で厳密導出 — float64 近似禁止、W-3 教訓)。往復が bit 厳密で
    /// ない (G 厳密・R/B 数 ulp) ことも同時に機械ピン — 将来の演算順変更を検出する。
    #[test]
    fn ycocg_transform_exact_bits() {
        let c = rgb_to_ycocg(Vec3::new(0.2, 0.6, 0.9));
        assert_eq!(
            [c.x.to_bits(), c.y.to_bits(), c.z.to_bits()],
            [0x3f133334, 0xbeb33333, 0x3cccccd0],
            "rgb_to_ycocg(0.2,0.6,0.9)"
        );
        let r = ycocg_to_rgb(c);
        assert_eq!(
            [r.x.to_bits(), r.y.to_bits(), r.z.to_bits()],
            [0x3e4cccd0, 0x3f19999a, 0x3f666668],
            "roundtrip (R/B は 2-3 ulp 差、G は厳密一致)"
        );
        // 純色は 2 の冪係数で厳密 (丸めゼロ)
        let red = rgb_to_ycocg(Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(
            [red.x.to_bits(), red.y.to_bits(), red.z.to_bits()],
            [0.25f32.to_bits(), 0.5f32.to_bits(), (-0.25f32).to_bits()],
            "primaries must be exact"
        );
    }

    /// wave 24-2: 境界式 lo/hi の厳密ビット (gamma=1: 0.4/0.6 は f32 近似値、
    /// gamma=1.25: 0.375/0.625 は厳密)。外れ値は hi に吸着、内側は不変。
    #[test]
    fn clamp_to_variance_exact_bounds_bits() {
        let mu = Vec3::new(0.5, 0.5, 0.5);
        let sig = Vec3::new(0.1, 0.1, 0.1);
        let c = clamp_to_variance(Vec3::new(0.95, 0.5, 0.5), mu, sig, 1.0);
        assert_eq!(c.x.to_bits(), 0x3f19999a, "0.95 clamps to hi=0.6_(f32)");
        assert_eq!(c.y.to_bits(), 0.5f32.to_bits(), "inside stays");
        let c = clamp_to_variance(Vec3::new(0.99, 0.01, 0.5), mu, sig, 1.25);
        assert_eq!(c.x.to_bits(), 0.625f32.to_bits(), "gamma 1.25 hi exact");
        assert_eq!(c.y.to_bits(), 0.375f32.to_bits(), "gamma 1.25 lo exact");
    }

    /// wave 24-3: 契約違反 (gamma<0 / NaN / ±inf) は std clamp の意味不明な
    /// panic ではなく契約メッセージで fail-loud。
    #[test]
    #[should_panic(expected = "gamma must be finite and non-negative")]
    fn clamp_to_variance_rejects_negative_gamma() {
        let _ = clamp_to_variance(
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::new(0.1, 0.1, 0.1),
            -0.5,
        );
    }

    #[test]
    #[should_panic(expected = "gamma must be finite and non-negative")]
    fn clamp_to_variance_rejects_nan_gamma() {
        let _ = clamp_to_variance(
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::new(0.1, 0.1, 0.1),
            f32::NAN,
        );
    }

    /// wave 24-4: WGSL ミラー (ユーティリティ関数集 — エントリポイント無しが
    /// 正しい契約) の naga 実パース + 3 関数実在 + 係数ミラートークン検査。
    #[test]
    fn wgsl_mirror_is_utility_functions_with_matching_coefficients() {
        let module =
            naga::front::wgsl::parse_str(TAA_YCOCG_WGSL).expect("taa_ycocg.wgsl must parse");
        assert!(
            module.entry_points.is_empty(),
            "utility shader must have no entry points (host-side contract)"
        );
        for name in ["rgb_to_ycocg", "ycocg_to_rgb", "clamp_to_variance"] {
            assert!(
                module
                    .functions
                    .iter()
                    .any(|f| f.1.name.as_deref() == Some(name)),
                "missing mirror fn {name}"
            );
        }
        for tok in ["0.25 * c.r", "0.5 * c.g", "clamp(c, lo, hi)"] {
            assert!(TAA_YCOCG_WGSL.contains(tok), "mirror token missing: {tok}");
        }
    }
}

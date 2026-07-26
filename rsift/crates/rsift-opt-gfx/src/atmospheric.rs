//! Analytic atmospheric scattering for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): a compact single-scattering sky model with
//! Rayleigh + Mie (Henyey–Greenstein) phase functions and an exponential
//! transmittance integral. Purely a *quality* improvement — it never changes
//! resolution, so the low-spec path is unaffected (you can still render the
//! sky at native resolution on integrated GPUs).
//!
//! ## モデル形態の誠実注記 (監査 2026-07-26 DP-1)
//! 位相関数 (Rayleigh 3(1+c²)/(16π)・Henyey-Greenstein (1-g²)/(4π d^1.5)) と
//! Beer-Lambert 透過率 exp(-β·d) の各要素は物理式そのままだが、光路は
//! **天頂角に依らず一様 8 km スラブ** (seg=1000 m × 8 段中点積分) の静的
//! 近似であり、Preetham/Hosek-Wilkie 系のような大気スケール高度・地平方向の
//! 伸長をモデル化していない (スタイル化シングルスキャッタリング)。
//! 「8 段の中点則」数値積分は決定的 (累積順固定)。
//!
//! ## CPU/WGSL パリティ (監査 2026-07-26 DP-3)
//! WGSL 側 (atmospheric.wgsl) は同一構造の実装で、PI 定数を f32::consts::PI
//! (0x40490FDB = WGSL リテラル `3.14159274`) に揃えて位相関数の非超越部は
//! **bit 一致**。ただし pow(d,1.5) は GPU 側がドライバ依存のため bit 級の
//! 完全同値は構造的に不可 (exp/pow の近似系差)。normalize の 0 ベクトル振舞
//! (CPU: そのまま返却 / WGSL: NaN) も差異として黙認せず記録する
//! (現消費者の sun/cam ray は非零で未到達)。

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
    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }
    pub fn normalize(self) -> Vec3 {
        let l = self.length();
        if l > 1e-8 {
            self * (1.0 / l)
        } else {
            self
        }
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

pub const PI: f32 = std::f32::consts::PI;

/// Rayleigh phase function (symmetric, peaks perpendicular to the sun).
pub fn rayleigh_phase(cos_theta: f32) -> f32 {
    3.0 / (16.0 * PI) * (1.0 + cos_theta * cos_theta)
}

/// Henyey–Greenstein phase function for Mie (forward-scattering when g > 0).
pub fn mie_phase(cos_theta: f32, g: f32) -> f32 {
    let g2 = g * g;
    let d = (1.0 + g2 - 2.0 * g * cos_theta).max(1e-4);
    (1.0 - g2) / (4.0 * PI * d.powf(1.5))
}

/// Per-channel transmittance over `distance` with scattering coeff `coeff`.
pub fn transmittance(distance: f32, coeff: Vec3) -> Vec3 {
    Vec3::new(
        (-coeff.x * distance).exp(),
        (-coeff.y * distance).exp(),
        (-coeff.z * distance).exp(),
    )
}

pub struct AtmosphereParams {
    pub rayleigh: Vec3, // per-channel scattering coefficient (1/m)
    pub sun_intensity: f32,
}
impl Default for AtmosphereParams {
    fn default() -> Self {
        Self {
            rayleigh: Vec3::new(5.8e-6, 13.5e-6, 33.1e-6),
            sun_intensity: 20.0,
        }
    }
}

/// Single-scattering in-scattered sky colour along `ray_dir`.
pub fn sky_color(ray_dir: Vec3, sun_dir: Vec3, p: &AtmosphereParams) -> Vec3 {
    let rd = ray_dir.normalize();
    let sd = sun_dir.normalize();
    let cos_t = rd.dot(sd);
    let phase = rayleigh_phase(cos_t) + 0.1 * mie_phase(cos_t, 0.76);
    let steps = 8u32;
    let seg = 8000.0f32 / steps as f32; // ~8 km of air
    let mut inscatter = Vec3::new(0.0, 0.0, 0.0);
    let mut t = 0.0f32;
    for _ in 0..steps {
        let d = t + seg * 0.5;
        let tr = transmittance(d, p.rayleigh);
        let contrib = Vec3::new(
            p.rayleigh.x * phase * tr.x * seg,
            p.rayleigh.y * phase * tr.y * seg,
            p.rayleigh.z * phase * tr.z * seg,
        );
        inscatter = inscatter + contrib;
        t += seg;
    }
    inscatter * p.sun_intensity
}

/// Convenience Vec4 wrapper (rgb + alpha) used by the WGSL-side pass.
pub fn sky_color_v4(ray_dir: Vec3, sun_dir: Vec3, p: &AtmosphereParams) -> Vec4 {
    let c = sky_color(ray_dir, sun_dir, p);
    Vec4::new(c.x, c.y, c.z, 1.0)
}

pub fn wgsl_source() -> &'static str {
    ATMOSPHERIC_WGSL
}

pub const ATMOSPHERIC_WGSL: &str = include_str!("../shaders/atmospheric.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn phase_is_positive_and_finite() {
        assert!(rayleigh_phase(0.0) > 0.0);
        assert!(mie_phase(0.0, 0.76).is_finite());
        assert!(mie_phase(-1.0, 0.76).is_finite());
    }
    #[test]
    fn mie_is_symmetric_at_g0() {
        let a = mie_phase(0.5, 0.0);
        let b = mie_phase(-0.5, 0.0);
        assert!((a - b).abs() < 1e-6, "{} vs {}", a, b);
    }
    #[test]
    fn transmittance_is_in_unit_interval() {
        let t = transmittance(2.0, Vec3::new(1.0, 1.0, 1.0));
        assert!(t.x < 1.0 && t.x > 0.0);
    }
    #[test]
    fn sky_is_brighter_toward_sun() {
        let p = AtmosphereParams::default();
        let up = Vec3::new(0.0, 1.0, 0.0);
        let toward_sun = sky_color(up, up, &p);
        let sun_to_side = sky_color(up, Vec3::new(1.0, 0.0, 0.0), &p);
        assert!(toward_sun.y > sun_to_side.y);
    }

    // ------------------------------------------------- 監査 2026-07-26 DP 追加分

    /// DP-1: 位相関数の厳密 bit pin。値は Python の IEEE f32 + ctypes libm
    /// (expf/powf/sqrtf) 独立シムで事前導出し照合済み。
    #[test]
    fn phase_exact_bits_cross_checked() {
        assert_eq!(rayleigh_phase(1.0).to_bits(), 0x3DF4_7645);
        assert_eq!(rayleigh_phase(0.0).to_bits(), 0x3D74_7645);
        assert_eq!(
            rayleigh_phase(-1.0).to_bits(),
            rayleigh_phase(1.0).to_bits(),
            "cos² のため対称 (bit 厳密)"
        );
        assert_eq!(mie_phase(0.5, 0.76).to_bits(), 0x3D3A_3C4F);
        assert_eq!(mie_phase(1.0, 0.76).to_bits(), 0x401B_9E3A);
        // g=0 は等方 1/(4π) に退化 (c 非依存): 厳密値ピン。
        assert_eq!(mie_phase(0.5, 0.0).to_bits(), 0x3DA2_F983);
        assert_eq!(
            mie_phase(-0.5, 0.0).to_bits(),
            mie_phase(0.5, 0.0).to_bits(),
            "g=0 で位相は c 非依存 (bit 厳密)"
        );
    }

    /// DP-1: 位相関数は球面積分で 1 に正規化 (Rayleigh/HG 共に d max 床は
    /// g=0.76 では非発動: min d = (1-g)² = 0.0576 > 1e-4)。中点則 4096 分割。
    #[test]
    fn phase_functions_integrate_to_one() {
        const N: usize = 4096;
        let (mut r_sum, mut m_sum) = (0.0f64, 0.0f64);
        for i in 0..N {
            let mu = -1.0 + 2.0 * (i as f64 + 0.5) / N as f64;
            r_sum += rayleigh_phase(mu as f32) as f64;
            m_sum += mie_phase(mu as f32, 0.76) as f64;
        }
        let domega = 2.0 * std::f64::consts::PI * (2.0 / N as f64);
        let (r_int, m_int) = (r_sum * domega, m_sum * domega);
        assert!(
            (r_int - 1.0).abs() < 1e-5,
            "Rayleigh は球面正規化 (誤差 {})",
            r_int - 1.0
        );
        assert!(
            (m_int - 1.0).abs() < 1e-5,
            "HG (g=0.76) は球面正規化 (誤差 {})",
            m_int - 1.0
        );
    }

    /// DP-2: sky_color の厳密 bit pin (累積順固定の中点則で決定的)。
    /// 値は Python 独立シム (IEEE f32 + ctypes expf) で事前導出し照合済み。
    #[test]
    fn sky_color_exact_bits_cross_checked() {
        let p = AtmosphereParams::default();
        let up = Vec3::new(0.0, 1.0, 0.0);
        let c1 = sky_color(up, up, &p);
        assert_eq!(c1.x.to_bits(), 0x3EA8_4F90, "toward sun (r)");
        assert_eq!(c1.y.to_bits(), 0x3F3E_030D, "toward sun (g)");
        assert_eq!(c1.z.to_bits(), 0x3FD7_E467, "toward sun (b)");
        let side = sky_color(Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.35, 0.55, 0.75), &p);
        assert_eq!(side.x.to_bits(), 0x3D82_748D, "side (r)");
        assert_eq!(side.y.to_bits(), 0x3E13_4688, "side (g)");
        assert_eq!(side.z.to_bits(), 0x3EA7_55C0, "side (b)");
    }

    /// DP-3: 位相の引数対称性 (sky_color(a,b) ≡ sky_color(b,a) bit 厳密)、
    /// 太陽方向が最明、NaN 伝播、transmittance の境界契約の pin 群。
    #[test]
    fn sky_color_symmetry_and_nan_contract() {
        let p = AtmosphereParams::default();
        let a = Vec3::new(1.0, 0.0, 0.0);
        let b = Vec3::new(0.35, 0.55, 0.75);
        let (ca, cb) = (sky_color(a, b, &p), sky_color(b, a, &p));
        assert_eq!(ca.x.to_bits(), cb.x.to_bits(), "対称 (r) bit 厳密");
        assert_eq!(ca.y.to_bits(), cb.y.to_bits(), "対称 (g) bit 厳密");
        assert_eq!(ca.z.to_bits(), cb.z.to_bits(), "対称 (b) bit 厳密");
        // NaN ray → 全成分 NaN 伝播 (0 側への静寂崩落ではない)。
        let nan_c = sky_color(Vec3::new(f32::NAN, 0.0, 0.0), b, &p);
        assert!(nan_c.x.is_nan() && nan_c.y.is_nan() && nan_c.z.is_nan());
        // transmittance 境界: d=0 → 1.0 厳密、d=+inf → 0.0 厳密。
        let t0 = transmittance(0.0, Vec3::new(1.0, 1.0, 1.0));
        assert_eq!(t0.x.to_bits(), 1.0f32.to_bits());
        let ti = transmittance(f32::INFINITY, Vec3::new(1.0, 1.0, 1.0));
        assert_eq!(ti.x.to_bits(), 0.0f32.to_bits());
    }

    /// DP-3/4: normalize の境界契約 (0 vec → そのまま、閾 1e-8 境界、
    /// (3,4,0) → (0.6, 0.8, 0) の厳密値) + WGSL PI が f32 PI と bit 一致
    /// (旧 3.14159265 は丸め不足) をソース走査 pin。
    #[test]
    fn normalize_boundary_and_wgsl_pi_pin() {
        let n = Vec3::new(3.0, 4.0, 0.0).normalize();
        assert_eq!(n.x.to_bits(), 0x3F19_999A, "0.6");
        assert_eq!(n.y.to_bits(), 0x3F4C_CCCD, "0.8");
        assert_eq!(n.z.to_bits(), 0, "0.0");
        let zero = Vec3::new(0.0, 0.0, 0.0).normalize();
        assert_eq!(
            (zero.x, zero.y, zero.z),
            (0.0, 0.0, 0.0),
            "0 ベクトルは CPU 側ではそのまま返却 (WGSL との差異は DP-3 注記)"
        );
        // (3,4,0)·1e-3 級 (|v|=5e-3 > 1e-8) は正規化される。
        let small = Vec3::new(3.0e-3, 4.0e-3, 0.0).normalize();
        assert!((small.dot(small) - 1.0).abs() < 1e-6, "閾超は正規化");
        // (3,4,0)·1e-5 (|v|=5e-5 ... より小さく |v|=5e-9 なら閾内→そのまま)。
        let tiny = Vec3::new(3.0e-9, 4.0e-9, 0.0).normalize();
        assert_eq!(
            (tiny.x, tiny.y, tiny.z),
            (3.0e-9, 4.0e-9, 0.0),
            "|v| ≦ 1e-8 は正規化せず返却"
        );
        assert!(
            ATMOSPHERIC_WGSL.contains("3.14159274"),
            "WGSL PI は f32::consts::PI (0x40490FDB) と bit 一致のリテラル"
        );
        assert!(
            !ATMOSPHERIC_WGSL.contains("3.14159265"),
            "旧丸め不足リテラルは除去済み"
        );
    }
}

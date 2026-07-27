//! Tile-based motion blur for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): gather `samples` taps along the screen-space
//! velocity vector and average them. Velocity comes from the G-buffer; the
//! per-pixel motion-blur amount is bounded by `max_velocity`, so fast-moving
//! geometry streaks but the frame stays at native resolution — pure quality
//! improvement, safe on integrated GPUs.
//!
//! 誠実注記 (wave 145 ES-3):
//! 1. **捕捉 59**: 旧配置 `t = i*inv - 0.5` (i∈[0,n)) はサンプル位置
//!    平均 -0.5/n ≠ 0 で、velocity≠0 のときブラー中心が逆行方向へ
//!    `v_max·velocity·0.5/n` だけ系統的にずれていた (修正前 RED 実測
//!    got=0x3EF9999A、rq es_mb 導出と完全一 致)。修正形
//!    `t = (i+0.5)*inv - 0.5` は ± 対称配置 (区分化重心と一致) で
//!    平均 exact 0、f32 でも各 t は 2 冪分数 (2 冪 n) で ± ペア和
//!    exact 0。**旧式では samples=1 でも t=-0.5 の端点** (=速度
//!    無関係で最大半幅のずれ) だった点も併せて根治。
//! 2. `inv = 1/samples` の丸め: n が 2 冪 (既定 8) では exact。
//!    非 2 冪 n では inv 自体に丸めが入り平均も厳密 1 倍から僅差 —
//!    ブラー近似の許容帯として受容 (fail-loud 対象外)。
//! 3. `samples=0` は従来 `inv=inf` → `acc(0)*inf=NaN` の静寂出力
//!    だった → ES-4 で fail-loud assert 化 (samples≥1・velocity/
//!    max_velocity 有限、wiring 実引数 default(8,0.1) で非発火)。
//! 4. Vec3/Vec4 の **Sub impl は crate+workspace 消費者ゼロ** の
//!    完全装飾 (census grep 機械確定、`uv + v*t` と `acc + sample`
//!    で Add/Mul のみ使用) → ES-2 で削除 (EN-2/EP-2 同型、型本体と
//!    Add/Mul/Default は消費あり維持)。
//! 5. `acc * inv` の平均は「合計×1/n」(求積の階段和、1/n 因子)。
//!    サンプラ契約は any Fn(Vec3)->Vec4 で G-buffer 取得コストは
//!    呼出側責務 (本モジュールは純粋数学のみ)。

use std::ops::{Add, Mul};

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
impl Mul<f32> for Vec4 {
    type Output = Vec4;
    fn mul(self, s: f32) -> Vec4 {
        Vec4::new(self.x * s, self.y * s, self.z * s, self.w * s)
    }
}

pub struct MotionBlurParams {
    pub samples: u32,
    pub max_velocity: f32,
}
impl Default for MotionBlurParams {
    fn default() -> Self {
        Self {
            samples: 8,
            max_velocity: 0.1,
        }
    }
}

/// Gather `samples` taps along `velocity`, centred on `uv`, and average.
pub fn motion_blur(
    uv: Vec3,
    velocity: Vec3,
    params: &MotionBlurParams,
    sample: &dyn Fn(Vec3) -> Vec4,
) -> Vec4 {
    // ES-4 fail-loud (旧来 samples=0 → inv=inf → acc(0)*inf=NaN 静寂出力、
    // NaN velocity は acc 全体 NaN 静寂伝播 — 契約明文化で根治、注記 3)。
    assert!(
        params.samples >= 1,
        "MotionBlurParams 契約違反: samples が 0"
    );
    assert!(
        params.max_velocity.is_finite(),
        "MotionBlurParams 契約違反: max_velocity が非有限"
    );
    assert!(
        velocity.x.is_finite() && velocity.y.is_finite() && velocity.z.is_finite(),
        "motion_blur 契約違反: velocity が非有限"
    );
    let v = velocity * params.max_velocity;
    let inv = 1.0 / params.samples as f32;
    let mut acc = Vec4::new(0.0, 0.0, 0.0, 0.0);
    for i in 0..params.samples {
        // 捕捉 59: ± 対称配置 (i+0.5)*inv - 0.5 (区分化重心、平均 exact 0)。
        // 旧 `i*inv - 0.5` は平均 -0.5/n の負方向偏向 (注記 1)。
        let t = ((i as f32) + 0.5) * inv - 0.5;
        acc = acc + sample(uv + v * t);
    }
    acc * inv
}

pub fn wgsl_source() -> &'static str {
    MOTION_BLUR_WGSL
}

pub const MOTION_BLUR_WGSL: &str = include_str!("../shaders/motion_blur.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zero_velocity_returns_center() {
        let c = motion_blur(
            Vec3::new(0.5, 0.5, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            &MotionBlurParams::default(),
            &|u: Vec3| Vec4::new(u.x + 1.0, 0.0, 0.0, 1.0),
        );
        assert!((c.x - 1.5).abs() < 1e-6, "center x = {}", c.x);
    }
    #[test]
    fn constant_field_is_unchanged() {
        let c = motion_blur(
            Vec3::new(0.5, 0.5, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            &MotionBlurParams {
                samples: 8,
                max_velocity: 0.2,
            },
            &|_u: Vec3| Vec4::new(0.3, 0.4, 0.5, 1.0),
        );
        assert!((c.x - 0.3).abs() < 1e-6 && (c.y - 0.4).abs() < 1e-6);
    }
    #[test]
    fn result_is_bounded() {
        let c = motion_blur(
            Vec3::new(0.5, 0.5, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            &MotionBlurParams {
                samples: 4,
                max_velocity: 0.5,
            },
            &|u: Vec3| Vec4::new(u.x, u.y, u.x + u.y, 1.0),
        );
        assert!(c.x.is_finite() && c.y.is_finite());
    }
}

#[cfg(test)]
mod strict_tests {
    use super::*;

    /// 捕捉 59 pin: サンプル位置 `t = i*inv - 0.5` (i∈[0,n)) は平均
    /// -0.5/n ≠ 0 の**負方向偏向** (velocity·max_velocity·0.5/n だけ
    /// ブラー中心が逆行方向へずれる)。修正形 `(i+0.5)*inv - 0.5` は
    /// ± 対称配置で平均 exact 0 (f32 2 冪分数経路、rq es_mb 導出:
    /// 対称ペア和 exact 0)。samples=8/uv.x=0.5/v.x=1.0/max_v=0.2 で
    /// identity sampler の out.x = 0.5 exact (0x3F000000)。
    /// 【修正前 RED 実証】旧式 got = 0x3EF9999A (0.4875 負偏向)。
    #[test]
    fn centered_samples_zero_bias_pin() {
        let c = motion_blur(
            Vec3::new(0.5, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            &MotionBlurParams {
                samples: 8,
                max_velocity: 0.2,
            },
            &|u: Vec3| Vec4::new(u.x, 0.0, 0.0, 1.0),
        );
        assert_eq!(
            c.x.to_bits(),
            0x3F00_0000,
            "対称配置 → 零バイアス exact 0.5 (rq es_mb; 旧式は 0x3EF9999A)"
        );
    }

    /// samples=1 → t=(0.5)*1-0.5=0.0 の一点集合 (zero blur、新旧一致)。
    #[test]
    fn samples_one_is_zero_blur() {
        let c = motion_blur(
            Vec3::new(0.25, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            &MotionBlurParams {
                samples: 1,
                max_velocity: 0.2,
            },
            &|u: Vec3| Vec4::new(u.x, 0.0, 0.0, 1.0),
        );
        assert_eq!(
            c.x.to_bits(),
            0x3E80_0000,
            "t=0 → sample(uv) そのまま (0.25)"
        );
    }

    /// velocity=0 は新旧一致 (t 無関係): out = sample(uv) bits pin
    /// (既存近似テストの厳密化)。
    #[test]
    fn zero_velocity_exact_bits_pin() {
        let c = motion_blur(
            Vec3::new(0.5, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            &MotionBlurParams::default(),
            &|u: Vec3| Vec4::new(u.x + 1.0, 0.0, 0.0, 1.0),
        );
        assert_eq!(c.x.to_bits(), 0x3FC0_0000, "1.5 exact");
    }

    /// ES-4 fail-loud: samples=0 → inv=1/0=inf → acc(0)*inf=NaN の
    /// 静寂出力を契約 assert で根治。
    #[test]
    #[should_panic(expected = "契約違反: samples が 0")]
    fn samples_zero_panics() {
        let _ = motion_blur(
            Vec3::new(0.5, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            &MotionBlurParams {
                samples: 0,
                max_velocity: 0.2,
            },
            &|u: Vec3| Vec4::new(u.x, 0.0, 0.0, 1.0),
        );
    }

    /// ES-4 fail-loud: velocity 非有限 (NaN) は panic (旧来 acc NaN
    /// 静寂伝播)。
    #[test]
    #[should_panic(expected = "契約違反: velocity が非有限")]
    fn nan_velocity_panics() {
        let _ = motion_blur(
            Vec3::new(0.5, 0.0, 0.0),
            Vec3::new(f32::NAN, 0.0, 0.0),
            &MotionBlurParams::default(),
            &|u: Vec3| Vec4::new(u.x, 0.0, 0.0, 1.0),
        );
    }

    /// ES-4 fail-loud: max_velocity 非有限でも panic。
    #[test]
    #[should_panic(expected = "契約違反: max_velocity が非有限")]
    fn nan_max_velocity_panics() {
        let _ = motion_blur(
            Vec3::new(0.5, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            &MotionBlurParams {
                samples: 8,
                max_velocity: f32::NAN,
            },
            &|u: Vec3| Vec4::new(u.x, 0.0, 0.0, 1.0),
        );
    }

    /// ES-2: Sub impl 削除後の型契約 pin — Vec3/Vec4 の Add/Mul/new/
    /// Default は維持 (削除による検証空洞を残さない、EP-2 様式)。
    #[test]
    fn vec_contract_after_sub_removal_pin() {
        let a = Vec3::new(1.0, 2.0, 3.0);
        let b = Vec3::new(0.5, 0.5, 0.5);
        let s = a + b;
        assert_eq!((s.x, s.y, s.z), (1.5, 2.5, 3.5), "Vec3 Add 経路");
        let m = a * 2.0;
        assert_eq!((m.x, m.y, m.z), (2.0, 4.0, 6.0), "Vec3 Mul<f32> 経路");
        let d = Vec4::default() + Vec4::new(1.0, 2.0, 3.0, 4.0);
        assert_eq!((d.x, d.w), (1.0, 4.0), "Vec4 Default+Add");
        let m4 = Vec4::new(1.0, 1.0, 1.0, 1.0) * 0.5;
        assert_eq!((m4.x, m4.w), (0.5, 0.5), "Vec4 Mul<f32>");
    }

    /// wgsl_source ≡ const identity pin (gpu_runtime:71 消費経路)。
    #[test]
    fn wgsl_identity_pin() {
        assert_eq!(wgsl_source(), MOTION_BLUR_WGSL);
        assert!(!MOTION_BLUR_WGSL.is_empty());
    }

    /// Default パラメタ pin: samples=8・max_velocity=0.1 (wiring:1790
    /// 実引数 golden の根拠)。
    #[test]
    fn default_params_pin() {
        let d = MotionBlurParams::default();
        assert_eq!(d.samples, 8);
        assert_eq!(d.max_velocity.to_bits(), 0x3DCC_CCCD, "0.1f32");
    }
}

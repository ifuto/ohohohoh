//! Depth-of-field (bokeh) via circle-of-confusion gather for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): compute a per-pixel circle-of-confusion from the
//! depth/focus distance, then gather an 8-tap disc. DoF is purely a *quality*
//! (cinematic) effect; resolution is unchanged, so it is safe to enable on
//! integrated GPUs (the gather can also run at a lower rate).
//!
//! 誠実注記 (wave 147 ET 監査):
//! 1. CoC は |depth - focus| × scale の前後対称な簡易モデル (被写界深度/
//!    絞り形状の物理概念は持たない標準的近似)。
//! 2. 捕捉 60: 旧 taps 順序は Σdy の f32 逐次和が 2^-24 でブラー重心が y に
//!    偏位 (identity 経路 2^-27) していた → 対称ペア順で Σ=0 exact へ根治。
//! 3. identity gather の 8v×(1/8) は非 2 冪段 (3v/5v) の丸めで exact 復元
//!    されない (0.1→+1ulp 等、rq golden)。sampler が恒等の wiring では DoF
//!    は ±1ulp 差の実質恒等変換 (chunk_dists→coc 変動を観測する経路はない)。
//! 4. NaN depth は clamp を透過 (捕捉 57 規律: self NaN のみ透過) して coc=NaN、
//!    gather では NaN<1e-3=false → ブラー経路で NaN 座標が sampler に届く。
//!    max_coc<0 (min>max)・max_coc NaN (引数 NaN) は clamp panic。
//! 5. Vec3/Vec4 の Sub impl は crate+workspace 消費者ゼロ (census grep 機械確定、
//!    本体は Add/Mul のみ) のため削除 (ES-2 同型)。Add/Mul/Default と型本体は維持。

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

pub struct DofParams {
    pub focus_dist: f32,
    pub scale: f32,
    pub max_coc: f32,
}
impl Default for DofParams {
    fn default() -> Self {
        Self {
            focus_dist: 10.0,
            scale: 0.05,
            max_coc: 16.0,
        }
    }
}

/// Circle of confusion radius (in pixels) for a given linear `depth`.
pub fn circle_of_confusion(depth: f32, p: &DofParams) -> f32 {
    ((depth - p.focus_dist).abs() * p.scale).clamp(0.0, p.max_coc)
}

/// 8-tap disc gather; returns the centre sample when CoC is ~0.
pub fn gather_blur(uv: Vec3, coc: f32, sample: &dyn Fn(Vec3) -> Vec4) -> Vec4 {
    if coc < 1e-3 {
        return sample(uv);
    }
    // 8 evenly spaced points on a unit disc.
    // (s = sin/cos 45° = FRAC_1_SQRT_2。リテラル近似 0.7071 から正確な定数へ)
    const S: f32 = std::f32::consts::FRAC_1_SQRT_2;
    // 捕捉 60: 旧順序は Σdy の f32 逐次和が 2^-24 で重心が y に偏位
    // (uv=0/coc=1 identity 経路 out.y=2^-27=0x32000000、rq et_dof 機械導出)。
    // 対称ペア順に並べ替えて各ペアを連続相殺させ Σdx=Σdy=0 exact に根治。
    let taps: [(f32, f32); 8] = [
        (1.0, 0.0),
        (-1.0, 0.0),
        (S, S),
        (-S, -S),
        (0.0, 1.0),
        (0.0, -1.0),
        (-S, S),
        (S, -S),
    ];
    let mut acc = Vec4::new(0.0, 0.0, 0.0, 0.0);
    for &(dx, dy) in taps.iter() {
        acc = acc + sample(uv + Vec3::new(dx * coc, dy * coc, 0.0));
    }
    acc * (1.0 / taps.len() as f32)
}

pub fn wgsl_source() -> &'static str {
    DEPTH_OF_FIELD_WGSL
}

pub const DEPTH_OF_FIELD_WGSL: &str = include_str!("../shaders/depth_of_field.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn in_focus_coc_is_zero() {
        let c = circle_of_confusion(10.0, &DofParams::default());
        assert_eq!(c, 0.0);
    }
    #[test]
    fn far_objects_have_larger_coc() {
        let near = circle_of_confusion(11.0, &DofParams::default());
        let far = circle_of_confusion(20.0, &DofParams::default());
        assert!(far > near);
    }
    #[test]
    fn coc_is_clamped() {
        let c = circle_of_confusion(100000.0, &DofParams::default());
        assert!((c - 16.0).abs() < 1e-6);
    }
    #[test]
    fn blur_of_constant_color_is_constant() {
        let c = gather_blur(Vec3::new(0.5, 0.5, 0.0), 8.0, &|_u: Vec3| {
            Vec4::new(0.2, 0.3, 0.4, 1.0)
        });
        assert!((c.x - 0.2).abs() < 1e-6 && (c.y - 0.3).abs() < 1e-6);
    }

    // wave 147 ET strict 群 (全 golden rq et_dof 事前導出)

    // 捕捉 60: 8-tap f32 逐次重心は ± exact 0 (修正前順序は Σdy=2^-24→out.y=2^-27=0x32000000 偏位)
    #[test]
    fn dof_taps_centroid_exact_zero() {
        let out = gather_blur(Vec3::new(0.0, 0.0, 0.0), 1.0, &|c: Vec3| {
            Vec4::new(c.x, c.y, 0.0, 1.0)
        });
        assert_eq!(out.x, 0.0, "Σdx exact 0 (rq)");
        assert_eq!(out.y, 0.0, "Σdy ペア順 exact 0 (旧順 0x32000000)");
    }

    #[test]
    fn dof_identity_gather_ulp_pin() {
        // 8v×1/8 は非 2 冪段 (3v/5v) 丸めで exact 復元されない (rq 導出)
        let c = gather_blur(Vec3::new(0.0, 0.0, 0.0), 4.0, &|_u: Vec3| {
            Vec4::new(0.1, 1.5, 0.0, 1.0)
        });
        assert_eq!(c.x.to_bits(), 0x3DCCCCCE, "0.1 → +1ulp (rq)");
        assert_eq!(c.y.to_bits(), 0x3FC00000, "1.5 → exact (2 冪和経路)");
    }

    #[test]
    fn dof_coc_values_and_boundary() {
        let p = DofParams::default();
        assert_eq!(
            circle_of_confusion(20.0, &p).to_bits(),
            0x3F000000,
            "d=20 → 0.5"
        );
        assert_eq!(
            circle_of_confusion(32.0, &p).to_bits(),
            0x3F8CCCCD,
            "d=32 → 1.1"
        );
        // 境界: d=10.02 は f32 逐次で 0.0010000229 > 1e-3 (直感 0.001 ちょうどは誤り、rq)
        assert_eq!(circle_of_confusion(10.02, &p).to_bits(), 0x3A831333);
    }

    // 前後対称性 pin (補完): |2-10|=|18-10|=8 → 0.4 (adversarial d 用、rq 導出)
    #[test]
    fn dof_coc_pre_post_symmetry() {
        let p = DofParams::default();
        assert_eq!(
            circle_of_confusion(2.0, &p).to_bits(),
            0x3ECCCCCD,
            "d=2 → 0.4"
        );
        assert_eq!(
            circle_of_confusion(18.0, &p).to_bits(),
            0x3ECCCCCD,
            "d=18 → 0.4"
        );
        assert_eq!(
            circle_of_confusion(2.0, &p),
            circle_of_confusion(18.0, &p),
            "abs 対称"
        );
    }

    #[test]
    fn dof_sampler_call_count_boundary() {
        use std::cell::Cell;
        let calls = Cell::new(0u32);
        let count = |c: Vec3| {
            calls.set(calls.get() + 1);
            Vec4::new(c.x, c.y, c.z, 1.0)
        };
        gather_blur(Vec3::new(0.0, 0.0, 0.0), 5e-4, &count);
        assert_eq!(calls.get(), 1, "coc<1e-3 → center のみ");
        gather_blur(Vec3::new(0.0, 0.0, 0.0), 0.5, &count);
        assert_eq!(calls.get(), 1 + 8, "0.5 → 8 taps");
        // coc=1e-3 ちょうど: `<` なのでブラー側 (閉区間 pin)
        gather_blur(Vec3::new(0.0, 0.0, 0.0), 1e-3, &count);
        assert_eq!(calls.get(), 1 + 8 + 8, "1e-3 inclusive ブラー");
    }

    #[test]
    fn dof_nan_transparency_and_path() {
        // 捕捉 57 規律: self NaN は clamp を透過
        assert!(circle_of_confusion(f32::NAN, &DofParams::default()).is_nan());
        // NaN coc は NaN<1e-3=false → ブラー経路・NaN uv が sampler に届く
        use std::cell::Cell;
        let nan_seen = Cell::new(0u32);
        let probe = |c: Vec3| {
            if c.x.is_nan() {
                nan_seen.set(nan_seen.get() + 1);
            }
            Vec4::new(0.0, 0.0, 0.0, 1.0)
        };
        gather_blur(Vec3::new(0.5, 0.5, 0.0), f32::NAN, &probe);
        assert_eq!(nan_seen.get(), 8, "NaN × coc → 全 tap NaN 座標");
    }

    #[test]
    #[should_panic(expected = "min > max")]
    fn dof_max_coc_negative_panics() {
        let p = DofParams {
            focus_dist: 10.0,
            scale: 0.05,
            max_coc: -1.0,
        };
        let _ = circle_of_confusion(20.0, &p);
    }

    #[test]
    #[should_panic(expected = "either was NaN")]
    fn dof_max_coc_nan_panics() {
        let p = DofParams {
            focus_dist: 10.0,
            scale: 0.05,
            max_coc: f32::NAN,
        };
        let _ = circle_of_confusion(20.0, &p);
    }

    #[test]
    fn dof_contract_pins() {
        assert!(
            std::f32::consts::FRAC_1_SQRT_2 == (0.5f32).sqrt(),
            "S 定数対応 (rq)"
        );
        assert_eq!(
            wgsl_source(),
            DEPTH_OF_FIELD_WGSL,
            "wgsl identity (&str 内容)"
        );
        let d = DofParams::default();
        assert!(d.focus_dist == 10.0 && d.scale.to_bits() == 0x3D4CCCCD && d.max_coc == 16.0);
        assert!(1.0f32 / 8.0 == 0.125, "1/8 2 冪 exact");
        // ES-2 同型 contract pin: Add/Mul/Default 維持 (Sub は削除)
        let a = Vec3::new(1.0, 2.0, 3.0) + Vec3::new(4.0, 5.0, 6.0);
        assert!(a.x == 5.0 && a.y == 7.0 && a.z == 9.0, "Vec3 Add");
        let m = a * 2.0;
        assert!(m.x == 10.0 && m.y == 14.0 && m.z == 18.0, "Vec3 Mul<f32>");
        let v = Vec4::new(1.0, 2.0, 3.0, 4.0) + Vec4::default();
        assert!(v.w == 4.0, "Vec4 Add+Default");
    }
}

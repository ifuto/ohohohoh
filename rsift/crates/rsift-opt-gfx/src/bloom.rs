//! HDR bloom for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): a soft-knee brightness prefilter, a separable
//! Gaussian blur step, and a final additive composite. Bloom only *adds*
//! light where it already exists, so it is a quality improvement with no
//! resolution change — safe for the low-spec path (the blur can also run at a
//! fraction of resolution to save bandwidth).

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

/// Luminance of a colour (Rec.709).
pub fn luma(c: Vec3) -> f32 {
    0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z
}

/// Soft-knee prefilter: keep energy above `threshold`, rolling off over
/// `knee` to avoid a hard cutoff.
pub fn prefilter(c: Vec3, threshold: f32, knee: f32) -> Vec3 {
    let l = luma(c);
    if l <= threshold {
        return Vec3::new(0.0, 0.0, 0.0);
    }
    let mut f = (l - threshold) / threshold.max(1e-4);
    // soft knee: smoothstep over [0, knee]
    if knee > 0.0 {
        let t = (l - threshold) / knee;
        let soft = t.clamp(0.0, 1.0);
        f = f * (soft * soft * (3.0 - 2.0 * soft));
    }
    c * f
}

/// 1D Gaussian blur of a row using 5 taps (weights normalized).
pub fn blur_row(src: &[Vec3], dst: &mut [Vec3], radius: usize) {
    let w = [0.0625f32, 0.25, 0.375, 0.25, 0.0625];
    let offs = [-2, -1, 0, 1, 2];
    let n = src.len();
    for i in 0..n {
        let mut acc = Vec3::new(0.0, 0.0, 0.0);
        for k in 0..5 {
            let j = (i as isize + offs[k] * radius as isize).clamp(0, n as isize - 1) as usize;
            acc = acc + src[j] * w[k];
        }
        dst[i] = acc;
    }
}

/// Additively composite bloom over the scene.
pub fn composite(scene: Vec3, bloom: Vec3, intensity: f32) -> Vec3 {
    (scene + bloom * intensity).clamp(0.0, 64.0)
}

pub fn wgsl_source() -> &'static str {
    BLOOM_WGSL
}

pub const BLOOM_WGSL: &str = include_str!("../shaders/bloom.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prefilter_keeps_bright() {
        let c = Vec3::new(2.0, 2.0, 2.0); // luma 2
        let p = prefilter(c, 1.0, 0.0);
        assert!(luma(p) > 0.0, "bright pixels pass prefilter");
    }
    #[test]
    fn prefilter_drops_dark() {
        let c = Vec3::new(0.2, 0.2, 0.2);
        let p = prefilter(c, 1.0, 0.0);
        assert_eq!(luma(p), 0.0);
    }
    #[test]
    fn blur_of_constant_is_constant() {
        let src: Vec<Vec3> = (0..16).map(|_| Vec3::new(1.0, 2.0, 3.0)).collect();
        let mut dst = vec![Vec3::default(); 16];
        blur_row(&src, &mut dst, 1);
        for d in &dst {
            assert!((d.x - 1.0).abs() < 1e-6 && (d.y - 2.0).abs() < 1e-6);
        }
    }
    #[test]
    fn composite_adds_energy() {
        let s = Vec3::new(0.5, 0.5, 0.5);
        let b = Vec3::new(0.2, 0.0, 0.0);
        let c = composite(s, b, 1.0);
        assert!((c.x - 0.7).abs() < 1e-6);
    }
}

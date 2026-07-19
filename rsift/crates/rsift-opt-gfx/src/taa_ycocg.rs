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
pub fn ycocg_to_rgb(c: Vec3) -> Vec3 {
    Vec3::new(c.x + c.y - c.z, c.x + c.z, c.x - c.y - c.z)
}

/// Variance clip: pull `color` into `mu ± gamma*sigma` per channel.
pub fn clamp_to_variance(color: Vec3, mu: Vec3, sigma: Vec3, gamma: f32) -> Vec3 {
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
}

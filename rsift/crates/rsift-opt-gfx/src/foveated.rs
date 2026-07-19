//! Foveated shading-rate (VRS) mask for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): a radial shading-rate field centred on the gaze
//! point. Full rate (1.0) at the fovea, falling to `min_rate` at the edge of
//! `radius`. Variable-rate shading spends GPU pixels where the eye actually
//! looks — on integrated GPUs this is nearly free performance with *no*
//! resolution downscale of the final image (it only changes shading rate).

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

/// Shading rate in [min_rate, 1.0]: 1.0 at the gaze, lower toward the edge.
pub fn shading_rate(uv: Vec3, gaze: Vec3, radius: f32, min_rate: f32) -> f32 {
    let dx = uv.x - gaze.x;
    let dy = uv.y - gaze.y;
    let d = (dx * dx + dy * dy).sqrt();
    let t = (d / radius.max(1e-4)).clamp(0.0, 1.0);
    (1.0 - t * (1.0 - min_rate)).clamp(min_rate, 1.0)
}

pub fn wgsl_source() -> &'static str {
    FOVEATED_WGSL
}

pub const FOVEATED_WGSL: &str = include_str!("../shaders/foveated.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn at_gaze_is_full_rate() {
        let r = shading_rate(
            Vec3::new(0.5, 0.5, 0.0),
            Vec3::new(0.5, 0.5, 0.0),
            0.4,
            0.25,
        );
        assert!((r - 1.0).abs() < 1e-6);
    }
    #[test]
    fn far_from_gaze_is_min_rate() {
        let r = shading_rate(
            Vec3::new(0.99, 0.99, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            0.4,
            0.25,
        );
        assert!((r - 0.25).abs() < 1e-6, "rate = {}", r);
    }
    #[test]
    fn rate_is_monotonic_with_distance() {
        let near = shading_rate(Vec3::new(0.55, 0.5, 0.0), Vec3::new(0.5, 0.5, 0.0), 0.4, 0.25);
        let far = shading_rate(Vec3::new(0.9, 0.5, 0.0), Vec3::new(0.5, 0.5, 0.0), 0.4, 0.25);
        assert!(far < near, "near={} far={}", near, far);
    }
}

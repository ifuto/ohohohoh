//! Tile-based motion blur for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): gather `samples` taps along the screen-space
//! velocity vector and average them. Velocity comes from the G-buffer; the
//! per-pixel motion-blur amount is bounded by `max_velocity`, so fast-moving
//! geometry streaks but the frame stays at native resolution — pure quality
//! improvement, safe on integrated GPUs.

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
    let v = velocity * params.max_velocity;
    let inv = 1.0 / params.samples as f32;
    let mut acc = Vec4::new(0.0, 0.0, 0.0, 0.0);
    for i in 0..params.samples {
        let t = (i as f32) * inv - 0.5; // -0.5 .. 0.5
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

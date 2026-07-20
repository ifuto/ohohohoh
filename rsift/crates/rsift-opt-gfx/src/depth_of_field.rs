//! Depth-of-field (bokeh) via circle-of-confusion gather for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): compute a per-pixel circle-of-confusion from the
//! depth/focus distance, then gather an 8-tap disc. DoF is purely a *quality*
//! (cinematic) effect; resolution is unchanged, so it is safe to enable on
//! integrated GPUs (the gather can also run at a lower rate).

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
    let taps: [(f32, f32); 8] = [
        (1.0, 0.0),
        (S, S),
        (0.0, 1.0),
        (-S, S),
        (-1.0, 0.0),
        (-S, -S),
        (0.0, -1.0),
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
}

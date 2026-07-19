//! Screen-space shadows (SSS) for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): march a ray toward the light through a depth field
//! (supplied via closure so the core math is unit-testable) and report
//! visibility. SSS recovers contact shadows that cascaded shadow maps miss —
//! a *quality* improvement. Cost is bounded by `max_steps`, so it is cheap on
//! integrated GPUs (and can be run at half resolution then upscaled).

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

pub struct SssParams {
    pub max_steps: u32,
    pub step_size: f32,
    pub max_dist: f32,
    pub bias: f32,
}
impl Default for SssParams {
    fn default() -> Self {
        Self {
            max_steps: 16,
            step_size: 0.1,
            max_dist: 10.0,
            bias: 0.02,
        }
    }
}

/// March toward the light; `sample_depth` returns the travelled distance to the
/// nearest occluder (or `f32::INFINITY` if empty). Returns 1.0 (lit) / 0.0
/// (shadowed).
pub fn cast_sss(
    pos: Vec3,
    light_dir: Vec3,
    params: &SssParams,
    sample_depth: &dyn Fn(Vec3) -> f32,
) -> f32 {
    let ld = light_dir.normalize();
    let mut v = pos + ld * params.bias;
    for _ in 0..params.max_steps {
        v = v + ld * params.step_size;
        let travelled = (v - pos).length();
        if travelled > params.max_dist {
            return 1.0;
        }
        let surf = sample_depth(v);
        if surf.is_infinite() {
            continue;
        }
        let diff = surf - travelled;
        if diff >= 0.0 && diff <= params.step_size * 2.0 {
            return 0.0;
        }
    }
    1.0
}

pub fn wgsl_source() -> &'static str {
    SCREEN_SPACE_SHADOW_WGSL
}

pub const SCREEN_SPACE_SHADOW_WGSL: &str = include_str!("../shaders/screen_space_shadow.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clear_path_is_lit() {
        let vis = cast_sss(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            &SssParams::default(),
            &|_p: Vec3| f32::INFINITY,
        );
        assert_eq!(vis, 1.0);
    }
    #[test]
    fn occluder_in_path_is_shadowed() {
        // Occluder at travelled ~0.5 within the march.
        let vis = cast_sss(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            &SssParams {
                max_steps: 16,
                step_size: 0.1,
                max_dist: 10.0,
                bias: 0.02,
            },
            &|p: Vec3| 0.05 + p.y, // occluder just ahead of the ray (within thickness)
        );
        assert_eq!(vis, 0.0);
    }
    #[test]
    fn beyond_max_dist_is_lit() {
        let vis = cast_sss(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            &SssParams {
                max_steps: 4,
                step_size: 0.1,
                max_dist: 0.3,
                bias: 0.0,
            },
            &|_p: Vec3| 5.0,
        );
        assert_eq!(vis, 1.0);
    }
}

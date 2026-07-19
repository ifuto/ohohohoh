//! Parallax occlusion mapping (POM) for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): march a view ray through a height field in tangent
//! space and, on the first layer below the surface, interpolate to find the
//! offset UV. Gives surfaces real depth from a height map with *no* geometry
//! cost — a quality improvement that integrated GPUs handle fine (just fewer
//! `layers`).

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

pub struct ParallaxParams {
    pub layers: u32,
    pub height_scale: f32,
}
impl Default for ParallaxParams {
    fn default() -> Self {
        Self {
            layers: 16,
            height_scale: 0.1,
        }
    }
}

/// Returns `(offset_uv, layer_depth)` for the first surface hit. `view_dir`
/// points from the surface toward the eye (tangent space, `z` = out of map).
pub fn parallax_occlusion(
    uv: Vec3,
    view_dir: Vec3,
    height: &dyn Fn(Vec3) -> f32,
    p: &ParallaxParams,
) -> (Vec3, f32) {
    let layers = p.layers.max(1);
    let layer_depth = 1.0 / layers as f32;
    let p_step = Vec3::new(view_dir.x, view_dir.y, 0.0)
        * (p.height_scale * layer_depth / view_dir.z.max(1e-3));
    let mut cur_uv = uv;
    let mut cur_layer = 0.0f32;
    let mut cur_depth = height(cur_uv);
    for _ in 0..layers {
        if cur_layer >= cur_depth {
            break;
        }
        cur_uv = cur_uv - p_step;
        cur_depth = height(cur_uv);
        cur_layer += layer_depth;
    }
    let prev_uv = cur_uv + p_step;
    let prev_depth = height(prev_uv);
    let after = cur_depth - cur_layer;
    let before = prev_depth - (cur_layer + layer_depth);
    let w = after / (after - before).max(1e-4);
    let final_uv = prev_uv * w + cur_uv * (1.0 - w);
    (final_uv, cur_layer)
}

pub fn wgsl_source() -> &'static str {
    PARALLAX_WGSL
}

pub const PARALLAX_WGSL: &str = include_str!("../shaders/parallax.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flat_surface_has_no_offset() {
        let (uv, _) = parallax_occlusion(
            Vec3::new(0.5, 0.5, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            &|_u: Vec3| 0.0,
            &ParallaxParams::default(),
        );
        assert!((uv.x - 0.5).abs() < 1e-6 && (uv.y - 0.5).abs() < 1e-6);
    }
    #[test]
    fn sloped_surface_offsets() {
        let (uv, _) = parallax_occlusion(
            Vec3::new(0.5, 0.5, 0.0),
            Vec3::new(0.3, 0.0, 1.0),
            &|u: Vec3| u.x.clamp(0.0, 1.0),
            &ParallaxParams {
                layers: 16,
                height_scale: 0.2,
            },
        );
        assert!((uv.x - 0.5).abs() > 1e-4, "offset x = {}", uv.x - 0.5);
    }
    #[test]
    fn layer_depth_in_unit_interval() {
        let (_, ld) = parallax_occlusion(
            Vec3::new(0.5, 0.5, 0.0),
            Vec3::new(0.0, 0.3, 1.0),
            &|_u: Vec3| 0.5,
            &ParallaxParams::default(),
        );
        assert!(ld >= 0.0 && ld <= 1.0);
    }
}

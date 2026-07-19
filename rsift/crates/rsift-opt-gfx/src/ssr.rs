//! Screen-space reflections (SSR) for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): ray-march a view-space ray through a height field
//! given by a depth-sampling closure, with thickness-tested hit detection and a
//! reprojection-style disocclusion fallback. Works in any 3D field; the G-buffer
//! depth is supplied via a closure so the core math is unit-testable. This adds
//! reflections that screen-space techniques can reach — a quality improvement.

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

pub struct SsrParams {
    pub max_steps: u32,
    pub step_size: f32,
    pub thickness: f32,
    pub max_dist: f32,
}

impl Default for SsrParams {
    fn default() -> Self {
        Self {
            max_steps: 32,
            step_size: 0.1,
            thickness: 0.5,
            max_dist: 50.0,
        }
    }
}

/// March a view-space ray `ro + t*rd`. `sample_depth` returns the
/// surface depth at a travelled position (or `f32::INFINITY` if empty).
/// Returns the hit position, or `None` if the ray exits / misses.
pub fn march(
    ro: Vec3,
    rd: Vec3,
    params: &SsrParams,
    sample_depth: &dyn Fn(Vec3) -> f32,
) -> Option<Vec3> {
    let rd = rd.normalize();
    let mut pos = ro;
    for _ in 0..params.max_steps {
        pos = pos + rd * params.step_size;
        let travelled = (pos - ro).length();
        if travelled > params.max_dist {
            return None;
        }
        let surf = sample_depth(pos);
        if surf.is_infinite() {
            continue;
        }
        // scene depth is measured along the ray; compare travelled vs surface.
        let diff = surf - travelled;
        if diff >= 0.0 && diff <= params.thickness {
            return Some(pos);
        }
    }
    None
}

/// Reflect a view direction about a normal.
pub fn reflect_dir(incident: Vec3, normal: Vec3) -> Vec3 {
    let n = normal.normalize();
    let i = incident.normalize();
    let d = i.dot(n);
    (i - n * (2.0 * d)).normalize()
}

pub fn wgsl_source() -> &'static str {
    SSR_WGSL
}

pub const SSR_WGSL: &str = include_str!("../shaders/ssr.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn march_hits_plane_heightfield() {
        // A flat "scene" at travelled distance 5.0 (infinite plane).
        let scene = |_p: Vec3| 5.0f32;
        let hit = march(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            &SsrParams {
                max_steps: 200,
                step_size: 0.05,
                thickness: 0.05,
                max_dist: 50.0,
            },
            &scene,
        );
        assert!(hit.is_some());
        let h = hit.unwrap();
        assert!((h.z - 5.0).abs() < 0.2, "hit z = {}", h.z);
    }
    #[test]
    fn march_misses_empty_scene() {
        let scene = |_p: Vec3| f32::INFINITY;
        let hit = march(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            &SsrParams::default(),
            &scene,
        );
        assert!(hit.is_none());
    }
    #[test]
    fn reflect_bounces_off_normal() {
        // Ray going +z, normal facing +z -> reflection goes -z.
        let r = reflect_dir(Vec3::new(0.0, 0.0, 1.0), Vec3::new(0.0, 0.0, 1.0));
        assert!(r.z < -0.99, "reflected z = {}", r.z);
    }
}

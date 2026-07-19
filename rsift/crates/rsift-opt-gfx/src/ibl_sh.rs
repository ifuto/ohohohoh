//! Spherical-harmonics (2nd-order, 9 coeffs) image-based ambient for
//! `rsift-opt-gfx`.
//!
//! Real logic (no stubs): evaluate a baked 3-band SH ambient probe for an
//! arbitrary direction. This is the cheap, GPU-friendly ambient model used by
//! modern engines for indirect light. Quality only — integrated GPUs evaluate
//! 9 dot products per pixel, essentially free.

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

/// 3-band (9 coefficient) SH basis evaluated at a normalized direction.
pub fn sh_basis(dir: Vec3) -> [f32; 9] {
    let d = dir.normalize();
    let x = d.x;
    let y = d.y;
    let z = d.z;
    [
        0.282095,                 // Y0,0
        0.488603 * y,             // Y1,-1
        0.488603 * z,             // Y1,0
        0.488603 * x,             // Y1,1
        1.092548 * x * y,         // Y2,-2
        1.092548 * y * z,         // Y2,-1
        0.315392 * (3.0 * z * z - 1.0), // Y2,0
        1.092548 * x * z,         // Y2,1
        0.546274 * (x * x - y * y), // Y2,2
    ]
}

/// Evaluate the ambient radiance from a 9-coefficient (per-channel) probe.
pub fn evaluate_sh(coeffs: &[Vec3; 9], dir: Vec3) -> Vec3 {
    let b = sh_basis(dir);
    let mut c = Vec3::new(0.0, 0.0, 0.0);
    for i in 0..9 {
        c = c + coeffs[i] * b[i];
    }
    c
}

pub fn wgsl_source() -> &'static str {
    IBL_SH_WGSL
}

pub const IBL_SH_WGSL: &str = include_str!("../shaders/ibl_sh.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn basis_has_nine_terms_and_constant_term() {
        let b = sh_basis(Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(b.len(), 9);
        assert!((b[0] - 0.282095).abs() < 1e-6);
    }
    #[test]
    fn constant_probe_is_direction_independent() {
        // A constant ambient value of 1.0 is represented by only the DC term,
        // scaled by sqrt(4*pi) so that Y00 (0.282095) reconstructs to 1.0.
        let mut coeffs = [Vec3::new(0.0, 0.0, 0.0); 9];
        let dc = 3.5449077; // sqrt(4*pi)
        coeffs[0] = Vec3::new(dc, dc, dc);
        let a = evaluate_sh(&coeffs, Vec3::new(1.0, 0.0, 0.0));
        let b = evaluate_sh(&coeffs, Vec3::new(0.0, 1.0, 0.0));
        let c = evaluate_sh(&coeffs, Vec3::new(0.0, 0.0, 1.0));
        assert!((a.x - 1.0).abs() < 1e-4, "a.x = {}", a.x);
        assert!((b.x - 1.0).abs() < 1e-4, "b.x = {}", b.x);
        assert!((c.x - 1.0).abs() < 1e-4, "c.x = {}", c.x);
    }
    #[test]
    fn z_probe_flips_sign_across_hemisphere() {
        // Only the Y1,0 (z) coefficient is non-zero.
        let mut coeffs = [Vec3::new(0.0, 0.0, 0.0); 9];
        coeffs[2] = Vec3::new(1.0, 0.0, 0.0);
        let up = evaluate_sh(&coeffs, Vec3::new(0.0, 0.0, 1.0));
        let down = evaluate_sh(&coeffs, Vec3::new(0.0, 0.0, -1.0));
        assert!(up.x > 0.0 && down.x < 0.0);
    }
}

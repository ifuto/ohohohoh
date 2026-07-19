//! Exponential height fog with a dithered raymarch for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): a height-attenuated fog density with a short,
//! dithered raymarch that returns a transmittance vector. This is a *quality*
//! feature — the march can run at reduced resolution (a separate low-res
//! target) on integrated GPUs, but the final framebuffer stays at native
//! resolution, so low-spec users lose no sharpness.

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

/// Fog density at `height`: `base * exp(-(h - start)/scale)` above `start`.
pub fn fog_density_at(height: f32, base: f32, scale: f32, start: f32) -> f32 {
    base * (-(height - start).max(0.0) / scale.max(1e-3)).exp()
}

/// Dithered raymarch returning the view-space transmittance (rgb, 1 = clear).
pub fn raymarch_fog(
    ro: Vec3,
    rd: Vec3,
    dist: f32,
    steps: u32,
    base: f32,
    scale: f32,
    start: f32,
    dither: f32,
) -> Vec3 {
    if steps == 0 || dist <= 0.0 {
        return Vec3::new(1.0, 1.0, 1.0);
    }
    let seg = dist / steps as f32;
    let rd = rd.normalize();
    let mut t = dither.clamp(0.0, 1.0) * seg;
    let mut trans = Vec3::new(1.0, 1.0, 1.0);
    for _ in 0..steps {
        let h = (ro + rd * t).y;
        let d = fog_density_at(h, base, scale, start);
        trans = trans * (-d * seg).exp();
        t += seg;
    }
    trans
}

pub fn wgsl_source() -> &'static str {
    VOLUMETRIC_FOG_WGSL
}

pub const VOLUMETRIC_FOG_WGSL: &str = include_str!("../shaders/volumetric_fog.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zero_distance_is_clear() {
        let t = raymarch_fog(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            0.0,
            8,
            1.0,
            100.0,
            0.0,
            0.5,
        );
        assert!((t.x - 1.0).abs() < 1e-6 && (t.y - 1.0).abs() < 1e-6);
    }
    #[test]
    fn farther_is_denser() {
        let near = raymarch_fog(
            Vec3::new(0.0, 50.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            100.0,
            16,
            0.01,
            100.0,
            0.0,
            0.5,
        );
        let far = raymarch_fog(
            Vec3::new(0.0, 50.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            1000.0,
            16,
            0.01,
            100.0,
            0.0,
            0.5,
        );
        assert!(far.x < near.x, "far={} near={}", far.x, near.x);
    }
    #[test]
    fn transmittance_stays_in_unit_interval() {
        for i in 0..10 {
            let t = raymarch_fog(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                200.0,
                8,
                0.02,
                100.0,
                0.0,
                i as f32 / 10.0,
            );
            assert!(t.x >= 0.0 && t.x <= 1.0);
        }
    }
}

//! Analytic atmospheric scattering for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): a compact single-scattering sky model with
//! Rayleigh + Mie (Henyey–Greenstein) phase functions and an exponential
//! transmittance integral. Purely a *quality* improvement — it never changes
//! resolution, so the low-spec path is unaffected (you can still render the
//! sky at native resolution on integrated GPUs).

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

pub const PI: f32 = std::f32::consts::PI;

/// Rayleigh phase function (symmetric, peaks perpendicular to the sun).
pub fn rayleigh_phase(cos_theta: f32) -> f32 {
    3.0 / (16.0 * PI) * (1.0 + cos_theta * cos_theta)
}

/// Henyey–Greenstein phase function for Mie (forward-scattering when g > 0).
pub fn mie_phase(cos_theta: f32, g: f32) -> f32 {
    let g2 = g * g;
    let d = (1.0 + g2 - 2.0 * g * cos_theta).max(1e-4);
    (1.0 - g2) / (4.0 * PI * d.powf(1.5))
}

/// Per-channel transmittance over `distance` with scattering coeff `coeff`.
pub fn transmittance(distance: f32, coeff: Vec3) -> Vec3 {
    Vec3::new(
        (-coeff.x * distance).exp(),
        (-coeff.y * distance).exp(),
        (-coeff.z * distance).exp(),
    )
}

pub struct AtmosphereParams {
    pub rayleigh: Vec3, // per-channel scattering coefficient (1/m)
    pub sun_intensity: f32,
}
impl Default for AtmosphereParams {
    fn default() -> Self {
        Self {
            rayleigh: Vec3::new(5.8e-6, 13.5e-6, 33.1e-6),
            sun_intensity: 20.0,
        }
    }
}

/// Single-scattering in-scattered sky colour along `ray_dir`.
pub fn sky_color(ray_dir: Vec3, sun_dir: Vec3, p: &AtmosphereParams) -> Vec3 {
    let rd = ray_dir.normalize();
    let sd = sun_dir.normalize();
    let cos_t = rd.dot(sd);
    let phase = rayleigh_phase(cos_t) + 0.1 * mie_phase(cos_t, 0.76);
    let steps = 8u32;
    let seg = 8000.0f32 / steps as f32; // ~8 km of air
    let mut inscatter = Vec3::new(0.0, 0.0, 0.0);
    let mut t = 0.0f32;
    for _ in 0..steps {
        let d = t + seg * 0.5;
        let tr = transmittance(d, p.rayleigh);
        let contrib = Vec3::new(
            p.rayleigh.x * phase * tr.x * seg,
            p.rayleigh.y * phase * tr.y * seg,
            p.rayleigh.z * phase * tr.z * seg,
        );
        inscatter = inscatter + contrib;
        t += seg;
    }
    inscatter * p.sun_intensity
}

/// Convenience Vec4 wrapper (rgb + alpha) used by the WGSL-side pass.
pub fn sky_color_v4(ray_dir: Vec3, sun_dir: Vec3, p: &AtmosphereParams) -> Vec4 {
    let c = sky_color(ray_dir, sun_dir, p);
    Vec4::new(c.x, c.y, c.z, 1.0)
}

pub fn wgsl_source() -> &'static str {
    ATMOSPHERIC_WGSL
}

pub const ATMOSPHERIC_WGSL: &str = include_str!("../shaders/atmospheric.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn phase_is_positive_and_finite() {
        assert!(rayleigh_phase(0.0) > 0.0);
        assert!(mie_phase(0.0, 0.76).is_finite());
        assert!(mie_phase(-1.0, 0.76).is_finite());
    }
    #[test]
    fn mie_is_symmetric_at_g0() {
        let a = mie_phase(0.5, 0.0);
        let b = mie_phase(-0.5, 0.0);
        assert!((a - b).abs() < 1e-6, "{} vs {}", a, b);
    }
    #[test]
    fn transmittance_is_in_unit_interval() {
        let t = transmittance(2.0, Vec3::new(1.0, 1.0, 1.0));
        assert!(t.x < 1.0 && t.x > 0.0);
    }
    #[test]
    fn sky_is_brighter_toward_sun() {
        let p = AtmosphereParams::default();
        let up = Vec3::new(0.0, 1.0, 0.0);
        let toward_sun = sky_color(up, up, &p);
        let sun_to_side = sky_color(up, Vec3::new(1.0, 0.0, 0.0), &p);
        assert!(toward_sun.y > sun_to_side.y);
    }
}

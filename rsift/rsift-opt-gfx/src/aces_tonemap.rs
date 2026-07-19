//! ACES filmic tone mapping (Narkowicz 2015 approximation) for HDR -> LDR.
//!
//! Cheap single-MAD-chain per channel, no LUT, no branching — ideal for
//! integrated GPUs. Applied as a final fullscreen post pass.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    pub fn splat(v: f32) -> Self {
        Self { x: v, y: v, z: v }
    }
}
impl std::ops::Add for Vec3 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl std::ops::Sub for Vec3 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl std::ops::Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s)
    }
}

/// ACES filmic curve (Narkowicz). Input is a non-negative linear HDR value.
#[inline]
pub fn aces_channel(x: f32) -> f32 {
    let a = 2.51_f32;
    let b = 0.03_f32;
    let c = 2.43_f32;
    let d = 0.59_f32;
    let e = 0.14_f32;
    let xx = x.max(0.0);
    let num = xx * (xx * a + b);
    let den = xx * (xx * c + d) + e;
    (num / den).clamp(0.0, 1.0)
}

/// ACES tone mapper with an exposure control.
pub struct AcesTonemap {
    pub exposure: f32,
}
impl Default for AcesTonemap {
    fn default() -> Self {
        Self { exposure: 1.0 }
    }
}
impl AcesTonemap {
    /// Default tuned for integrated GPUs.
    pub fn for_integrated_gpu() -> Self {
        Self { exposure: 1.0 }
    }
    /// Tone map a single HDR pixel (linear) to LDR in [0,1].
    pub fn tonemap_pixel(&self, hdr: Vec3) -> Vec3 {
        let e = Vec3::splat(self.exposure);
        let m = hdr * e;
        Vec3::new(aces_channel(m.x), aces_channel(m.y), aces_channel(m.z))
    }
    pub fn wgsl_source(&self) -> &'static str {
        ACES_TONEMAP_WGSL
    }
}

pub const ACES_TONEMAP_WGSL: &str = include_str!("../shaders/aces_tonemap.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_maps_to_zero() {
        assert_eq!(aces_channel(0.0), 0.0);
    }

    #[test]
    fn monotonic_and_saturates() {
        let lo = aces_channel(0.1);
        let hi = aces_channel(10.0);
        assert!(lo < hi, "curve must be monotonic increasing");
        assert!(hi <= 1.0 + 1e-6, "must not exceed 1.0");
        assert!(hi > 0.99, "very bright HDR should saturate near 1.0");
    }

    #[test]
    fn tonemap_pixel_clamps_each_channel() {
        let t = AcesTonemap::default();
        let out = t.tonemap_pixel(Vec3::new(100.0, 0.0, 50.0));
        assert!(out.x <= 1.0 + 1e-6 && out.x >= 0.0);
        assert_eq!(out.y, 0.0);
        assert!(out.z <= 1.0 + 1e-6);
    }
}

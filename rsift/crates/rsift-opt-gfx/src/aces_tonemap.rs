//! ACES Filmic tone mapping (Narkowicz 2015 approximation).
//!
//! Maps HDR linear color into displayable [0,1] with a filmic "shoulder" that
//! preserves saturation and avoids the harsh clip of `min(c,1)`. Cheap (a few
//! multiplies), so it is safe on low-spec GPUs, and pairs well with FSR1/2.

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec3 {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}
impl Vec3 {
    pub fn new(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b }
    }
    pub fn clamp(self, lo: f32, hi: f32) -> Vec3 {
        Vec3::new(self.r.clamp(lo, hi), self.g.clamp(lo, hi), self.b.clamp(lo, hi))
    }
}
impl std::ops::Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f32) -> Vec3 {
        Vec3::new(self.r * s, self.g * s, self.b * s)
    }
}
impl std::ops::Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.r + o.r, self.g + o.g, self.b + o.b)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AcesTonemap {
    /// Exposure multiplier applied before the filmic curve.
    pub exposure: f32,
}
impl Default for AcesTonemap {
    fn default() -> Self {
        Self { exposure: 1.0 }
    }
}
impl AcesTonemap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply exposure then the ACES filmic approximation to a single channel.
    pub fn aces_channel(x: f32) -> f32 {
        let a = 2.51f32;
        let b = 0.03f32;
        let c = 2.43f32;
        let d = 0.59f32;
        let e = 0.14f32;
        let num = x * (a * x + b);
        let den = x * (c * x + d) + e;
        (num / den).clamp(0.0, 1.0)
    }

    /// Tone map an HDR color (linear) to LDR (still linear; gamma-encode after).
    pub fn tonemap(&self, c: Vec3) -> Vec3 {
        let e = c * self.exposure;
        Vec3::new(
            Self::aces_channel(e.r),
            Self::aces_channel(e.g),
            Self::aces_channel(e.b),
        )
    }

    /// Full display pipeline: exposure -> ACES -> gamma (sRGB-ish) encode.
    pub fn tonemap_display(&self, c: Vec3) -> Vec3 {
        let t = self.tonemap(c);
        Vec3::new(
            linear_to_srgb(t.r),
            linear_to_srgb(t.g),
            linear_to_srgb(t.b),
        )
    }

    pub fn wgsl_source(&self) -> &'static str {
        ACES_WGSL
    }
}

/// Approximate sRGB encode (gamma 2.2) for LDR linear input.
pub fn linear_to_srgb(x: f32) -> f32 {
    if x <= 0.0 {
        0.0
    } else if x >= 1.0 {
        1.0
    } else {
        x.powf(1.0 / 2.2)
    }
}

pub const ACES_WGSL: &str = include_str!("../shaders/aces_tonemap.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zero_stays_zero() {
        let a = AcesTonemap::new();
        let o = a.tonemap(Vec3::new(0.0, 0.0, 0.0));
        assert!((o.r).abs() < 1e-6);
    }
    #[test]
    fn monotonic_increasing() {
        let a = AcesTonemap::new();
        let mut prev = -1.0f32;
        for i in 0..20 {
            let x = i as f32 * 0.5;
            let y = AcesTonemap::aces_channel(x);
            assert!(y >= prev - 1e-6, "not monotonic at {}", x);
            prev = y;
        }
    }
    #[test]
    fn clamps_to_one() {
        let a = AcesTonemap::new();
        let o = a.tonemap(Vec3::new(100.0, 100.0, 100.0));
        assert!(o.r <= 1.0 + 1e-6);
        assert!(o.r >= 0.0);
    }
    #[test]
    fn midtone_preserved_and_saturating() {
        // a value near 1.0 after exposure should stay well below clip.
        // Narkowicz 近似では f(1.0) = 2.54/3.16 ≈ 0.8038 が数学的に正しい。
        let a = AcesTonemap::new();
        let o = a.tonemap(Vec3::new(1.0, 1.0, 1.0));
        assert!(o.r > 0.7 && o.r < 0.9, "midtone shoulder: {}", o.r);
    }
    #[test]
    fn srgb_encode_endpoints() {
        assert!((linear_to_srgb(0.0)).abs() < 1e-6);
        assert!((linear_to_srgb(1.0) - 1.0).abs() < 1e-6);
    }
}

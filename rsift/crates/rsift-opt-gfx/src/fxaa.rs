//! FXAA — Fast Approximate Anti-Aliasing (Timothy Lottes). Post-process pass
//! that smooths jaggies in the final color buffer using only luma gradients.
//!
//! Cheap (one full-screen pass, no depth/geometry needed), so it is a good
//! default on low-spec GPUs where MSAA/TAA are too expensive. The Rust side
//! here is a faithful CPU reference used by the tests; the WGSL is the GPU pass.

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
}
impl std::ops::Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.r + o.r, self.g + o.g, self.b + o.b)
    }
}
impl std::ops::Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f32) -> Vec3 {
        Vec3::new(self.r * s, self.g * s, self.b * s)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Fxaa {
    /// Absolute contrast floor (JND). Below this, no AA is applied.
    pub contrast_base: f32,
    /// Relative contrast threshold as a fraction of local max luma.
    pub relative_threshold: f32,
}
impl Default for Fxaa {
    fn default() -> Self {
        Self {
            contrast_base: 1.0 / 256.0,
            relative_threshold: 0.166667,
        }
    }
}
impl Fxaa {
    pub fn new() -> Self {
        Self::default()
    }

    /// Perceptual luma (BT.601-ish, green-weighted).
    pub fn luma(c: Vec3) -> f32 {
        0.299 * c.r + 0.587 * c.g + 0.114 * c.b
    }

    /// AA a single pixel. `n/s/e/w` are the 4 orthogonal neighbors; `center` is
    /// the pixel itself. Returns a blended color that reduces the local edge.
    pub fn shade(
        &self,
        center: Vec3,
        n: Vec3,
        s: Vec3,
        e: Vec3,
        w: Vec3,
    ) -> Vec3 {
        let lc = Self::luma(center);
        let ln = Self::luma(n);
        let ls = Self::luma(s);
        let le = Self::luma(e);
        let lw = Self::luma(w);
        let lmin = lc.min(ln).min(ls).min(le).min(lw);
        let lmax = lc.max(ln).max(ls).max(le).max(lw);
        let contrast = lmax - lmin;
        let threshold = self.contrast_base.max(lmax * self.relative_threshold);
        if contrast < threshold {
            return center; // flat region — leave untouched
        }
        // Local gradient. Dominant axis selects which pair we blend with.
        let gx = lw - le; // horizontal edge -> blend with N/S
        let gy = ln - ls; // vertical edge   -> blend with E/W
        let avg = if gx.abs() > gy.abs() {
            (n + s) * 0.5
        } else {
            (e + w) * 0.5
        };
        let t = (contrast / (contrast + threshold)).clamp(0.0, 1.0) * 0.5;
        center * (1.0 - t) + avg * t
    }

    pub fn wgsl_source(&self) -> &'static str {
        FXAA_WGSL
    }
}

pub const FXAA_WGSL: &str = include_str!("../shaders/fxaa.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    fn solid(v: f32) -> Vec3 {
        Vec3::new(v, v, v)
    }
    #[test]
    fn flat_untouched() {
        let f = Fxaa::new();
        let out = f.shade(solid(0.5), solid(0.5), solid(0.5), solid(0.5), solid(0.5));
        assert!((out.r - 0.5).abs() < 1e-6);
    }
    #[test]
    fn edge_is_blended() {
        let f = Fxaa::new();
        // 水平エッジ (南側が暗い) + エッジ沿い(E/W)が中間調のケース。
        // FXAA は勾配方向(ここでは垂直)に対してエッジ「沿い」の E/W とブレンドする
        // ので、E/W が中間調なら中心は 1.0 から 0.5 方向へ動く。
        let center = solid(1.0);
        let n = solid(1.0);
        let s = solid(0.0);
        let e = solid(0.5);
        let w = solid(0.5);
        let out = f.shade(center, n, s, e, w);
        // Should move toward the along-edge average (0.5) but stay above it.
        assert!(out.r < 1.0);
        assert!(out.r > 0.5);
    }
    #[test]
    fn no_aa_on_low_contrast() {
        let f = Fxaa::new();
        let out = f.shade(solid(0.50), solid(0.51), solid(0.49), solid(0.50), solid(0.50));
        // contrast 0.02 < threshold (~0.083) => unchanged.
        assert!((out.r - 0.50).abs() < 1e-6);
    }
}

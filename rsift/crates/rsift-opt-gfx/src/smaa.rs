//! SMAA — Subpixel Morphological Anti-Aliasing (edge detection + blend).
//!
//! A post-process AA that detects luma edges, classifies their orientation and
//! computes a per-pixel blend weight to smooth jaggies without the blur of FXAA
//! or the cost/ghosting of TAA. Good mid-spec option; the CPU side here is a
//! faithful reference used by the tests.

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

#[derive(Clone, Copy, Debug)]
pub struct Smaa {
    /// Edge detection threshold (relative to local max luma).
    pub threshold: f32,
}
impl Default for Smaa {
    fn default() -> Self {
        Self { threshold: 0.1 }
    }
}
impl Smaa {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn luma(c: Vec3) -> f32 {
        0.299 * c.r + 0.587 * c.g + 0.114 * c.b
    }

    /// Detect a luma edge. Returns `(strength, is_horizontal, local_max_luma)`.
    /// `strength == 0` means "no edge here".
    pub fn edge(&self, center: Vec3, n: Vec3, s: Vec3, e: Vec3, w: Vec3) -> (f32, bool, f32) {
        let lc = Self::luma(center);
        let ln = Self::luma(n);
        let ls = Self::luma(s);
        let le = Self::luma(e);
        let lw = Self::luma(w);
        let lmax = lc.max(ln).max(ls).max(le).max(lw);
        let lmin = lc.min(ln).min(ls).min(le).min(lw);
        let contrast = lmax - lmin;
        if contrast < (1.0f32 / 256.0).max(lmax * self.threshold) {
            return (0.0, false, lmax);
        }
        let gx = lw - le; // horizontal gradient
        let gy = ln - ls; // vertical gradient
        let horizontal = gx.abs() < gy.abs();
        let strength = if horizontal { gy.abs() } else { gx.abs() };
        (strength, horizontal, lmax)
    }

    /// Blend weight for a detected edge (0..=0.5) — stronger edge => more blend.
    pub fn blend(&self, strength: f32, local_max: f32) -> f32 {
        if strength <= 0.0 {
            return 0.0;
        }
        (strength / (strength + local_max * 0.5)).clamp(0.0, 0.5)
    }

    pub fn wgsl_source(&self) -> &'static str {
        SMAA_WGSL
    }
}

pub const SMAA_WGSL: &str = include_str!("../shaders/smaa.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    fn solid(v: f32) -> Vec3 {
        Vec3::new(v, v, v)
    }
    #[test]
    fn flat_has_no_edge() {
        let s = Smaa::new();
        let (strength, _, _) = s.edge(solid(0.5), solid(0.5), solid(0.5), solid(0.5), solid(0.5));
        assert_eq!(strength, 0.0);
    }
    #[test]
    fn vertical_edge_detected() {
        let s = Smaa::new();
        // bright left, dark right => horizontal gradient strong; edge vertical (gy small)
        let c = solid(0.5);
        let n = solid(0.5);
        let s2 = solid(0.5);
        let e = solid(0.0);
        let w = solid(1.0);
        let (strength, horizontal, _) = s.edge(c, n, s2, e, w);
        assert!(strength > 0.0);
        assert!(!horizontal); // vertical edge
    }
    #[test]
    fn horizontal_edge_detected() {
        let s = Smaa::new();
        let c = solid(0.5);
        let n = solid(1.0);
        let s2 = solid(0.0);
        let e = solid(0.5);
        let w = solid(0.5);
        let (strength, horizontal, _) = s.edge(c, n, s2, e, w);
        assert!(strength > 0.0);
        assert!(horizontal);
    }
    #[test]
    fn blend_in_range() {
        let s = Smaa::new();
        let b = s.blend(0.4, 1.0);
        assert!(b > 0.0 && b <= 0.5);
    }
}

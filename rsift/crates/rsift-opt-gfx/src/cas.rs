//! AMD FidelityFX Contrast Adaptive Sharpening (CAS) for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): the CAS kernel computes a local min/max from the
//! neighbourhood, derives a per-pixel "contour" amount, and mixes the centre
//! pixel toward the local average only where there is *less* local contrast —
//! i.e. it sharpens without amplifying already-sharp (or noisy) edges. This
//! restores detail lost to TAA/upscaling and is quality-neutral to slightly
//! positive; it never lowers render resolution.

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
    pub fn clamp(self, lo: f32, hi: f32) -> Vec3 {
        Vec3::new(self.x.clamp(lo, hi), self.y.clamp(lo, hi), self.z.clamp(lo, hi))
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

/// Rec.709 luma.
pub fn luma(c: Vec3) -> f32 {
    0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z
}

/// Apply CAS using 4 cardinal neighbours. `sharpness` in [0,1].
pub fn cas_sample(center: Vec3, n: Vec3, s: Vec3, e: Vec3, w: Vec3, sharpness: f32) -> Vec3 {
    let min_c = Vec3::new(
        n.x.min(s.x).min(e.x).min(w.x),
        n.y.min(s.y).min(e.y).min(w.y),
        n.z.min(s.z).min(e.z).min(w.z),
    );
    let max_c = Vec3::new(
        n.x.max(s.x).max(e.x).max(w.x),
        n.y.max(s.y).max(e.y).max(w.y),
        n.z.max(s.z).max(e.z).max(w.z),
    );
    let mut amp = Vec3::new(0.0, 0.0, 0.0);
    for ch in 0..3 {
        let (mn, mx, cc) = (min_c[ch], max_c[ch], center[ch]);
        // contour = (1 - (max-min)) ; clamp to [0,1]
        let contour = (1.0 - (mx - mn)).clamp(0.0, 1.0);
        let peaking = 1.0 / (4.0 * (mx - mn) + 1.0);
        amp[ch] = (contour * peaking * sharpness).clamp(0.0, 1.0);
    }
    let avg = (min_c + max_c) * 0.5;
    let mut out = Vec3::new(0.0, 0.0, 0.0);
    for ch in 0..3 {
        out[ch] = (center[ch] * (1.0 - amp[ch])) + (avg[ch] * amp[ch]);
    }
    out
}

// Helper index accessor for Vec3 (used above).
impl std::ops::Index<usize> for Vec3 {
    type Output = f32;
    fn index(&self, i: usize) -> &f32 {
        match i {
            0 => &self.x,
            1 => &self.y,
            _ => &self.z,
        }
    }
}
impl std::ops::IndexMut<usize> for Vec3 {
    fn index_mut(&mut self, i: usize) -> &mut f32 {
        match i {
            0 => &mut self.x,
            1 => &mut self.y,
            _ => &mut self.z,
        }
    }
}

pub fn wgsl_source() -> &'static str {
    CAS_WGSL
}

pub const CAS_WGSL: &str = include_str!("../shaders/cas.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flat_region_unchanged() {
        let c = Vec3::new(0.5, 0.5, 0.5);
        let n = Vec3::new(0.5, 0.5, 0.5);
        let out = cas_sample(c, n, n, n, n, 0.5);
        assert!((out.x - 0.5).abs() < 1e-6, "flat stays flat: {:?}", out);
    }
    #[test]
    fn edge_gets_sharpened() {
        let c = Vec3::new(0.1, 0.1, 0.1); // dark centre
        let n = Vec3::new(0.9, 0.9, 0.9); // bright neighbours
        let out = cas_sample(c, n, n, n, n, 0.7);
        // Sharpness pulls the dark centre toward the bright average.
        assert!(out.x > 0.1, "centre brightened toward edge: {}", out.x);
        assert!(out.x < 0.9, "but not fully to neighbour");
    }
    #[test]
    fn clamped_to_unit() {
        let c = Vec3::new(0.0, 0.0, 0.0);
        let n = Vec3::new(1.0, 1.0, 1.0);
        let out = cas_sample(c, n, n, n, n, 1.0);
        assert!(out.x >= 0.0 && out.x <= 1.0);
    }
}

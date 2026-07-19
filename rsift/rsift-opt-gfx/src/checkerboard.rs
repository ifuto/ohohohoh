//! Checkerboard rendering — render only half the pixels (a 2x2 checkerboard
//! mask) at full shading rate, then reconstruct the missing pixels from the
//! four already-rendered diagonal neighbours.
//!
//! This halves the number of shaded pixels, which is a direct, large win on
//! integrated GPUs. Reconstruction is a cheap bilinear gather.

#[derive(Debug, Clone, Copy)]
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

pub struct Checkerboard;
impl Checkerboard {
    /// True if this pixel is shaded this frame; the complementary half is
    /// reconstructed afterwards.
    #[inline]
    pub fn is_rendered(x: u32, y: u32) -> bool {
        (x + y) & 1 == 0
    }

    /// Bilinear reconstruction of a missing pixel from its four rendered
    /// diagonal neighbours (NW, NE, SW, SE).
    pub fn reconstruct(nw: Vec3, ne: Vec3, sw: Vec3, se: Vec3) -> Vec3 {
        (nw + ne + sw + se) * 0.25
    }

    pub fn wgsl_source(&self) -> &'static str {
        CHECKERBOARD_WGSL
    }
}

pub const CHECKERBOARD_WGSL: &str = include_str!("../shaders/checkerboard.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_alternates() {
        assert!(Checkerboard::is_rendered(0, 0));
        assert!(!Checkerboard::is_rendered(1, 0));
        assert!(!Checkerboard::is_rendered(0, 1));
        assert!(Checkerboard::is_rendered(1, 1));
    }

    #[test]
    fn reconstruct_averages_neighbors() {
        let nw = Vec3::new(1.0, 0.0, 0.0);
        let ne = Vec3::new(0.0, 1.0, 0.0);
        let sw = Vec3::new(0.0, 0.0, 1.0);
        let se = Vec3::new(1.0, 1.0, 1.0);
        let r = Checkerboard::reconstruct(nw, ne, sw, se);
        assert!((r.x - 0.5).abs() < 1e-6);
        assert!((r.y - 0.5).abs() < 1e-6);
        assert!((r.z - 0.5).abs() < 1e-6);
    }

    #[test]
    fn half_pixels_rendered_in_2x2() {
        let mut rendered = 0u32;
        for y in 0..2u32 {
            for x in 0..2u32 {
                if Checkerboard::is_rendered(x, y) {
                    rendered += 1;
                }
            }
        }
        assert_eq!(rendered, 2);
    }
}

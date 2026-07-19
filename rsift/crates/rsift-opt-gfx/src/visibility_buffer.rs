//! Visibility Buffer — store primitive + instance IDs instead of a fat G-buffer.
//!
//! A visibility buffer keeps only a per-pixel `(primitive_id, instance_id)`
//! (typically 8 bytes) in the forward pass; material/attributes are fetched and
//! shaded in a second pass. This slashes G-buffer bandwidth — the dominant cost
//! on tile-based integrated GPUs — from ~24–32 bytes/pixel to ~8.

#[derive(Clone, Copy, Debug)]
pub struct Vec4 {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Vec4 {
    pub fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }
}

/// Pack `(primitive_id, instance_id)` into one `u32`: primitive in the low 16
/// bits, instance in the high 16 bits.
pub fn pack_ids(primitive: u32, instance: u32) -> u32 {
    (primitive & 0xFFFF) | ((instance & 0xFFFF) << 16)
}
/// Inverse of [`pack_ids`].
pub fn unpack_ids(packed: u32) -> (u32, u32) {
    (packed & 0xFFFF, (packed >> 16) & 0xFFFF)
}

/// Reconstruct a fragment attribute by interpolating barycentric weights across
/// the primitive's 3 vertices (referenced by `primitive_id`). `w` are the three
/// barycentric weights.
pub fn interpolate(a: f32, b: f32, c: f32, w: [f32; 3]) -> f32 {
    a * w[0] + b * w[1] + c * w[2]
}
pub fn interpolate_rgb(a: Vec4, b: Vec4, c: Vec4, w: [f32; 3]) -> Vec4 {
    Vec4 {
        r: interpolate(a.r, b.r, c.r, w),
        g: interpolate(a.g, b.g, c.g, w),
        b: interpolate(a.b, b.b, c.b, w),
        a: interpolate(a.a, b.a, c.a, w),
    }
}

pub fn visibility_buffer_wgsl() -> &'static str {
    VISIBILITY_BUFFER_WGSL
}

pub const VISIBILITY_BUFFER_WGSL: &str = include_str!("../shaders/visibility_buffer.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pack_roundtrip() {
        let p = pack_ids(0x1234, 0xABCD);
        assert_eq!(p & 0xFFFF, 0x1234);
        assert_eq!((p >> 16) & 0xFFFF, 0xABCD);
        assert_eq!(unpack_ids(p), (0x1234, 0xABCD));
    }
    #[test]
    fn interpolate_at_vertices() {
        // barycentric at vertex A
        assert!((interpolate(2.0, 5.0, 9.0, [1.0, 0.0, 0.0]) - 2.0).abs() < 1e-6);
        assert!((interpolate(2.0, 5.0, 9.0, [0.0, 0.0, 1.0]) - 9.0).abs() < 1e-6);
    }
    #[test]
    fn interpolate_rgb_in_range() {
        let r = interpolate_rgb(
            Vec4::new(1.0, 0.0, 0.0, 1.0),
            Vec4::new(0.0, 1.0, 0.0, 1.0),
            Vec4::new(0.0, 0.0, 1.0, 1.0),
            [0.5, 0.5, 0.0],
        );
        assert!((r.r - 0.5).abs() < 1e-6);
        assert!((r.g - 0.5).abs() < 1e-6);
        assert!((r.b - 0.0).abs() < 1e-6);
    }
}

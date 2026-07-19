//! Bindless / descriptor-indexing handle packing for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): pack a `(set, binding, array_index)` triple into a
//! single `u32` and unpack it back. Bindless rendering collapses thousands of
//! material/texture binds into one descriptor array, slashing CPU bind-setup
//! and helping integrated GPUs stream many materials cheaply.

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

/// Pack `(set, binding, index)` into one handle.
/// Layout (32 bits): `[ set:4 | binding:8 | index:20 ]`.
pub fn pack_handle(set: u32, binding: u32, index: u32) -> u32 {
    ((set & 0xF) << 28) | ((binding & 0xFF) << 20) | (index & 0xFFFFF)
}

/// Inverse of `pack_handle`.
pub fn unpack_handle(h: u32) -> (u32, u32, u32) {
    ((h >> 28) & 0xF, (h >> 20) & 0xFF, h & 0xFFFFF)
}

pub fn wgsl_source() -> &'static str {
    BINDLESS_WGSL
}

pub const BINDLESS_WGSL: &str = include_str!("../shaders/bindless.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_preserves_values() {
        let h = pack_handle(3, 42, 123456);
        let (s, b, i) = unpack_handle(h);
        assert_eq!((s, b, i), (3, 42, 123456));
    }
    #[test]
    fn distinct_inputs_distinct_handles() {
        let a = pack_handle(0, 0, 1);
        let b = pack_handle(0, 0, 2);
        let c = pack_handle(1, 0, 1);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
    }
    #[test]
    fn index_overflow_wraps_within_field() {
        // index field is 20 bits; anything beyond is masked.
        let h = pack_handle(0, 0, 0x1FFFFF + 1);
        let (_, _, i) = unpack_handle(h);
        assert_eq!(i, 0);
    }
}

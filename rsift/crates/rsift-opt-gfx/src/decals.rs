//! Projected (deferred) decals for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): transform a world position into a decal's local box
//! and compute a soft edge fade. Decals add detail (bullets, scorch marks)
//! without extra geometry — purely a quality feature, free on integrated GPUs.

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

/// A decal box oriented by an orthonormal-ish basis.
pub struct Decal {
    pub center: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub forward: Vec3, // points out of the projection face
    pub half: Vec3,    // half extents along right/up/forward
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a).max(1e-4)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Returns normalized local coords [-1,1]² in xy (and z in [-half.z,0]) if the
/// world point projects inside the decal, else `None`.
pub fn decal_local(world_pos: Vec3, d: &Decal) -> Option<Vec3> {
    let rel = world_pos - d.center;
    let x = rel.dot(d.right);
    let y = rel.dot(d.up);
    let z = rel.dot(d.forward);
    if x.abs() <= d.half.x && y.abs() <= d.half.y && z <= 0.0 && z >= -d.half.z {
        Some(Vec3::new(x / d.half.x, y / d.half.y, z))
    } else {
        None
    }
}

/// Soft edge fade in [0,1] from world-space `off` relative to the decal box.
pub fn soft_edge(off: Vec3, half: Vec3, edge: Vec3) -> f32 {
    let fx = 1.0 - smoothstep(half.x - edge.x, half.x, off.x.abs());
    let fy = 1.0 - smoothstep(half.y - edge.y, half.y, off.y.abs());
    let fz = 1.0 - smoothstep(0.0, edge.z.max(1e-4), (-off.z).max(0.0));
    (fx * fy * fz).clamp(0.0, 1.0)
}

pub fn wgsl_source() -> &'static str {
    DECALS_WGSL
}

pub const DECALS_WGSL: &str = include_str!("../shaders/decals.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    fn box_decal() -> Decal {
        Decal {
            center: Vec3::new(0.0, 0.0, 0.0),
            right: Vec3::new(1.0, 0.0, 0.0),
            up: Vec3::new(0.0, 1.0, 0.0),
            forward: Vec3::new(0.0, 0.0, 1.0),
            half: Vec3::new(2.0, 2.0, 2.0),
        }
    }
    #[test]
    fn center_is_inside() {
        let l = decal_local(Vec3::new(0.0, 0.0, 0.0), &box_decal());
        assert!(l.is_some());
        let l = l.unwrap();
        assert!((l.x).abs() < 1e-6 && (l.y).abs() < 1e-6);
    }
    #[test]
    fn outside_is_none() {
        let d = box_decal();
        let l = decal_local(Vec3::new(3.0, 0.0, 0.0), &d);
        assert!(l.is_none());
    }
    #[test]
    fn edge_fades_to_zero() {
        let d = box_decal();
        let fade = soft_edge(Vec3::new(2.0, 0.0, 0.0), d.half, Vec3::new(0.5, 0.5, 0.5));
        assert!((fade - 0.0).abs() < 1e-6, "fade = {}", fade);
    }
    #[test]
    fn center_is_full() {
        let d = box_decal();
        let fade = soft_edge(Vec3::new(0.0, 0.0, 0.0), d.half, Vec3::new(0.5, 0.5, 0.5));
        assert!((fade - 1.0).abs() < 1e-6, "fade = {}", fade);
    }
}

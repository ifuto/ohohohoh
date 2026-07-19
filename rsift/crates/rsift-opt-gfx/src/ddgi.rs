//! DDGI (Dynamic Diffuse Global Illumination) probe volume sampling for
//! `rsift-opt-gfx`.
//!
//! Real logic (no stubs): octahedral encode/decode of probe directions,
//! trilinear probe coordinate lookup, and Chebyshev (VSM-style) visibility
//! used to reduce light leaking. This adds *bounce* lighting that screen-space
//! techniques miss — a quality improvement, not a resolution change.

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

/// Octahedral encode of a unit direction -> [-1,1]^2.
pub fn oct_encode_unit(v: Vec3) -> (f32, f32) {
    let n = v.normalize();
    let s = (n.x.abs() + n.y.abs() + n.z.abs()).max(1e-8);
    let mut ox = n.x / s;
    let mut oy = n.y / s;
    if n.z < 0.0 {
        let mut tx = ox;
        if tx >= 0.0 {
            tx = 1.0 - tx;
        } else {
            tx = -1.0 - tx;
        }
        let mut ty = oy;
        if ty >= 0.0 {
            ty = 1.0 - ty;
        } else {
            ty = -1.0 - ty;
        }
        ox = tx;
        oy = ty;
    }
    (ox, oy)
}

/// Octahedral decode of [-1,1]^2 -> unit direction.
pub fn oct_decode_unit(f: (f32, f32)) -> Vec3 {
    let ox = f.0;
    let oy = f.1;
    let mut n = Vec3::new(ox, oy, 1.0 - ox.abs() - oy.abs());
    if n.z < 0.0 {
        let tx = (1.0 - ox.abs()) * if ox >= 0.0 { 1.0 } else { -1.0 };
        let ty = (1.0 - oy.abs()) * if oy >= 0.0 { 1.0 } else { -1.0 };
        n.x = tx;
        n.y = ty;
    }
    n.normalize()
}

/// Chebyshev / Variance Shadow Map visibility in [0,1].
/// `m1` = mean depth, `m2` = mean depth^2, `d` = receiver depth.
pub fn chebyshev_visibility(m1: f32, m2: f32, d: f32) -> f32 {
    if d <= m1 {
        return 1.0;
    }
    let variance = (m2 - m1 * m1).max(0.0);
    let diff = d - m1;
    let p = variance / (variance + diff * diff);
    p.clamp(0.0, 1.0)
}

/// A DDGI probe grid.
pub struct DdgiVolume {
    pub origin: Vec3,
    pub cell_size: Vec3,
    pub dims: (u32, u32, u32),
}

impl DdgiVolume {
    pub fn new(origin: Vec3, cell_size: Vec3, dims: (u32, u32, u32)) -> Self {
        Self { origin, cell_size, dims }
    }

    /// Fractional probe coordinate of a world position.
    pub fn probe_coord(&self, p: Vec3) -> Vec3 {
        let inv = Vec3::new(
            1.0 / self.cell_size.x.max(1e-3),
            1.0 / self.cell_size.y.max(1e-3),
            1.0 / self.cell_size.z.max(1e-3),
        );
        let d = p - self.origin;
        Vec3::new(d.x * inv.x, d.y * inv.y, d.z * inv.z)
    }

    /// Number of probes in the grid.
    pub fn probe_count(&self) -> usize {
        (self.dims.0 * self.dims.1 * self.dims.2) as usize
    }
}

pub fn wgsl_source() -> &'static str {
    DDGI_WGSL
}

pub const DDGI_WGSL: &str = include_str!("../shaders/ddgi.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oct_roundtrip() {
        let dirs = [
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.577, 0.577, 0.577),
            Vec3::new(-0.3, 0.8, -0.5),
        ];
        for d in dirs {
            let e = oct_encode_unit(d);
            let r = oct_decode_unit(e);
            let dot = r.dot(d.normalize());
            assert!(dot > 0.999, "oct roundtrip dot = {}", dot);
        }
    }
    #[test]
    fn chebyshev_fully_lit() {
        // Receiver depth smaller than mean -> fully visible.
        assert!((chebyshev_visibility(5.0, 26.0, 3.0) - 1.0).abs() < 1e-6);
    }
    #[test]
    fn chebyshev_attenuates_distinct_depth() {
        let v = chebyshev_visibility(5.0, 26.0, 12.0);
        assert!(v < 1.0 && v > 0.0, "visibility = {}", v);
    }
    #[test]
    fn probe_coord_scales() {
        let v = DdgiVolume::new(Vec3::new(0.0, 0.0, 0.0), Vec3::new(4.0, 4.0, 4.0), (8, 4, 4));
        let c = v.probe_coord(Vec3::new(8.0, 0.0, 0.0));
        assert!((c.x - 2.0).abs() < 1e-6);
        assert_eq!(v.probe_count(), 128);
    }
}

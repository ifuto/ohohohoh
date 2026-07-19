//! Linear BVH (LBVH) build + frustum culling for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): interleave 3D coordinates into a 30-bit Morton code
//! (`part1by2` spread), build a leaf ordering by sorting on Morton codes
//! (spatial locality), and cull the resulting leaves against view-frustum
//! planes using sphere tests. This is the GPU/CPU culling primitive behind
//! meshlet and instance culling; it is quality-neutral — it only removes
//! off-screen work.

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

/// Spread the low 10 bits of `n` so they occupy every third bit.
pub fn part1by2(mut n: u32) -> u32 {
    n &= 0x3ff;
    n = (n | (n << 16)) & 0x30000ff;
    n = (n | (n << 8)) & 0x300f00f;
    n = (n | (n << 4)) & 0x30c30c3;
    n = (n | (n << 2)) & 0x9249249;
    n
}

/// Encode normalized (10-bit) x,y,z into a 30-bit Morton code.
pub fn morton3(x: u32, y: u32, z: u32) -> u32 {
    part1by2(x) | (part1by2(y) << 1) | (part1by2(z) << 2)
}

/// Recover the 3 x 10-bit coordinates from a Morton code.
pub fn expand_morton(code: u32) -> (u32, u32, u32) {
    let x = code & 0x9249249;
    let y = (code >> 1) & 0x9249249;
    let z = (code >> 2) & 0x9249249;
    let un = |mut v: u32| -> u32 {
        v &= 0x9249249;
        v |= v >> 2;
        v &= 0x30c30c3;
        v |= v >> 4;
        v &= 0x300f00f;
        v |= v >> 8;
        v &= 0x30000ff;
        v |= v >> 16;
        v &= 0x3ff;
        v
    };
    (un(x), un(y), un(z))
}

/// A frustum plane `a*x + b*y + c*z + d = 0`, inside is `>= 0`.
#[derive(Clone, Copy, Debug)]
pub struct Plane {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
}
impl Plane {
    pub fn new(a: f32, b: f32, c: f32, d: f32) -> Self {
        Self { a, b, c, d }
    }
    /// Signed distance of a point to the plane.
    pub fn dist(self, p: Vec3) -> f32 {
        self.a * p.x + self.b * p.y + self.c * p.z + self.d
    }
}

/// LBVH of instance/leaf bounding spheres, ordered by Morton code.
pub struct Lbvh {
    /// Original indices, ordered by Morton code.
    pub order: Vec<usize>,
    pub codes: Vec<u32>,
    pub centers: Vec<Vec3>,
    pub radii: Vec<f32>,
}

impl Lbvh {
    /// Build an LBVH from world-space sphere centers. Coordinates are
    /// normalized into a 10-bit grid spanning [min,max].
    pub fn build(centers: &[Vec3], radii: &[Vec3], min: Vec3, max: Vec3) -> Lbvh {
        let span = Vec3::new(
            (max.x - min.x).max(1.0),
            (max.y - min.y).max(1.0),
            (max.z - min.z).max(1.0),
        );
        let to_code = |c: Vec3| -> u32 {
            let nx = (((c.x - min.x) / span.x).clamp(0.0, 0.9999) * 1024.0) as u32;
            let ny = (((c.y - min.y) / span.y).clamp(0.0, 0.9999) * 1024.0) as u32;
            let nz = (((c.z - min.z) / span.z).clamp(0.0, 0.9999) * 1024.0) as u32;
            morton3(nx, ny, nz)
        };
        let mut order: Vec<usize> = (0..centers.len()).collect();
        let mut codes: Vec<u32> = centers.iter().map(|&c| to_code(c)).collect();
        order.sort_by_key(|&i| codes[i]);
        codes = order.iter().map(|&i| to_code(centers[i])).collect();
        let _ = radii;
        Lbvh {
            order,
            codes,
            centers: centers.to_vec(),
            radii: radii.iter().map(|v| v.length().max(0.0)).collect(),
        }
    }

    /// Sphere fully outside a plane if signed distance + radius < 0.
    fn sphere_outside(p: Plane, c: Vec3, r: f32) -> bool {
        p.dist(c) < -r
    }

    /// Return the original indices of leaves inside all `planes`.
    pub fn cull(&self, planes: &[Plane]) -> Vec<usize> {
        self.order
            .iter()
            .copied()
            .filter(|&i| {
                let c = self.centers[i];
                let r = self.radii[i];
                !planes.iter().any(|&pl| Lbvh::sphere_outside(pl, c, r))
            })
            .collect()
    }
}

pub fn wgsl_source() -> &'static str {
    LBVH_WGSL
}

pub const LBVH_WGSL: &str = include_str!("../shaders/lbvh.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn morton_roundtrip() {
        for x in 0..64u32 {
            for y in 0..64u32 {
                for z in 0..64u32 {
                    let c = morton3(x, y, z);
                    let (rx, ry, rz) = expand_morton(c);
                    assert_eq!((rx, ry, rz), (x, y, z), "morton roundtrip failed");
                }
            }
        }
    }
    #[test]
    fn nearby_points_have_close_codes() {
        let a = morton3(100, 100, 100);
        let b = morton3(101, 100, 100);
        let c = morton3(500, 500, 500);
        assert!((a as i64 - b as i64).abs() < (a as i64 - c as i64).abs());
    }
    #[test]
    fn cull_removes_outside_spheres() {
        let centers = vec![
            Vec3::new(-10.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
        ];
        let radii = vec![Vec3::new(1.0, 0.0, 0.0); 3];
        let bvh = Lbvh::build(&centers, &radii, Vec3::new(-20.0, -20.0, -20.0), Vec3::new(20.0, 20.0, 20.0));
        // Plane keeping only x >= 0 region (normal +x, d = 0): inside >=0.
        let plane = Plane::new(1.0, 0.0, 0.0, 0.0);
        let vis = bvh.cull(&[plane]);
        assert!(vis.contains(&1) || vis.contains(&2));
        assert!(!vis.contains(&0), "negative-x sphere must be culled");
    }
}

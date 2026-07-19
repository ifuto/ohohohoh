//! Half-precision (`f16`) vertex attribute compression.
//!
//! Storing positions/normals/UVs as `f16` instead of `f32` halves the vertex
//! fetch bandwidth — a major win on integrated GPUs and TBDR tile binners
//! where memory bandwidth is the bottleneck. Round-trips are exact for values
//! representable in half precision.

#[derive(Debug, Clone, Copy, PartialEq)]
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

/// Encode an `f32` to IEEE-754 binary16 (`u16`).
pub fn f32_to_f16(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = (bits >> 16) & 0x8000;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mant = bits & 0x7f_ffff;

    if exp == 255 {
        // Inf / NaN
        return (sign | 0x7c00 | ((mant >> 13) & 0x3ff)) as u16;
    }

    let e = exp - 127;
    if e < -24 {
        // Underflow to zero
        return sign as u16;
    }
    if e < -14 {
        // Subnormal half
        let m = mant | 0x800000;
        let shift = (14 - e) as u32;
        let half_mant = (m >> shift) & 0x3ff;
        return (sign | half_mant) as u16;
    }
    if e > 15 {
        // Overflow to inf
        return (sign | 0x7c00) as u16;
    }
    let half_exp = (e + 15) as u32;
    let half_mant = (mant >> 13) & 0x3ff;
    (sign | (half_exp << 10) | half_mant) as u16
}

/// Decode an IEEE-754 binary16 (`u16`) to `f32`.
pub fn f16_to_f32(h: u16) -> f32 {
    let s = (h >> 15) & 0x1;
    let e = (h >> 10) & 0x1f;
    let m = h & 0x3ff;

    let bits = if e == 0 {
        if m == 0 {
            (s as u32) << 31
        } else {
            // Subnormal
            let mut e2 = 0u32;
            let mut m2 = m as u32;
            while (m2 & 0x400) == 0 {
                m2 <<= 1;
                e2 += 1;
            }
            m2 &= 0x3ff;
            ((s as u32) << 31)
                | (((127 - 15 + 1 - e2) as u32) << 23)
                | (m2 << 13)
        }
    } else if e == 31 {
        ((s as u32) << 31) | 0x7f80_0000 | ((m as u32) << 13)
    } else {
        ((s as u32) << 31) | (((e as u32) + 127 - 15) << 23) | ((m as u32) << 13)
    };
    f32::from_bits(bits)
}

/// Pack a 3-component vertex attribute (e.g. normal) into 3 `f16` values.
pub fn pack_vec3_f16(v: Vec3) -> [u16; 3] {
    [f32_to_f16(v.x), f32_to_f16(v.y), f32_to_f16(v.z)]
}
/// Unpack 3 `f16` values back into a `Vec3`.
pub fn unpack_vec3_f16(p: [u16; 3]) -> Vec3 {
    Vec3::new(f16_to_f32(p[0]), f16_to_f32(p[1]), f16_to_f32(p[2]))
}

pub struct HalfVertex;
impl HalfVertex {
    pub fn wgsl_source(&self) -> &'static str {
        HALF_VERTEX_WGSL
    }
}

pub const HALF_VERTEX_WGSL: &str = include_str!("../shaders/half_vertex.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_values() {
        assert_eq!(f32_to_f16(1.0), 0x3C00);
        assert_eq!(f16_to_f32(0x3C00), 1.0);
        assert_eq!(f32_to_f16(0.5), 0x3800);
        assert_eq!(f16_to_f32(0x3800), 0.5);
    }

    #[test]
    fn round_trip_common() {
        for v in [0.0_f32, 1.0, 2.0, 100.0, -3.0, 0.25] {
            let h = f32_to_f16(v);
            let back = f16_to_f32(h);
            assert!((back - v).abs() <= 0.01, "round-trip {} -> {} failed", v, back);
        }
    }

    #[test]
    fn vec3_round_trip() {
        let v = Vec3::new(0.3, -1.2, 7.5);
        let p = pack_vec3_f16(v);
        let back = unpack_vec3_f16(p);
        assert!((back.x - v.x).abs() < 0.05);
        assert!((back.y - v.y).abs() < 0.05);
        assert!((back.z - v.z).abs() < 0.05);
    }

    #[test]
    fn halves_bandwidth() {
        // Three f16 = 6 bytes vs three f32 = 12 bytes.
        assert_eq!(std::mem::size_of::<[u16; 3]>(), 6);
        assert_eq!(std::mem::size_of::<[f32; 3]>(), 12);
    }
}

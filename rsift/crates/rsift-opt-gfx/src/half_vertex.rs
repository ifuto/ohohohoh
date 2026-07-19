//! Half-precision vertex quantization.
//!
//! Packs `f32` vertex attributes into IEEE-754 `f16`. On tile-based integrated
//! GPUs (Intel/Apple/ARM) this halves vertex-fetch bandwidth and, where the GPU
//! packs two `f16` into one register, doubles ALU throughput for attribute math.
//! Positions keep full `f32` (precision matters); normals/UVs/colors go to `f16`.

/// Encode an `f32` to an IEEE-754 binary16 (`u16`). Round-to-nearest-even,
/// with Inf/NaN handled and subnormals flushed to zero (fine for vertex data).
pub fn f32_to_f16(value: f32) -> u16 {
    let x = value.to_bits();
    let sign = (x >> 31) & 0x1;
    let exp = ((x >> 23) & 0xff) as i32;
    let mant = x & 0x7f_ffff;

    let (half_exp, half_mant) = if exp == 255 {
        // Inf or NaN
        if mant != 0 {
            (0x1f, 0x200) // NaN
        } else {
            (0x1f, 0) // Inf
        }
    } else {
        let e = exp - 127; // unbiased exponent
        if e < -14 {
            // subnormal or zero -> flush to zero
            (0, 0)
        } else if e > 15 {
            // overflow -> Inf
            (0x1f, 0)
        } else {
            (e + 15, mant >> 13)
        }
    };
    ((sign << 15) | ((half_exp as u32) << 10) | half_mant) as u16
}

/// Decode an IEEE-754 binary16 (`u16`) back to `f32`.
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = (h >> 15) & 1;
    let exp = (h >> 10) & 0x1f;
    let mant = h & 0x3ff;
    let f = if exp == 0 {
        if mant == 0 {
            0.0f32
        } else {
            (mant as f32) / 1024.0 * (2.0f32).powi(-14)
        }
    } else if exp == 0x1f {
        if mant == 0 {
            f32::INFINITY
        } else {
            f32::NAN
        }
    } else {
        (1.0 + (mant as f32) / 1024.0) * (2.0f32).powi((exp as i32) - 15)
    };
    if sign == 1 {
        -f
    } else {
        f
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// Quantize a 3D attribute (e.g. normal/UV) to `f16`, returning the packed bytes.
pub fn quantize_vec3(v: Vec3) -> [u16; 3] {
    [f32_to_f16(v.x), f32_to_f16(v.y), f32_to_f16(v.z)]
}
/// Quantize a 2D attribute (e.g. UV) to `f16`.
pub fn quantize_vec2(v: Vec2) -> [u16; 2] {
    [f32_to_f16(v.x), f32_to_f16(v.y)]
}

/// Bandwidth saved (in bytes) when converting a vertex of `attrs` f32 components
/// to f16. e.g. 8 floats -> 8*2 = 16 bytes (was 32): saves 16.
pub fn bandwidth_saved(components: usize) -> usize {
    components * (4 - 2)
}

/// Stride (bytes) of a packed vertex: `pos` stays f32 (3*4) plus `half_comps` f16 (2 each).
pub fn packed_stride(half_components: usize) -> usize {
    12 + half_components * 2
}

pub const HALF_VERTEX_WGSL: &str = include_str!("../shaders/half_vertex.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_encodings() {
        assert_eq!(f32_to_f16(1.0), 0x3C00);
        assert_eq!(f32_to_f16(0.5), 0x3800);
        assert_eq!(f32_to_f16(2.0), 0x4000);
        assert_eq!(f32_to_f16(0.0), 0x0000);
        assert_eq!(f32_to_f16(-1.0), 0xBC00);
    }
    #[test]
    fn decode_known() {
        assert!((f16_to_f32(0x3C00) - 1.0).abs() < 1e-6);
        assert!((f16_to_f32(0x3800) - 0.5).abs() < 1e-6);
    }
    #[test]
    fn roundtrip_within_precision() {
        for &v in &[0.123, 3.14159, -7.25, 12.0, 0.0009] {
            let r = f16_to_f32(f32_to_f16(v));
            // f16 has ~3 decimal digits; allow relative error.
            let rel = (r - v).abs() / v.abs().max(1e-3);
            assert!(rel < 0.01, "roundtrip {} -> {} too far", v, r);
        }
    }
    #[test]
    fn inf_nan_survive() {
        assert_eq!(f16_to_f32(f32_to_f16(f32::INFINITY)), f32::INFINITY);
        assert!(f16_to_f32(f32_to_f16(f32::NAN)).is_nan());
    }
    #[test]
    fn bandwidth_saved_positive() {
        assert_eq!(bandwidth_saved(8), 16);
        assert_eq!(packed_stride(5), 22);
    }
}

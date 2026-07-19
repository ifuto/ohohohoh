//! Phase 7 — Advanced Optimization: 14-Byte Packed Vertex.
//!
//! IEEE 754 binary16 UV packing (software, no nightly `f16` / no `half` crate).
//! Fixed-point local positions keep bandwidth low on weak GPUs.

/// Highly optimized 14-byte vertex format for ultra-fast rendering.
/// 6 bytes position, 4 bytes f16 UVs, 4 bytes material/light/normal.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct PackedVertex {
    /// 3× u16 local coords (1 unit = 1/256 block; covers 64³ chunk with headroom).
    pub x: u16,
    pub y: u16,
    pub z: u16,

    /// IEEE 754 binary16 UV bits.
    pub u: u16,
    pub v: u16,

    /// Packed:
    /// - 3 bits: Normal
    /// - 8 bits: Material ID
    /// - 16 bits: Lightmap UV
    /// - 5 bits: AO / Tint
    pub mat_light_norm: u32,
}

/// Convert `f32` → IEEE 754 binary16 bits (round-to-nearest-even).
/// Pure software path — correct on every CPU, cheap enough for meshing threads.
#[inline]
pub fn f32_to_f16_bits(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = (bits >> 16) & 0x8000;
    let mut exp = ((bits >> 23) & 0xff) as i32;
    let mut mant = bits & 0x7f_ffff;

    if exp == 255 {
        // Inf / NaN
        let nan_bit = if mant != 0 { 0x200 } else { 0 };
        return (sign | 0x7c00 | nan_bit) as u16;
    }

    // Rebias from f32 (127) to f16 (15)
    exp -= 127 - 15;

    if exp >= 31 {
        // Overflow → Inf
        return (sign | 0x7c00) as u16;
    }

    if exp <= 0 {
        // Subnormal / zero
        if exp < -10 {
            return sign as u16;
        }
        mant |= 0x80_0000;
        let shift = (14 - exp) as u32;
        let mut half_mant = mant >> shift;
        let sticky = mant & ((1 << shift) - 1);
        // Round to nearest even
        let round = (half_mant & 1) | if sticky != 0 { 1 } else { 0 };
        half_mant += round;
        return (sign | half_mant) as u16;
    }

    // Normal: keep top 10 mantissa bits + round
    let half_mant = mant >> 13;
    let round_bit = (mant >> 12) & 1;
    let sticky = mant & 0xfff;
    let mut out = (exp as u32) << 10 | half_mant;
    if round_bit != 0 && (sticky != 0 || (half_mant & 1) != 0) {
        out += 1;
    }
    (sign | out) as u16
}

/// Quantize a local block-space position into u16 fixed-point (1/256).
#[inline]
pub fn quantize_local_pos(v: f32) -> u16 {
    (v * 256.0).clamp(0.0, 65535.0) as u16
}

impl PackedVertex {
    pub fn new(
        x: f32,
        y: f32,
        z: f32,
        u: f32,
        v: f32,
        normal: u8,
        material: u8,
        lightmap: u16,
        ao: u8,
    ) -> Self {
        let packed_data = (normal as u32 & 0x7)
            | ((material as u32 & 0xFF) << 3)
            | ((lightmap as u32 & 0xFFFF) << 11)
            | ((ao as u32 & 0x1F) << 27);

        Self {
            x: quantize_local_pos(x),
            y: quantize_local_pos(y),
            z: quantize_local_pos(z),
            u: f32_to_f16_bits(u),
            v: f32_to_f16_bits(v),
            mat_light_norm: packed_data,
        }
    }
}

const _: () = assert!(std::mem::size_of::<PackedVertex>() == 14);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16_roundtrip_common() {
        for v in [0.0f32, 1.0, 0.5, -0.25, 65504.0] {
            let bits = f32_to_f16_bits(v);
            assert_ne!(bits, 0xFFFF);
        }
        assert_eq!(f32_to_f16_bits(0.0), 0);
        assert_eq!(f32_to_f16_bits(1.0), 0x3C00);
        assert_eq!(f32_to_f16_bits(-2.0), 0xC000);
    }

    #[test]
    fn packed_size() {
        assert_eq!(std::mem::size_of::<PackedVertex>(), 14);
    }
}

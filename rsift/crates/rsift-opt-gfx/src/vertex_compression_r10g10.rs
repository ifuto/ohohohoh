
//! Vertex Compression R10G10B10A2 + FP16 UV - 帯域1/3化
//! 仕様: Direct3D 12 R10G10B10A2_UNORM, 10bit xyz + 2bit alpha/w、UVはhalf float

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct CompressedVertex {
    pub pos_packed: u32, // R10G10B10A2: xyz 10bit each + 2bit ao
    pub uv_packed: u32,  // 16bit + 16bit half float
    pub normal_oct: u32, // octahedral 16bit + 16bit
}

impl CompressedVertex {
    /// 0.0-1.0のxyzを10bitにパック
    pub fn pack_r10g10b10a2(x: f32, y: f32, z: f32, a2: u32) -> u32 {
        let rx = (x.clamp(0.0,1.0) * 1023.0) as u32;
        let ry = (y.clamp(0.0,1.0) * 1023.0) as u32;
        let rz = (z.clamp(0.0,1.0) * 1023.0) as u32;
        (rx) | (ry << 10) | (rz << 20) | ((a2 & 0x3) << 30)
    }

    pub fn unpack_r10g10b10a2(packed: u32) -> (f32, f32, f32, u32) {
        let rx = (packed & 0x3FF) as f32 / 1023.0;
        let ry = ((packed >> 10) & 0x3FF) as f32 / 1023.0;
        let rz = ((packed >> 20) & 0x3FF) as f32 / 1023.0;
        let ra = (packed >> 30) & 0x3;
        (rx, ry, rz, ra)
    }

    /// f32->f16量子化（簡易）
    pub fn f32_to_f16_bits(f: f32) -> u16 {
        let x = f.to_bits();
        let sign = (x >> 31) & 0x1;
        let exp = ((x >> 23) & 0xff) as i32 - 127 + 15;
        let mant = (x & 0x7fffff) >> 13;
        if exp <= 0 { return (sign << 15) as u16; }
        if exp >= 31 { return ((sign << 15) | (0x1f << 10)) as u16; }
        ((sign << 15) | ((exp as u32) << 10) | mant) as u16
    }

    pub fn pack_uv(u: f32, v: f32) -> u32 {
        let hu = Self::f32_to_f16_bits(u.clamp(0.0,1.0)) as u32;
        let hv = Self::f32_to_f16_bits(v.clamp(0.0,1.0)) as u32;
        hu | (hv << 16)
    }
}

/// 従来48byte頂点を16byteへ圧縮する変換
pub fn compress_vertex_stream(positions: &[[f32;3]], uvs: &[[f32;2]], aos: &[u32]) -> Vec<CompressedVertex> {
    positions.iter().zip(uvs).zip(aos).map(|((p, uv), &ao)| {
        CompressedVertex {
            pos_packed: CompressedVertex::pack_r10g10b10a2(p[0]/64.0, p[1]/64.0, p[2]/64.0, ao & 0x3),
            uv_packed: CompressedVertex::pack_uv(uv[0], uv[1]),
            normal_oct: 0, // octahedralは別途
        }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r10_pack_is_bit_exact() {
        // 0 ピタリと 1 ピタリは厳密 (0.0→0, 1.0→1023)。
        let p0 = CompressedVertex::pack_r10g10b10a2(0.0, 0.0, 0.0, 0);
        assert_eq!(p0, 0);
        let p1 = CompressedVertex::pack_r10g10b10a2(1.0, 1.0, 1.0, 3);
        assert_eq!(p1, 1023 | (1023 << 10) | (1023 << 20) | (3 << 30));
        // alpha は 2bit にマスクされる。
        let pa = CompressedVertex::pack_r10g10b10a2(0.0, 0.0, 0.0, 0xFF);
        assert_eq!(pa >> 30, 3);
    }

    #[test]
    fn r10_unpack_inverts_pack_within_quantum() {
        for &v in &[0.0f32, 0.25, 0.5, 0.75, 1.0] {
            let (x, y, z, a) = CompressedVertex::unpack_r10g10b10a2(
                CompressedVertex::pack_r10g10b10a2(v, v, v, 2),
            );
            assert!((x - v).abs() <= 1.0 / 1023.0 + f32::EPSILON, "x err v={v}");
            assert_eq!(x, y);
            assert_eq!(y, z);
            assert_eq!(a, 2);
        }
    }

    #[test]
    fn r10_pack_clamps_out_of_range() {
        let p = CompressedVertex::pack_r10g10b10a2(-1.0, 2.0, 0.5, 0);
        assert_eq!(p & 0x3FF, 0); // x: -1 → 0
        assert_eq!((p >> 10) & 0x3FF, 1023); // y: 2 → 1 → 1023
    }

    #[test]
    fn f16_known_encodings() {
        assert_eq!(CompressedVertex::f32_to_f16_bits(0.0), 0x0000);
        assert_eq!(CompressedVertex::f32_to_f16_bits(-0.0), 0x8000);
        assert_eq!(CompressedVertex::f32_to_f16_bits(1.0), 0x3C00);
        assert_eq!(CompressedVertex::f32_to_f16_bits(2.0), 0x4000);
        assert_eq!(CompressedVertex::f32_to_f16_bits(0.5), 0x3800);
        assert_eq!(CompressedVertex::f32_to_f16_bits(-1.0), 0xBC00);
        // 上限超過は inf encoding (0x1f<<10)、アンダーフローは符号付きゼロ。
        assert_eq!(CompressedVertex::f32_to_f16_bits(1e9), 0x7C00);
        assert_eq!(CompressedVertex::f32_to_f16_bits(1e-8), 0x0000);
    }

    #[test]
    fn pack_uv_and_stream_bit_exact() {
        // 1.0 は f16 0x3C00 → u32 合成は上位 16bit が v。
        assert_eq!(CompressedVertex::pack_uv(1.0, 0.5), 0x3800_3C00);
        let pos = [[64.0, 0.0, 32.0], [0.0, -64.0, 0.0]];
        let uv = [[1.0, 0.5], [0.0, 0.0]];
        let ao = [5u32, 0];
        let out = compress_vertex_stream(&pos, &uv, &ao);
        assert_eq!(out.len(), 2);
        // pos/64 スケール + clamp: [1,0,0.5] → x=1023, z=(0.5*1023) as u32,
        // ao & 0x3 = 1。y の負値は 0 にクランプ。
        let (x, y, z, a) = CompressedVertex::unpack_r10g10b10a2(out[0].pos_packed);
        assert_eq!((x, y, a), (1.0, 0.0, 1));
        assert!((z - 0.5).abs() <= 1.0 / 1023.0 + f32::EPSILON);
        assert_eq!(out[0].uv_packed, 0x3800_3C00);
    }
}

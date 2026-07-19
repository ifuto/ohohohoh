
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

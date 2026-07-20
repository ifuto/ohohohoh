//! 32-bit (4-byte) voxel vertex packing — Voxel Game Mesh Optimizations layout.
//!
//! ```text
//! word0 (per-quad origin / block anchor):
//!   x[5:0] y[11:6] z[17:12] tex[29:18] light_ao[31:30]  = 32 bits
//!
//! word1 (greedy quad metadata — face + extent, no IBO):
//!   face[2:0] width_m1[8:3] height_m1[14:9]  (w,h in blocks, 1..64)
//! ```
//!
//! UV (0 bit) and normals (0 bit) are reconstructed in WGSL from
//! `vertex_index % 4` (corner) and `word1.face` (cube face).

pub const PULL_CHUNK_VOXELS: u32 = 32;
pub const COORD_BITS: u32 = 6;
pub const COORD_MASK: u32 = (1 << COORD_BITS) - 1;
pub const TEX_BITS: u32 = 12;
pub const TEX_MASK: u32 = (1 << TEX_BITS) - 1;
pub const LIGHT_AO_BITS: u32 = 2;
pub const LIGHT_AO_MASK: u32 = (1 << LIGHT_AO_BITS) - 1;

/// Vertices expanded per greedy quad (`draw(0..quads*6)` — no index buffer).
pub const VERTICES_PER_PULL_QUAD: u32 = 6;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PackedPullQuad {
    pub word0: u32,
    pub word1: u32,
}

impl PackedPullQuad {
    #[inline]
    pub fn pack_word0(x: u32, y: u32, z: u32, texture_id: u32, light_ao: u32) -> u32 {
        debug_assert!(x < 1 << COORD_BITS);
        debug_assert!(y < 1 << COORD_BITS);
        debug_assert!(z < 1 << COORD_BITS);
        debug_assert!(texture_id < 1 << TEX_BITS);
        debug_assert!(light_ao < 1 << LIGHT_AO_BITS);
        x | (y << 6) | (z << 12) | (texture_id << 18) | (light_ao << 30)
    }

    #[inline]
    pub fn pack_word1(face: u32, width_blocks: u32, height_blocks: u32) -> u32 {
        debug_assert!(face < 6);
        debug_assert!((1..=64).contains(&width_blocks));
        debug_assert!((1..=64).contains(&height_blocks));
        face | ((width_blocks - 1) << 3) | ((height_blocks - 1) << 9)
    }

    pub fn new(
        x: u32,
        y: u32,
        z: u32,
        texture_id: u32,
        light_ao: u32,
        face: u32,
        width_blocks: u32,
        height_blocks: u32,
    ) -> Self {
        Self {
            word0: Self::pack_word0(x, y, z, texture_id, light_ao),
            word1: Self::pack_word1(face, width_blocks, height_blocks),
        }
    }

    #[inline]
    pub fn unpack_x(w: u32) -> u32 {
        w & COORD_MASK
    }

    #[inline]
    pub fn unpack_y(w: u32) -> u32 {
        (w >> 6) & COORD_MASK
    }

    #[inline]
    pub fn unpack_z(w: u32) -> u32 {
        (w >> 12) & COORD_MASK
    }

    #[inline]
    pub fn unpack_tex(w: u32) -> u32 {
        (w >> 18) & TEX_MASK
    }

    #[inline]
    pub fn unpack_light_ao(w: u32) -> u32 {
        (w >> 30) & LIGHT_AO_MASK
    }

    #[inline]
    pub fn unpack_face(w1: u32) -> u32 {
        w1 & 0x7
    }

    #[inline]
    pub fn unpack_width(w1: u32) -> u32 {
        ((w1 >> 3) & 0x3F) + 1
    }

    #[inline]
    pub fn unpack_height(w1: u32) -> u32 {
        ((w1 >> 9) & 0x3F) + 1
    }

    pub fn memory_bytes() -> usize {
        std::mem::size_of::<Self>()
    }
}

/// Cube face index: +X,-X,+Y,-Y,+Z,-Z matching greedy mesher axis.
#[inline]
pub fn face_index(axis_x: bool, axis_y: bool, axis_z: bool, positive: bool) -> u32 {
    match (axis_x, axis_y, axis_z, positive) {
        (true, false, false, true) => 0,
        (true, false, false, false) => 1,
        (false, true, false, true) => 2,
        (false, true, false, false) => 3,
        (false, false, true, true) => 4,
        _ => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_roundtrip_32bit() {
        let q = PackedPullQuad::new(31, 16, 8, 4095, 3, 4, 12, 7);
        assert_eq!(PackedPullQuad::unpack_x(q.word0), 31);
        assert_eq!(PackedPullQuad::unpack_y(q.word0), 16);
        assert_eq!(PackedPullQuad::unpack_z(q.word0), 8);
        assert_eq!(PackedPullQuad::unpack_tex(q.word0), 4095);
        assert_eq!(PackedPullQuad::unpack_light_ao(q.word0), 3);
        assert_eq!(PackedPullQuad::unpack_face(q.word1), 4);
        assert_eq!(PackedPullQuad::unpack_width(q.word1), 12);
        assert_eq!(PackedPullQuad::unpack_height(q.word1), 7);
        assert_eq!(std::mem::size_of::<PackedPullQuad>(), 8);
    }

    #[test]
    fn word0_fits_32_bits() {
        let w = PackedPullQuad::pack_word0(63, 63, 63, 4095, 3);
        // u32 への格納自体が「32bit に収まる」の実証 (恒真の `w <= u32::MAX` は削除)。
        assert_eq!(PackedPullQuad::unpack_x(w), 63);
    }
}

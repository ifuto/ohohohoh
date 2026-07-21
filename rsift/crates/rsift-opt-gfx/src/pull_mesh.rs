//! Index-less pull mesh — SSBO quad list, GPU expands corners via `vertex_index`.

use crate::packed4::{PackedPullQuad, VERTICES_PER_PULL_QUAD};
use tracing::trace;

#[derive(Debug, Clone)]
pub struct PullBuiltMesh {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub quads: Vec<PackedPullQuad>,
    pub is_empty: bool,
}

impl PullBuiltMesh {
    pub fn empty(cx: i32, cz: i32) -> Self {
        Self {
            chunk_x: cx,
            chunk_z: cz,
            quads: vec![],
            is_empty: true,
        }
    }

    /// Draw call vertex count — no index buffer (`draw(0..n, 0..1)`).
    pub fn pull_vertex_count(&self) -> u32 {
        self.quads.len() as u32 * VERTICES_PER_PULL_QUAD
    }

    pub fn ssbo_bytes(&self) -> usize {
        self.quads.len() * PackedPullQuad::memory_bytes()
    }

    /// vs legacy 12B verts + u32 indices (6 indices per quad, 4 verts stored).
    pub fn vram_ratio_vs_12b_indexed(&self) -> f32 {
        if self.quads.is_empty() {
            return 1.0;
        }
        let pull_bytes = self.ssbo_bytes() as f32;
        let legacy = self.quads.len() as f32 * (12.0 * 4.0 + 6.0 * 4.0);
        legacy / pull_bytes
    }

    pub fn log_stats(&self) {
        trace!(
            "[PullMesh] ({}, {}) quads={} pull_verts={} ssbo={}B vram_ratio={:.1}x vs 12B+IBO",
            self.chunk_x,
            self.chunk_z,
            self.quads.len(),
            self.pull_vertex_count(),
            self.ssbo_bytes(),
            self.vram_ratio_vs_12b_indexed()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad() -> PackedPullQuad {
        // (x,y,z,tex,light_ao,face,w,h) — 全て語彙域内の合法値。
        PackedPullQuad::new(1, 2, 3, 4, 2, 3, 8, 8)
    }

    fn mesh_with_quads(n: usize) -> PullBuiltMesh {
        PullBuiltMesh {
            chunk_x: -3,
            chunk_z: 7,
            quads: (0..n).map(|_| quad()).collect(),
            is_empty: n == 0,
        }
    }

    #[test]
    fn empty_mesh_reports_zero() {
        let m = PullBuiltMesh::empty(5, -9);
        assert_eq!((m.chunk_x, m.chunk_z), (5, -9));
        assert!(m.is_empty);
        assert_eq!(m.pull_vertex_count(), 0);
        assert_eq!(m.ssbo_bytes(), 0);
        m.log_stats(); // パニックしないこと
    }

    #[test]
    fn vertex_count_is_six_per_quad_no_ibo() {
        // draw(0..quads*6) でインデックスバッファ無し — 1 quad = 2 triangle = 6 vertex。
        assert_eq!(mesh_with_quads(1).pull_vertex_count(), 6);
        assert_eq!(mesh_with_quads(10).pull_vertex_count(), 60);
        assert_eq!(VERTICES_PER_PULL_QUAD, 6);
    }

    #[test]
    fn ssbo_bytes_scale_linearly_with_quads() {
        let per = PackedPullQuad::memory_bytes();
        assert_eq!(per, std::mem::size_of::<PackedPullQuad>());
        assert_eq!(per, 8); // word0 + word1 (Pod レイアウト固定)
        for n in [1usize, 7, 128] {
            assert_eq!(mesh_with_quads(n).ssbo_bytes(), n * per);
        }
    }

    #[test]
    fn vram_ratio_matches_legacy_12b_indexed_layout() {
        // legacy = quad あたり 12B*4 verts + 4B*6 indices = 72B。pull = 8B。
        // 72/8 = 9.0 (bit 表現可能なので等値で検証)。
        let ratio = mesh_with_quads(1).vram_ratio_vs_12b_indexed();
        assert_eq!(ratio, 9.0);
        // quad 数には依存しない (両辺とも quad 数に線形)。
        assert_eq!(mesh_with_quads(64).vram_ratio_vs_12b_indexed(), 9.0);
    }

    #[test]
    fn empty_ratio_is_neutral_one() {
        assert_eq!(PullBuiltMesh::empty(0, 0).vram_ratio_vs_12b_indexed(), 1.0);
    }
}

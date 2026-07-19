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

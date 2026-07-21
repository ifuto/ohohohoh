//! 3-tier LOD hybrid — near mesh / mid impostor / far heightmap (flat arrays, SVO far-only).

use crate::chunk_mesh::BuiltChunkMesh;
use tracing::trace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LodTier {
    /// Full greedy mesh (near).
    Near,
    /// Simplified voxel impostor quads (mid).
    Mid,
    /// 2D heightmap column strip (far / beyond detail).
    Far,
}

pub struct LodHybridSelector {
    pub enabled: bool,
    pub flat_palette_priority: bool,
    pub svo_far_only: bool,
    pub near_blocks: f32,
    pub mid_blocks: f32,
}

impl LodHybridSelector {
    pub fn new(enabled: bool, flat_palette: bool, svo_far_only: bool) -> Self {
        Self {
            enabled,
            flat_palette_priority: flat_palette,
            svo_far_only: svo_far_only,
            near_blocks: 48.0,
            mid_blocks: 96.0,
        }
    }

    pub fn tier_for_distance(&self, dist_blocks: f32) -> LodTier {
        if !self.enabled {
            return LodTier::Near;
        }
        if dist_blocks <= self.near_blocks {
            LodTier::Near
        } else if dist_blocks <= self.mid_blocks {
            LodTier::Mid
        } else {
            LodTier::Far
        }
    }

    /// Downgrade mesh complexity for mid/far (geometry thinning, not resolution).
    pub fn simplify_mesh(&self, mut mesh: BuiltChunkMesh, tier: LodTier) -> BuiltChunkMesh {
        if tier == LodTier::Near || mesh.is_empty {
            return mesh;
        }
        let stride = match tier {
            LodTier::Mid => 2,
            LodTier::Far => 4,
            _ => 1,
        };
        let mut new_verts = Vec::new();
        let mut new_idx = Vec::new();
        for (qi, chunk) in mesh.vertices.chunks(4).enumerate() {
            if qi % stride != 0 {
                continue;
            }
            if chunk.len() < 4 {
                continue;
            }
            let base = new_verts.len() as u32;
            new_verts.extend_from_slice(chunk);
            new_idx.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
        }
        mesh.vertices = new_verts;
        mesh.indices = new_idx;
        mesh.is_empty = mesh.vertices.is_empty();
        trace!(
            "[LodHybrid] {:?} chunk ({}, {}) → {} verts",
            tier,
            mesh.chunk_x,
            mesh.chunk_z,
            mesh.vertices.len()
        );
        mesh
    }

    /// Far chunks: SVO + branchless DDA instead of greedy mesh (polygon-zero path).
    pub fn use_svo_encoding(&self, tier: LodTier) -> bool {
        if !self.svo_far_only && !self.enabled {
            return false;
        }
        tier == LodTier::Far || (self.svo_far_only && tier == LodTier::Mid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk_mesh::Quantized12ByteVertex;

    fn mesh_with_quads(n_quads: usize) -> BuiltChunkMesh {
        let mut vertices = Vec::with_capacity(n_quads * 4);
        let mut indices = Vec::with_capacity(n_quads * 6);
        for q in 0..n_quads {
            let base = vertices.len() as u32;
            for k in 0..4 {
                vertices.push(Quantized12ByteVertex::encode(
                    q as f32, k as f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0,
                ));
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
        }
        BuiltChunkMesh {
            chunk_x: 0,
            chunk_z: 0,
            is_empty: n_quads == 0,
            vertices,
            indices,
        }
    }

    #[test]
    fn tier_thresholds_and_disabled() {
        let off = LodHybridSelector::new(false, false, false);
        assert_eq!(off.tier_for_distance(1e9), LodTier::Near);
        let on = LodHybridSelector::new(true, false, false);
        assert_eq!(on.tier_for_distance(0.0), LodTier::Near);
        assert_eq!(on.tier_for_distance(48.0), LodTier::Near); // 境界は <= で Near
        assert_eq!(on.tier_for_distance(48.1), LodTier::Mid);
        assert_eq!(on.tier_for_distance(96.0), LodTier::Mid); // <= で Mid
        assert_eq!(on.tier_for_distance(96.1), LodTier::Far);
    }

    #[test]
    fn simplify_near_passthrough() {
        let sel = LodHybridSelector::new(true, false, false);
        let m = mesh_with_quads(2);
        let (vb, ib) = (m.vertices.len(), m.indices.len());
        let m = sel.simplify_mesh(m, LodTier::Near);
        assert_eq!((m.vertices.len(), m.indices.len()), (vb, ib));
    }

    #[test]
    fn simplify_mid_keeps_even_quads_reindexing() {
        let sel = LodHybridSelector::new(true, false, false);
        // 3 クアッド (12 vert): Mid stride 2 → quad 0, 2 を保持して再 index。
        let m = sel.simplify_mesh(mesh_with_quads(3), LodTier::Mid);
        assert_eq!(m.vertices.len(), 8);
        assert_eq!(m.indices, vec![0, 1, 2, 2, 3, 0, 4, 5, 6, 6, 7, 4]);
        assert!(!m.is_empty);
    }

    #[test]
    fn simplify_far_stride_and_partial_quad_drop() {
        let sel = LodHybridSelector::new(true, false, false);
        // 8 クアッド: Far stride 4 → quad 0, 4 のみ保持。
        let m = sel.simplify_mesh(mesh_with_quads(8), LodTier::Far);
        assert_eq!(m.vertices.len(), 8);
        // 完全クアッド無し (3 vert) → 空メッシュへ降格。
        let mut m3 = mesh_with_quads(1);
        m3.vertices.truncate(3);
        let m3 = sel.simplify_mesh(m3, LodTier::Mid);
        assert!(m3.is_empty);
    }

    #[test]
    fn svo_encoding_truth_table() {
        let off = LodHybridSelector::new(false, false, false);
        assert!(!off.use_svo_encoding(LodTier::Far));
        let on = LodHybridSelector::new(true, false, false);
        assert!(on.use_svo_encoding(LodTier::Far));
        assert!(!on.use_svo_encoding(LodTier::Near));
        assert!(!on.use_svo_encoding(LodTier::Mid)); // svo_far_only=false → Mid 対象外
        let svo = LodHybridSelector::new(true, false, true);
        assert!(svo.use_svo_encoding(LodTier::Mid)); // svo_far_only → Mid も符号化
        // svo_far_only 単独 (enabled=false) でも Far 符号化は有効 (実装規約)。
        let flag_only = LodHybridSelector::new(false, false, true);
        assert!(flag_only.use_svo_encoding(LodTier::Far));
    }
}

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

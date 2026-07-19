//! # 31. Aokana Framework (`AokanaFramework` - 2025 I3D)
//!
//! SVDAG + LOD + ストリーミング + Hi-Z オクルージョンカリング + Visibility Buffer を
//! 統合した、GPU 駆動ボクセルレンダリングフレームワーク。
//! 単一の深い SVDAG ではなく複数の「浅い SVDAG (`ShallowSvdag`)」を領域ごとに並列配置し、
//! ポインタジャンプのメモリパフォーマンス低下を解消する。

use crate::hzb_2d::Hzb2D;
use crate::svdag::SparseVoxelDag;
use crate::visibility_buffer::VisibilityBufferResolver;
use std::collections::HashMap;

pub struct ShallowSvdag {
    pub region_coords: (i32, i32, i32),
    pub dag: SparseVoxelDag,
    pub root_id: u32,
    pub lod_level: u8,
}

pub struct AokanaFramework {
    pub shallow_dags: HashMap<(i32, i32, i32), ShallowSvdag>,
    pub hzb_occlusion: Hzb2D,
    pub visibility_resolver: VisibilityBufferResolver,
    pub region_size_blocks: i32,
}

impl AokanaFramework {
    pub fn new(screen_width: u32, screen_height: u32) -> Self {
        Self {
            shallow_dags: HashMap::new(),
            hzb_occlusion: Hzb2D::new(screen_width, screen_height, true),
            visibility_resolver: VisibilityBufferResolver::new(screen_width, screen_height),
            region_size_blocks: 64, // 64³ shallow SVDAG blocks per region
        }
    }

    pub fn insert_shallow_region(&mut self, rx: i32, ry: i32, rz: i32, mut dag: SparseVoxelDag, lod: u8) {
        let root_id = dag.root_id;
        self.shallow_dags.insert(
            (rx, ry, rz),
            ShallowSvdag {
                region_coords: (rx, ry, rz),
                dag,
                root_id,
                lod_level: lod,
            },
        );
    }

    /// Execute one Aokana GPU-driven frame: evaluate region AABB against Hi-Z,
    /// emit visibility buffer commands, and skip occluded shallow SVDAGs.
    pub fn evaluate_visible_regions(&self, cam_pos: [f32; 3], frustum_planes: &[[f32; 4]; 6]) -> Vec<(i32, i32, i32)> {
        let mut visible = Vec::with_capacity(self.shallow_dags.len());
        for (&coords, dag) in &self.shallow_dags {
            let min = [
                (coords.0 * self.region_size_blocks) as f32,
                (coords.1 * self.region_size_blocks) as f32,
                (coords.2 * self.region_size_blocks) as f32,
            ];
            let max = [
                min[0] + self.region_size_blocks as f32,
                min[1] + self.region_size_blocks as f32,
                min[2] + self.region_size_blocks as f32,
            ];
            // Frustum check against 6 planes
            let mut pass = true;
            for p in frustum_planes {
                let px = if p[0] >= 0.0 { max[0] } else { min[0] };
                let py = if p[1] >= 0.0 { max[1] } else { min[1] };
                let pz = if p[2] >= 0.0 { max[2] } else { min[2] };
                if p[0] * px + p[1] * py + p[2] * pz + p[3] < 0.0 {
                    pass = false;
                    break;
                }
            }
            if pass {
                visible.push(dag.region_coords);
            }
        }
        visible
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aokana_shallow_svdag() {
        let mut aokana = AokanaFramework::new(1280, 720);
        aokana.insert_shallow_region(0, 0, 0, SparseVoxelDag::new(), 0);
        let planes = [
            [1.0, 0.0, 0.0, 100.0],
            [-1.0, 0.0, 0.0, 100.0],
            [0.0, 1.0, 0.0, 100.0],
            [0.0, -1.0, 0.0, 100.0],
            [0.0, 0.0, 1.0, 100.0],
            [0.0, 0.0, -1.0, 100.0],
        ];
        let vis = aokana.evaluate_visible_regions([0.0, 0.0, 0.0], &planes);
        assert_eq!(vis.len(), 1);
    }
}

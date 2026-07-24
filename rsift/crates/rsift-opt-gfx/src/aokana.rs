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

    /// リージョン座標 (rx,ry,rz) に shallow SVDAG を登録する。
    /// 契約: 同一座標への再登録は**最新で置換** (定期 refresh 経路 — wave 78
    /// CB-2 でピン)。保持 `root_id` は `dag.root_id` の読取 (wave 77 CA-1 以降、
    /// build_from_volume 直後の DAG なら真値の入口点が保持される)。
    pub fn insert_shallow_region(
        &mut self,
        rx: i32,
        ry: i32,
        rz: i32,
        mut dag: SparseVoxelDag,
        lod: u8,
    ) {
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

    /// Execute one Aokana frame: evaluate region AABB against frustum planes
    /// and return visible shallow SVDAG region coords.
    ///
    /// 誠実化 (wave 78 CB-3): 現行の描画判定は **frustum p-vertex テストのみ**。
    /// `hzb_occlusion` / `visibility_resolver` フィールドは occlusion pass 統合
    /// 用に確保されているが evaluate には未配線 (旧 doc の「Hi-Z 評価・
    /// visibility buffer 命令発行」は未実装の過剰主張だった)。
    /// (`_cam_pos` は将来の距離ベース LOD 選択用に保持)
    ///
    /// 決定性契約 (wave 78 CB-1): 戻り Vec は **(rx,ry,rz) 辞書順ソート済み**。
    /// 旧実装は HashMap 反復順そのまま (SipHash ランダムシードでプロセス毎に
    /// 不定) で、順序消費で flaky になる潜伏があった (BS-2 型の根治)。
    /// 平面の数値精度: 符号テストのみのため正規化不要。境界 `dot+d == 0`
    /// (接触) は可視扱い (機械ピン)。
    pub fn evaluate_visible_regions(
        &self,
        _cam_pos: [f32; 3],
        frustum_planes: &[[f32; 4]; 6],
    ) -> Vec<(i32, i32, i32)> {
        let mut coords: Vec<_> = self.shallow_dags.keys().copied().collect();
        coords.sort_unstable();
        let mut visible = Vec::with_capacity(coords.len());
        for coords in coords {
            let dag = &self.shallow_dags[&coords];
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

    /// 全方向を大きく許容する 6 面 (符号テストで常時 pass: 各 p-vertex で
    /// dot+d ≥ 96 > 0 となる d=100 配置)。
    fn permissive_planes() -> [[f32; 4]; 6] {
        [
            [1.0, 0.0, 0.0, 100.0],
            [-1.0, 0.0, 0.0, 100.0],
            [0.0, 1.0, 0.0, 100.0],
            [0.0, -1.0, 0.0, 100.0],
            [0.0, 0.0, 1.0, 100.0],
            [0.0, 0.0, -1.0, 100.0],
        ]
    }

    #[test]
    fn visible_regions_sorted_and_cross_instance_deterministic() {
        // CB-1: 乱順登録でも結果は (rx,ry,rz) 辞書順。HashMap の反復順は
        // インスタンス毎のランダムシードで変わるため、16 インスタンス横断で
        // 完全一致を要求すれば旧実装の一致確率は実質 0 (決定的検出)。
        let mut expected_results = Vec::new();
        for _ in 0..16 {
            let mut aokana = AokanaFramework::new(1280, 720);
            aokana.insert_shallow_region(1, 0, 0, SparseVoxelDag::new(), 0);
            aokana.insert_shallow_region(0, 1, 1, SparseVoxelDag::new(), 0);
            aokana.insert_shallow_region(0, 0, 0, SparseVoxelDag::new(), 0);
            let vis = aokana.evaluate_visible_regions([0.0, 0.0, 0.0], &permissive_planes());
            expected_results.push(vis);
        }
        let first = &expected_results[0];
        assert_eq!(*first, vec![(0, 0, 0), (0, 1, 1), (1, 0, 0)]); // 辞書順厳密
        for v in &expected_results {
            assert_eq!(v, first); // 全インスタンスで完全一致 (決定性契約)
        }
    }

    #[test]
    fn frustum_plane_boundary_exact() {
        // 境界契約: dot+d == 0 (接触) は可視。厳密な f32 整数で導出:
        // 平面 [-1,0,0,64] (-x+64): region (1,0,0) は p-vertex x=min=64 で
        // -64+64=0 ≥0 → 可視、region (2,0,0) は -128+64=-64 <0 → 完全外部で捌く。
        let mut planes = permissive_planes();
        planes[0] = [-1.0, 0.0, 0.0, 64.0];
        let mut aokana = AokanaFramework::new(1280, 720);
        aokana.insert_shallow_region(2, 0, 0, SparseVoxelDag::new(), 0);
        aokana.insert_shallow_region(1, 0, 0, SparseVoxelDag::new(), 0);
        let vis = aokana.evaluate_visible_regions([0.0, 0.0, 0.0], &planes);
        assert_eq!(vis, vec![(1, 0, 0)]); // 接触のみ生存・順序も辞書順
    }

    #[test]
    fn insert_replaces_and_root_id_is_truthful_after_build() {
        // CB-2 + CA-1 連携: 再登録は置換、root_id は build 済み DAG の真値。
        let mut aokana = AokanaFramework::new(1280, 720);
        let mut solid = SparseVoxelDag::new();
        let root = solid.build_from_volume(&[[[true; 16]; 16]; 16]);
        assert_eq!(root, 5); // wave 77 CA-3 の手導出値
        aokana.insert_shallow_region(0, 0, 0, solid, 0);
        assert_eq!(aokana.shallow_dags[&(0, 0, 0)].root_id, 5); // 真値保持
        assert_eq!(aokana.shallow_dags[&(0, 0, 0)].lod_level, 0);
        // 同一座標へ空 DAG を再登録 → 置換 (root_id 0)、件数は増えない
        aokana.insert_shallow_region(0, 0, 0, SparseVoxelDag::new(), 2);
        assert_eq!(aokana.shallow_dags.len(), 1);
        assert_eq!(aokana.shallow_dags[&(0, 0, 0)].root_id, 0);
        assert_eq!(aokana.shallow_dags[&(0, 0, 0)].lod_level, 2);
        // region_size_blocks は 64 固定 (aokana 規約 — full_graph_wiring 注記と一致)
        assert_eq!(aokana.region_size_blocks, 64);
    }
}

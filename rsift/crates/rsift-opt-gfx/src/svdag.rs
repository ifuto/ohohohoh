//! # 15. Sparse Voxel DAG (`SparseVoxelDag` / SVDAG)
//!
//! SVO (Sparse Voxel Octree) を有向非巡回グラフ (DAG) に縮約し、
//! 同一のサブツリー構造を持つノードをハッシュプール (`node_pool`) でポインタ共有・統合する。
//! 数十億ボクセルのシーンにおけるメモリフットプリントを 1〜3 桁圧縮する。

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SvdagNodeData {
    pub child_mask: u8,        // 8-bit occupancy mask for octree children
    pub children: [u32; 8],    // Child node IDs (`u32::MAX` = none / leaf)
}

#[derive(Debug, Clone)]
pub struct SparseVoxelDag {
    pub nodes: Vec<SvdagNodeData>,
    node_pool: HashMap<SvdagNodeData, u32>,
    pub root_id: u32,
}

impl Default for SparseVoxelDag {
    fn default() -> Self {
        Self::new()
    }
}

impl SparseVoxelDag {
    pub fn new() -> Self {
        let mut dag = Self {
            nodes: Vec::with_capacity(1024),
            node_pool: HashMap::with_capacity(1024),
            root_id: u32::MAX,
        };
        // 基底ノードは「空リーフ」のみ事前登録する (solid 基底は登録しない
        // 事実契約 — 初めての solid ボクセルが動的に id 1 を得る)。
        // id 0 = 空リーフ { mask 0, children all u32::MAX } は de-facto 契約
        // であり、初期 root_id == 0 は「空世界の root」を意味する (CA-2)。
        let empty_leaf = SvdagNodeData { child_mask: 0, children: [u32::MAX; 8] };
        dag.root_id = dag.insert_node(empty_leaf);
        dag
    }

    /// Insert or share an identical subtree node in $O(1)$ amortized time.
    ///
    /// 決定性契約: node_pool は lookup のみに使われ、ID は挿入順の連番。
    /// 同一の挿入列は常に同一の ID 列・`nodes` 内容を与える (機械ピン)。
    pub fn insert_node(&mut self, node: SvdagNodeData) -> u32 {
        if let Some(&id) = self.node_pool.get(&node) {
            return id;
        }
        let id = self.nodes.len() as u32;
        self.node_pool.insert(node.clone(), id);
        self.nodes.push(node);
        id
    }

    /// Build a bottom-up compressed SVDAG from a 3D binary voxel array (`volume: 16x16x16`).
    ///
    /// 契約 (wave 77 CA-1): 構築した root で `self.root_id` を**更新**し、
    /// 同じ値を返す。旧実装は root_id を「空世界 root」 id 0 のまま放置し、
    /// 保持者 (aokana ShallowSvdag) に stale な入口点を配っていた。
    pub fn build_from_volume(&mut self, volume: &[[[bool; 16]; 16]; 16]) -> u32 {
        self.root_id = self.build_octree_recursive(volume, 0, 0, 0, 16);
        self.root_id
    }

    fn build_octree_recursive(&mut self, volume: &[[[bool; 16]; 16]; 16], x: usize, y: usize, z: usize, size: usize) -> u32 {
        if size == 1 {
            let solid = volume[x][y][z];
            let leaf = SvdagNodeData {
                child_mask: if solid { 1 } else { 0 },
                children: [u32::MAX; 8],
            };
            return self.insert_node(leaf);
        }

        let half = size / 2;
        let mut child_mask = 0u8;
        let mut children = [u32::MAX; 8];

        for i in 0..8 {
            let cx = x + ((i & 1) != 0) as usize * half;
            let cy = y + ((i & 2) != 0) as usize * half;
            let cz = z + ((i & 4) != 0) as usize * half;
            let child_id = self.build_octree_recursive(volume, cx, cy, cz, half);
            let child_node = &self.nodes[child_id as usize];
            if child_node.child_mask != 0 {
                child_mask |= 1 << i;
                children[i] = child_id;
            }
        }

        let node = SvdagNodeData { child_mask, children };
        self.insert_node(node)
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_svdag_deduplication() {
        let mut dag = SparseVoxelDag::new();
        let mut vol = [[[false; 16]; 16]; 16];
        // Fill half volume with identical pattern
        for x in 0..8 {
            for y in 0..8 {
                for z in 0..8 {
                    vol[x][y][z] = true;
                    vol[x + 8][y + 8][z + 8] = true;
                }
            }
        }
        let root = dag.build_from_volume(&vol);
        assert!(root != u32::MAX);
        assert!(dag.node_count() < 300, "Identical subtrees must be deduplicated across DAG");
    }

    /// テスト用不変量走査: 全ノードで mask bit ⟺ children[i] != MAX、
    /// children[i] が指す ID は有効範囲内、nodes[0] は空リーフ。
    fn assert_dag_invariants(dag: &SparseVoxelDag) {
        assert_eq!(
            dag.nodes[0],
            SvdagNodeData {
                child_mask: 0,
                children: [u32::MAX; 8]
            },
            "id 0 = 空リーフの de-facto 契約"
        );
        for (idx, n) in dag.nodes.iter().enumerate() {
            let all_max = n.children.iter().all(|&c| c == u32::MAX);
            if all_max {
                // リーフ形 (size==1 由来): mask は solid フラグで 0/1 のみ。
                assert!(
                    n.child_mask <= 1,
                    "リーフ形ノードの mask は 0/1 必須 (node {idx}: mask {})",
                    n.child_mask
                );
                continue;
            }
            // 中間ノード: mask bit は「子孫に solid を含む子へのリンク」の
            // 存在と一対一 (build_octree_recursive の prune 契約)。
            for i in 0..8 {
                let bit = (n.child_mask >> i) & 1 == 1;
                let linked = n.children[i] != u32::MAX;
                assert_eq!(
                    bit, linked,
                    "mask bit と children リンクの一対一 (node {idx}, oct {i})"
                );
                if linked {
                    assert!(
                        (n.children[i] as usize) < dag.nodes.len(),
                        "children[{i}] は有効 ID 必須 (node {idx})"
                    );
                }
            }
        }
    }

    #[test]
    fn empty_volume_keeps_base_root_and_updates_root_id() {
        let mut dag = SparseVoxelDag::new();
        assert_eq!(dag.root_id, 0); // 初期 root_id = 空リーフ (空世界)
        assert_eq!(dag.node_count(), 1);
        let root = dag.build_from_volume(&[[[false; 16]; 16]; 16]);
        assert_eq!(root, 0); // 全空は空リーフに縮約
        assert_eq!(dag.root_id, 0); // CA-1 でも不変 (root == 0 で一致)
        assert_eq!(dag.node_count(), 1);
        assert_dag_invariants(&dag);
    }

    #[test]
    fn single_voxel_exact_chain() {
        // (0,0,0) のみ solid → 深さ優先で oct0 の鎖のみ。全値手導出。
        let mut dag = SparseVoxelDag::new();
        let mut vol = [[[false; 16]; 16]; 16];
        vol[0][0][0] = true;
        let root = dag.build_from_volume(&vol);
        assert_eq!(dag.node_count(), 6);
        assert_eq!(root, 5);
        assert_eq!(dag.root_id, 5); // CA-1: build が root_id を真値に更新
        let m = u32::MAX;
        assert_eq!(
            dag.nodes[1],
            SvdagNodeData {
                child_mask: 1,
                children: [m; 8]
            }
        );
        assert_eq!(
            dag.nodes[2],
            SvdagNodeData {
                child_mask: 1,
                children: [1, m, m, m, m, m, m, m]
            }
        );
        assert_eq!(
            dag.nodes[3],
            SvdagNodeData {
                child_mask: 1,
                children: [2, m, m, m, m, m, m, m]
            }
        );
        assert_eq!(
            dag.nodes[4],
            SvdagNodeData {
                child_mask: 1,
                children: [3, m, m, m, m, m, m, m]
            }
        );
        assert_eq!(
            dag.nodes[5],
            SvdagNodeData {
                child_mask: 1,
                children: [4, m, m, m, m, m, m, m]
            }
        );
        assert_dag_invariants(&dag);
    }

    #[test]
    fn all_solid_exact_dedup_chain() {
        // 全 solid → 各レベルで完全 dedup、鎖は {255, [k; 8]}。全値手導出。
        let mut dag = SparseVoxelDag::new();
        let vol = [[[true; 16]; 16]; 16];
        let root = dag.build_from_volume(&vol);
        assert_eq!(dag.node_count(), 6);
        assert_eq!(root, 5);
        assert_eq!(dag.root_id, 5);
        let m = u32::MAX;
        assert_eq!(
            dag.nodes[1],
            SvdagNodeData {
                child_mask: 1,
                children: [m; 8]
            }
        );
        assert_eq!(
            dag.nodes[2],
            SvdagNodeData {
                child_mask: 255,
                children: [1; 8]
            }
        );
        assert_eq!(
            dag.nodes[3],
            SvdagNodeData {
                child_mask: 255,
                children: [2; 8]
            }
        );
        assert_eq!(
            dag.nodes[4],
            SvdagNodeData {
                child_mask: 255,
                children: [3; 8]
            }
        );
        assert_eq!(
            dag.nodes[5],
            SvdagNodeData {
                child_mask: 255,
                children: [4; 8]
            }
        );
        assert_dag_invariants(&dag);
    }

    #[test]
    fn two_adjacent_voxels_share_parent_with_mask3() {
        // (0,0,0),(1,0,0): 最下位ノードが mask 3 でリーフ id 1 を 2 度指す。
        let mut dag = SparseVoxelDag::new();
        let mut vol = [[[false; 16]; 16]; 16];
        vol[0][0][0] = true;
        vol[1][0][0] = true;
        let root = dag.build_from_volume(&vol);
        assert_eq!(dag.node_count(), 6);
        assert_eq!((root, dag.root_id), (5, 5));
        let m = u32::MAX;
        assert_eq!(
            dag.nodes[2],
            SvdagNodeData {
                child_mask: 3,
                children: [1, 1, m, m, m, m, m, m]
            }
        );
        assert_eq!(
            dag.nodes[3],
            SvdagNodeData {
                child_mask: 1,
                children: [2, m, m, m, m, m, m, m]
            }
        );
        assert_dag_invariants(&dag);
    }

    #[test]
    fn build_is_deterministic_across_instances() {
        // パターン volume ((x+y+z)%3==0) を 2 インスタンスで構築 → 完全一致。
        let mut vol = [[[false; 16]; 16]; 16];
        for x in 0..16 {
            for y in 0..16 {
                for z in 0..16 {
                    vol[x][y][z] = (x + y + z) % 3 == 0;
                }
            }
        }
        let mut a = SparseVoxelDag::new();
        let mut b = SparseVoxelDag::new();
        let ra = a.build_from_volume(&vol);
        let rb = b.build_from_volume(&vol);
        assert_eq!(ra, rb);
        assert_eq!(a.nodes, b.nodes); // 同一挿入列 → 同一 ID/内容 (決定性契約)
        assert_dag_invariants(&a);
    }
}

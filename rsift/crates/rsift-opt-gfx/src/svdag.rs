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
        // Insert leaf empty/solid base nodes
        let empty_leaf = SvdagNodeData { child_mask: 0, children: [u32::MAX; 8] };
        dag.root_id = dag.insert_node(empty_leaf);
        dag
    }

    /// Insert or share an identical subtree node in $O(1)$ amortized time.
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
    pub fn build_from_volume(&mut self, volume: &[[[bool; 16]; 16]; 16]) -> u32 {
        self.build_octree_recursive(volume, 0, 0, 0, 16)
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
}

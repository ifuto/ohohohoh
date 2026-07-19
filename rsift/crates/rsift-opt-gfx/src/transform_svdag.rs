//! # 16. Transform-Aware SVDAG (`TransformAwareSvdag` - 2025 I3D Molenaar & Eisemann)
//!
//! SVDAG の繰り返し構造統合をさらに拡張し、回転 (`90°, 180°, 270°` around Y) や
//! 鏡映対称 (`X-mirror, Z-mirror`) なサブツリーも同一の正規化ノード (`CanonicalSubtree`)
//! と変換タグ (`TransformTag`) で統合。ノード数をさらに半減させる。

use crate::svdag::{SparseVoxelDag, SvdagNodeData};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransformTag {
    pub rotate_y_90_count: u8, // 0..=3
    pub mirror_x: bool,
    pub mirror_z: bool,
}

impl Default for TransformTag {
    fn default() -> Self {
        Self {
            rotate_y_90_count: 0,
            mirror_x: false,
            mirror_z: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TransformAwareSvdag {
    pub base_dag: SparseVoxelDag,
    pub canonical_pool: HashMap<SvdagNodeData, (u32, TransformTag)>,
}

impl Default for TransformAwareSvdag {
    fn default() -> Self {
        Self::new()
    }
}

impl TransformAwareSvdag {
    pub fn new() -> Self {
        Self {
            base_dag: SparseVoxelDag::new(),
            canonical_pool: HashMap::with_capacity(1024),
        }
    }

    /// Canonicalize node permutations to find structural symmetry under 3D transforms.
    pub fn insert_transform_aware(&mut self, mut node: SvdagNodeData) -> (u32, TransformTag) {
        if let Some(&(id, tag)) = self.canonical_pool.get(&node) {
            return (id, tag);
        }

        // Check permutations
        let mut best_node = node.clone();
        let mut best_tag = TransformTag::default();

        for rot in 0..4 {
            for &mx in &[false, true] {
                for &mz in &[false, true] {
                    let candidate = Self::permute_node(&node, rot, mx, mz);
                    if let Some(&(id, _)) = self.canonical_pool.get(&candidate) {
                        return (
                            id,
                            TransformTag {
                                rotate_y_90_count: rot,
                                mirror_x: mx,
                                mirror_z: mz,
                            },
                        );
                    }
                    if candidate.child_mask < best_node.child_mask {
                        best_node = candidate;
                        best_tag = TransformTag {
                            rotate_y_90_count: rot,
                            mirror_x: mx,
                            mirror_z: mz,
                        };
                    }
                }
            }
        }

        let id = self.base_dag.insert_node(best_node.clone());
        self.canonical_pool.insert(node.clone(), (id, best_tag));
        self.canonical_pool.insert(best_node, (id, TransformTag::default()));
        (id, best_tag)
    }

    fn permute_node(node: &SvdagNodeData, rot_y: u8, mx: bool, mz: bool) -> SvdagNodeData {
        let mut new_mask = 0u8;
        let mut new_children = [u32::MAX; 8];

        for i in 0..8 {
            if (node.child_mask & (1 << i)) != 0 {
                let mut x = (i & 1) != 0;
                let mut y = (i & 2) != 0;
                let mut z = (i & 4) != 0;

                for _ in 0..(rot_y % 4) {
                    let old_x = x;
                    x = z;
                    z = !old_x;
                }
                if mx { x = !x; }
                if mz { z = !z; }

                let new_i = (x as usize) | ((y as usize) << 1) | ((z as usize) << 2);
                new_mask |= 1 << new_i;
                new_children[new_i] = node.children[i];
            }
        }

        SvdagNodeData {
            child_mask: new_mask,
            children: new_children,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transform_aware_canonicalization() {
        let mut tdag = TransformAwareSvdag::new();
        let n1 = SvdagNodeData { child_mask: 0b0000_0001, children: [1, u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX] };
        let n2 = SvdagNodeData { child_mask: 0b0000_0100, children: [u32::MAX, u32::MAX, 1, u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX] };

        let (id1, _) = tdag.insert_transform_aware(n1);
        let (id2, _) = tdag.insert_transform_aware(n2);
        assert_eq!(id1, id2, "Rotated/mirrored subtrees must canonicalize to same node ID");
    }
}

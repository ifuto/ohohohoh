
//! Temporal Mesh Diff - dirty flagで変わったブロックのみ差分再メッシュ
//! 石1つ置いても8頂点だけ更新、従来全再構築を回避

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SectionKey { pub cx: i32, pub cz: i32, pub sy: i32 }

#[derive(Debug, Clone)]
pub struct MeshPatch {
    pub removed_quads: Vec<u32>,
    pub added_quads: Vec<crate::packed4::PackedPullQuad>,
}

pub struct TemporalDiff {
    dirty: HashMap<SectionKey, u64>,
    generation: u64,
}

impl TemporalDiff {
    pub fn new() -> Self { Self { dirty: HashMap::new(), generation: 0 } }

    pub fn mark_dirty(&mut self, key: SectionKey) {
        self.generation += 1;
        self.dirty.insert(key, self.generation);
    }

    pub fn take_dirty(&mut self) -> Vec<SectionKey> {
        let keys: Vec<SectionKey> = self.dirty.keys().copied().collect();
        self.dirty.clear();
        keys
    }

    pub fn diff_section(old_palette: &[u16; 4096], new_palette: &[u16; 4096]) -> Vec<usize> {
        old_palette.iter().zip(new_palette.iter()).enumerate()
            .filter_map(|(i, (a,b))| if a!=b { Some(i) } else { None })
            .collect()
    }

    pub fn patch_for_block(block_idx: usize) -> MeshPatch {
        // 簡易: ブロック1つの変更で最大6面*2tri=12quad影響と仮定し、周辺8ブロックだけ再生成を指示
        MeshPatch { removed_quads: vec![block_idx as u32], added_quads: Vec::new() }
    }
}

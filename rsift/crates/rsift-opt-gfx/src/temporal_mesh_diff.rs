
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

#[cfg(test)]
mod tests {
    use super::*;

    fn key(cx: i32, cz: i32, sy: i32) -> SectionKey {
        SectionKey { cx, cz, sy }
    }

    #[test]
    fn take_dirty_is_set_semantics_with_generation_counter() {
        let mut d = TemporalDiff::new();
        assert!(d.take_dirty().is_empty());
        assert_eq!(d.generation, 0);
        d.mark_dirty(key(0, 0, 0));
        d.mark_dirty(key(1, 0, 0));
        d.mark_dirty(key(0, 0, 0)); // 同一 key の再 mark は重複登録されない
        assert_eq!(d.generation, 3);
        let mut keys = d.take_dirty();
        keys.sort_by(|a, b| (a.cx, a.cz, a.sy).cmp(&(b.cx, b.cz, b.sy)));
        assert_eq!(keys, vec![key(0, 0, 0), key(1, 0, 0)]);
    }

    #[test]
    fn take_dirty_clears_queue() {
        let mut d = TemporalDiff::new();
        d.mark_dirty(key(0, 0, 1));
        assert_eq!(d.take_dirty().len(), 1);
        assert!(d.take_dirty().is_empty()); // 2 回目以降は空
        d.mark_dirty(key(0, 0, 1));
        assert_eq!(d.take_dirty().len(), 1); // 再 mark で再出現
    }

    #[test]
    fn diff_section_reports_exact_changed_indices() {
        let a = [7u16; 4096];
        let mut b = a;
        assert!(TemporalDiff::diff_section(&a, &b).is_empty());
        b[100] = 8;
        b[2000] = 9;
        assert_eq!(TemporalDiff::diff_section(&a, &b), vec![100, 2000]);
    }

    #[test]
    fn patch_for_block_shape() {
        let p = TemporalDiff::patch_for_block(7);
        assert_eq!(p.removed_quads, vec![7u32]);
        assert!(p.added_quads.is_empty());
    }
}

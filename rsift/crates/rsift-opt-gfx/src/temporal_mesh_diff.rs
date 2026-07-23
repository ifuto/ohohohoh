//! Temporal Mesh Diff — dirty セクションの**決定的追跡**とパレット差分列挙。
//!
//! ## wave 69 BS-1: 責務の誠実な範囲 (旧ヘッダ主張の訂正)
//! 旧 doc「石1つ置いても8頂点だけ更新、従来全再構築を回避」は本モジュールの
//! コードに根拠のない主張だった (該当機構は存在しない)。本モジュールの
//! 実際の責務は次の 2 点に限定する:
//! 1. セクション単位の dirty トラッキング (`mark_dirty` / `take_dirty`) —
//!    HashMap イテレーション順の非決定性を排した**ソート済み決定的 drain**。
//! 2. 新旧パレットの厳密な変更ボクセル index 列挙 (`diff_section`)。
//! メッシュ差分再生成そのものは `diff_mesh.rs` (bitmask dirty + 真 patch
//! 機構、監査済) と mesher が担当する。`diff_section` の live 配線
//! (worldgen 差分→部分再メッシュ) は将来の配線候補として記録する
//! (消費者追加方針: 削除せず候補記録)。

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SectionKey {
    pub cx: i32,
    pub cz: i32,
    pub sy: i32,
}

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
    pub fn new() -> Self {
        Self {
            dirty: HashMap::new(),
            generation: 0,
        }
    }

    pub fn mark_dirty(&mut self, key: SectionKey) {
        self.generation += 1;
        self.dirty.insert(key, self.generation);
    }

    /// dirty キーを全て取り出しキューを空にする。
    /// **wave 69 BS-2**: 返り順は (cx, cz, sy) 昇順にソート済みで決定的。
    /// 旧実装は HashMap イテレーション順 (RandomState 由来、プロセス毎に
    /// 不定) をそのまま返しており、消費側がコスト均一なカウンタ集計のみ
    /// だったために被害は潜在化していたが、キー同一次第の処理 (例
    /// 予算切り詰め時の後回し identity) が入れば実行毎に分岐し得る
    /// 非決定性ハザードだった。規則を契約として固定する。
    pub fn take_dirty(&mut self) -> Vec<SectionKey> {
        let mut keys: Vec<SectionKey> = self.dirty.keys().copied().collect();
        keys.sort_unstable();
        self.dirty.clear();
        keys
    }

    pub fn diff_section(old_palette: &[u16; 4096], new_palette: &[u16; 4096]) -> Vec<usize> {
        old_palette
            .iter()
            .zip(new_palette.iter())
            .enumerate()
            .filter_map(|(i, (a, b))| if a != b { Some(i) } else { None })
            .collect()
    }

    /// **プレースホルダ見積 (wave 69 BS-1 で誠実化)**:
    /// ブロック 1 個の変更通知に対し、識別子ブロック 1 個分を
    /// `removed_quads` に格納した patch を返す (旧コメントの「最大
    /// 6 面×2tri=12 quad 影響・周辺 8 ブロック再生成指示」「8 頂点だけ
    /// 更新」は実装根拠のない主張であり撤回)。返り patch の
    /// `removed_quads.len()` は常に 1 でコスト見積 counter 専用。
    /// 真のブロック単位メッシュ patch 生成は新旧パレットを要し、本関数の
    /// 入力では計算不可能 — `diff_mesh.rs` (真 patch 機構) を利用すること。
    ///
    /// **契約**: block_idx はセクション内 index (0..4096)。live 呼出は
    /// `packed & 0xFFF` (full_graph_wiring:760) で常に適合。
    pub fn patch_for_block(block_idx: usize) -> MeshPatch {
        assert!(
            block_idx < 4096,
            "patch_for_block 契約違反: block_idx={block_idx} はセクション外"
        );
        MeshPatch {
            removed_quads: vec![block_idx as u32],
            added_quads: Vec::new(),
        }
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

    /// wave 69 BS-2: take_dirty の返り順は挿入順・HashMap 状態に依らず
    /// (cx, cz, sy) 昇順で決定的 (旧実装は HashMap 順=プロセス毎不定)。
    /// 敵対的挿入順 (降順・インターリーブ) でも出力はソート済み。
    #[test]
    fn take_dirty_returns_sorted_deterministic_order() {
        let mut d = TemporalDiff::new();
        let entries = [
            key(3, -1, 2),
            key(-2, 7, 0),
            key(3, -1, 0),
            key(0, 0, 0),
            key(-2, 7, 3),
            key(3, -1, 1),
        ];
        for &k in &entries {
            d.mark_dirty(k);
        }
        let got = d.take_dirty();
        let mut sorted = entries.to_vec();
        sorted.sort();
        assert_eq!(got, sorted, "drain must be deterministically sorted");
        // generation カウンタは mark 回数で厳密に進む (重複 mark 含む)
        assert_eq!(d.generation, entries.len() as u64);
        // 再 mark → 再 drain も同一規則
        for &k in entries.iter().rev() {
            d.mark_dirty(k);
        }
        assert_eq!(d.take_dirty(), sorted);
    }

    /// wave 69 BS-3: block_idx のセクション内契約 (0..4096) を入口強制。
    /// 4095 は受理 (live 呼出 `packed & 0xFFF` の最大値)。
    #[test]
    fn patch_for_block_boundary_acceptance() {
        let p = TemporalDiff::patch_for_block(4095);
        assert_eq!(p.removed_quads, vec![4095u32]);
    }

    #[test]
    #[should_panic(expected = "patch_for_block 契約違反")]
    fn patch_for_block_rejects_out_of_section() {
        let _ = TemporalDiff::patch_for_block(4096);
    }
}

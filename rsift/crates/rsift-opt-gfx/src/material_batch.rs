//! Material batching + opaque/transparent draw-range split (Tier 1).
//! Sodium-style: group by material to kill texture binds.
//!
//! 【wave 152 EX 監査注記】
//! 1. **捕捉 64 [小] (削除構造の数学逸脱として記録)**: 削除した
//!    `FloraInstance::new` の量子化は位置 `(x * 256.0).clamp(..) as u16`
//!    (as キャスト = ゼロ方向 truncate、−0.5LSB 系統偏向、捕捉 59/60 同型の
//!    中心化欠落) なのに yaw は `.round()` (最近傍) で、同一構造内で丸め
//!    規則が不統一だった。根治は 3 の構造除去。
//! 2. **捕捉 65 [中] (wiring 側で根治)**: wiring の debug_assert は
//!    translucent **quads** 件数と translucent **draw ranges** 件数を誤等置
//!    していた。同一 mat の連続 translucent quad 2 個で debug 誤爆
//!    (TDD RED 機械記録: left=2, right=1)。根治は Σquad_count 不変式への
//!    修正 + draw 件数系の report 実配線 (§7 消化 15、`let _ =` 破棄根治)。
//! 3. **§7 消化 15 不可能証明削除**: `FloraInstance` / `InstancedFloraRenderer`
//!    / `draw_call_count` / `split_opaque_transparent` は crates/ 全体 census
//!    grep で消費者ゼロ、wiring の実データ経路 (quads/bin 列) にも供給点が
//!    存在せず、quad 列を位置/スケールに見立てる接続は意味論捏造 = 偽装禁止
//!    抵触のため接続不能。証明の上で完全削除した (wave 149 GC-4 anim 系判例)。
//!    `draw_call_count` は加えて `build_draws` を O(n log n) で二重実行する
//!    無駄実装だった (真値は groups.len() 系列で O(1)〜O(n))。
//! 4. **契約注記 (防御ガード非追加、到達不可性の証明付き)**: wiring は
//!    `quad_materials` (u32) を `as u16` で fold して push する。65536 超の
//!    material id は同値類に折り畳まれるが、block-state 実空間 (~2.6 万)
//!    では 65535 を超えないため実害ゼロ — 契約としてここに明記する。
//!    同一 `quad_index` の二重 push は防御しない (sorted 走査で range 重複
//!    しうる) が、wiring は `enumerate()` 由来の [0,len) を 1 回ずつ push
//!    するため到達不可。`compress_map` の `prev + 1` は `u32::MAX` で
//!    debug wrap panic しうるが同理由で到達不可 (ガード追加は非本質 =
//!    yagni として見送り、本注記で契約化)。`indices.is_empty()` の
//!    `continue` も `or_default()` 構築経路上は空 Vec が残り得ない防御的
//!    到達不可ガードである。
//! 5. **エンコーディング正規化**: 本ファイルは全 187 行が CRLF だった
//!    (wave 146 exporter 判例に従い) 本 wave で LF に一括正規化した。

use std::collections::BTreeMap;

/// One drawable run after material grouping (同一 material の連続 quad run)。
#[derive(Debug, Clone, Copy)]
pub struct BatchedDraw {
    pub material_id: u16,
    pub first_quad: u32,
    pub quad_count: u32,
    pub translucent: bool,
}

#[derive(Debug, Default)]
pub struct MaterialBatcher {
    /// material_id → list of quad indices (opaque)
    groups: BTreeMap<u16, Vec<u32>>,
    /// material_id → list of quad indices (translucent)
    translucent: BTreeMap<u16, Vec<u32>>,
}

impl MaterialBatcher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.groups.clear();
        self.translucent.clear();
    }

    pub fn push_quad(&mut self, material_id: u16, quad_index: u32, translucent: bool) {
        let map = if translucent {
            &mut self.translucent
        } else {
            &mut self.groups
        };
        map.entry(material_id).or_default().push(quad_index);
    }

    /// Merge consecutive quads into draw ranges (minimizes draw calls).
    pub fn build_draws(&self) -> (Vec<BatchedDraw>, Vec<BatchedDraw>) {
        (
            Self::compress_map(&self.groups, false),
            Self::compress_map(&self.translucent, true),
        )
    }

    fn compress_map(map: &BTreeMap<u16, Vec<u32>>, translucent: bool) -> Vec<BatchedDraw> {
        let mut out = Vec::with_capacity(map.len());
        for (&material_id, indices) in map {
            if indices.is_empty() {
                continue;
            }
            let mut sorted = indices.clone();
            sorted.sort_unstable();
            let mut start = sorted[0];
            let mut count = 1u32;
            let mut prev = sorted[0];
            for &idx in &sorted[1..] {
                if idx == prev + 1 {
                    count += 1;
                    prev = idx;
                } else {
                    out.push(BatchedDraw {
                        material_id,
                        first_quad: start,
                        quad_count: count,
                        translucent,
                    });
                    start = idx;
                    count = 1;
                    prev = idx;
                }
            }
            out.push(BatchedDraw {
                material_id,
                first_quad: start,
                quad_count: count,
                translucent,
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type DrawTuple = (u16, u32, u32, bool);

    fn tuples(draws: &[BatchedDraw]) -> Vec<DrawTuple> {
        draws
            .iter()
            .map(|d| (d.material_id, d.first_quad, d.quad_count, d.translucent))
            .collect()
    }

    #[test]
    fn batch_merges_runs() {
        let mut b = MaterialBatcher::new();
        for i in 0..5 {
            b.push_quad(1, i, false);
        }
        b.push_quad(1, 10, false);
        let (opaque, _) = b.build_draws();
        assert_eq!(opaque.len(), 2);
        assert_eq!(opaque[0].quad_count, 5);
        assert_eq!(opaque[1].first_quad, 10);
    }

    /// 【wave 152 EX 厳密 golden】merge 条件行列 (rq ex_mb (5) 導出):
    /// mat9=[3,4,7,8] → (9,first=3,count=2),(9,first=7,count=2)、
    /// mat5=[0,1] → (5,0,2)。BTreeMap 昇順で mat5 が先。binned 総数 6。
    #[test]
    fn batch_merge_matrix_golden_strict() {
        let mut b = MaterialBatcher::new();
        // 挿入順は意図的に scrambles (sort 経路を実稼働させる)。
        for (mat, idx) in [(9u16, 7u32), (9, 3), (5, 1), (9, 8), (5, 0), (9, 4)] {
            b.push_quad(mat, idx, false);
        }
        let (opaque, translucent) = b.build_draws();
        assert_eq!(
            tuples(&opaque),
            vec![
                (5u16, 0u32, 2u32, false),
                (9u16, 3u32, 2u32, false),
                (9u16, 7u32, 2u32, false),
            ],
            "merge matrix golden (rq ex_mb)"
        );
        assert!(translucent.is_empty(), "全員 opaque");
        let binned: u32 = opaque.iter().map(|d| d.quad_count).sum();
        assert_eq!(binned, 6, "Σquad_count == push 回数 6 (完全性)");
    }

    /// 【wave 152 EX 厳密 golden】dual-map 分割 (rq ex_mb (3) 導出):
    /// wiring ER-2 素材 [5(T),12(T),30(F)] 相当を module 層で厳密化。
    #[test]
    fn batch_translucent_dual_maps_golden_strict() {
        let mut b = MaterialBatcher::new();
        b.push_quad(5, 0, true);
        b.push_quad(12, 1, true);
        b.push_quad(30, 2, false);
        let (opaque, translucent) = b.build_draws();
        assert_eq!(tuples(&opaque), vec![(30u16, 2u32, 1u32, false)]);
        assert_eq!(
            tuples(&translucent),
            vec![(5u16, 0u32, 1u32, true), (12u16, 1u32, 1u32, true)],
            "translucent 側も BTreeMap 昇順・run 厳密 (rq ex_mb)"
        );
        // 同一 mat 連続 translucent quad = 捕捉 65 の module 層 golden:
        // [5,5] → 1 run (first=0, count=2)、ranges≠quads を構造 pin。
        let mut c = MaterialBatcher::new();
        c.push_quad(5, 0, true);
        c.push_quad(5, 1, true);
        let (_, t2) = c.build_draws();
        assert_eq!(
            tuples(&t2),
            vec![(5u16, 0u32, 2u32, true)],
            "quads 2 個 = 1 range: 件数混同は数学的に誤り (捕捉 65 module pin)"
        );
    }

    /// 【wave 152 EX】挿入順独立性: push 順 [30,5,12] でも BTreeMap key
    /// 昇順 [5,12,30] で draw 列が一意 (rq ex_mb (7))。決定性 pin。
    #[test]
    fn batch_insertion_order_btree_ascending_strict() {
        let mut b = MaterialBatcher::new();
        b.push_quad(30, 0, false);
        b.push_quad(5, 1, false);
        b.push_quad(12, 2, false);
        let (opaque, _) = b.build_draws();
        assert_eq!(
            tuples(&opaque),
            vec![
                (5u16, 1u32, 1u32, false),
                (12u16, 2u32, 1u32, false),
                (30u16, 0u32, 1u32, false),
            ],
            "material_id 昇順・first_quad は push 時の実 index (rq ex_mb)"
        );
    }

    /// 【wave 152 EX 契約 pin】同一 quad_index の二重 push は防御しない:
    /// sorted=[4,4] で 4≠4+1 → 2 ranges (重複) となる現行挙動を pin
    /// (注記 4: wiring 経路では到達不可)。回帰で防御追加した場合は
    /// 本 pin の更新をもって意図変更を可視化する。
    #[test]
    fn batch_duplicate_index_disjoint_runs_pin() {
        let mut b = MaterialBatcher::new();
        b.push_quad(7, 4, false);
        b.push_quad(7, 4, false);
        let (opaque, _) = b.build_draws();
        assert_eq!(
            tuples(&opaque),
            vec![(7u16, 4u32, 1u32, false), (7u16, 4u32, 1u32, false)],
            "duplicate index → 2 runs (防御なし契約、rq ex_mb (6))"
        );
    }

    /// 【wave 152 EX】clear 後の完全再利用: 前データ非混入・空 golden。
    #[test]
    fn batch_clear_reuse_strict() {
        let mut b = MaterialBatcher::new();
        b.push_quad(3, 0, false);
        b.push_quad(3, 1, false);
        b.push_quad(8, 2, true);
        b.clear();
        let (o0, t0) = b.build_draws();
        assert!(o0.is_empty() && t0.is_empty(), "clear 後は空 golden");
        b.push_quad(3, 5, false);
        let (o1, t1) = b.build_draws();
        assert_eq!(tuples(&o1), vec![(3u16, 5u32, 1u32, false)]);
        assert!(t1.is_empty(), "clear で translucent map も残さない");
    }

    /// 【wave 152 EX】空入力 golden: (0 draws, 0 draws)。
    #[test]
    fn batch_empty_golden_strict() {
        let b = MaterialBatcher::new();
        let (o, t) = b.build_draws();
        assert!(o.is_empty() && t.is_empty(), "empty golden (rq ex_mb (2))");
    }
}

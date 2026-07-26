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
//!
//! # 監査 2026-07-26 (wave 122 DV) — 契約公表
//!
//! - **DV-1**: wiring 駆動形状の公表 — `full_graph_wiring:782-790` は毎
//!   フレーム chunk_keys **全件**を無条件 `mark_dirty` (sy=i%4) する。
//!   「変更検出」の本来フィルタは wiring に無く全件 dirty 駆動だが、
//!   時分割キュー (time_slice) への供給源としては一貫 (BS-1 の責務範囲
//!   と整合)。同型 soak pin で実証。
//! - **DV-2**: wiring `packed_key` の厳密 bit レイアウト: cx 10bit<<20 /
//!   cz 10bit<<10 / sy 10bit。セクション外は折り畳み (cx=1024≡0、sy=-1
//!   ≡1023、cz=-1≡1023)。patch 駆動値 `packed & 0xFFF` は cz 下位 2bit
//!   と sy 10bit の混在 (CI-2 誠実注記) — rq 導出厳密値で pin 化。
//! - **DV-3**: `dirty: HashMap<SectionKey, u64>` の値 (generation) は
//!   書き込まれるが `take_dirty` 経路ではキーのみ返却され**消費者ゼロ**
//!   (世代カウンタ `self.generation` 自体は pin 済)。将来の世代比較用の
//!   保持として誠実公表。
//! - **DV-4**: `diff_section` の live 消費者ゼロは継続 (census
//!   2026-07-26: full_graph_wiring は patch_for_block のみ呼出)。BS-1
//!   候補記録の追認 + 全差異列挙の決定性 pin 強化。

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

    // ================= wave 122 DV: 厳密契約ピン群 =================
    // 全厳密値は rq (dv_packed.rq, RQ.md v2) で事前導出・assert 通過済。

    /// DV-2: wiring packed_key の厳密 bit レイアウト (cx 10bit<<20 /
    /// cz 10bit<<10 / sy 10bit) と折り畳み、patch 駆動値の混在契約。
    /// full_graph_wiring:792-796 同型の再現式で pin (rq 導出値に 1:1 対応)。
    #[test]
    fn wiring_packed_key_bit_layout_exact() {
        let packed = |cx: i32, cz: i32, sy: i32| -> u32 {
            (((cx as u32) & 0x3FF) << 20) | (((cz as u32) & 0x3FF) << 10) | ((sy as u32) & 0x3FF)
        };
        // rq 導出厳密値 (dv_packed.rq 出力)
        assert_eq!(packed(0, 3, 4095), 4095u32, "(0,3,4095): 3072|1023");
        assert_eq!(packed(0, 3, 4095) & 0xFFF, 4095u32);
        assert_eq!(packed(5, 1, 5), 5_243_909u32, "(5,1,5)=0x500405");
        assert_eq!(packed(5, 1, 5) & 0xFFF, 1029u32, "drive=1029");
        assert_eq!(
            packed(-1, 0, 0),
            0x3FF0_0000u32,
            "cx=-1 は 1023<<20=1072693248"
        );
        // 折り畳み衝突 (公表): cx=1024≡0、sy=-1≡1023、cz=-1 の drive=3072
        assert_eq!(packed(1024, 0, 0), packed(0, 0, 0), "cx fold 1024≡0");
        assert_eq!(packed(0, 0, -1) & 0xFFF, 1023u32, "sy=-1≡1023");
        assert_eq!(
            packed(0, -1, 0) & 0xFFF,
            3072u32,
            "cz=-1: 下位 2bit=3 <<10=3072 (CI-2 混在の pin 化)"
        );
        // patch 駆動値は常に <4096 (patch_for_block 契約適合、全領域構造的保証)
        for cz in 0i32..4 {
            for sy in 0i32..4 {
                assert!(packed(7, cz, sy) & 0xFFF < 4096);
            }
        }
    }

    /// DV-1: wiring 同型 soak — 毎フレーム全 chunk_keys を無条件
    /// mark_dirty (sy=i%4) → drain (sorted) → packed_key → 駆動値の
    /// 全経路が決定的かつ契約内であることの実証 (full_graph_wiring
    /// :782-797 と同一形状の再現)。
    #[test]
    fn wiring_shape_always_all_dirty_soak() {
        let mut d = TemporalDiff::new();
        let chunk_keys = [(3i32, 5i32), (7, -2), (100, 40)];
        // 3 フレーム: 毎回「変更検出フィルタ無し」で全件 mark (DV-1 の形状)
        for _frame in 0..3 {
            let mut dirty_sections = 0u32;
            for (i, k) in chunk_keys.iter().enumerate() {
                d.mark_dirty(key(k.0, k.1, i as i32 % 4));
                dirty_sections += 1;
            }
            assert_eq!(dirty_sections, 3, "全件無条件 mark (フィルタ無し)");
            let taken = d.take_dirty(); // sorted drain
            assert_eq!(taken.len(), 3);
            let mut sorted = vec![key(3, 5, 0), key(7, -2, 1), key(100, 40, 2)];
            sorted.sort();
            assert_eq!(taken, sorted, "drain は決定的ソート (BS-2 規則)");
            // packed_key 変換 → patch 駆動値は全件契約内
            for tkey in &taken {
                let pk = (((tkey.cx as u32) & 0x3FF) << 20)
                    | (((tkey.cz as u32) & 0x3FF) << 10)
                    | ((tkey.sy as u32) & 0x3FF);
                let drive = (pk & 0xFFF) as usize;
                assert!(drive < 4096, "patch_for_block 契約満たす駆動値");
            }
        }
        // sy = i%4 系列の検算 pin (rq: 0..8 → 連結 1230123)
        let sy_series: Vec<i32> = (0i32..8).map(|i| i % 4).collect();
        assert_eq!(sy_series, vec![0, 1, 2, 3, 0, 1, 2, 3]);
    }

    /// DV-3: dirty map の値 (generation) は書き込まれるが take_dirty
    /// 経路で消費者ゼロ — 保持の誠実公表 pin。値の実在は map 内部で
    /// 確認できるが、返却されるのはキーのみ。
    #[test]
    fn generation_value_written_but_never_read_kept() {
        let mut d = TemporalDiff::new();
        d.mark_dirty(key(1, 2, 3)); // gen=1
        d.mark_dirty(key(1, 2, 3)); // 重複 mark → gen=2、値は 2 に上書き
        assert_eq!(d.generation, 2);
        // map 内部の値は generation そのもの (書き込み確かに存在)
        assert_eq!(d.dirty[&key(1, 2, 3)], 2u64, "値は直近 generation");
        // しかし take_dirty の出力に値は現れない (消費者ゼロの保持設計)
        let out = d.take_dirty();
        assert_eq!(out, vec![key(1, 2, 3)], "返却はキーのみ (値は消費されない)");
        assert!(d.dirty.is_empty());
    }

    /// DV-4: diff_section 決定性 pin 強化 — 全 4096 差異の完全列挙は
    /// index 昇順そのまま (enumerate 由来)、先頭/末尾含む境界も厳密。
    /// live 消費者ゼロ継続 (census 2026-07-26) の候補保持を追認。
    #[test]
    fn diff_section_full_enumeration_is_identity_order() {
        let a = [1u16; 4096];
        let b = [2u16; 4096]; // 全要素が差異
        let diff = TemporalDiff::diff_section(&a, &b);
        assert_eq!(diff.len(), 4096, "全差異は 4096 件");
        assert_eq!(diff[0], 0);
        assert_eq!(diff[4095], 4095);
        // 決定的: 同一入力では常に同一出力 (境界 index 単独も厳密)
        let a2 = [9u16; 4096];
        let mut b2 = a2;
        b2[0] = 10;
        b2[4095] = 10;
        assert_eq!(TemporalDiff::diff_section(&a2, &b2), vec![0usize, 4095]);
    }
}

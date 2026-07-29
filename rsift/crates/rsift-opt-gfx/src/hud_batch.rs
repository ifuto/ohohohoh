//! ImmediatelyFast 逆輸入 — HUD/テキスト/ネームタグの即時モード描画を 1 ドローコールに集約。
//!
//! ImmediatelyFast の本質 = 「(texture, blend, scissor, layer) 単位でマージ」。
//! ここでは tessellate 済みの頂点ボクスを受け取り、単一の続きバッファに
//! 再パックして、キー別の描画範囲 (base_vertex..index_count) を O(1) で再生する。
//!
//! 【wave 155 FA 監査注記】
//! 1. **捕捉 71 [中] (slot_for 返却非一致の冗長 3 段帳尻構造)**: `slot_for`
//!    は `slot = self.ranges.len()` (末尾) を返すのに、新規キーの実配置は
//!    層昇順挿入位置 `insert_pos` — 両者が非一致。旧 push_quad はこれを
//!    (a) 誤 range への `base_vertex` 設定 attempt (index_count>0 で skip
//!    されるのみ)、(b) `map.get_mut(&key).map(|_| ())` no-op、(c) 直後の
//!    `self.map[&key]` 再取得、の冗長 3 段で辛うじて帳尻を合わせていた
//!    (base_vertex は finalize が絶対 index 方式で非消費のため非顕在化)。
//!    根治: slot_for は `insert_pos` を直接返す + push_quad を単一路に
//!    簡素化 (挙動同一は interleave/repeat-after-insert golden で pin)。
//! 2. **捕捉 72 [小] (rect 供給値契約不一致、捕捉 62/63 クラス同型)**:
//!    module 契約は `[x,y,w,h]` なのに wiring は w に x_end
//!    (8.0+v*120.0) を供給 → 棒グラフの幅が常に +8px 過大 (v=0 で幅 0
//!    であるべき所に 8px の非ゼロ棒、v=1 で 128/120=+6.7% 幅過大)。
//!    TDD RED 機械記録: bar0 v=0 で幅 bits 8.0≠0.0 (rq fa_hud 予想一致)。
//!    根治: wiring は `v.clamp(0.0,1.0)*120.0` を w として供給 (端 = 8+vw
//!    で設計どおり)。
//! 3. **§7 消化 18**: wiring は `finish` 結果 (BatchView: ranges/indices/
//!    vertices) と `draw_calls_saved` を `drop(let _)` 両方破棄していた
//!    → report へ `hud_quads`/`hud_draw_ranges`/`hud_draw_calls_saved`
//!    を実配線 (全値 scene 由来 report 値→確定的 4/4/0、det pin 正当)。
//! 4. **捕捉 73 [中] (非連続同一キー結合の未実装 = doc 主張との乖離)**:
//!    旧 finalize_indices は range の first_index..+index_count slice
//!    のみで、非連続同一キー push (A,B,A) では A range 区間に中間 B の
//!    run が混入して二重描画系 (TDD golden RED 機械記録:
//!    right=[...,0,1,2,0,2,3,8,9,10,8,10,11] vs left=[...,4,5,6,4,6,7])。
//!    根治: chunks run 記録 + finalize run 単位再配置 (by_slot 1 pass
//!    集約、O(chunks)、range×chunk 二重走査回避で低スペック整合)。
//! 5. **誠実撤回注記 (mojibake 誤読)**: 監査中に push_rect doc を
//!    「実線<U+FFFD×2>形」(置換文字 2 個入り) と文字化けしていると
//!    誤読したが、od byte 照合では
//!    「実線矩形」E7 9F A9=矩 の健全な UTF-8 で、表示段の誤読に過ぎ
//!    なかった (一次情報確認規律に従い撤回を記録、修正対象なし)。
//! 6. **契約注記**: (a) BatchKey は構造体全体が Hash のため同一 layer で
//!    texture/blend 違いは別 range (ImmediatelyFast 本来の texture bind
//!    削減と整合)。(b) indices は絶対頂点 index で保持、DrawRange.
//!    base_vertex は常に 0 が正解 (GL 送信時 base vertex =0) — 捕捉 71 の
//!    誤設定 attempt 対象だった名残の契約をここに明記 (first_index の
//!    契約は注記 7 で明確化)。(c) capacity 超過は Vec の通常成長
//!    (book-keeping、warn なし設計)。(d) 旧 `slot_for` の map ずらし
//!    (*v >= insert_pos → +1) は wave 190 GJ の append O(1) 化で構造自体が
//!    消滅 (ずらし不変式の維持責務ごと除去、注記 7)。
//! 7. **【wave 190 GJ】捕捉 129 [中] + 軽量化**: BatchView (ranges,
//!    indices) の自己整合化と frame 定常ゼロ再割当化。
//!    (a) **捕捉 129**: 旧 DrawRange.first_index は「挿入前 len 由来」の
//!    まま露出し、finalize 露出 buffer とは layer interleave/repeat 系で
//!    **全 range が他者 quad の slice を指す不整合**だった (python 機械
//!    実証: interleave 3 系・repeat 2 系で帰属照合 PROOF-OK、wiring が
//!    first_index 非消費かつ昇順連続 push のみで非顕在化)。根治:
//!    finalize が露出 buffer の累積確定位置で上書き (両 golden 値更新
//!    + gj_first_index_addresses_own_quads_contract が同質性を pin)。
//!    (b) **軽量化**: 新キー処理を append O(1) 化 (旧は insert + map 全値
//!    /chunks 全件の +1 ずらし = 新キー毎 O(keys)+O(chunks))、finalize を
//!    層 stable sort + two-pass 直接構築へ (旧 vec![Vec; K] by_slot + Vec
//!    返却差替えを廃止、定常ゼロ再割当 = gj_scratch_capacity_reuse_pin)。
//!    層昇順の等価性 (append 順 stable sort ≡ 旧挿入順) は独立 spec
//!    reference との 1024 系列 corpus 照合で機械立証
//!    (gj_finalize_matches_independent_spec_corpus)。

use std::collections::HashMap;

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct HudVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: u32, // ABGR パック
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BatchKey {
    pub texture_id: u32,
    /// 0 = 通常alpha, 1 = 加法, 2 = 乗算系
    pub blend: u8,
    pub scissor_id: u16,
    /// 描画順序保持用レイヤ (小さいほど下)。同一キー内は追加入順。
    pub layer: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct DrawRange {
    pub key: BatchKey,
    pub first_index: u32,
    pub index_count: u32,
    pub base_vertex: i32,
}

#[derive(Default)]
pub struct HudBatch {
    vertices: Vec<HudVertex>,
    indices: Vec<u32>,
    ranges: Vec<DrawRange>,
    /// キー → ranges 内 index のマップ (マージ用)
    map: HashMap<BatchKey, usize>,
    /// 【wave 155 FA-1 捕捉 73 根治】push 毎の chunk 基点
    /// (slot=range index, indices 内 offset)。非連続同一キーの真の
    /// 結合 (run 単位再配置) に必要。begin_frame でクリア。
    chunks: Vec<(u32, u32)>,
    quads_coalesced: u32,
}

impl HudBatch {
    pub fn new(quad_capacity: usize) -> Self {
        Self {
            vertices: Vec::with_capacity(quad_capacity * 4),
            indices: Vec::with_capacity(quad_capacity * 6),
            ranges: Vec::with_capacity(64),
            map: HashMap::with_capacity(64),
            chunks: Vec::with_capacity(quad_capacity),
            quads_coalesced: 0,
        }
    }

    /// フレーム開始時。容量は残してクリア (ゼロ再割当)。
    pub fn begin_frame(&mut self) {
        self.vertices.clear();
        self.indices.clear();
        self.ranges.clear();
        self.map.clear();
        self.chunks.clear();
        self.quads_coalesced = 0;
    }

    /// 4 頂点 (quad) を既に tessellate 済みの形で追加。
    ///
    /// `verts`=`0..4`, インデックス (0,1,2 0,2,3) 相当を自動接続。
    /// 【wave 155 FA-1 捕捉 71 根治】slot は slot_for の返却をそのまま使う
    /// (旧冗長 3 段を簡素化、挙動同一 golden pin 済)。【wave 190 GJ】
    /// slot は append 順 id (層昇順 materialize は finalize に移譲、
    /// push 経路は map 照会のみ O(1))。
    pub fn push_quad(&mut self, key: BatchKey, verts: [HudVertex; 4]) {
        let slot = self.slot_for(key);
        let base = self.vertices.len() as u32;
        self.vertices.extend_from_slice(&verts);
        let idx_off = self.indices.len() as u32;
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        self.chunks.push((slot as u32, idx_off)); // 捕捉 73 根治: run 記録
        self.ranges[slot].index_count += 6;
        self.quads_coalesced += 1;
    }

    /// テキスト 1 glyph 分: 左下原点の矩形。
    pub fn push_glyph(
        &mut self,
        key: BatchKey,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        uv: [(f32, f32); 2],
        color: u32,
    ) {
        let vs = [
            HudVertex { pos: [x, y], uv: [uv[0].0, uv[0].1], color },
            HudVertex { pos: [x + w, y], uv: [uv[1].0, uv[0].1], color },
            HudVertex { pos: [x + w, y + h], uv: [uv[1].0, uv[1].1], color },
            HudVertex { pos: [x, y + h], uv: [uv[0].0, uv[1].1], color },
        ];
        self.push_quad(key, vs);
    }

    /// 実線矩形 (塗り)。uv は atlas 白ピクセル。【wave 155 FA】契約:
    /// rect は [x, y, w, h] (捕捉 72: w に x_end を供給する旧 wiring の
    /// 誤りは wiring 側で根治、w=0 は幅 0 の空矩形として受理)。
    pub fn push_rect(&mut self, key: BatchKey, rect: [f32; 4], color: u32, white_uv: (f32, f32)) {
        let [x, y, w, h] = rect;
        self.push_glyph(key, x, y, w, h, [white_uv, white_uv], color);
    }

    fn slot_for(&mut self, key: BatchKey) -> usize {
        if let Some(&i) = self.map.get(&key) {
            return i;
        }
        // 【wave 190 GJ】append O(1) 化: 新規キーは末尾追加のみ (旧実装は
        // 層昇順位置への insert + map 全値/chunks 全件の +1 ずらし = 新キー
        // 毎に O(keys)+O(chunks))。層昇順の materialize は finalize の
        // stable sort が担う (append 順の層 stable sort ≡ 旧挿入順は
        // 帰納的に同一: 等層は共に作成順保持、spec corpus 1024 系列で
        // 機械等価立証)。first_index は finalize が露出 buffer 位置で
        // 上書き確定するため作成時は 0 (捕捉 129 明確化)。
        let slot = self.ranges.len();
        self.ranges.push(DrawRange {
            key,
            first_index: 0,
            index_count: 0,
            base_vertex: 0,
        });
        self.map.insert(key, slot);
        slot
    }

    /// 完成した GPU ビュー。`Upload` は呼び出し側の責務。
    /// 【wave 190 GJ】ranges/indices は scratch 内構築物を露出
    /// (契約: ranges は層昇順、first_index は露出 indices 上の自 range
    /// 開始位置 — 捕捉 129 明確化)。
    pub fn finish<'s>(&'s self, scratch: &'s mut BatchOutput) -> BatchView<'s> {
        self.finalize_into(scratch);
        BatchView {
            vertices: &self.vertices,
            indices: &scratch.indices,
            ranges: &scratch.ranges,
            quad_count: self.quads_coalesced,
        }
    }

    /// 連続頂点 + 非連続同一キー結合済みの最終インデックス列を
    /// scratch へ直接構築 (容量再利用で定常ゼロ再割当)。
    ///
    /// 【wave 155 FA-1 捕捉 73 [中] 根治】旧実装は range の
    /// `first_index..first_index+index_count` をそのまま切り出すだけで、
    /// **非連続同一キー push (A,B,A) では A の range 区間に中間の B の
    /// index run が混入**。根治: push 毎の chunk 基点 (slot→range idx,
    /// indices 内 offset) を記録し run 単位で再配置 (ImmediatelyFast の
    /// Text batching と同等、テキストは文字順≠キー順で非連続が本質)。
    ///
    /// 【wave 190 GJ 捕捉 129 [中] + 軽量化】(a) **捕捉 129 根治**: 旧
    /// first_index は「挿入前 len 由来」のまま露出で、layer interleave/
    /// repeat 系では露出 indices 上の**他者 slice** を指す不整合だった
    /// (python 機械実証、wiring が first_index 非消費で非顕在化)。
    /// 根治: finalize が露出 buffer の累積位置で上書き確定。(b) 旧
    /// `vec![Vec; K]` by_slot 集約 (K も中身も frame 毎に新規割当) と
    /// Vec 返却 (finish 内の差替えで旧容量を毎 frame 喪失) を廃止し、
    /// 層 stable sort (order) + 累積 offset (offs) の two-pass 直接構築へ
    /// — O(chunks + K) のまま定常ゼロ再割当 (scratch 容量再利用、
    /// gj_scratch_capacity_reuse_pin が構造 pin、alloc 回数は
    /// /tmp/gj_probe 計測で報告)。出力は独立 spec reference と 1024
    /// 系列 corpus で機械等価 (gj_finalize_matches_independent_spec_corpus)。
    fn finalize_into(&self, scratch: &mut BatchOutput) {
        scratch.indices.clear();
        scratch.ranges.clear();
        scratch.order.clear();
        scratch.offs.clear();
        let k = self.ranges.len();
        if k == 0 {
            return;
        }
        // 1) append 順 slot の層 stable sort (≡ 旧挿入順、等層は作成順)。
        scratch.order.extend(0..k as u32);
        scratch
            .order
            .sort_by_key(|&s| self.ranges[s as usize].key.layer);
        // 2) finalize 順の累積で first_index を確定し ranges を複写、
        //    slot 書込カーソル (offs) を各ブロック先頭に初期化。
        scratch.offs.resize(k, 0);
        let mut cum = 0u32;
        for &s in &scratch.order {
            let r = self.ranges[s as usize];
            scratch.ranges.push(DrawRange {
                key: r.key,
                first_index: cum,
                index_count: r.index_count,
                base_vertex: 0,
            });
            scratch.offs[s as usize] = cum;
            cum += r.index_count;
        }
        // 3) chunk (push 順) を final 位置へ複写 — slot 内順は push 順保持
        //    (カーソル前進で stable)。総量は常に cum == self.indices.len()。
        scratch.indices.resize(cum as usize, 0);
        for &(slot, off) in &self.chunks {
            let dst = scratch.offs[slot as usize] as usize;
            let start = off as usize;
            scratch.indices[dst..dst + 6].copy_from_slice(&self.indices[start..start + 6]);
            scratch.offs[slot as usize] += 6;
        }
    }

    pub fn draw_calls_saved(&self) -> u32 {
        self.quads_coalesced
            .saturating_sub(self.ranges.iter().filter(|r| r.index_count > 0).count() as u32)
    }
}

pub struct BatchView<'a> {
    pub vertices: &'a [HudVertex],
    pub indices: &'a [u32],
    pub ranges: &'a [DrawRange],
    pub quad_count: u32,
}

/// finish の作業領域兼一時保持。【wave 190 GJ】ranges/order/offs を追加
/// して全バッファ容量再利用 (定常で frame 毎の再割当ゼロ、wiring の
/// ObjectPool<BatchOutput> 確保回避設計と整合)。
#[derive(Default)]
pub struct BatchOutput {
    pub indices: Vec<u32>,
    /// finalize 構築物: 層昇順・露出確定 first_index の range 列
    /// (BatchView.ranges がここを指す)。
    ranges: Vec<DrawRange>,
    /// finalize 作業領域: 層 stable sort の slot 順列。
    order: Vec<u32>,
    /// finalize 作業領域: slot 毎の書込カーソル (初期値=ブロック先頭)。
    offs: Vec<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(t: u32, blend: u8, layer: u16) -> BatchKey {
        BatchKey { texture_id: t, blend, scissor_id: 0, layer }
    }

    fn mkquad(x: f32) -> [HudVertex; 4] {
        [
            HudVertex { pos: [x, 0.0], uv: [0.0; 2], color: 0xFF00FF00 },
            HudVertex { pos: [x + 1.0, 0.0], uv: [0.0; 2], color: 0xFF00FF00 },
            HudVertex { pos: [x + 1.0, 1.0], uv: [0.0; 2], color: 0xFF00FF00 },
            HudVertex { pos: [x, 1.0], uv: [0.0; 2], color: 0xFF00FF00 },
        ]
    }

    #[test]
    fn merges_same_key_into_one_range() {
        let mut b = HudBatch::new(64);
        b.begin_frame();
        let k = key(7, 0, 0);
        b.push_quad(k, mkquad(0.0));
        b.push_quad(k, mkquad(2.0));
        b.push_quad(k, mkquad(4.0));
        let mut out = BatchOutput::default();
        let view = b.finish(&mut out);
        assert_eq!(view.ranges.len(), 1);
        assert_eq!(view.ranges[0].index_count, 18);
        assert_eq!(view.vertices.len(), 12);
        assert_eq!(view.indices.len(), 18);
        assert_eq!(b.draw_calls_saved(), 2);
    }

    #[test]
    fn layers_sort_draw_ranges() {
        let mut b = HudBatch::new(64);
        b.begin_frame();
        b.push_quad(key(1, 0, 9), mkquad(0.0));
        b.push_quad(key(2, 0, 0), mkquad(1.0));
        let mut out = BatchOutput::default();
        let view = b.finish(&mut out);
        assert_eq!(view.ranges[0].key.texture_id, 2); // layer 0 first
    }

    /// 【wave 155 FA-1】quad index パターン厳密 golden (rq fa_hud (2)):
    /// quad q → base=4q、indices は (b, b+1, b+2, b, b+2, b+3)。
    /// merges_same_key の index **列**を厳密化 (件数だけでなく値まで)。
    #[test]
    fn same_key_index_sequence_golden_strict() {
        let mut b = HudBatch::new(64);
        b.begin_frame();
        let k = key(7, 0, 0);
        b.push_quad(k, mkquad(0.0));
        b.push_quad(k, mkquad(2.0));
        b.push_quad(k, mkquad(4.0));
        let mut out = BatchOutput::default();
        let view = b.finish(&mut out);
        let want: Vec<u32> = (0..3u32)
            .flat_map(|q| {
                let b = 4 * q;
                [b, b + 1, b + 2, b, b + 2, b + 3]
            })
            .collect();
        assert_eq!(view.indices, &want[..], "index 列 golden (rq fa_hud)");
        assert_eq!(
            (view.ranges[0].first_index, view.ranges[0].index_count),
            (0, 18),
            "merge → 単一 range fi=0 count=18"
        );
        assert_eq!(
            view.ranges[0].base_vertex, 0,
            "絶対 index 方式で base_vertex=0 契約"
        );
    }

    /// 【wave 155 FA-1 捕捉 71 pin / wave 190 GJ 捕捉 129 契約明確化】
    /// 層 interleave: push(L9 base0), push(L0 base4), push(L5 base8) →
    /// ranges は layer 昇順 [L0,L5,L9]、finalize = [L0 slice]++[L5 slice]
    /// ++[L9 slice] (rq fa_hud (3))。**first_index は finalize 露出 buffer
    /// 位置 (0/6/12)** — 旧値 [6,12,0] は「挿入前 len 由来」で露出 buffer
    /// 上の他者 slice を指していた不整合 (python 機械実証、捕捉 129 で根治、
    /// wiring は first_index 非消費のため非顕在化していた)。
    #[test]
    fn layer_interleave_golden_strict() {
        let mut b = HudBatch::new(64);
        b.begin_frame();
        b.push_quad(key(1, 0, 9), mkquad(0.0)); // base 0
        b.push_quad(key(2, 0, 0), mkquad(0.0)); // base 4
        b.push_quad(key(3, 0, 5), mkquad(0.0)); // base 8
        let mut out = BatchOutput::default();
        let view = b.finish(&mut out);
        let got: Vec<(u16, u32, u32)> = view
            .ranges
            .iter()
            .map(|r| (r.key.layer, r.first_index, r.index_count))
            .collect();
        assert_eq!(
            got,
            vec![(0u16, 0u32, 6u32), (5, 6, 6), (9, 12, 6)],
            "層昇順 range + finalize 露出位置 first_index (wave 190 GJ 機械導出)"
        );
        assert_eq!(
            view.indices,
            &[
                4, 5, 6, 4, 6, 7, // L0 slice (fi 6..12)
                8, 9, 10, 8, 10, 11, // L5 slice (fi 12..18)
                0, 1, 2, 0, 2, 3, // L9 slice (fi 0..6)
            ][..],
            "finalize 再配置 golden (rq fa_hud)"
        );
    }

    /// 【wave 155 FA-1 捕捉 71 pin / wave 190 GJ 捕捉 129 契約明確化】
    /// repeat-after-insert: L9 挿入後に L0 を前方挿入 (旧 map index ずらし
    /// 系) → L9 再 push は map hit で正しく合流 (rq fa_hud (4))。
    /// first_index は finalize 露出 buffer 位置 (0/6) — 旧値 (6,0) は
    /// 他者/混在 slice を指した不整合からの明確化 (捕捉 129)。
    #[test]
    fn repeat_after_insert_golden_strict() {
        let mut b = HudBatch::new(64);
        b.begin_frame();
        b.push_quad(key(1, 0, 9), mkquad(0.0)); // base 0
        b.push_quad(key(2, 0, 0), mkquad(0.0)); // base 4 (旧: 前方挿入 pos 0)
        b.push_quad(key(1, 0, 9), mkquad(0.0)); // base 8、map hit
        let mut out = BatchOutput::default();
        let view = b.finish(&mut out);
        let got: Vec<(u16, u32, u32)> = view
            .ranges
            .iter()
            .map(|r| (r.key.layer, r.first_index, r.index_count))
            .collect();
        assert_eq!(
            got,
            vec![(0u16, 0u32, 6u32), (9u16, 6u32, 12u32)],
            "合流後 slot 整合 + finalize 露出位置 first_index (wave 190 GJ)"
        );
        assert_eq!(
            view.indices,
            &[4, 5, 6, 4, 6, 7, 0, 1, 2, 0, 2, 3, 8, 9, 10, 8, 10, 11][..],
            "合流後 finalize golden (rq fa_hud)"
        );
    }

    /// 【wave 155 FA-1 捕捉 72 module 契約 golden】push_rect [x,y,w,h]:
    /// rect=[8,8,120,10] → verts (8,8),(128,8),(128,18),(8,18) 全 dyadic
    /// exact; w=0 → x1==x0 (空矩形、wiring の v=0 棒 0 根治の受理側 pin)。
    #[test]
    fn push_rect_contract_golden_strict() {
        let mut b = HudBatch::new(64);
        b.begin_frame();
        b.push_rect(
            key(0, 0, 0),
            [8.0, 8.0, 120.0, 10.0],
            0xFF00FF00,
            (0.0, 0.0),
        );
        b.push_rect(key(1, 0, 1), [8.0, 20.0, 0.0, 10.0], 0xFF00FF00, (0.0, 0.0));
        let mut out = BatchOutput::default();
        let view = b.finish(&mut out);
        let p = |i: usize| {
            (
                view.vertices[i].pos[0].to_bits(),
                view.vertices[i].pos[1].to_bits(),
            )
        };
        assert_eq!(p(0), (0x4100_0000u32, 0x4100_0000u32), "(8,8)");
        assert_eq!(p(1), (0x4300_0000u32, 0x4100_0000u32), "(128,8)");
        assert_eq!(p(2), (0x4300_0000u32, 0x4190_0000u32), "(128,18)");
        assert_eq!(p(3), (0x4100_0000u32, 0x4190_0000u32), "(8,18)");
        assert_eq!(
            view.vertices[5].pos[0].to_bits(),
            view.vertices[4].pos[0].to_bits(),
            "w=0 → x1==x0 (幅 0 exact、捕捉 72 受理側)"
        );
    }

    /// 【wave 155 FA-3】begin_frame 完全リセット pin + finish 空 golden:
    /// 再利用で前 frame の ranges/vertices/quads/saved が残らない。
    #[test]
    fn begin_frame_reuse_and_empty_golden_strict() {
        let mut b = HudBatch::new(64);
        b.begin_frame();
        b.push_quad(key(1, 0, 0), mkquad(0.0));
        b.begin_frame();
        let mut out = BatchOutput::default();
        let view = b.finish(&mut out);
        assert!(view.ranges.is_empty() && view.vertices.is_empty() && view.indices.is_empty());
        assert_eq!(view.quad_count, 0, "quads_coalesced も 0 リセット");
        assert_eq!(
            b.draw_calls_saved(),
            0,
            "saturating_sub で負にならない (0-0)"
        );
    }

    /// 【wave 190 GJ 捕捉 129】BatchView 自己整合 pin: 各 range の
    /// indices[first_index..first_index+index_count] が参照する quad は
    /// その range のキーで push されたもののみ (finalize 露出 buffer 基準
    /// の同質性)。旧契約 (挿入前 len 由来 fi) では層 interleave/repeat 系で
    /// 全 range が他者 slice を指した (python 機械実証 PROOF-OK 2 系)。
    #[test]
    fn gj_first_index_addresses_own_quads_contract() {
        let mut b = HudBatch::new(64);
        b.begin_frame();
        // x 目印で quad 帰属を識別 (全 dyadic exact、cast 安全)。
        b.push_quad(key(1, 0, 9), mkquad(10.0)); // tex1 L9
        b.push_quad(key(2, 0, 0), mkquad(20.0)); // tex2 L0
        b.push_quad(key(3, 0, 5), mkquad(30.0)); // tex3 L5
        b.push_quad(key(1, 0, 9), mkquad(40.0)); // tex1 repeat (非連続)
        b.push_quad(key(2, 0, 0), mkquad(50.0)); // tex2 repeat (非連続)
        let mut out = BatchOutput::default();
        let view = b.finish(&mut out);
        let want: std::collections::HashMap<u32, Vec<u32>> =
            [(1u32, vec![10u32, 40]), (3, vec![30]), (2, vec![20, 50])]
                .into_iter()
                .collect();
        let mut prev_layer = None;
        for r in view.ranges {
            if let Some(p) = prev_layer {
                assert!(r.key.layer > p, "層昇順 strict (異なる layer)");
            }
            prev_layer = Some(r.key.layer);
            let begin = r.first_index as usize;
            let sl = &view.indices[begin..begin + r.index_count as usize];
            let mut xs: Vec<u32> = sl
                .chunks_exact(6)
                .map(|c| view.vertices[c[0] as usize].pos[0] as u32)
                .collect();
            xs.sort_unstable();
            assert_eq!(
                xs, want[&r.key.texture_id],
                "range tex={} layer={} の slice 帰属 (捕捉 129)",
                r.key.texture_id, r.key.layer
            );
        }
        assert_eq!(view.indices.len(), 30, "5 quads × 6 indices");
    }

    /// 【wave 190 GJ】finalize SPEC 等価 corpus: 独立 reference
    /// (spec: キーを (layer, 初出順) で並べ各キーの chunk run を push 順に
    /// 連結、fi は累積) と finish 出力を全 4^5=1024 系列で機械照合。
    /// 旧実装は fi 契約相違で本テスト RED、新実装で完全一致 GREEN。
    #[test]
    fn gj_finalize_matches_independent_spec_corpus() {
        let keys = [key(1, 0, 9), key(2, 0, 0), key(3, 0, 5), key(4, 0, 0)];
        let mut seq = [0usize; 5];
        for n in 0..1024u32 {
            let mut m = n;
            for d in &mut seq {
                *d = (m % 4) as usize;
                m /= 4;
            }
            let mut b = HudBatch::new(8);
            b.begin_frame();
            for (i, &c) in seq.iter().enumerate() {
                b.push_quad(keys[c], mkquad(i as f32));
            }
            let mut out = BatchOutput::default();
            let view = b.finish(&mut out);
            // reference: (layer, 初出順) でキーを並べ run 連結。
            let mut app = [usize::MAX; 4];
            for (i, &c) in seq.iter().enumerate() {
                if app[c] == usize::MAX {
                    app[c] = i;
                }
            }
            let mut fseen: Vec<usize> = (0..4).filter(|&c| app[c] != usize::MAX).collect();
            fseen.sort_by_key(|&c| (keys[c].layer, app[c]));
            let mut ref_idx: Vec<u32> = Vec::new();
            let mut ref_ranges: Vec<(u16, u32, u32)> = Vec::new();
            for &c in &fseen {
                let fi = ref_idx.len() as u32;
                for (i, &s) in seq.iter().enumerate() {
                    if s == c {
                        let base = 4 * i as u32;
                        ref_idx.extend_from_slice(&[
                            base,
                            base + 1,
                            base + 2,
                            base,
                            base + 2,
                            base + 3,
                        ]);
                    }
                }
                ref_ranges.push((keys[c].layer, fi, (ref_idx.len() as u32) - fi));
            }
            let got_ranges: Vec<(u16, u32, u32)> = view
                .ranges
                .iter()
                .map(|r| (r.key.layer, r.first_index, r.index_count))
                .collect();
            assert_eq!(
                (&view.indices[..], got_ranges),
                (&ref_idx[..], ref_ranges),
                "corpus n={n} seq={seq:?} (spec 等価)"
            );
        }
    }

    /// 【wave 190 GJ】等層の range 順 = キー作成順 (stable sort 契約) pin。
    /// L1 に tex5→tex2→tex9 の順で初出させ、L0/L9 で挟んでも [5,2,9]。
    /// (green-today 構造 pin: 旧挿入実装も同順だった既知性質の財産化。)
    #[test]
    fn gj_equal_layer_creation_order_golden() {
        let mut b = HudBatch::new(64);
        b.begin_frame();
        b.push_quad(key(7, 0, 0), mkquad(0.0));
        b.push_quad(key(5, 0, 1), mkquad(0.0));
        b.push_quad(key(9, 0, 9), mkquad(0.0));
        b.push_quad(key(2, 0, 1), mkquad(0.0));
        b.push_quad(key(9, 0, 1), mkquad(0.0));
        let mut out = BatchOutput::default();
        let view = b.finish(&mut out);
        let got: Vec<(u16, u32)> = view
            .ranges
            .iter()
            .map(|r| (r.key.layer, r.key.texture_id))
            .collect();
        assert_eq!(
            got,
            vec![(0u16, 7u32), (1, 5), (1, 2), (1, 9), (9, 9)],
            "等層 L1 は tex 作成順 5→2→9 (stable 契約)"
        );
    }

    /// 【wave 190 GJ】BatchOutput scratch 容量再利用 pin (定常ゼロ再割当の
    /// 構造): 大 frame の容量を小 frame でも保持 (単調非減)・内容正確。
    /// 旧実装は finish 毎に indices Vec を差替えて容量を失い本テスト RED。
    #[test]
    fn gj_scratch_capacity_reuse_pin() {
        let mut b = HudBatch::new(8);
        let mut out = BatchOutput::default();
        b.begin_frame();
        for i in 0..100u32 {
            b.push_quad(key(i % 5, 0, (i % 3) as u16), mkquad(i as f32));
        }
        let v1 = b.finish(&mut out);
        assert_eq!(v1.indices.len(), 600);
        drop(v1);
        let cap_idx = out.indices.capacity();
        let cap_rng = out.ranges.capacity();
        assert!(
            cap_idx >= 600 && cap_rng >= 5,
            "frame1 容量記録 ({cap_idx},{cap_rng})"
        );
        b.begin_frame();
        b.push_quad(key(0, 0, 0), mkquad(1.0));
        let v2 = b.finish(&mut out);
        assert_eq!(v2.indices.len(), 6);
        assert_eq!(v2.ranges.len(), 1);
        drop(v2);
        assert!(
            out.indices.capacity() >= cap_idx && out.ranges.capacity() >= cap_rng,
            "小 frame で容量保持 ({},{}) >= ({cap_idx},{cap_rng})",
            out.indices.capacity(),
            out.ranges.capacity()
        );
    }
}

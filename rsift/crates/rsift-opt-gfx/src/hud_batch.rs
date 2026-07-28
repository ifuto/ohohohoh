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
//!    「実線��形」と文字化けしていると誤読したが、od byte 照合では
//!    「実線矩形」E7 9F A9=矩 の健全な UTF-8 で、表示段の誤読に過ぎ
//!    なかった (一次情報確認規律に従い撤回を記録、修正対象なし)。
//! 6. **契約注記**: (a) BatchKey は構造体全体が Hash のため同一 layer で
//!    texture/blend 違いは別 range (ImmediatelyFast 本来の texture bind
//!    削減と整合)。(b) indices は絶対頂点 index で保持、DrawRange.
//!    base_vertex は常に 0 が正解 (GL 送信時 base vertex =0、finalize は
//!    first_index/index_count のみ消費) — 捕捉 71 の誤設定 attempt 対象
//!    だった名残の契約をここに明記。(c) capacity 超過は Vec の通常
//!    成長 (book-keeping、warn なし設計)。(d) `slot_for` の map ずらし
//!    (*v >= insert_pos → +1) は挿入時のみ必要で、根治後も整合規則は
//!    golden (注記 1 の repeat-after-insert) で財産化済。

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
    /// 【wave 155 FA-1 捕捉 71 根治】slot は slot_for が返す挿入位置を
    /// そのまま使う (旧冗長 3 段: no-op get_mut/map 再取得/誤 range への
    /// base_vertex 設定 attempt を簡素化、挙動同一 golden pin 済)。
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
        // レイヤ順維持: 昇順に挿入する場所を探す (小さいレイヤ = 先に描画)
        let insert_pos = self
            .ranges
            .iter()
            .position(|r| r.key.layer > key.layer)
            .unwrap_or(self.ranges.len());
        // map の参照先 index をずらす (捕捉 73 根治の chunk 記録も同規則で
        // 一緒にずらす — 既 push の slot 記録が stale 化しない不変式)
        for v in self.map.values_mut() {
            if *v >= insert_pos {
                *v += 1;
            }
        }
        for c in &mut self.chunks {
            if c.0 as usize >= insert_pos {
                c.0 += 1;
            }
        }
        self.ranges.insert(
            insert_pos,
            DrawRange { key, first_index: self.indices.len() as u32, index_count: 0, base_vertex: 0 },
        );
        self.map.insert(key, insert_pos);
        // 捕捉 71 根治: 返却は実配置 insert_pos (旧は末尾 len で非一致、
        // 冗長 3 段で帳尻していた)。挙動同一は golden pin 済。
        insert_pos
    }

    /// 完成した GPU ビュー。`Upload` は呼び出し側の責務。
    pub fn finish<'s>(&'s self, scratch: &'s mut BatchOutput) -> BatchView<'s> {
        scratch.indices = self.finalize_indices();
        BatchView {
            vertices: &self.vertices,
            indices: &scratch.indices,
            ranges: &self.ranges,
            quad_count: self.quads_coalesced,
        }
    }

    /// 連続頂点 + 非連続同一キー結合済みの最終インデックス列。
    /// 【wave 155 FA-1 捕捉 73 [中] 根治】旧実装は range の
    /// `first_index..first_index+index_count` をそのまま切り出すだけで、
    /// **非連続同一キー push (A,B,A) では A の range 区間に中間の B の
    /// index run が混入** (doc 主張の「非連続 push の結合」は未実装、
    /// A range で B の quad が二重描画される誤描画系)。wiring は昇順連続
    /// push のみで非顕在化構造だった。根治: push 毎の chunk 基点
    /// (slot→range idx, indices 内 offset) を記録し、finalize は range
    /// ごとに自キーの run だけを複写する真の再配置へ (ImmediatelyFast の
    /// Text batching と同等、テキストは文字順≠キー順で非連続が本質)。
    /// golden は repeat-after-insert strict (rq fa_hud (4)) が財産化。
    fn finalize_indices(&self) -> Vec<u32> {
        // slot ごとの chunk offset 群に 1 pass で集約 (O(chunks)、
        // 従来の range×chunk 二重走査より低スペック整合)。
        let mut by_slot: Vec<Vec<u32>> = vec![Vec::new(); self.ranges.len()];
        for &(slot, off) in &self.chunks {
            by_slot[slot as usize].push(off);
        }
        let mut out = Vec::with_capacity(self.indices.len());
        for (slot, r) in self.ranges.iter().enumerate() {
            if r.index_count == 0 {
                continue;
            }
            for &off in &by_slot[slot] {
                let start = off as usize;
                out.extend_from_slice(&self.indices[start..start + 6]);
            }
        }
        out
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

#[derive(Default)]
pub struct BatchOutput {
    pub indices: Vec<u32>,
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

    /// 【wave 155 FA-1 捕捉 71 pin】層 interleave: push(L9 base0),
    /// push(L0 base4), push(L5 base8) → ranges は layer 昇順 [L0,L5,L9] で
    /// first_index=[6,12,0]、finalize = [L0 slice]++[L5 slice]++[L9 slice]
    /// (rq fa_hud (3))。slot_for 根治後も挙動同一であることを厳密照合。
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
            vec![(0u16, 6u32, 6u32), (5, 12, 6), (9, 0, 6)],
            "層昇順 range + 挿入前 len 由来 first_index (rq fa_hud)"
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

    /// 【wave 155 FA-1 捕捉 71 pin】repeat-after-insert: L9 挿入後に L0 を
    /// 前方挿入 (map index ずらし) → L9 再 push は map hit で slot 1 に
    /// 正しく合流 (rq fa_hud (4))。根治後の単一路でも同一結果を厳密化。
    #[test]
    fn repeat_after_insert_golden_strict() {
        let mut b = HudBatch::new(64);
        b.begin_frame();
        b.push_quad(key(1, 0, 9), mkquad(0.0)); // base 0
        b.push_quad(key(2, 0, 0), mkquad(0.0)); // base 4、前方挿入 pos 0
        b.push_quad(key(1, 0, 9), mkquad(0.0)); // base 8、map hit slot 1
        let mut out = BatchOutput::default();
        let view = b.finish(&mut out);
        let got: Vec<(u16, u32, u32)> = view
            .ranges
            .iter()
            .map(|r| (r.key.layer, r.first_index, r.index_count))
            .collect();
        assert_eq!(
            got,
            vec![(0u16, 6u32, 6u32), (9u16, 0u32, 12u32)],
            "map ずらし後も slot 整合 (rq fa_hud)"
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
}

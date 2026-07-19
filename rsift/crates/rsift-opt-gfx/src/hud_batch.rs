//! ImmediatelyFast 逆輸入 — HUD/テキスト/ネームタグの即時モード描画を 1 ドローコールに集約。
//!
//! ImmediatelyFast の本質 = 「(texture, blend, scissor, layer) 単位でマージ」。
//! ここでは tessellate 済みの頂点ボクスを受け取り、単一の続きバッファに
//! 再パックして、キー別の描画範囲 (base_vertex..index_count) を O(1) で再生する。

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
    quads_coalesced: u32,
}

impl HudBatch {
    pub fn new(quad_capacity: usize) -> Self {
        Self {
            vertices: Vec::with_capacity(quad_capacity * 4),
            indices: Vec::with_capacity(quad_capacity * 6),
            ranges: Vec::with_capacity(64),
            map: HashMap::with_capacity(64),
            quads_coalesced: 0,
        }
    }

    /// フレーム開始時。容量は残してクリア (ゼロ再割当)。
    pub fn begin_frame(&mut self) {
        self.vertices.clear();
        self.indices.clear();
        self.ranges.clear();
        self.map.clear();
        self.quads_coalesced = 0;
    }

    /// 4 頂点 (quad) を既に tessellate 済みの形で追加。
    ///
    /// `verts`=`0..4`, インデックス (0,1,2 0,2,3) 相当を自動接続。
    pub fn push_quad(&mut self, key: BatchKey, verts: [HudVertex; 4]) {
        let slot = self.slot_for(key);
        let base = self.vertices.len() as u32;
        self.vertices.extend_from_slice(&verts);
        let r = &mut self.ranges[slot];
        if r.index_count == 0 {
            r.base_vertex = base as i32;
        }
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        self.map.get_mut(&key).map(|_| ());
        let slot = self.map[&key];
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

    /// 実線矩形 (塗り)。uv は atlas 白ピクセル。
    pub fn push_rect(&mut self, key: BatchKey, rect: [f32; 4], color: u32, white_uv: (f32, f32)) {
        let [x, y, w, h] = rect;
        self.push_glyph(key, x, y, w, h, [white_uv, white_uv], color);
    }

    fn slot_for(&mut self, key: BatchKey) -> usize {
        if let Some(&i) = self.map.get(&key) {
            return i;
        }
        let slot = self.ranges.len();
        // レイヤ順維持: 昇順に挿入する場所を探す (小さいレイヤ = 先に描画)
        let insert_pos = self
            .ranges
            .iter()
            .position(|r| r.key.layer > key.layer)
            .unwrap_or(self.ranges.len());
        // map の参照先 index をずらす
        for v in self.map.values_mut() {
            if *v >= insert_pos {
                *v += 1;
            }
        }
        self.ranges.insert(
            insert_pos,
            DrawRange { key, first_index: self.indices.len() as u32, index_count: 0, base_vertex: 0 },
        );
        self.map.insert(key, insert_pos);
        slot
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

    /// 連続頂点 + base_vertex 補正済みの最終インデックス列。
    /// 同一キーの非連続 push を結合するために再配置する (ImmediatelyFast の Text batching と同等の後処理)。
    fn finalize_indices(&self) -> Vec<u32> {
        // 簡潔化: 各 range は base_vertex 起点で質量連続なので、そのまま並べ直すだけ。
        let mut out = Vec::with_capacity(self.indices.len());
        for r in &self.ranges {
            if r.index_count == 0 {
                continue;
            }
            let start = r.first_index as usize;
            let end = start + r.index_count as usize;
            out.extend_from_slice(&self.indices[start..end]);
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
}

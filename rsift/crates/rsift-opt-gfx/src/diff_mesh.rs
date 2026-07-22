//! Section-level differential mesh updates (Tier 2).
//! Block edits dirty 16³ sections only — never rebuild the whole column.

use std::collections::HashMap;

pub const SECTION_SIZE: i32 = 16;
pub const SECTIONS_Y: i32 = 24; // -64..320 → 24 sections

/// セクションメッシュの差分パッチ (GPU アップロード単位)。
///
/// `quad_count` と `vertex_bytes` / `index_bytes` の整合 (stride 倍数、
/// quad×6 インデックス数等) は本モジュールでは**検証しない** (生成側の責務)。
/// 2026-07-22 wave 31 で契約明文化。
#[derive(Debug, Clone)]
pub struct MeshPatch {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub section_y: i32,
    pub vertex_bytes: Vec<u8>,
    pub index_bytes: Vec<u8>,
    pub quad_count: u32,
}

#[derive(Debug, Default)]
pub struct DiffMeshUpdater {
    /// (cx, cz) → bitset of dirty section indices (bit 0 = lowest Y section)
    dirty: HashMap<(i32, i32), u32>,
    /// Cached section mesh blobs for upload
    patches: Vec<MeshPatch>,
}

impl DiffMeshUpdater {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn block_to_section_y(block_y: i32) -> i32 {
        // world Y -64 → section 0
        (block_y + 64).div_euclid(SECTION_SIZE)
    }

    /// 注意 (**垂直のみ**): 本関数は垂直方向の隣接セクション伝播だけを行う。
    /// ブロックが chunk 端 (local x/z ∈ {0,15}) にある場合、隣接チャンクの
    /// セクションメッシュもこのブロックに依存し陳腐化し得るが、本 API は
    /// ブロックの chunk 内座標を受け取らないため**水平伝播は呼び出し側の
    /// 責務** (該当時は隣接 (cx±1, cz) / (cx, cz±1) にも同じ block_y で
    /// 呼ぶこと)。2026-07-22 wave 31 で契約明文化。
    pub fn mark_block_dirty(&mut self, chunk_x: i32, chunk_z: i32, block_y: i32) {
        let sy = Self::block_to_section_y(block_y);
        if sy < 0 || sy >= SECTIONS_Y {
            return;
        }
        let bits = self.dirty.entry((chunk_x, chunk_z)).or_insert(0);
        *bits |= 1u32 << sy;
        // Neighbor sections if on boundary
        let local_y = (block_y + 64).rem_euclid(SECTION_SIZE);
        if local_y == 0 && sy > 0 {
            *bits |= 1u32 << (sy - 1);
        }
        if local_y == SECTION_SIZE - 1 && sy + 1 < SECTIONS_Y {
            *bits |= 1u32 << (sy + 1);
        }
    }

    pub fn dirty_sections(&self, chunk_x: i32, chunk_z: i32) -> Vec<i32> {
        let mut out = Vec::new();
        if let Some(bits) = self.dirty.get(&(chunk_x, chunk_z)) {
            for i in 0..SECTIONS_Y {
                if bits & (1 << i) != 0 {
                    out.push(i);
                }
            }
        }
        out
    }

    pub fn clear_chunk(&mut self, chunk_x: i32, chunk_z: i32) {
        self.dirty.remove(&(chunk_x, chunk_z));
    }

    pub fn take_dirty_mask(&mut self, chunk_x: i32, chunk_z: i32) -> u32 {
        self.dirty.remove(&(chunk_x, chunk_z)).unwrap_or(0)
    }

    /// Register a rebuilt section patch for GPU upload.
    pub fn push_patch(&mut self, patch: MeshPatch) {
        // Replace existing patch for same section
        if let Some(pos) = self.patches.iter().position(|p| {
            p.chunk_x == patch.chunk_x
                && p.chunk_z == patch.chunk_z
                && p.section_y == patch.section_y
        }) {
            self.patches[pos] = patch;
        } else {
            self.patches.push(patch);
        }
    }

    pub fn drain_patches(&mut self) -> Vec<MeshPatch> {
        std::mem::take(&mut self.patches)
    }

    pub fn pending_patch_count(&self) -> usize {
        self.patches.len()
    }

    pub fn dirty_chunk_count(&self) -> usize {
        self.dirty.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_neighbors_on_boundary() {
        let mut d = DiffMeshUpdater::new();
        // Y=-64 is section 0 local_y=0 → also dirties nothing below
        d.mark_block_dirty(0, 0, -64);
        let s = d.dirty_sections(0, 0);
        assert!(s.contains(&0));
        // Y=-49 is top of section 0 → dirties section 1
        d.mark_block_dirty(0, 0, -49);
        let s = d.dirty_sections(0, 0);
        assert!(s.contains(&0) && s.contains(&1));
    }

    /// wave 31-1: block_to_section_y の厳密境界列 + 範囲外は非 dirty。
    #[test]
    fn section_y_mapping_exact_boundaries() {
        // (block_y + 64).div_euclid(16) の厳密列。
        let table = [
            (-65, -1), // 範囲外下 (拒否)
            (-64, 0),
            (-49, 0),
            (-48, 1),
            (0, 4),
            (255, 19),
            (304, 23),
            (319, 23),
            (320, 24), // 範囲外上 (拒否)
        ];
        for (y, want) in table {
            assert_eq!(DiffMeshUpdater::block_to_section_y(y), want, "y={y}");
        }
        let mut d = DiffMeshUpdater::new();
        d.mark_block_dirty(0, 0, -65);
        d.mark_block_dirty(0, 0, 320);
        assert_eq!(d.dirty_chunk_count(), 0, "範囲外 y は dirty を残さない");
    }

    /// wave 31-2: 境界伝播の bit 厳密ピン (上下端クランプ含む) と
    /// take_dirty_mask の排他消費。
    #[test]
    fn dirty_bits_exact_with_boundary_clamps_and_take_exclusive() {
        let mut d = DiffMeshUpdater::new();
        // y=0: sy=4, local 0 → sy-1=3 も伝播 → 0b11000 = 0x18
        d.mark_block_dirty(0, 0, 0);
        assert_eq!(d.take_dirty_mask(0, 0), 0x18);
        // take は排他消費: 直後は 0、別チャンクは無関係
        assert_eq!(d.take_dirty_mask(0, 0), 0);
        assert_eq!(d.take_dirty_mask(1, 1), 0);
        // y=319: sy=23, local 15 だが sy+1=24 は範囲外 → bit 23 のみ
        d.mark_block_dirty(0, 0, 319);
        assert_eq!(d.take_dirty_mask(0, 0), 1u32 << 23);
        // y=-64: sy=0, local 0 だが sy>0 不成立 → bit 0 のみ
        d.mark_block_dirty(0, 0, -64);
        assert_eq!(d.take_dirty_mask(0, 0), 1);
        // 中央 (y=8): sy=4 local 8 → bit 4 のみ
        d.mark_block_dirty(0, 0, 8);
        assert_eq!(d.take_dirty_mask(0, 0), 1 << 4);
        // 合算: y=304 (sy=23 local 0 → bit 23,22) + y=300 (sy=22 local 12 → bit 22)
        d.mark_block_dirty(0, 0, 304);
        d.mark_block_dirty(0, 0, 300);
        assert_eq!(d.take_dirty_mask(0, 0), (1 << 23) | (1 << 22));
    }

    /// wave 31-3: dirty_sections は昇順決定的。
    #[test]
    fn dirty_sections_ascending_deterministic() {
        let mut d = DiffMeshUpdater::new();
        // bit 0 (y=-64) / bit 2 (y=-32: sy=2 local 0 → bit 2,1) / bit 4 (y=0 で bit 4,3 も)
        d.mark_block_dirty(0, 0, -64); // bit 0
        d.mark_block_dirty(0, 0, -32); // sy=2 local 0 → bits 2,1
        d.mark_block_dirty(0, 0, 0); // sy=4 local 0 → bits 4,3
        assert_eq!(d.dirty_sections(0, 0), vec![0, 1, 2, 3, 4]);
        assert!(d.dirty_sections(9, 9).is_empty(), "未登録チャンクは空");
    }

    /// wave 31-4: push_patch は同一セクションを新で置換、drain で全回収。
    #[test]
    fn push_patch_replaces_same_section_and_drains_all() {
        let mut d = DiffMeshUpdater::new();
        let patch = |quad: u32, sy: i32| MeshPatch {
            chunk_x: 1,
            chunk_z: 2,
            section_y: sy,
            vertex_bytes: vec![7; 16],
            index_bytes: vec![3; 24],
            quad_count: quad,
        };
        d.push_patch(patch(10, 5));
        d.push_patch(patch(99, 5)); // 同一セクション → 置換
        d.push_patch(patch(20, 6)); // 別セクション → 追加
        assert_eq!(d.pending_patch_count(), 2);
        let out = d.drain_patches();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].quad_count, 99, "置換後は新パッチが生きる");
        assert_eq!(out[1].quad_count, 20);
        assert_eq!(d.pending_patch_count(), 0, "drain 後は空");
    }
}

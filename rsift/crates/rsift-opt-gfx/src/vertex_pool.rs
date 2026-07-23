//! Vertex pool — single GPU-side buffer slots (no per-chunk VBO alloc).

use crate::chunk_mesh::{BuiltChunkMesh, Quantized12ByteVertex};
use std::collections::HashMap;
use tracing::debug;

#[derive(Debug, Clone, Copy)]
pub struct PoolSlot {
    pub vertex_offset: u32,
    pub vertex_count: u32,
    pub index_offset: u32,
    pub index_count: u32,
    pub generation: u32,
}

#[derive(Debug)]
pub struct VertexPool {
    pub capacity_vertices: usize,
    pub capacity_indices: usize,
    pub vertex_bytes: usize,
    pub slots: HashMap<(i32, i32), PoolSlot>,
    pub vertex_cursor: u32,
    pub index_cursor: u32,
    pub generation: u32,
    pub ring_uploads: u64,
    /// 容量超過 (oversize) で拒否したアップロード回数 (telemetry)。
    /// 空メッシュによる除去は契約内の正常経路のため計数しない。
    pub oversize_rejects: u64,
}

impl VertexPool {
    pub fn new(capacity_vertices: usize) -> Self {
        let capacity_indices = capacity_vertices * 3 / 2;
        Self {
            capacity_vertices,
            capacity_indices,
            vertex_bytes: std::mem::size_of::<Quantized12ByteVertex>(),
            slots: HashMap::new(),
            vertex_cursor: 0,
            index_cursor: 0,
            generation: 0,
            ring_uploads: 0,
            oversize_rejects: 0,
        }
    }

    /// Adaptive pool size from hardware tier.
    pub fn adaptive() -> Self {
        let hw = rsift_api::AdaptivePerfEngine::hardware();
        let rp = rsift_api::AdaptivePerfEngine::render_profile(hw);
        let mb = rp.bump_arena_mb.max(4);
        let verts = (mb * 1024 * 1024) / std::mem::size_of::<Quantized12ByteVertex>();
        debug!("[VertexPool] capacity {} verts (~{} MB)", verts, mb);
        Self::new(verts)
    }

    /// Allocate or refresh a slot for chunk mesh data.
    ///
    /// # 契約 (機械ピン)
    /// - 戻り値が `None` のとき、(chunk_x, chunk_z) の slot は**必ず存在しない**。
    ///   空メッシュによる除去と容量超過拒否のどちらでもこの不変量は同じ
    ///   (wave 75 BY-1: 拒否時に旧 slot を残すと、将来 slots を描画に使う
    ///   消費者が stale geometry を参照し得るため一貫化)。
    /// - 同一 chunk の再アップロードは refresh: 旧領域は ring 上に放棄され
    ///   (回収は ring reset まで遅延)、新 slot がマッピングを置き換える。
    pub fn upload_mesh(&mut self, mesh: &BuiltChunkMesh) -> Option<PoolSlot> {
        if mesh.is_empty() {
            self.slots.remove(&(mesh.chunk_x, mesh.chunk_z));
            return None;
        }
        let vcount = mesh.vertices.len() as u32;
        let icount = mesh.indices.len() as u32;
        if vcount as usize > self.capacity_vertices || icount as usize > self.capacity_indices {
            self.slots.remove(&(mesh.chunk_x, mesh.chunk_z));
            self.oversize_rejects += 1;
            debug!(
                "[VertexPool] oversize mesh rejected (got {}v/{}i, cap {}v/{}i)",
                vcount, icount, self.capacity_vertices, self.capacity_indices
            );
            return None;
        }
        if self.vertex_cursor as usize + vcount as usize > self.capacity_vertices as usize
            || self.index_cursor as usize + icount as usize > self.capacity_indices as usize
        {
            self.reset_ring();
        }
        let slot = PoolSlot {
            vertex_offset: self.vertex_cursor,
            vertex_count: vcount,
            index_offset: self.index_cursor,
            index_count: icount,
            generation: self.generation,
        };
        self.vertex_cursor += vcount;
        self.index_cursor += icount;
        self.ring_uploads += 1;
        self.slots.insert((mesh.chunk_x, mesh.chunk_z), slot);
        Some(slot)
    }

    /// ring を先頭に巻き戻し、全 slot を無効化する。
    /// generation は `wrapping_add(1)` (u32 一周を許容する ring 設計の
    /// トレードオフ。2^32 回 reset 後の世代衝突は実時間到達不能として採用)。
    fn reset_ring(&mut self) {
        self.vertex_cursor = 0;
        self.index_cursor = 0;
        self.generation = self.generation.wrapping_add(1);
        self.slots.clear();
        debug!("[VertexPool] ring buffer reset (gen={})", self.generation);
    }

    pub fn active_chunks(&self) -> usize {
        self.slots.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mesh(cx: i32, cz: i32, quads: usize) -> BuiltChunkMesh {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for _ in 0..quads {
            let base = vertices.len() as u32;
            for k in 0..4 {
                vertices.push(Quantized12ByteVertex::encode(
                    0.0, k as f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0,
                ));
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
        }
        BuiltChunkMesh {
            chunk_x: cx,
            chunk_z: cz,
            vertices,
            indices,
        }
    }

    #[test]
    fn capacity_derived_and_upload_advances_cursors() {
        let mut pool = VertexPool::new(1024);
        assert_eq!(pool.capacity_indices, 1536); // 1.5x
        assert_eq!(pool.vertex_bytes, 12); // Quantized12ByteVertex は 12B
        let s0 = pool.upload_mesh(&mesh(0, 0, 2)).unwrap(); // 8v/12i
        assert_eq!((s0.vertex_offset, s0.vertex_count), (0, 8));
        assert_eq!((s0.index_offset, s0.index_count), (0, 12));
        assert_eq!(s0.generation, 0);
        let s1 = pool.upload_mesh(&mesh(1, 0, 1)).unwrap(); // 4v/6i
        assert_eq!((s1.vertex_offset, s1.index_offset), (8, 12));
        assert_eq!(pool.active_chunks(), 2);
        assert_eq!(pool.ring_uploads, 2);
    }

    #[test]
    fn empty_mesh_evicts_and_returns_none() {
        let mut pool = VertexPool::new(128);
        assert!(pool.upload_mesh(&mesh(0, 0, 0)).is_none()); // 空は slot 化されない
        assert_eq!(pool.active_chunks(), 0);
        let _ = pool.upload_mesh(&mesh(1, 1, 1)).unwrap();
        assert!(pool.upload_mesh(&mesh(1, 1, 0)).is_none());
        assert_eq!(pool.active_chunks(), 0); // 空アップロードで slot 除去
    }

    #[test]
    fn oversize_mesh_rejected_without_cursor_advance() {
        let mut pool = VertexPool::new(4);
        assert_eq!(pool.capacity_indices, 6);
        let big = mesh(9, 9, 2); // 8 vert > 容量 4
        assert!(pool.upload_mesh(&big).is_none());
        assert_eq!(pool.vertex_cursor, 0);
        assert_eq!(pool.index_cursor, 0);
        assert!(pool.slots.is_empty());
    }

    #[test]
    fn ring_reset_on_exhaustion_with_generation_advance() {
        let mut pool = VertexPool::new(12); // indices cap 18
        // 2 クアッド (8v/12i) + 1 クアッド (4v/6i) = 丁度満杯 (reset 無し: > 比較)。
        let _ = pool.upload_mesh(&mesh(0, 0, 2)).unwrap();
        let s = pool.upload_mesh(&mesh(1, 0, 1)).unwrap();
        assert_eq!(s.vertex_offset, 8);
        assert_eq!(pool.generation, 0);
        // 次の 1 クアッド要求で 12+4 > 12 → ring reset: 世代 up, offset 0 から。
        let s2 = pool.upload_mesh(&mesh(2, 0, 1)).unwrap();
        assert_eq!(s2.vertex_offset, 0);
        assert_eq!(s2.generation, 1);
        assert_eq!(pool.active_chunks(), 1); // reset で旧 slot は全消去
        assert_eq!(pool.ring_uploads, 3);
    }

    #[test]
    fn oversize_reject_evicts_stale_slot_and_counts() {
        // BY-1: 有効 slot 保有 chunk への oversize refresh は拒否 + evict。
        // 「None ⇒ slot 無し」で空メッシュ経路と不変量を揃える (全手導出)。
        let mut pool = VertexPool::new(4); // idx cap = 4*3/2 = 6
        let s = pool.upload_mesh(&mesh(0, 0, 1)).unwrap(); // 4v/6i 丁度適合
        assert_eq!((s.vertex_offset, s.index_offset, s.generation), (0, 0, 0));
        assert_eq!((pool.vertex_cursor, pool.index_cursor), (4, 6));
        // 同 chunk に 8v/12i (cap 超過): None + stale slot 除去 + 計数 1
        assert!(pool.upload_mesh(&mesh(0, 0, 2)).is_none());
        assert_eq!(pool.active_chunks(), 0);
        assert_eq!(pool.oversize_rejects, 1);
        assert_eq!((pool.vertex_cursor, pool.index_cursor), (4, 6)); // cursor 不動
        assert_eq!(pool.ring_uploads, 1);
        // slot 非保有の chunk への oversize も同契約 (None + 計数のみ)
        assert!(pool.upload_mesh(&mesh(7, 7, 2)).is_none());
        assert_eq!(pool.oversize_rejects, 2);
        // 空メッシュ除去は正常経路: reject 計数しない
        assert!(pool.upload_mesh(&mesh(0, 0, 0)).is_none());
        assert_eq!(pool.oversize_rejects, 2);
    }

    #[test]
    fn refresh_replaces_mapping_and_abandons_old_region() {
        // refresh: 旧領域は ring 上に放棄 (cursor は巻き戻らない)、
        // マッピングは新 slot が置き換える (手導出: 4v/6i → 8v/12i)。
        let mut pool = VertexPool::new(16); // idx cap 24
        let s0 = pool.upload_mesh(&mesh(0, 0, 1)).unwrap();
        let s1 = pool.upload_mesh(&mesh(0, 0, 2)).unwrap();
        assert_eq!((s0.vertex_offset, s0.vertex_count), (0, 4));
        assert_eq!(
            (
                s1.vertex_offset,
                s1.index_offset,
                s1.vertex_count,
                s1.index_count
            ),
            (4, 6, 8, 12)
        );
        assert_eq!(pool.active_chunks(), 1); // マッピングは 1 件に置換
        assert_eq!(pool.ring_uploads, 2);
        assert_eq!(pool.generation, 0);
        let g = pool.slots[&(0, 0)];
        assert_eq!((g.vertex_offset, g.vertex_count, g.generation), (4, 8, 0));
    }

    #[test]
    fn exact_capacity_boundary_fits_then_next_resets() {
        // `>` 比較の境界: cursor + count == capacity は適合 (reset しない)。
        let mut pool = VertexPool::new(8); // idx cap 12
        let s = pool.upload_mesh(&mesh(0, 0, 2)).unwrap(); // 8v/12i 丁度
        assert_eq!((s.vertex_offset, s.generation), (0, 0));
        let s2 = pool.upload_mesh(&mesh(1, 0, 1)).unwrap(); // 8+4 > 8 → reset
        assert_eq!(
            (s2.vertex_offset, s2.index_offset, s2.generation),
            (0, 0, 1)
        );
        assert_eq!(pool.active_chunks(), 1);
    }

    #[test]
    fn generation_wraps_on_reset_and_floor_ratio_pin() {
        // wrapping_add 契約: MAX → 0 (pub フィールド直書き経路で強制)。
        let mut pool = VertexPool::new(4);
        pool.generation = u32::MAX;
        let _ = pool.upload_mesh(&mesh(0, 0, 1)).unwrap(); // cursor 4v/6i
        let s = pool.upload_mesh(&mesh(1, 0, 1)).unwrap(); // 4+4 > 4 → reset
        assert_eq!(s.generation, 0); // MAX.wrapping_add(1) == 0

        // capacity_indices = v*3/2 の floor: 5 → 7 (quad 4v/6i は適合)。
        let mut pool = VertexPool::new(5);
        assert_eq!(pool.capacity_indices, 7);
        let s = pool.upload_mesh(&mesh(0, 0, 1)).unwrap();
        assert_eq!((s.vertex_count, s.index_count), (4, 6));
    }
}

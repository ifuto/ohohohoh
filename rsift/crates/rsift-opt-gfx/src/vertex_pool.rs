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
    pub fn upload_mesh(&mut self, mesh: &BuiltChunkMesh) -> Option<PoolSlot> {
        if mesh.is_empty {
            self.slots.remove(&(mesh.chunk_x, mesh.chunk_z));
            return None;
        }
        let vcount = mesh.vertices.len() as u32;
        let icount = mesh.indices.len() as u32;
        if vcount as usize > self.capacity_vertices || icount as usize > self.capacity_indices {
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
            is_empty: quads == 0,
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
}

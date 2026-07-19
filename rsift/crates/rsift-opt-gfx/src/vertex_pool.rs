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

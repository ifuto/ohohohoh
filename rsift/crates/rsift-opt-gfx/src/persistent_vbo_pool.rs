//! Persistent mapped VBO pool — one giant GPU buffer, bucket allocator, MDI batch.
//!
//! Eliminates per-chunk VBO alloc/free (driver GC). Combined with
//! `multi_draw_indexed_indirect`, all visible chunks draw in one command.

use crate::chunk_mesh::{BuiltChunkMesh, Quantized12ByteVertex, VERTEX_STRIDE_BYTES};
use crate::gpu_culling::DrawIndexedIndirectArgs;
use crate::packed4::PackedPullQuad;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};
use wgpu::util::DeviceExt;

const DEFAULT_POOL_MB_HIGH: usize = 1024;
const DEFAULT_POOL_MB_MEDIUM: usize = 512;
const DEFAULT_POOL_MB_LOW: usize = 256;

#[derive(Debug, Clone, Copy)]
pub struct PersistentSlot {
    pub vertex_offset: u32,
    pub vertex_count: u32,
    pub index_offset: u32,
    pub index_count: u32,
    pub generation: u32,
    pub mdi_index: u32,
}

#[derive(Debug)]
struct FreeBlock {
    offset: u32,
    size: u32,
}

/// CPU staging + bucket allocator (works without GPU device).
#[derive(Debug)]
pub struct PersistentVboPool {
    pub pool_bytes: usize,
    pub vertex_capacity: usize,
    pub index_capacity: usize,
    pub vertex_staging: Vec<u8>,
    pub index_staging: Vec<u8>,
    vertex_free: Vec<FreeBlock>,
    index_free: Vec<FreeBlock>,
    vertex_high_water: u32,
    index_high_water: u32,
    pub slots: HashMap<(i32, i32), PersistentSlot>,
    pub generation: u32,
    pub uploads: u64,
    pub reuses: u64,
    pub bucket_allocs: u64,
    pub mdi_commands: Vec<DrawIndexedIndirectArgs>,
}

impl PersistentVboPool {
    pub fn new(pool_mb: usize) -> Self {
        let pool_bytes = pool_mb * 1024 * 1024;
        let vertex_capacity = pool_bytes * 3 / 4 / VERTEX_STRIDE_BYTES;
        let index_capacity = pool_bytes / 4 / 4;
        info!(
            "[PersistentVbo] allocating {} MB ({} verts, {} indices)",
            pool_mb, vertex_capacity, index_capacity
        );
        Self {
            pool_bytes,
            vertex_capacity,
            index_capacity,
            vertex_staging: vec![0u8; vertex_capacity * VERTEX_STRIDE_BYTES],
            index_staging: vec![0u8; index_capacity * 4],
            vertex_free: Vec::new(),
            index_free: Vec::new(),
            vertex_high_water: 0,
            index_high_water: 0,
            slots: HashMap::new(),
            generation: 0,
            uploads: 0,
            reuses: 0,
            bucket_allocs: 0,
            mdi_commands: Vec::new(),
        }
    }

    pub fn adaptive() -> Self {
        let hw = rsift_api::AdaptivePerfEngine::hardware();
        let rp = rsift_api::AdaptivePerfEngine::render_profile(hw);
        let mb = match hw.tier {
            rsift_api::PerformanceTier::High => rp.persistent_vbo_mb.max(DEFAULT_POOL_MB_HIGH),
            rsift_api::PerformanceTier::Medium => rp.persistent_vbo_mb.max(DEFAULT_POOL_MB_MEDIUM),
            _ => DEFAULT_POOL_MB_LOW,
        };
        Self::new(mb)
    }

    fn alloc_from_free_list(
        free: &mut Vec<FreeBlock>,
        need: u32,
        high_water: &mut u32,
        cap: usize,
        bucket_hits: &mut u64,
    ) -> Option<u32> {
        if let Some(pos) = free.iter().position(|b| b.size >= need) {
            let block = free.remove(pos);
            if block.size > need {
                free.push(FreeBlock {
                    offset: block.offset + need,
                    size: block.size - need,
                });
            }
            *bucket_hits += 1;
            return Some(block.offset);
        }
        let off = *high_water;
        if off as usize + need as usize > cap {
            return None;
        }
        *high_water = off + need;
        Some(off)
    }

    fn return_to_free_list(free: &mut Vec<FreeBlock>, offset: u32, size: u32) {
        free.push(FreeBlock { offset, size });
        free.sort_by_key(|b| b.offset);
        let mut merged: Vec<FreeBlock> = Vec::new();
        for block in free.drain(..) {
            if let Some(last) = merged.last_mut() {
                if last.offset + last.size == block.offset {
                    last.size += block.size;
                    continue;
                }
            }
            merged.push(block);
        }
        *free = merged;
    }

    /// Write mesh into pool bucket; reuses freed space when chunk reloads.
    pub fn upload_mesh(&mut self, mesh: &BuiltChunkMesh) -> Option<PersistentSlot> {
        if mesh.is_empty {
            self.release(mesh.chunk_x, mesh.chunk_z);
            return None;
        }
        let vcount = mesh.vertices.len() as u32;
        let icount = mesh.indices.len() as u32;
        if vcount as usize > self.vertex_capacity || icount as usize > self.index_capacity {
            warn!("[PersistentVbo] chunk ({}, {}) too large", mesh.chunk_x, mesh.chunk_z);
            return None;
        }

        if let Some(old) = self.slots.remove(&(mesh.chunk_x, mesh.chunk_z)) {
            Self::return_to_free_list(&mut self.vertex_free, old.vertex_offset, old.vertex_count);
            Self::return_to_free_list(&mut self.index_free, old.index_offset, old.index_count);
            self.reuses += 1;
        }

        let voff = Self::alloc_from_free_list(
            &mut self.vertex_free,
            vcount,
            &mut self.vertex_high_water,
            self.vertex_capacity,
            &mut self.bucket_allocs,
        )?;
        let ioff = Self::alloc_from_free_list(
            &mut self.index_free,
            icount,
            &mut self.index_high_water,
            self.index_capacity,
            &mut self.bucket_allocs,
        )?;

        let vb = voff as usize * VERTEX_STRIDE_BYTES;
        let ib = ioff as usize * 4;
        let verts_bytes = bytemuck::cast_slice(&mesh.vertices);
        self.vertex_staging[vb..vb + verts_bytes.len()].copy_from_slice(verts_bytes);
        let idx_bytes = bytemuck::cast_slice(&mesh.indices);
        self.index_staging[ib..ib + idx_bytes.len()].copy_from_slice(idx_bytes);

        let mdi_index = self.mdi_commands.len() as u32;
        self.mdi_commands.push(DrawIndexedIndirectArgs {
            index_count: icount,
            instance_count: 1,
            first_index: ioff,
            base_vertex: voff as i32,
            first_instance: mdi_index,
        });

        let slot = PersistentSlot {
            vertex_offset: voff,
            vertex_count: vcount,
            index_offset: ioff,
            index_count: icount,
            generation: self.generation,
            mdi_index,
        };
        self.slots.insert((mesh.chunk_x, mesh.chunk_z), slot);
        self.uploads += 1;
        Some(slot)
    }

    pub fn release(&mut self, cx: i32, cz: i32) {
        if let Some(old) = self.slots.remove(&(cx, cz)) {
            Self::return_to_free_list(&mut self.vertex_free, old.vertex_offset, old.vertex_count);
            Self::return_to_free_list(&mut self.index_free, old.index_offset, old.index_count);
        }
    }

    /// Rebuild MDI command list from active slots (one indirect arg per chunk).
    pub fn rebuild_mdi(&mut self) {
        self.mdi_commands.clear();
        let mut keys: Vec<_> = self.slots.keys().copied().collect();
        keys.sort();
        for (cx, cz) in keys {
            let s = self.slots[&(cx, cz)];
            self.mdi_commands.push(DrawIndexedIndirectArgs {
                index_count: s.index_count,
                instance_count: 1,
                first_index: s.index_offset,
                base_vertex: s.vertex_offset as i32,
                first_instance: self.mdi_commands.len() as u32,
            });
        }
    }

    pub fn active_chunks(&self) -> usize {
        self.slots.len()
    }

    pub fn utilization(&self) -> f32 {
        let v_used = self.vertex_high_water as f32 / self.vertex_capacity as f32;
        let i_used = self.index_high_water as f32 / self.index_capacity as f32;
        v_used.max(i_used)
    }
}

/// GPU-side persistent buffers + MDI draw.
pub struct PersistentVboPoolGpu {
    pub cpu: PersistentVboPool,
    vertex_buffer: Option<wgpu::Buffer>,
    index_buffer: Option<wgpu::Buffer>,
    indirect_buffer: Option<wgpu::Buffer>,
    device: Option<Arc<wgpu::Device>>,
    queue: Option<Arc<wgpu::Queue>>,
    pub mapped_vertex: bool,
    pub draws_submitted: u64,
}

impl PersistentVboPoolGpu {
    pub fn new(cpu: PersistentVboPool) -> Self {
        Self {
            cpu,
            vertex_buffer: None,
            index_buffer: None,
            indirect_buffer: None,
            device: None,
            queue: None,
            mapped_vertex: false,
            draws_submitted: 0,
        }
    }

    pub fn adaptive() -> Self {
        Self::new(PersistentVboPool::adaptive())
    }

    pub fn ensure_gpu(
        &mut self,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
    ) {
        if self.vertex_buffer.is_some() {
            self.device = Some(device);
            self.queue = Some(queue);
            return;
        }
        let try_map = false; // wgpu 0.20: use write_buffer hot path
        self.mapped_vertex = try_map;

        let v_size = self.cpu.vertex_staging.len() as u64;
        let i_size = self.cpu.index_staging.len() as u64;
        let indirect_cap = 65536usize;
        let ind_size = (indirect_cap * std::mem::size_of::<DrawIndexedIndirectArgs>()) as u64;

        self.vertex_buffer = Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rsift Persistent VBO"),
            contents: &self.cpu.vertex_staging,
            usage: wgpu::BufferUsages::VERTEX
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::STORAGE,
        }));
        self.index_buffer = Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rsift Persistent IBO"),
            contents: &self.cpu.index_staging,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        }));
        self.indirect_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift MDI Indirect"),
            size: ind_size,
            usage: wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        }));

        info!(
            "[PersistentVbo] GPU buffers ready v={:.0}MB i={:.0}MB mapped={}",
            v_size as f64 / (1024.0 * 1024.0),
            i_size as f64 / (1024.0 * 1024.0),
            try_map
        );
        self.device = Some(device);
        self.queue = Some(queue);
    }

    /// Patch GPU buffer regions for newly uploaded chunk slots.
    pub fn flush_slot(&mut self, slot: &PersistentSlot, mesh: &BuiltChunkMesh) {
        let Some(queue) = self.queue.as_ref() else { return };
        let Some(vb) = self.vertex_buffer.as_ref() else { return };
        let Some(ib) = self.index_buffer.as_ref() else { return };

        let voff = slot.vertex_offset as usize * VERTEX_STRIDE_BYTES;
        queue.write_buffer(vb, voff as u64, bytemuck::cast_slice(&mesh.vertices));
        let ioff = slot.index_offset as usize * 4;
        queue.write_buffer(ib, ioff as u64, bytemuck::cast_slice(&mesh.indices));
    }

    pub fn flush_mdi(&mut self) {
        let Some(queue) = self.queue.as_ref() else { return };
        let Some(ib) = self.indirect_buffer.as_ref() else { return };
        self.cpu.rebuild_mdi();
        if !self.cpu.mdi_commands.is_empty() {
            queue.write_buffer(ib, 0, bytemuck::cast_slice(&self.cpu.mdi_commands));
        }
    }

    /// Single or multi `draw_indexed_indirect` for all visible chunks.
    pub fn draw_all_indirect<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        chunk_count: u32,
    ) -> bool {
        let Some(indirect) = self.indirect_buffer.as_ref() else {
            return false;
        };
        if chunk_count == 0 || self.cpu.mdi_commands.is_empty() {
            return false;
        }
        let count = chunk_count.min(self.cpu.mdi_commands.len() as u32);
        let stride = std::mem::size_of::<DrawIndexedIndirectArgs>() as u64;
        let multi = self
            .device
            .as_ref()
            .map(|d| d.features().contains(wgpu::Features::MULTI_DRAW_INDIRECT))
            .unwrap_or(false);
        if multi {
            pass.multi_draw_indexed_indirect(indirect, 0, count);
        } else {
            for i in 0..count {
                pass.draw_indexed_indirect(indirect, i as u64 * stride);
            }
        }
        true
    }

    pub fn vertex_buffer(&self) -> Option<&wgpu::Buffer> {
        self.vertex_buffer.as_ref()
    }

    pub fn index_buffer(&self) -> Option<&wgpu::Buffer> {
        self.index_buffer.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk_mesh::Quantized12ByteVertex;

    fn tiny_mesh(cx: i32, cz: i32, n: usize) -> BuiltChunkMesh {
        BuiltChunkMesh {
            chunk_x: cx,
            chunk_z: cz,
            vertices: vec![Quantized12ByteVertex::encode(0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0); n],
            indices: (0..(n as u32)).collect(),
            is_empty: false,
        }
    }

    #[test]
    fn bucket_reuse_freed_space() {
        let mut pool = PersistentVboPool::new(4);
        let m1 = tiny_mesh(0, 0, 100);
        let s1 = pool.upload_mesh(&m1).unwrap();
        pool.release(0, 0);
        let m2 = tiny_mesh(1, 1, 100);
        let s2 = pool.upload_mesh(&m2).unwrap();
        assert_eq!(s2.vertex_offset, s1.vertex_offset);
        assert!(pool.bucket_allocs >= 2);
    }

    #[test]
    fn mdi_one_command_per_chunk() {
        let mut pool = PersistentVboPool::new(8);
        pool.upload_mesh(&tiny_mesh(0, 0, 50)).unwrap();
        pool.upload_mesh(&tiny_mesh(1, 0, 50)).unwrap();
        pool.rebuild_mdi();
        assert_eq!(pool.mdi_commands.len(), 2);
    }
}

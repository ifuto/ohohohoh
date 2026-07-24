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
    /// upload 成功回数 (telemetry)。
    pub uploads: u64,
    /// refresh (既存 slot の解放を伴う再 upload) 回数 (telemetry)。
    pub reuses: u64,
    /// free-list 経由で確保できたブロック数 (vertex/index それぞれ最大
    /// +1/upload。high-water からの新規確保は計上しない)。
    pub bucket_allocs: u64,
    /// 容量超過 (oversize) で拒否した upload 回数 (telemetry)。
    /// 空メッシュ除去・確保失敗 (frag 枯渇) は計上しない。
    pub oversize_rejects: u64,
    pub mdi_commands: Vec<DrawIndexedIndirectArgs>,
}

impl PersistentVboPool {
    pub fn new(pool_mb: usize) -> Self {
        let pool_bytes = pool_mb * 1024 * 1024;
        // 容量配分 (wave 76 BZ-6): 主 workload は all-quad (1 面 = 4v/6i、
        // 要素比 v:i = 1:1.5)。両側が同時に枯渇する数学的最適は bytes 比
        // v:i = (4×12):(6×4) = 48:24 = 2:1。旧 3:1 配分は要素数 v==i
        // (確かに =pool_bytes/16) で、all-quad では index 側が 2/3 充填で
        // 先に尽き vertex 側の 1/3 が恒常的に死蔵していた。
        let vertex_capacity = pool_bytes * 2 / 3 / VERTEX_STRIDE_BYTES;
        let index_capacity = pool_bytes / 3 / 4;
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
            oversize_rejects: 0,
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

    /// 確保の巻き戻し (wave 76 BZ-1): high-water 先端からの確保なら
    /// high-water を巻き戻し、free-list 由来なら free list へ返却する
    /// (返却は sort+merge で消費時の remainder と再統合され、元ブロックに
    /// 厳密復元される — 片側 `?` だと所有者の居ない領域が永久リークする)。
    fn rollback_alloc(free: &mut Vec<FreeBlock>, high_water: &mut u32, offset: u32, size: u32) {
        if offset + size == *high_water {
            *high_water = offset;
        } else {
            Self::return_to_free_list(free, offset, size);
        }
    }

    /// Write mesh into pool bucket; reuses freed space when chunk reloads.
    ///
    /// # 契約 (機械ピン, wave 76)
    /// - 戻り値が `None` のとき (chunk_x, chunk_z) の slot は**存在しない**
    ///   (空メッシュ除去・oversize 拒否・確保失敗の全経路で一貫 — BZ-2)。
    ///   旧実装は oversize 拒否時に旧 slot を残存させ、rebuild_mdi 経由で
    ///   stale geometry が描画され続ける経路があった。
    /// - `mdi_commands` は本関数では append のみ (旧 chunk の命令は除去
    ///   しない)。`release` も同様。GPU 消費前に `rebuild_mdi` で真値へ
    ///   再配置すること (live 実施: render_pipeline:855)。
    pub fn upload_mesh(&mut self, mesh: &BuiltChunkMesh) -> Option<PersistentSlot> {
        if mesh.is_empty() {
            self.release(mesh.chunk_x, mesh.chunk_z);
            return None;
        }
        let vcount = mesh.vertices.len() as u32;
        let icount = mesh.indices.len() as u32;
        if vcount as usize > self.vertex_capacity || icount as usize > self.index_capacity {
            self.release(mesh.chunk_x, mesh.chunk_z);
            self.oversize_rejects += 1;
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
        let ioff = match Self::alloc_from_free_list(
            &mut self.index_free,
            icount,
            &mut self.index_high_water,
            self.index_capacity,
            &mut self.bucket_allocs,
        ) {
            Some(ioff) => ioff,
            None => {
                // BZ-1: vertex 側の確保を巻き戻す (旧実装は `?` で早期 return
                // し、voff 領域が所有者不在のまま永久リークしていた)。
                Self::rollback_alloc(
                    &mut self.vertex_free,
                    &mut self.vertex_high_water,
                    voff,
                    vcount,
                );
                return None;
            }
        };

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
    ///
    /// 契約 (wave 76 BZ-5): keys sort により決定的。各 slot の `mdi_index`
    /// は本関数で再配置された**真値**に書き戻される (upload 時の append
    /// 位置は rebuild 後に stale 化するため、旧実装の読み置きは嘘だった)。
    pub fn rebuild_mdi(&mut self) {
        self.mdi_commands.clear();
        let mut keys: Vec<_> = self.slots.keys().copied().collect();
        keys.sort();
        for (cx, cz) in keys {
            let mdi_index = self.mdi_commands.len() as u32;
            let s = self.slots[&(cx, cz)];
            self.mdi_commands.push(DrawIndexedIndirectArgs {
                index_count: s.index_count,
                instance_count: 1,
                first_index: s.index_offset,
                base_vertex: s.vertex_offset as i32,
                first_instance: mdi_index,
            });
            self.slots
                .insert((cx, cz), PersistentSlot { mdi_index, ..s });
        }
    }

    pub fn active_chunks(&self) -> usize {
        self.slots.len()
    }

    /// プール占有率 (0.0-1.0): v/i high-water の大きい方。
    /// freed 領域は差し引かない watermark 方式 (「過去最大の占有」を見る
    /// 指標で、瞬時の空き率ではない)。容量 0 (縮退入力) では 0/0=NaN を
    /// 生むため 0.0 に倒す (wave 76 BZ-3: NaN=観測欠測は安全側に倒す)。
    pub fn utilization(&self) -> f32 {
        if self.vertex_capacity == 0 || self.index_capacity == 0 {
            return 0.0;
        }
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
        }
    }

    /// v/i を独立に指定するヘルパ (index 先行枯渇・oversize シナリオ用)。
    fn manual_mesh(cx: i32, cz: i32, v: usize, i: usize) -> BuiltChunkMesh {
        BuiltChunkMesh {
            chunk_x: cx,
            chunk_z: cz,
            vertices: vec![
                Quantized12ByteVertex::encode(0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0);
                v
            ],
            indices: (0..(i as u32)).collect(),
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

    #[test]
    fn index_alloc_failure_rolls_back_high_water_vertex_region() {
        // BZ-1 シナリオ A (high-water 巻き戻し経路、全手導出):
        // new(1): v_cap=58254, i_cap=87381 (2:1 配分の厳密値)。
        let mut pool = PersistentVboPool::new(1);
        let m1 = manual_mesh(0, 0, 4, 87381); // index を丁度満杯にする
        let s1 = pool.upload_mesh(&m1).unwrap();
        assert_eq!((s1.vertex_offset, s1.index_offset), (0, 0));
        assert_eq!((pool.vertex_high_water, pool.index_high_water), (4, 87381));
        // vertex は確保可能 (4→8) だが index は 87381+6 > 87381 で失敗
        assert!(pool.upload_mesh(&manual_mesh(1, 1, 4, 6)).is_none());
        // 修復後: vertex 確保は巻き戻され 4 に戻る (旧実装は 8 のままリーク)
        assert_eq!((pool.vertex_high_water, pool.index_high_water), (4, 87381));
        assert!(pool.vertex_free.is_empty()); // hw rewind は free-list 化しない
        assert_eq!(pool.uploads, 1);
        assert_eq!(pool.active_chunks(), 1); // m1 は生存
    }

    #[test]
    fn index_alloc_failure_restores_free_list_block_exactly() {
        // BZ-1 シナリオ B (free-list 復元経路、全手導出):
        let mut pool = PersistentVboPool::new(1); // v_cap 58254, i_cap 87381
        pool.upload_mesh(&manual_mesh(0, 0, 8, 8)).unwrap(); // v[0,8)  i[0,8)
        pool.upload_mesh(&manual_mesh(1, 1, 8, 8)).unwrap(); // v[8,16) i[8,16)
        pool.upload_mesh(&manual_mesh(2, 2, 1, 87365)).unwrap(); // i[16,87381) 丁度
        assert_eq!((pool.vertex_high_water, pool.index_high_water), (17, 87381));
        pool.release(0, 0); // v_free=[(0,8)], i_free=[(0,8)]

        // (3,3): vertex は hw 経路 (17→27)、index 失敗 → 巻き戻し 17
        assert!(pool.upload_mesh(&manual_mesh(3, 3, 10, 10)).is_none());
        assert_eq!(pool.vertex_high_water, 17);
        assert_eq!(pool.vertex_free.len(), 1);
        assert_eq!(
            (pool.vertex_free[0].offset, pool.vertex_free[0].size),
            (0, 8)
        );
        // (4,4): vertex は free-list 経路で (0,8) 消費 → 残余 (4,4)、
        // index 失敗 → (0,4) 返却が (4,4) と merge し (0,8) に厳密復元
        assert!(pool.upload_mesh(&manual_mesh(4, 4, 4, 10)).is_none());
        assert_eq!(pool.vertex_high_water, 17);
        assert_eq!(pool.vertex_free.len(), 1);
        assert_eq!(
            (pool.vertex_free[0].offset, pool.vertex_free[0].size),
            (0, 8)
        );
        assert_eq!(pool.uploads, 3);
        assert_eq!(pool.active_chunks(), 2); // (1,1),(2,2) 生存
    }

    #[test]
    fn oversize_reject_evicts_slot_and_none_means_absent() {
        // BZ-2 (BY-1 同型): oversize 拒否で「None ⇒ slot 無し」を一貫化。
        let mut pool = PersistentVboPool::new(1); // v_cap 58254
        let s = pool.upload_mesh(&manual_mesh(0, 0, 4, 6)).unwrap();
        assert_eq!((s.vertex_offset, s.index_offset), (0, 0));
        // 同 chunk に v=58255 (>58254) の refresh: 拒否 + 旧 slot 解放
        assert!(pool.upload_mesh(&manual_mesh(0, 0, 58255, 6)).is_none());
        assert_eq!(pool.active_chunks(), 0);
        assert_eq!(pool.oversize_rejects, 1);
        assert_eq!(pool.vertex_high_water, 4); // 拒否は確保前なので不動
        assert_eq!(pool.vertex_free.len(), 1); // 旧 slot の領域は解放済み
        assert_eq!(pool.index_free.len(), 1);
        // slot 非保有の chunk への oversize も同契約 (計数のみ)
        assert!(pool.upload_mesh(&manual_mesh(9, 9, 58255, 6)).is_none());
        assert_eq!(pool.oversize_rejects, 2);
    }

    #[test]
    fn rebuild_mdi_updates_slots_and_content_exact() {
        // BZ-5: rebuild が slot.mdi_index を sort 位置の真値へ書き戻す。
        let mut pool = PersistentVboPool::new(1);
        // (2,0) を先に upload → append 順と sort 順を意図的にずらす
        let a = pool.upload_mesh(&manual_mesh(2, 0, 8, 12)).unwrap(); // v[0,8) i[0,12)
        let b = pool.upload_mesh(&manual_mesh(0, 0, 4, 6)).unwrap(); // v[8,12) i[12,18)
        assert_eq!((a.mdi_index, b.mdi_index), (0, 1)); // upload append 順
        pool.rebuild_mdi();
        assert_eq!(pool.mdi_commands.len(), 2);
        // sort 順: (0,0) → (2,0)。slot.mdi_index が真値に更新
        assert_eq!(
            (pool.slots[&(0, 0)].mdi_index, pool.slots[&(2, 0)].mdi_index),
            (0, 1)
        );
        let c0 = &pool.mdi_commands[0];
        assert_eq!(
            (
                c0.index_count,
                c0.instance_count,
                c0.first_index,
                c0.base_vertex,
                c0.first_instance
            ),
            (6, 1, 12, 8, 0)
        );
        let c1 = &pool.mdi_commands[1];
        assert_eq!(
            (
                c1.index_count,
                c1.instance_count,
                c1.first_index,
                c1.base_vertex,
                c1.first_instance
            ),
            (12, 1, 0, 0, 1)
        );
        // 再 rebuild は冪等 (内容・mdi_index 不変)
        pool.rebuild_mdi();
        assert_eq!(pool.mdi_commands.len(), 2);
        assert_eq!(
            (pool.slots[&(0, 0)].mdi_index, pool.slots[&(2, 0)].mdi_index),
            (0, 1)
        );
    }

    #[test]
    fn capacity_rebalance_utilization_and_zero_capacity_exact() {
        // BZ-6: 2:1 bytes 配分の厳密値 (1 MiB = 1048576 B 手導出):
        // v_cap = 1048576×2/3 (→699050) /12 (→58254) / i_cap = 1048576/3
        // (→349525) /4 (→87381)。staging バイト長も従う。
        let pool = PersistentVboPool::new(1);
        assert_eq!(pool.vertex_capacity, 58254);
        assert_eq!(pool.index_capacity, 87381);
        assert_eq!(pool.vertex_staging.len(), 699048); // 58254×12
        assert_eq!(pool.index_staging.len(), 349524); // 87381×4

        // utilization: v 半使用で f32 厳密 0.5 (29127/58254 は両側 f32 厳密
        // 表現可能な 2 の冪商)。i 側 43690/87381 < 0.5 なので max は v。
        let mut pool = PersistentVboPool::new(1);
        pool.upload_mesh(&manual_mesh(0, 0, 29127, 43690)).unwrap();
        assert_eq!(pool.vertex_high_water, 29127);
        assert_eq!(pool.utilization(), 0.5);
        // BZ-3: 容量 0 は 0.0 (NaN でなく安全側)。upload は oversize 拒否。
        let mut pool = PersistentVboPool::new(0);
        assert_eq!(pool.utilization(), 0.0);
        assert!(pool.upload_mesh(&manual_mesh(0, 0, 1, 1)).is_none());
        assert_eq!(pool.oversize_rejects, 1);
    }
}

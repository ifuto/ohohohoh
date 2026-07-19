//! # 21. Gigabuffer / Mega Buffer (`GigaBufferSuballocator`)
//!
//! エンジン起動時に巨大単一バッファ（例: 400 MB）を一括確保し、全チャンクのメッシュデータを
//! サブアロケーションとして内部プールから切り出す。バッファ生成/破棄 API コールを回避。

use crate::gpu_arena::{ArenaHandle, GpuArena};
use std::sync::Mutex;

pub struct GigaBufferSuballocator {
    pub arena: Mutex<GpuArena>,
    pub total_capacity_bytes: u64,
}

impl GigaBufferSuballocator {
    pub fn new(capacity_mb: u64) -> Self {
        let cap = capacity_mb.saturating_mul(1024 * 1024);
        Self {
            arena: Mutex::new(GpuArena::new(cap, 256)), // 256B alignment for SSBO/VBO
            total_capacity_bytes: cap,
        }
    }

    pub fn allocate_mesh_slice(&self, size_bytes: u64) -> Option<ArenaHandle> {
        if let Ok(mut guard) = self.arena.lock() {
            guard.alloc(size_bytes)
        } else {
            None
        }
    }

    pub fn free_mesh_slice(&self, handle: ArenaHandle) {
        if let Ok(mut guard) = self.arena.lock() {
            guard.free(handle);
        }
    }

    pub fn used_bytes(&self) -> u64 {
        self.arena.lock().map(|a| a.used_bytes()).unwrap_or(0)
    }

    pub fn free_bytes(&self) -> u64 {
        self.arena.lock().map(|a| a.free_bytes()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gigabuffer_suballocator() {
        let gb = GigaBufferSuballocator::new(100);
        let h1 = gb.allocate_mesh_slice(4096).unwrap();
        let h2 = gb.allocate_mesh_slice(8192).unwrap();
        assert!(h2.offset >= h1.offset + h1.size);
        gb.free_mesh_slice(h1);
        assert!(gb.free_bytes() > 0);
    }
}

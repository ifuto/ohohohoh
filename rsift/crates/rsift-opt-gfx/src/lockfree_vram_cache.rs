//! # 22. Lock-Free VRAM Mesh Cache (`LockFreeVramMeshCache`)
//!
//! ロックフリー FIFO ページ置換方式による VRAM メッシュキャッシュ。
//! マルチスレッドでの並行チャンクメッシュ生成とロックフリーアップロードを実現する。

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VramCacheHandle {
    pub slot_idx: u32,
    pub chunk_key: (i32, i32),
    pub generation: u32,
}

pub struct LockFreeVramMeshCache {
    pub max_slots: usize,
    chunk_keys: Vec<AtomicU64>, // packed (x, z)
    generations: Vec<AtomicU32>,
    head: AtomicUsize,
    pub hits: AtomicU64,
    pub misses: AtomicU64,
}

impl LockFreeVramMeshCache {
    pub fn new(max_slots: usize) -> Self {
        let mut keys = Vec::with_capacity(max_slots);
        let mut gens = Vec::with_capacity(max_slots);
        for _ in 0..max_slots {
            keys.push(AtomicU64::new(u64::MAX));
            gens.push(AtomicU32::new(0));
        }
        Self {
            max_slots,
            chunk_keys: keys,
            generations: gens,
            head: AtomicUsize::new(0),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    fn pack_key(cx: i32, cz: i32) -> u64 {
        ((cx as u32) as u64) | (((cz as u32) as u64) << 32)
    }

    pub fn lookup_or_insert(&self, cx: i32, cz: i32) -> (VramCacheHandle, bool) {
        let key = Self::pack_key(cx, cz);
        for i in 0..self.max_slots {
            if self.chunk_keys[i].load(Ordering::Acquire) == key {
                self.hits.fetch_add(1, Ordering::Relaxed);
                let gen = self.generations[i].load(Ordering::Acquire);
                return (
                    VramCacheHandle {
                        slot_idx: i as u32,
                        chunk_key: (cx, cz),
                        generation: gen,
                    },
                    true,
                );
            }
        }

        // Miss: evict oldest slot lock-free via atomic head increment
        self.misses.fetch_add(1, Ordering::Relaxed);
        let slot = self.head.fetch_add(1, Ordering::Relaxed) % self.max_slots;
        self.chunk_keys[slot].store(key, Ordering::Release);
        let new_gen = self.generations[slot].fetch_add(1, Ordering::Relaxed) + 1;

        (
            VramCacheHandle {
                slot_idx: slot as u32,
                chunk_key: (cx, cz),
                generation: new_gen,
            },
            false,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lockfree_vram_mesh_cache() {
        let cache = LockFreeVramMeshCache::new(4);
        let (h1, hit1) = cache.lookup_or_insert(10, 20);
        assert!(!hit1);
        let (h2, hit2) = cache.lookup_or_insert(10, 20);
        assert!(hit2);
        assert_eq!(h1.slot_idx, h2.slot_idx);
    }
}

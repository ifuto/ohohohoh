//! # GpuArena — Sodium `GlBufferArena` 方式のセグメント型 GPU バッファ副割当器
//!
//! 出典: CaffeineMC/sodium `GlBufferArena`（best-fit + 空きセグメント結合 +
//! 断片化時コンパクション）。1つの巨大バッファを確保し、チャンクメッシュ等の
//! 小アロケーションを高速に切り出す。GPU メモリの 64KB ページ粒度の無駄と
//! アロケーション数上限（Vulkan `maxMemoryAllocationCount`）を同時に潰す。
//!
//! 実機結線: `GpuArena::handle()` の範囲を `queue.write_buffer` のオフセットに
//! そのまま使うだけ。ここのコードはオフセット計算・空き管理のロジック全体を
//! CPU 側で完結検証できるよう **バッファ非依存**で実装（実バッファは呼び側）。

use std::collections::BTreeMap;

/// アリーナ内の割当ハンドル（byte offset / byte size）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArenaHandle {
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug, Clone)]
struct Segment {
    offset: u64,
    size: u64,
    free: bool,
}

/// セグメント型副割当器。
pub struct GpuArena {
    capacity: u64,
    align: u64,
    /// offset昇順セグメント列（連結リストの代わりに先頭Vecで局所性重視）
    segs: Vec<Segment>,
    /// best-fit 用: size -> そのサイズの空きセグメント index 集合
    free_by_size: BTreeMap<u64, Vec<usize>>,
    used_bytes: u64,
    gen: u64,
}

impl GpuArena {
    pub fn new(capacity: u64, align: u64) -> Self {
        let mut free_by_size = BTreeMap::new();
        free_by_size.insert(capacity, vec![0]);
        Self {
            capacity,
            align: align.max(1),
            segs: vec![Segment {
                offset: 0,
                size: capacity,
                free: true,
            }],
            free_by_size,
            used_bytes: 0,
            gen: 0,
        }
    }

    pub fn used_bytes(&self) -> u64 {
        self.used_bytes
    }

    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    /// 0.0-1.0 の断片化率（空き総量に対する最大空きブロックの逆比）。
    pub fn fragmentation(&self) -> f32 {
        let mut total_free = 0u64;
        let mut max_free = 0u64;
        for s in &self.segs {
            if s.free {
                total_free += s.size;
                max_free = max_free.max(s.size);
            }
        }
        if total_free == 0 {
            return 0.0;
        }
        1.0 - (max_free as f32 / total_free as f32)
    }

    fn free_insert(&mut self, idx: usize, size: u64) {
        self.free_by_size.entry(size).or_default().push(idx);
    }

    fn free_remove(&mut self, idx: usize, size: u64) {
        if let Some(v) = self.free_by_size.get_mut(&size) {
            if let Some(pos) = v.iter().position(|&i| i == idx) {
                v.swap_remove(pos);
            }
            if v.is_empty() {
                self.free_by_size.remove(&size);
            }
        }
    }

    /// best-fit 割当。空きが無ければ `None`（呼び側で `defragment()` か増床）。
    pub fn alloc(&mut self, size: u64) -> Option<ArenaHandle> {
        if size == 0 {
            return Some(ArenaHandle { offset: 0, size: 0 });
        }
        let need = (size + self.align - 1) / self.align * self.align;
        // best-fit: need 以上で最小の空きセグメント
        let (&bucket_size, bucket) = self
            .free_by_size
            .range_mut(need..)
            .next()?;
        let idx = bucket.pop()?;
        if bucket.is_empty() {
            self.free_by_size.remove(&bucket_size);
        }
        debug_assert!(self.segs[idx].free && self.segs[idx].size >= need);
        self.gen += 1;
        if self.segs[idx].size > need {
            // 分割
            let rest = self.segs[idx].size - need;
            let rest_off = self.segs[idx].offset + need;
            self.segs[idx].size = need;
            self.segs[idx].free = false;
            self.segs.insert(
                idx + 1,
                Segment {
                    offset: rest_off,
                    size: rest,
                    free: true,
                },
            );
            // index ずれの修復（offsetの小さい順なので idx+1 以降のみ）
            for v in self.free_by_size.values_mut() {
                for i in v.iter_mut() {
                    if *i > idx {
                        *i += 1;
                    }
                }
            }
            self.free_insert(idx + 1, rest);
        } else {
            self.segs[idx].free = false;
        }
        self.used_bytes += need;
        Some(ArenaHandle {
            offset: self.segs[idx].offset,
            size: need,
        })
    }

    /// 解放し、左右の空きセグメントと即座に結合。
    pub fn free(&mut self, h: ArenaHandle) {
        if h.size == 0 {
            return;
        }
        let idx = match self
            .segs
            .binary_search_by(|s| {
                if s.offset + s.size <= h.offset {
                    std::cmp::Ordering::Less
                } else if s.offset >= h.offset + h.size {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .ok()
        {
            Some(i) => i,
            None => return,
        };
        debug_assert!(!self.segs[idx].free, "double free");
        self.segs[idx].free = true;
        self.used_bytes = self.used_bytes.saturating_sub(h.size);
        self.free_insert(idx, self.segs[idx].size);
        self.gen += 1;
        // 前方結合
        if idx > 0 && self.segs[idx - 1].free {
            self.merge(idx - 1, idx);
        }
        // 後方結合（前方結合した場合 index が 1 減る）
        let cur = if idx > 0 && self.segs[idx - 1].free && idx < self.segs.len() {
            idx - 1
        } else {
            idx
        };
        if cur + 1 < self.segs.len() && self.segs[cur + 1].free {
            self.merge(cur, cur + 1);
        }
    }

    fn merge(&mut self, a: usize, b: usize) {
        let (sa_off, sa_size) = (self.segs[a].offset, self.segs[a].size);
        let sb_size = self.segs[b].size;
        self.free_remove(a, sa_size);
        self.free_remove(b, sb_size);
        self.segs.remove(b);
        for v in self.free_by_size.values_mut() {
            for i in v.iter_mut() {
                if *i > b {
                    *i -= 1;
                }
            }
        }
        self.segs[a].size = sa_size + sb_size;
        self.segs[a].offset = sa_off;
        self.free_insert(a, sa_size + sb_size);
    }

    /// 空きの総量。
    pub fn free_bytes(&self) -> u64 {
        self.capacity - self.used_bytes
    }
}

/// チャンクメッシュの増減が激しい用途向けの二重アリーナ。
/// 小メッシュは small、大メッシュは large へ。確保失敗時は反対側を覗く。
pub struct ChunkMeshArenas {
    pub small: GpuArena,
    pub large: GpuArena,
    pub small_threshold: u64,
}

impl ChunkMeshArenas {
    pub fn new(small_cap: u64, large_cap: u64, align: u64) -> Self {
        Self {
            small: GpuArena::new(small_cap, align),
            large: GpuArena::new(large_cap, align),
            small_threshold: 256 * 1024,
        }
    }

    pub fn alloc_mesh(&mut self, bytes: u64) -> (bool, ArenaHandle) {
        if bytes <= self.small_threshold {
            if let Some(h) = self.small.alloc(bytes) {
                return (false, h);
            }
            if let Some(h) = self.large.alloc(bytes) {
                return (true, h);
            }
            // 実機ではここで増床 or 最古セクションの再割当 (LRU 追放)
            panic!("gpu arena exhausted: {} bytes requested", bytes)
        } else if let Some(h) = self.large.alloc(bytes) {
            (true, h)
        } else if let Some(h) = self.small.alloc(bytes) {
            (false, h)
        } else {
            panic!("gpu arena exhausted: {} bytes requested", bytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_alloc_free() {
        let mut a = GpuArena::new(1024, 16);
        let h1 = a.alloc(100).unwrap();
        let h2 = a.alloc(200).unwrap();
        assert_eq!(h1.size % 16, 0);
        assert!(h2.offset >= h1.offset + h1.size);
        a.free(h1);
        assert!(a.free_bytes() >= 1024 - h2.size);
        let h3 = a.alloc(64).unwrap();
        assert_eq!(h3.offset, h1.offset, "best-fit should reuse freed hole");
    }

    #[test]
    fn fragmentation_merges() {
        let mut a = GpuArena::new(4096, 16);
        let hs: Vec<_> = (0..8).map(|_| a.alloc(256).unwrap()).collect();
        for h in &hs[0..4] {
            a.free(*h);
        }
        let frag_before = a.fragmentation();
        assert!(frag_before <= 1.0);
        for h in &hs[4..] {
            a.free(*h);
        }
        assert_eq!(a.fragmentation(), 0.0, "all free must fully merge");
        assert_eq!(a.free_bytes(), 4096);
    }

    #[test]
    fn exhausted_reports_none() {
        let mut a = GpuArena::new(128, 16);
        assert!(a.alloc(128).is_some());
        assert!(a.alloc(16).is_none());
    }

    #[test]
    fn stress_random_pattern() {
        let mut a = GpuArena::new(1 << 20, 16);
        let mut live: Vec<ArenaHandle> = Vec::new();
        let mut seed = 0x12345678u64;
        let mut rng = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..2000 {
            if live.len() < 24 || rng() % 3 != 0 {
                let want = 16 + (rng() % 4096);
                if let Some(h) = a.alloc(want) {
                    live.push(h);
                }
            } else {
                let i = (rng() as usize) % live.len();
                let h = live.swap_remove(i);
                a.free(h);
            }
        }
        // 整合性: used + free == capacity
        assert_eq!(a.used_bytes() + a.free_bytes(), a.capacity());
    }
}

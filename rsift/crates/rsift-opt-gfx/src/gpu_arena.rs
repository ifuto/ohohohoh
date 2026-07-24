//! # GpuArena & Off-Heap Lock-Free Ring Buffer IPC
//!
//! 1) `GpuArena`: Sodium `GlBufferArena` 方式の best-fit + コンパクション型 GPU バッファ副割当器。
//! 2) `SharedRingBuffer`: Cache-Line アライン (`align(128)`) されたロックフリー SPSC リングバッファ IPC。
//! 3) `HazardQueue`: エポックベースの非同期メモリ回収/バッファ解放待機キュー。
//!
//! JNI/オフヒープメモリと GPU 転送スレッド間のゼロコピー通信において Mutex および
//! Java GC スパイク (Stop-The-World) を完全に排除。

use std::cell::UnsafeCell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, AtomicU64, Ordering};

/// Cache-Line Aligned (128 bytes for x86_64 and Apple Silicon) Lock-Free SPSC Ring Buffer.
#[repr(C, align(128))]
pub struct SharedRingBuffer<T: Copy, const BUFFER_SIZE: usize> {
    _pad0: [u8; 128],
    pub head: AtomicUsize,
    _pad1: [u8; 128 - std::mem::size_of::<AtomicUsize>()],
    pub tail: AtomicUsize,
    _pad2: [u8; 128 - std::mem::size_of::<AtomicUsize>()],
    pub data: [UnsafeCell<T>; BUFFER_SIZE],
}

unsafe impl<T: Copy + Send, const BUFFER_SIZE: usize> Sync for SharedRingBuffer<T, BUFFER_SIZE> {}
unsafe impl<T: Copy + Send, const BUFFER_SIZE: usize> Send for SharedRingBuffer<T, BUFFER_SIZE> {}

impl<T: Copy + Default, const BUFFER_SIZE: usize> SharedRingBuffer<T, BUFFER_SIZE> {
    pub fn new() -> Self {
        assert!(BUFFER_SIZE.is_power_of_two(), "BUFFER_SIZE must be a power of two");
        Self {
            _pad0: [0; 128],
            head: AtomicUsize::new(0),
            _pad1: [0; 128 - std::mem::size_of::<AtomicUsize>()],
            tail: AtomicUsize::new(0),
            _pad2: [0; 128 - std::mem::size_of::<AtomicUsize>()],
            data: std::array::from_fn(|_| UnsafeCell::new(T::default())),
        }
    }

    #[inline]
    pub fn try_push(&self, item: T) -> Result<(), T> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) >= BUFFER_SIZE {
            return Err(item);
        }
        let slot = head & (BUFFER_SIZE - 1);
        unsafe {
            *self.data[slot].get() = item;
        }
        self.head.store(head.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    #[inline]
    pub fn try_pop(&self) -> Option<T> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail == head {
            return None;
        }
        let slot = tail & (BUFFER_SIZE - 1);
        let item = unsafe { *self.data[slot].get() };
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(item)
    }

    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        head.wrapping_sub(tail)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Epoch-based deferred reclamation queue (`HazardQueue`) for pending buffer handles.
pub struct HazardQueue {
    pending: Vec<(u64, ArenaHandle)>,
    current_epoch: AtomicU64,
}

impl HazardQueue {
    pub fn new() -> Self {
        Self {
            pending: Vec::with_capacity(256),
            current_epoch: AtomicU64::new(1),
        }
    }

    pub fn advance_epoch(&self) -> u64 {
        self.current_epoch.fetch_add(1, Ordering::SeqCst)
    }

    pub fn current_epoch(&self) -> u64 {
        self.current_epoch.load(Ordering::SeqCst)
    }

    pub fn defer_free(&mut self, handle: ArenaHandle, retire_epoch: u64) {
        self.pending.push((retire_epoch, handle));
    }

    pub fn reclaim(&mut self, completed_epoch: u64, arena: &mut GpuArena) -> usize {
        let mut reclaimed = 0;
        let mut i = 0;
        while i < self.pending.len() {
            if self.pending[i].0 <= completed_epoch {
                let (_, handle) = self.pending.swap_remove(i);
                arena.free(handle);
                reclaimed += 1;
            } else {
                i += 1;
            }
        }
        reclaimed
    }
}

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
    segs: Vec<Segment>,
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

    /// 世代カウンタ (alloc/free の構造化変更ごとに +1)。
    /// 外部キャッシュの無効化トリガ等の消費者に公開する
    /// (消費者配線方針: 死に状態として私有で抱え込まない、wave 94 CR-B)。
    pub fn generation(&self) -> u64 {
        self.gen
    }

    pub fn capacity(&self) -> u64 {
        self.capacity
    }

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

    pub fn alloc(&mut self, size: u64) -> Option<ArenaHandle> {
        if size == 0 {
            return Some(ArenaHandle { offset: 0, size: 0 });
        }
        // アライン繰上げのオーバーフローは確保不能 (None) — 生の加算だと
        // u64 端でラップして小さい need に化け、実在セグメントを
        // 誤認割当する (wave 94 CR-A: 境界機械ピンで検出)。
        let need = match size.checked_add(self.align - 1) {
            Some(s) => s / self.align * self.align,
            None => return None,
        };
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
        debug_assert!(
            self.segs[idx].size == h.size,
            "free size mismatch: handle={} seg={} (used_bytes 会計が破壊される呼び出しバグ)",
            h.size,
            self.segs[idx].size
        );
        self.segs[idx].free = true;
        self.used_bytes = self.used_bytes.saturating_sub(h.size);
        self.free_insert(idx, self.segs[idx].size);
        self.gen += 1;
        if idx > 0 && self.segs[idx - 1].free {
            self.merge(idx - 1, idx);
        }
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

    pub fn free_bytes(&self) -> u64 {
        self.capacity - self.used_bytes
    }
}

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
    fn test_ring_buffer_ipc() {
        let rb: SharedRingBuffer<u64, 16> = SharedRingBuffer::new();
        assert!(rb.try_push(12345).is_ok());
        assert!(rb.try_push(67890).is_ok());
        assert_eq!(rb.try_pop(), Some(12345));
        assert_eq!(rb.try_pop(), Some(67890));
        assert_eq!(rb.try_pop(), None);
    }

    #[test]
    fn test_hazard_queue() {
        let mut hq = HazardQueue::new();
        let mut arena = GpuArena::new(1024, 16);
        let handle = arena.alloc(128).unwrap();
        let epoch = hq.advance_epoch();
        hq.defer_free(handle, epoch);
        assert_eq!(arena.used_bytes(), 128);
        assert_eq!(hq.reclaim(epoch - 1, &mut arena), 0);
        assert_eq!(hq.reclaim(epoch, &mut arena), 1);
        assert_eq!(arena.used_bytes(), 0);
    }

    /// wave 94: アライン繰上げの u64 オーバーフローは確保不能 (None) — 旧実装は
    /// ラップして小さい need に化け実在セグメントを誤認割当した (CR-A)。
    /// 契約境界 (capacity 丁度成功 / +1 拒否) も併せて機械固定。
    #[test]
    fn alloc_overflow_and_capacity_bounds() {
        let mut a = GpuArena::new(1024, 16);
        assert!(a.alloc(u64::MAX).is_none(), "no wrap to small need");
        assert!(a.alloc(u64::MAX - 3).is_none(), "checked_add guard");
        assert!(a.alloc(0).is_some(), "0 割当は従前通り受理 (退化契約)");
        let mut b = GpuArena::new(1024, 16);
        assert!(
            b.alloc(1024)
                .is_some_and(|h| h.offset == 0 && h.size == 1024),
            "capacity 丁度は成功"
        );
        assert!(b.alloc(1).is_none(), "exhausted");
        let mut c = GpuArena::new(1024, 16);
        assert!(c.alloc(1025).is_none(), "need 1040 > capacity");
    }

    /// wave 94: best-fit + 分割 + 併合の正準レイアウトを Python 機械検算値で
    /// 厳密ピン (need 100→112/200→208/64→64/48→48、free 後 used=320/free=704、
    /// 全解放後は単一 1024B セグメントへ完全併合)。世代カウンタ経由の
    /// generation() 消費者面も同時固定 (CR-B)。
    #[test]
    fn alloc_free_exact_layout_machine_verified() {
        let mut a = GpuArena::new(1024, 16);
        let a1 = a.alloc(100).unwrap();
        let a2 = a.alloc(200).unwrap();
        assert_eq!((a1.offset, a1.size), (0, 112));
        assert_eq!((a2.offset, a2.size), (112, 208));
        a.free(a1);
        let a3 = a.alloc(64).unwrap();
        let a4 = a.alloc(48).unwrap();
        assert_eq!(
            (a3.offset, a3.size),
            (0, 64),
            "best-fit: 112B 穴を先頭から分割再利用"
        );
        assert_eq!((a4.offset, a4.size), (64, 48), "分割残余 48B の丁度再利用");
        assert_eq!(a.used_bytes(), 320);
        assert_eq!(a.free_bytes(), 704);
        assert_eq!(
            a.fragmentation().to_bits(),
            0.0f32.to_bits(),
            "残 free が単一 704B"
        );
        assert_eq!(a.generation(), 5, "4 alloc + 1 free");
        a.free(a2);
        a.free(a4);
        a.free(a3);
        assert_eq!(a.used_bytes(), 0);
        assert_eq!(a.free_bytes(), 1024);
        assert_eq!(
            a.fragmentation().to_bits(),
            0.0f32.to_bits(),
            "全解放で完全併合"
        );
        assert_eq!(a.generation(), 8, "generation は構造化変更数に厳密追従");
    }
}

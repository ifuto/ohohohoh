//! Triple buffering for CPU→GPU mesh uploads (Tier 2).
//! Three slots in flight eliminate map/unmap stalls (DX12/Vulkan best practice).

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

#[derive(Debug)]
struct Slot<T> {
    data: T,
    generation: u64,
}

/// Lock-free-ish triple buffer: CPU writes slot W, GPU reads slot R, third is free.
///
/// ## 不変条件 (2026-07-22 wave 26 監査で機械固定)
/// 1. CPU 書き込み先スロットは read_idx (現公開スロット) と**絶対に一致しない**
///    — 書き込み先は `(read + 1) % 3` のローテーションで選ぶ。旧実装の
///    `free_idx(w, r)` は `(w, r) = (0, 0)` の定常状態 (2 回目の with_cpu_write
///    直後に到達) で read と同じ 0 を返し、3 回目以降は**公開中スロット自身を
///    CPU が上書きする単一バッファ退化**に陥っていた (produce-consume 破綻、
///    根治は「write_slots_rotate_without_touching_published」の回帰ピン参照)。
/// 2. スロットデータの相互排他は Mutex のみ (atomic は index/世代の公開順付)。
/// 3. 世代番号は完了した CPU write 毎に 1 増加 (減ることはない)。
pub struct TripleBuffer<T: Clone + Default> {
    slots: Mutex<[Slot<T>; 3]>,
    /// Index currently owned by CPU writer.
    write_idx: AtomicUsize,
    /// Index currently published for GPU.
    read_idx: AtomicUsize,
    /// Latest published generation.
    published_gen: AtomicU64,
    next_gen: AtomicU64,
}

impl<T: Clone + Default> TripleBuffer<T> {
    pub fn new() -> Self {
        Self {
            slots: Mutex::new([
                Slot {
                    data: T::default(),
                    generation: 0,
                },
                Slot {
                    data: T::default(),
                    generation: 0,
                },
                Slot {
                    data: T::default(),
                    generation: 0,
                },
            ]),
            write_idx: AtomicUsize::new(0),
            read_idx: AtomicUsize::new(1),
            published_gen: AtomicU64::new(0),
            next_gen: AtomicU64::new(1),
        }
    }

    /// 次の書き込み先 = 現公開 (read) スロットの 1 つ先のローテーション。
    /// 公開スロットとの不一致が定義により自明 (wave 26 の退化バグ根治)。
    fn next_write_idx(read: usize) -> usize {
        (read + 1) % 3
    }

    /// Begin writing next frame's CPU data into the free slot.
    pub fn begin_cpu_write(&self) -> usize {
        let r = self.read_idx.load(Ordering::Acquire);
        let free = Self::next_write_idx(r);
        self.write_idx.store(free, Ordering::Release);
        free
    }

    pub fn with_cpu_write<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let idx = self.begin_cpu_write();
        let mut slots = self.slots.lock().unwrap();
        let result = f(&mut slots[idx].data);
        let gen = self.next_gen.fetch_add(1, Ordering::AcqRel);
        slots[idx].generation = gen;
        drop(slots);
        self.end_cpu_write(idx);
        result
    }

    pub fn end_cpu_write(&self, idx: usize) {
        // Publish: swap read to the slot we just wrote.
        self.read_idx.store(idx, Ordering::Release);
        let slots = self.slots.lock().unwrap();
        self.published_gen
            .store(slots[idx].generation, Ordering::Release);
    }

    pub fn begin_gpu_read(&self) -> T {
        let idx = self.read_idx.load(Ordering::Acquire);
        let slots = self.slots.lock().unwrap();
        slots[idx].data.clone()
    }

    pub fn published_generation(&self) -> u64 {
        self.published_gen.load(Ordering::Acquire)
    }
}

impl<T: Clone + Default> Default for TripleBuffer<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_gpu_handoff() {
        let buf = TripleBuffer::<Vec<u8>>::new();
        buf.with_cpu_write(|d| {
            d.clear();
            d.extend_from_slice(&[1, 2, 3]);
        });
        let view = buf.begin_gpu_read();
        assert_eq!(view, vec![1, 2, 3]);
        assert!(buf.published_generation() >= 1);
    }

    /// wave 26-1: 書き込みスロットはローテーション (2,0,1,2,0,1,...) し、
    /// いかなる begin 時点でも現公開スロットと一致しないことの機械ピン。
    /// 旧実装は 3 回目以降 [2,0,0,0,...] と単一スロットに退化していた
    /// (このテストは修正前コードで確実に赤になる回帰検出器)。
    #[test]
    fn write_slots_rotate_without_touching_published() {
        let buf = TripleBuffer::<Vec<u8>>::new();
        let mut seq = Vec::new();
        for k in 1..=7usize {
            let idx = buf.begin_cpu_write();
            let published = buf.read_idx.load(Ordering::Acquire);
            assert_ne!(
                idx, published,
                "write #{k}: writing into the published slot is forbidden"
            );
            seq.push(idx);
            {
                let mut slots = buf.slots.lock().unwrap();
                slots[idx].generation = k as u64;
            }
            buf.end_cpu_write(idx);
        }
        assert_eq!(seq, vec![2, 0, 1, 2, 0, 1, 2], "rotation from read slot");
    }

    /// wave 26-2: 世代番号は完了 write 毎に 1 増加 (欠測・逆転なし)。
    #[test]
    fn generations_increase_strictly_per_completed_write() {
        let buf = TripleBuffer::<Vec<u8>>::new();
        for want in 1..=4u64 {
            buf.with_cpu_write(|d| d.push(want as u8));
            assert_eq!(buf.published_generation(), want, "after {want}-th write");
        }
    }

    /// wave 26-3: 並行 produce/consume で撕裂フレーム (部分的 write) を
    /// 観測しないこと、および最終 write が最終的に読めることの smoke。
    /// 撕裂検出は「payload は常に書き込まれた完全形のいずれかと一致」で判定。
    #[test]
    fn concurrent_producer_consumer_never_reads_torn_frame() {
        let buf = TripleBuffer::<Vec<u8>>::new();
        let payloads: Vec<Vec<u8>> = (0..40u8).map(|i| vec![i; (i as usize % 7) + 4]).collect();
        std::thread::scope(|s| {
            let bref = &buf; // &T は Copy — move クロージャへ各々コピーで共有
            let p = payloads.clone();
            let writer = s.spawn(move || {
                for pl in &p {
                    bref.with_cpu_write(|d| {
                        d.clear();
                        d.extend_from_slice(pl);
                    });
                }
            });
            let reader = s.spawn(move || {
                for _ in 0..400 {
                    let v = bref.begin_gpu_read();
                    if v.is_empty() {
                        continue; // 初期 default スロット (公開前のみ観測し得る)
                    }
                    assert!(v.iter().all(|&b| b == v[0]), "torn frame observed: {v:?}");
                }
            });
            writer.join().unwrap();
            reader.join().unwrap();
        });
        // 最終 write は最終 publish — 少なくともその後の読みで必ず観測できる
        let last = buf.begin_gpu_read();
        assert_eq!(
            last,
            *payloads.last().unwrap(),
            "last written frame must become visible"
        );
    }
}

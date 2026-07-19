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

    fn free_idx(write: usize, read: usize) -> usize {
        match (write, read) {
            (0, 1) | (1, 0) => 2,
            (0, 2) | (2, 0) => 1,
            _ => 0,
        }
    }

    /// Begin writing next frame's CPU data into the free slot.
    pub fn begin_cpu_write(&self) -> usize {
        let w = self.write_idx.load(Ordering::Acquire);
        let r = self.read_idx.load(Ordering::Acquire);
        let free = Self::free_idx(w, r);
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
}

//! Lock-free SPSC ring buffer (Disruptor-inspired) for packet streaming.

use std::sync::atomic::{AtomicUsize, Ordering};

const DEFAULT_CAPACITY: usize = 65536;

pub struct PacketRingBuffer {
    slots: Vec<Option<RingSlot>>,
    capacity: usize,
    write_idx: AtomicUsize,
    read_idx: AtomicUsize,
}

#[derive(Clone)]
pub struct RingSlot {
    pub timestamp_us: u64,
    pub direction: u8,
    pub packet_id: u32,
    pub payload: Vec<u8>,
}

impl PacketRingBuffer {
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.next_power_of_two().max(1024);
        Self {
            slots: (0..cap).map(|_| None).collect(),
            capacity: cap,
            write_idx: AtomicUsize::new(0),
            read_idx: AtomicUsize::new(0),
        }
    }

    pub fn with_default_capacity() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }

    #[inline]
    pub fn try_push(&mut self, slot: RingSlot) -> bool {
        let w = self.write_idx.load(Ordering::Relaxed);
        let r = self.read_idx.load(Ordering::Acquire);
        if w.wrapping_sub(r) >= self.capacity {
            return false; // full
        }
        let idx = w & (self.capacity - 1);
        self.slots[idx] = Some(slot);
        self.write_idx.store(w.wrapping_add(1), Ordering::Release);
        true
    }

    #[inline]
    pub fn try_pop(&mut self) -> Option<RingSlot> {
        let r = self.read_idx.load(Ordering::Relaxed);
        let w = self.write_idx.load(Ordering::Acquire);
        if r == w {
            return None;
        }
        let idx = r & (self.capacity - 1);
        let slot = self.slots[idx].take();
        self.read_idx.store(r.wrapping_add(1), Ordering::Release);
        slot
    }

    pub fn len(&self) -> usize {
        self.write_idx.load(Ordering::Relaxed)
            .wrapping_sub(self.read_idx.load(Ordering::Relaxed))
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

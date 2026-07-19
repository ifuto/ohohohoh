
//! Shared Memory RingBuffer - JavaとRustで同一FileChannel.map共有
//! MappedByteBuffer <-> memmap2で構造体をリングに書き込むだけ、JNIコール1フレーム1回

use std::sync::atomic::{AtomicU64, Ordering};
use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct ChunkUpdate {
    pub packed_pos: u64,
    pub state: u32,
    pub _pad: u32,
}

pub struct SharedRingBuffer {
    capacity: usize,
    head: AtomicU64,
    tail: AtomicU64,
    buffer: Vec<ChunkUpdate>,
}

impl SharedRingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            head: AtomicU64::new(0),
            tail: AtomicU64::new(0),
            buffer: vec![ChunkUpdate { packed_pos: 0, state: 0, _pad: 0 }; capacity],
        }
    }

    pub fn push(&self, item: ChunkUpdate) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) >= self.capacity as u64 {
            return false; // full
        }
        let idx = (head as usize) % self.capacity;
        // Unsafeだがリングバッファは単一Writer/Reader前提で安全
        unsafe {
            let ptr = self.buffer.as_ptr() as *mut ChunkUpdate;
            ptr.add(idx).write(item);
        }
        self.head.store(head+1, Ordering::Release);
        true
    }

    pub fn pop(&self) -> Option<ChunkUpdate> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail >= head { return None; }
        let idx = (tail as usize) % self.capacity;
        let item = unsafe { *self.buffer.as_ptr().add(idx) };
        self.tail.store(tail+1, Ordering::Release);
        Some(item)
    }

    pub fn len(&self) -> usize {
        (self.head.load(Ordering::Relaxed).wrapping_sub(self.tail.load(Ordering::Relaxed))) as usize
    }
}

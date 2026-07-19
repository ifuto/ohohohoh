//! Chunk random ticks — only chunks present in WorldMirror.

use rayon::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct ChunkTickDomain {
    chunks: Vec<ChunkState>,
}

#[derive(Clone)]
struct ChunkState {
    coords: (i32, i32),
    random_tick_count: u32,
    block_tick_queue: u32,
}

impl ChunkTickDomain {
    pub fn empty() -> Self {
        Self { chunks: Vec::new() }
    }

    pub fn new(_tier: rsift_api::PerformanceTier) -> Self {
        Self::empty()
    }

    pub fn ingest_from_mirror(&mut self, chunks: &[crate::world_mirror::JvmChunkState]) {
        self.chunks.clear();
        self.chunks.reserve(chunks.len());
        for c in chunks {
            if c.loaded == 0 {
                continue;
            }
            self.chunks.push(ChunkState {
                coords: (c.chunk_x, c.chunk_z),
                random_tick_count: c.random_ticks_remaining.max(1).min(3),
                block_tick_queue: c.block_tick_queue,
            });
        }
    }

    pub fn write_back(&self, chunks: &mut [crate::world_mirror::JvmChunkState]) {
        for src in &self.chunks {
            if let Some(dst) = chunks
                .iter_mut()
                .find(|c| c.chunk_x == src.coords.0 && c.chunk_z == src.coords.1)
            {
                dst.block_tick_queue = src.block_tick_queue;
                dst.random_ticks_remaining = src.random_tick_count;
            }
        }
    }

    #[inline]
    fn tick_chunk(c: &mut ChunkState) {
        for _ in 0..c.random_tick_count {
            let _ = (c.coords.0.wrapping_mul(17)).wrapping_add(c.coords.1.wrapping_mul(31));
        }
        if c.block_tick_queue > 0 {
            c.block_tick_queue -= 1;
        }
    }

    pub fn tick(&mut self, parallel: bool, stats: &AtomicU64) -> u64 {
        let n = self.chunks.len();
        if n == 0 {
            stats.store(0, Ordering::Relaxed);
            return 0;
        }
        if parallel && n >= 32 {
            self.chunks.par_iter_mut().for_each(Self::tick_chunk);
        } else {
            for c in &mut self.chunks {
                Self::tick_chunk(c);
            }
        }
        stats.store(n as u64, Ordering::Relaxed);
        n as u64
    }
}

//! # 37. GigaVoxels / Brick-Based Streaming (`GigaVoxelsBrickStreaming` - Crassin et al.)
//!
//! レイマーチング中にピクセル単位で、レイが期待するブリック (小さなボクセル塊: `8x8x8`) に
//! ヒットしたかを判定し、ヒットしなければ `BrickRequestQueue` をトリガーして非同期
//! ワーカースレッドへ要求。実際に見えているボクセルブリックのみを VRAM にストリーミング常駐させる。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

pub const BRICK_SIZE: usize = 8;
pub const BRICK_VOL: usize = BRICK_SIZE * BRICK_SIZE * BRICK_SIZE;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BrickKey(pub i32, pub i32, pub i32, pub u8); // (x, y, z, lod)

#[derive(Clone, Debug)]
pub struct VoxelBrick {
    pub voxels: [u16; BRICK_VOL],
    pub last_used_frame: u64,
}

pub struct BrickPool {
    pub max_bricks: usize,
    pub resident_bricks: HashMap<BrickKey, usize>,
    pub pool_slots: Vec<VoxelBrick>,
}

impl BrickPool {
    pub fn new(max_bricks: usize) -> Self {
        Self {
            max_bricks,
            resident_bricks: HashMap::with_capacity(max_bricks),
            pool_slots: Vec::with_capacity(max_bricks),
        }
    }

    pub fn allocate_or_touch(&mut self, key: BrickKey, frame: u64) -> usize {
        if let Some(&idx) = self.resident_bricks.get(&key) {
            self.pool_slots[idx].last_used_frame = frame;
            return idx;
        }

        let idx = if self.pool_slots.len() < self.max_bricks {
            let idx = self.pool_slots.len();
            self.pool_slots.push(VoxelBrick {
                voxels: [0; BRICK_VOL],
                last_used_frame: frame,
            });
            idx
        } else {
            // Evict oldest brick
            let mut oldest_idx = 0;
            let mut oldest_frame = u64::MAX;
            for (i, b) in self.pool_slots.iter().enumerate() {
                if b.last_used_frame < oldest_frame {
                    oldest_frame = b.last_used_frame;
                    oldest_idx = i;
                }
            }
            if let Some((&k, _)) = self.resident_bricks.iter().find(|(_, &v)| v == oldest_idx) {
                self.resident_bricks.remove(&k);
            }
            self.pool_slots[oldest_idx].last_used_frame = frame;
            oldest_idx
        };

        self.resident_bricks.insert(key, idx);
        idx
    }
}

pub struct GigaVoxelsBrickStreaming {
    pub pool: Arc<Mutex<BrickPool>>,
    pub request_queue: Arc<Mutex<VecDeque<BrickKey>>>,
}

impl GigaVoxelsBrickStreaming {
    pub fn new(max_bricks: usize) -> Self {
        Self {
            pool: Arc::new(Mutex::new(BrickPool::new(max_bricks))),
            request_queue: Arc::new(Mutex::new(VecDeque::with_capacity(1024))),
        }
    }

    /// Called by raymarching shader or simulation loop when a brick is missing (`cache miss`).
    pub fn request_brick(&self, key: BrickKey) {
        if let Ok(mut q) = self.request_queue.lock() {
            if !q.contains(&key) {
                q.push_back(key);
            }
        }
    }

    /// Process up to `budget` queued brick loading requests per frame.
    pub fn process_requests(&self, budget: usize, frame: u64, generator: impl Fn(BrickKey) -> [u16; BRICK_VOL]) -> usize {
        let mut processed = 0;
        let mut keys = Vec::new();
        if let Ok(mut q) = self.request_queue.lock() {
            while let Some(key) = q.pop_front() {
                keys.push(key);
                if keys.len() >= budget {
                    break;
                }
            }
        }

        if let Ok(mut pool) = self.pool.lock() {
            for key in keys {
                let slot_idx = pool.allocate_or_touch(key, frame);
                pool.pool_slots[slot_idx].voxels = generator(key);
                processed += 1;
            }
        }
        processed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gigavoxels_streaming_and_eviction() {
        let streamer = GigaVoxelsBrickStreaming::new(2);
        streamer.request_brick(BrickKey(0, 0, 0, 0));
        streamer.request_brick(BrickKey(1, 0, 0, 0));
        streamer.request_brick(BrickKey(2, 0, 0, 0));

        let p = streamer.process_requests(3, 10, |_| [42; BRICK_VOL]);
        assert_eq!(p, 3);
        assert_eq!(streamer.pool.lock().unwrap().resident_bricks.len(), 2);
    }
}

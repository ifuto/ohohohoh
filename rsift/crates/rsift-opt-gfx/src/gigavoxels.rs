//! # 37. GigaVoxels / Brick-Based Streaming (`GigaVoxelsBrickStreaming` - Crassin et al.)
//!
//! レイマーチング中にピクセル単位で、レイが期待するブリック (小さなボクセル塊: `8x8x8`) に
//! ヒットしたかを判定し、ヒットしなければ `BrickRequestQueue` をトリガーして非同期
//! ワーカースレッドへ要求。実際に見えているボクセルブリックのみを VRAM にストリーミング常駐させる。
//!
//! 【wave 166 FL (2026-07-28)】消費者: `FullGraphWiring` の brick 生成
//! 経路 (`request_brick`/`process_requests` → report 実フィールド
//! gigavoxels_processed/gigavoxels_resident)。捕捉 99 [小]: budget=0 で
//! pop 先行により 1 件処理する契約違反を根治 (「高々 budget」厳格化)。
//! 捕捉 100 [小]: max_bricks=0 で空 pool へ index panic する構成を
//! 構築時 fail-loud で明示拒否 (契約逸脱口の閉塞)。

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
        // 【wave 166 FL 捕捉 100】0 は割当時に空 pool への index panic
        // (rq fl_giga (2): slots.len()<max が恒偽で evict 経路到達、
        // pool_slots[0] OOB) となる契約違反構成 → 構築時に明示拒否。
        assert!(
            max_bricks >= 1,
            "GigaVoxelsBrickStreaming: max_bricks must be >= 1 (got {max_bricks})"
        );
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
        // 【wave 166 FL 捕捉 99】旧構造は pop してから `keys.len() >= budget`
        // を確認するため budget=0 でも 1 件生成・アップロードしてしまった
        // (契約「高々 budget」違反、rq (1))。0 件要求は無処理で即返す。
        if budget == 0 {
            return 0;
        }
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

    // ==================== wave 166 (FL) strict ====================

    /// 捕捉 99 [小]: budget 契約は「高々 budget」。旧構造は pop してから
    /// 件数確認するため budget=0 でも 1 件生成・アップロードしてしまう
    /// (rq fl_giga (1))。0 では一切処理しない契約 pin。
    #[test]
    fn fl_budget_zero_processes_nothing() {
        let streamer = GigaVoxelsBrickStreaming::new(4);
        streamer.request_brick(BrickKey(0, 0, 0, 0));
        streamer.request_brick(BrickKey(1, 0, 0, 0));
        let p = streamer.process_requests(0, 7, |_| [1; BRICK_VOL]);
        assert_eq!(p, 0, "budget=0 は生成なし (pop もしない)");
        assert_eq!(
            streamer.request_queue.lock().unwrap().len(),
            2,
            "要求は queue に保持 (次 tick 以降の処理対象)"
        );
    }

    /// 捕捉 100 [小]: max_bricks=0 は割当時に空 pool へ index panic となる
    /// 契約違反構成 (rq (2))。契約は構築時に fail-loud 明示拒否
    /// (capture 85 系)。旧来の黙殺後爆発を構造的に潰す。
    #[test]
    #[should_panic(expected = "max_bricks must be >= 1")]
    fn fl_new_rejects_zero_capacity() {
        let _ = GigaVoxelsBrickStreaming::new(0);
    }

    /// 捕捉 99 の部分 budget: 2/3 で残 1 件が次フレームへ持ち越される。
    #[test]
    fn fl_budget_partial_carryover() {
        let streamer = GigaVoxelsBrickStreaming::new(4);
        for x in 0..3i32 {
            streamer.request_brick(BrickKey(x, 0, 0, 0));
        }
        assert_eq!(streamer.process_requests(2, 1, |_| [0; BRICK_VOL]), 2);
        assert_eq!(streamer.request_queue.lock().unwrap().len(), 1, "残 1 件");
        assert_eq!(streamer.process_requests(2, 2, |_| [0; BRICK_VOL]), 1);
    }

    /// eviction のキー厳密 pin: 最古 (last_used_frame 最小) が退去する
    /// (rq (4))。既存試験は len のみ — キー集合を厳密固定する。
    #[test]
    fn fl_evicts_oldest_key_exactly() {
        let streamer = GigaVoxelsBrickStreaming::new(2);
        let a = BrickKey(0, 0, 0, 0);
        let b = BrickKey(1, 0, 0, 0);
        let c = BrickKey(2, 0, 0, 0);
        streamer.request_brick(a);
        let _ = streamer.process_requests(1, 1, |_| [0; BRICK_VOL]);
        streamer.request_brick(b);
        let _ = streamer.process_requests(1, 2, |_| [0; BRICK_VOL]);
        streamer.request_brick(c);
        let _ = streamer.process_requests(1, 3, |_| [0; BRICK_VOL]);
        let pool = streamer.pool.lock().unwrap();
        assert!(!pool.resident_bricks.contains_key(&a), "A(最古) 退去");
        assert!(pool.resident_bricks.contains_key(&b) && pool.resident_bricks.contains_key(&c));
    }

    /// 【wave 166 FL adversarial(e) 強化】タッチ後の LRU 順位を厳密 pin。
    /// 変異 `last_used_frame < oldest_frame` → `>` はスキャン不更新化
    /// (oldest_idx が初期値 0 に固定) となり、touch 無し系列では真の最古が
    /// 常に slot 0 と一致して構造吸収される (wave 166 (e) 一次実測)。
    /// touch で slot 0 の frame を新しくした系列では真の最古 ≠ slot 0
    /// となるため吸収が破れ、常時 slot 0 退去変異が RED となる。
    #[test]
    fn fl_evicts_lru_after_touch_exactly() {
        let streamer = GigaVoxelsBrickStreaming::new(2);
        let a = BrickKey(0, 0, 0, 0);
        let b = BrickKey(1, 0, 0, 0);
        let c = BrickKey(2, 0, 0, 0);
        streamer.request_brick(a);
        let _ = streamer.process_requests(1, 1, |_| [0; BRICK_VOL]);
        streamer.request_brick(b);
        let _ = streamer.process_requests(1, 2, |_| [0; BRICK_VOL]);
        // touch: A の last_used_frame を 3 へ更新 (B=frame 2 が真の最古)。
        streamer.request_brick(a);
        let _ = streamer.process_requests(1, 3, |_| [0; BRICK_VOL]);
        streamer.request_brick(c);
        let _ = streamer.process_requests(1, 4, |_| [0; BRICK_VOL]);
        let pool = streamer.pool.lock().unwrap();
        assert!(pool.resident_bricks.contains_key(&a), "touch 済み A は残留");
        assert!(!pool.resident_bricks.contains_key(&b), "真の最古 B が退去");
        assert!(pool.resident_bricks.contains_key(&c), "新規 C は残留");
    }

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

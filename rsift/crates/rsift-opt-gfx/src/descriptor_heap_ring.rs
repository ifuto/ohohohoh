
//! Descriptor Heap Ring Allocator - D3D12 CBV/SRV/UAVのO(1)確保を実現
//! d3d12-descriptor-heapクレートの調査に基づく: 線形リングでフレーム毎リセット、GPU完了後に再利用。
//! 低スペPCでもDescriptor生成ボトルネックを排除。

use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy)]
pub struct DescriptorHandle {
    pub cpu_offset: u32,
    pub gpu_offset: u64,
    pub index: u32,
}

pub struct DescriptorHeapRing {
    capacity: u32,
    descriptor_size: u32,
    head: Mutex<u32>,
    // 注: 旧 `tail: u32` は初期化後一度も読み書きされないデッドフィールドで、
    // doc の「埋まったら tail を進める」は未実装の約束だった (2026-07-21 監査)。
    frame_fence: AtomicU64,
    allocations: AtomicU64,
}

impl DescriptorHeapRing {
    pub fn new(capacity: u32, descriptor_size: u32) -> Self {
        Self {
            capacity,
            descriptor_size,
            head: Mutex::new(0),
            frame_fence: AtomicU64::new(0),
            allocations: AtomicU64::new(0),
        }
    }

    /// O(1)でDescriptor確保。リング末端に達すると 0 へラップする。
    /// 注意: フェンス完了確認は行わない設計 (呼び出し側でフレーム同期保証が前提) —
    /// 「tail を進める退避」は存在しない (2026-07-21 監査で虚偽コメントを訂正)。
    pub fn alloc(&self, count: u32) -> Option<DescriptorHandle> {
        let mut head = self.head.lock();
        let start = *head;
        // saturating: 非オーバーフロー入力では `start + count` と厳密同値。
        // 旧実装は head と count の組合せで u32 境界を跨ぐと debug ビルドが
        // panic し release は暗黙 wrap していた (2026-07-22 監査で摘出)。
        let end = start.saturating_add(count);
        if end >= self.capacity {
            // ラップアラウンド: 0から再開（GPUフェンスが完了している前提）
            if count > self.capacity {
                return None;
            }
            *head = count;
            self.allocations.fetch_add(1, Ordering::Relaxed);
            Some(DescriptorHandle { cpu_offset: 0, gpu_offset: 0, index: 0 })
        } else {
            *head = end;
            self.allocations.fetch_add(1, Ordering::Relaxed);
            Some(DescriptorHandle {
                cpu_offset: start * self.descriptor_size,
                gpu_offset: start as u64 * self.descriptor_size as u64,
                index: start,
            })
        }
    }

    /// フレーム終了時に呼び出し、headをリセット（GPU完了後）
    pub fn reset(&self, gpu_completed_fence: u64) {
        let completed = self.frame_fence.load(Ordering::Acquire);
        if gpu_completed_fence >= completed {
            *self.head.lock() = 0;
        }
    }

    pub fn stats(&self) -> (u64, u32) {
        (self.allocations.load(Ordering::Relaxed), *self.head.lock())
    }
}

/// CBV/SRV/UAVとSamplerを分離管理するための二重リング
pub struct DualHeapRing {
    pub cbv_srv_uav: DescriptorHeapRing,
    pub sampler: DescriptorHeapRing,
}

impl DualHeapRing {
    pub fn new(cbv_capacity: u32, sampler_capacity: u32, desc_size: u32, sampler_size: u32) -> Self {
        Self {
            cbv_srv_uav: DescriptorHeapRing::new(cbv_capacity, desc_size),
            sampler: DescriptorHeapRing::new(sampler_capacity, sampler_size),
        }
    }
}

#[cfg(test)]
mod strict_tests {
    use super::*;

    #[test]
    fn sequential_alloc_offsets_scale_with_descriptor_size() {
        let ring = DescriptorHeapRing::new(16, 64);
        let a = ring.alloc(2).unwrap();
        assert_eq!((a.cpu_offset, a.gpu_offset, a.index), (0, 0, 0));
        let b = ring.alloc(3).unwrap();
        assert_eq!(
            (b.cpu_offset, b.gpu_offset, b.index),
            (2 * 64, 2 * 64, 2),
            "cpu/gpu offset は index * descriptor_size 規則"
        );
        assert_eq!(ring.stats(), (2, 5), "(確保回数, head)");
    }

    #[test]
    fn boundary_fit_wraps_and_oversize_rejected() {
        let ring = DescriptorHeapRing::new(16, 32);
        assert!(ring.alloc(12).is_some(), "head = 12");
        // 監査上の仕様固定: end >= capacity (12+4=16) は「境界ぴったり」でも
        // wrap として index 0 払い出し (末尾の 4 個は利用されない保守設計)。
        let w = ring.alloc(4).unwrap();
        assert_eq!((w.cpu_offset, w.gpu_offset, w.index), (0, 0, 0));
        assert_eq!(ring.stats(), (2, 4));
        assert!(ring.alloc(17).is_none(), "capacity 超過は None");
        assert_eq!(ring.stats(), (2, 4), "None 経路は帳簿を進めない");
    }

    #[test]
    fn full_capacity_alloc_takes_wrap_path() {
        let ring = DescriptorHeapRing::new(16, 32);
        let h = ring.alloc(16).unwrap();
        assert_eq!(h.index, 0, "count == capacity も wrap 経路で index 0");
        assert_eq!(ring.stats().1, 16, "head = count (capacity と一致し得る)");
        let next = ring.alloc(1).unwrap();
        assert_eq!(next.index, 0, "head==capacity からの次回確保も wrap");
        assert_eq!(ring.stats(), (2, 1));
    }

    #[test]
    fn reset_clears_head_when_fence_satisfied() {
        let ring = DescriptorHeapRing::new(32, 16);
        ring.alloc(8).unwrap();
        assert_eq!(ring.stats().1, 8);
        ring.reset(0); // frame_fence 初期値 0: 0 >= 0 で充足
        assert_eq!(ring.stats(), (1, 0), "\"リングが埋まったら tail を進める\" ではなく全量リセット設計");
        ring.alloc(2).unwrap();
        ring.reset(u64::MAX);
        assert_eq!(ring.stats(), (2, 0));
    }

    #[test]
    fn no_overflow_when_head_near_u32_max() {
        // 回帰: 旧 `start + count` は u32 境界で debug panic / release 暗黙 wrap。
        let ring = DescriptorHeapRing::new(u32::MAX, 1);
        let big = ring.alloc(u32::MAX - 1).unwrap();
        assert_eq!(big.index, 0);
        assert_eq!(ring.stats().1, u32::MAX - 1);
        let wrapped = ring.alloc(4).unwrap();
        assert_eq!(wrapped.index, 0, "saturating 後 end >= capacity で wrap");
        assert_eq!(ring.stats().1, 4);
        assert!(ring.alloc(u32::MAX).is_some(), "count == capacity は wrap 経路で許容");
    }

    #[test]
    fn dual_ring_isolates_cbv_and_sampler() {
        let dual = DualHeapRing::new(8, 4, 32, 96);
        let c = dual.cbv_srv_uav.alloc(2).unwrap();
        let s = dual.sampler.alloc(2).unwrap();
        assert_eq!((c.index, s.index), (0, 0), "両リングとも独立に index 0 開始");
        let c2 = dual.cbv_srv_uav.alloc(1).unwrap();
        assert_eq!(c2.cpu_offset, 64, "descriptor_size 32 スケール");
        let s2 = dual.sampler.alloc(1).unwrap();
        assert_eq!(s2.cpu_offset, 192, "sampler は descriptor_size 96 スケール");
        assert!(dual.sampler.alloc(9).is_none(), "sampler capacity 4 超過は None");
        assert!(
            dual.cbv_srv_uav.alloc(3).is_some(),
            "sampler 側の失敗は cbv 側の残容量に影響しない"
        );
    }
}

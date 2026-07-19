
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
    tail: u32,
    frame_fence: AtomicU64,
    allocations: AtomicU64,
}

impl DescriptorHeapRing {
    pub fn new(capacity: u32, descriptor_size: u32) -> Self {
        Self {
            capacity,
            descriptor_size,
            head: Mutex::new(0),
            tail: 0,
            frame_fence: AtomicU64::new(0),
            allocations: AtomicU64::new(0),
        }
    }

    /// O(1)でDescriptor確保。リングが埋まったら自動的にtailを進める（古いフレームはGPU完了済み想定）
    pub fn alloc(&self, count: u32) -> Option<DescriptorHandle> {
        let mut head = self.head.lock();
        let start = *head;
        let end = start + count;
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

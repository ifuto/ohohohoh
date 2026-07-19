//! Phase 5 — GPU-driven Voxel Framework (ESVO / SVDAG).
//! Buffers are allocated lazily — never reserve 384MB up-front on low-spec PCs.

use crate::device::Dx12Device;
use crate::error::Dx12Result;
use crate::resources::{create_default_buffer, GpuBuffer};

#[cfg(windows)]
use windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;

pub struct EsvoNode {
    pub child_mask: u8,
    pub leaf_mask: u8,
    pub child_ptr: u32,
    pub material_id: u32,
}

pub struct SvdagNode {
    pub mask: u32,
    pub child_ptr: u32,
}

/// Default page size when first used (4MB) — grows on demand.
pub const VOXEL_PAGE_BYTES: u64 = 4 * 1024 * 1024;
/// Hard cap so we never silently eat 384MB VRAM on weak GPUs.
pub const VOXEL_SVDAG_CAP_BYTES: u64 = 64 * 1024 * 1024;
pub const VOXEL_ESVO_CAP_BYTES: u64 = 128 * 1024 * 1024;

pub struct VoxelFramework {
    pub svdag_buffer: Option<GpuBuffer>,
    pub esvo_buffer: Option<GpuBuffer>,
    pub svdag_capacity: u64,
    pub esvo_capacity: u64,
}

impl VoxelFramework {
    /// Construct with zero GPU allocation (cheap boot / low-spec).
    pub fn new(_device: &Dx12Device) -> Dx12Result<Self> {
        tracing::info!(
            "[VoxelFramework] lazy init — 0 bytes reserved (page={}MB cap svdag={}MB esvo={}MB)",
            VOXEL_PAGE_BYTES / (1024 * 1024),
            VOXEL_SVDAG_CAP_BYTES / (1024 * 1024),
            VOXEL_ESVO_CAP_BYTES / (1024 * 1024),
        );
        Ok(Self {
            svdag_buffer: None,
            esvo_buffer: None,
            svdag_capacity: 0,
            esvo_capacity: 0,
        })
    }

    #[cfg(windows)]
    pub fn ensure_svdag(&mut self, device: &Dx12Device, needed: u64) -> Dx12Result<&GpuBuffer> {
        let want = needed
            .max(VOXEL_PAGE_BYTES)
            .next_power_of_two()
            .min(VOXEL_SVDAG_CAP_BYTES);
        if self.svdag_capacity < want {
            self.svdag_buffer = Some(create_default_buffer(
                device,
                want,
                D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
                "svdag_buffer",
            )?);
            self.svdag_capacity = want;
            tracing::info!("[VoxelFramework] SVDAG allocated {}MB", want / (1024 * 1024));
        }
        Ok(self.svdag_buffer.as_ref().unwrap())
    }

    #[cfg(windows)]
    pub fn ensure_esvo(&mut self, device: &Dx12Device, needed: u64) -> Dx12Result<&GpuBuffer> {
        let want = needed
            .max(VOXEL_PAGE_BYTES)
            .next_power_of_two()
            .min(VOXEL_ESVO_CAP_BYTES);
        if self.esvo_capacity < want {
            self.esvo_buffer = Some(create_default_buffer(
                device,
                want,
                D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
                "esvo_buffer",
            )?);
            self.esvo_capacity = want;
            tracing::info!("[VoxelFramework] ESVO allocated {}MB", want / (1024 * 1024));
        }
        Ok(self.esvo_buffer.as_ref().unwrap())
    }

    pub fn resident_bytes(&self) -> u64 {
        self.svdag_capacity + self.esvo_capacity
    }

    pub fn release(&mut self) {
        self.svdag_buffer = None;
        self.esvo_buffer = None;
        self.svdag_capacity = 0;
        self.esvo_capacity = 0;
    }
}

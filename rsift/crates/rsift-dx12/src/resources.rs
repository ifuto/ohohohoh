//! Phase 2 — GPU resources (upload heaps, structured buffers, persistent VBO).

use crate::device::Dx12Device;
use crate::error::Dx12Result;
use crate::win::{DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};
use tracing::debug;

#[cfg(windows)]
use windows::Win32::Graphics::Direct3D12::*;

pub struct GpuBuffer {
    pub size: u64,
    #[cfg(windows)]
    pub resource: ID3D12Resource,
}

#[cfg(windows)]
pub fn create_upload_buffer(device: &Dx12Device, size: u64, label: &str) -> Dx12Result<GpuBuffer> {
    unsafe {
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_UPLOAD,
            CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
            MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
            CreationNodeMask: 0,
            VisibleNodeMask: 0,
        };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Alignment: 0,
            Width: size,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_UNKNOWN,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: D3D12_RESOURCE_FLAG_NONE,
        };
        let mut resource: Option<ID3D12Resource> = None;
        device.device.CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_NONE,
            &desc,
            D3D12_RESOURCE_STATE_GENERIC_READ,
            None,
            &mut resource,
        )?;
        debug!("[Dx12Resource] upload buffer '{}' {}B", label, size);
        Ok(GpuBuffer {
            size,
            resource: resource.unwrap(),
        })
    }
}

#[cfg(windows)]
pub fn create_default_buffer(
    device: &Dx12Device,
    size: u64,
    flags: D3D12_RESOURCE_FLAGS,
    label: &str,
) -> Dx12Result<GpuBuffer> {
    unsafe {
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            ..Default::default()
        };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: size,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_UNKNOWN,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: flags,
            Alignment: 0,
        };
        let mut resource: Option<ID3D12Resource> = None;
        device.device.CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_NONE,
            &desc,
            D3D12_RESOURCE_STATE_COMMON,
            None,
            &mut resource,
        )?;
        debug!("[Dx12Resource] default buffer '{}' {}B", label, size);
        Ok(GpuBuffer {
            size,
            resource: resource.unwrap(),
        })
    }
}

#[cfg(windows)]
pub fn upload_slice(buffer: &GpuBuffer, data: &[u8]) -> Dx12Result<()> {
    unsafe {
        let mapped = std::mem::MaybeUninit::<D3D12_RANGE>::zeroed();
        let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
        buffer.resource.Map(0, Some(mapped.as_ptr()), Some(&mut ptr))?;
        std::ptr::copy_nonoverlapping(
            data.as_ptr(),
            ptr as *mut u8,
            data.len().min(buffer.size as usize),
        );
        buffer.resource.Unmap(0, None);
    }
    Ok(())
}

/// 1 GB persistent VBO pool (single committed buffer).
pub const PERSISTENT_VBO_BYTES: u64 = 1024 * 1024 * 1024;

pub struct PersistentVboPool {
    pub pool: GpuBuffer,
    pub cursor: u64,
}

impl PersistentVboPool {
    pub fn new(device: &Dx12Device) -> Dx12Result<Self> {
        let pool = create_default_buffer(
            device,
            PERSISTENT_VBO_BYTES,
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            "persistent_vbo_1gb",
        )?;
        Ok(Self { pool, cursor: 0 })
    }

    pub fn allocate(&mut self, bytes: u64) -> Option<u64> {
        if self.cursor + bytes > self.pool.size {
            return None;
        }
        let off = self.cursor;
        self.cursor += bytes;
        Some(off)
    }
}

pub struct StagingBelt {
    pub pool: GpuBuffer,
    pub cursor: u64,
    pub mapped_ptr: *mut u8,
}

impl StagingBelt {
    #[cfg(windows)]
    pub fn new(device: &Dx12Device, size: u64) -> Dx12Result<Self> {
        let pool = create_upload_buffer(device, size, "staging_belt")?;
        let mut mapped_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
        unsafe {
            pool.resource.Map(0, None, Some(&mut mapped_ptr))?;
        }
        Ok(Self {
            pool,
            cursor: 0,
            mapped_ptr: mapped_ptr as *mut u8,
        })
    }

    #[cfg(windows)]
    pub fn write(&mut self, data: &[u8]) -> Option<u64> {
        let size = data.len() as u64;
        if self.cursor + size > self.pool.size {
            return None;
        }
        let offset = self.cursor;
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), self.mapped_ptr.add(offset as usize), data.len());
        }
        self.cursor += size;
        Some(offset)
    }
}

/// SM 6.6 Bindless Textures (Dynamic Resources)
pub struct BindlessDescriptorHeap {
    #[cfg(windows)]
    pub heap: ID3D12DescriptorHeap,
    pub capacity: u32,
    pub cursor: u32,
}

impl BindlessDescriptorHeap {
    #[cfg(windows)]
    pub fn new(device: &Dx12Device, capacity: u32) -> Dx12Result<Self> {
        unsafe {
            let desc = D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                NumDescriptors: capacity,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                NodeMask: 0,
            };
            let heap: ID3D12DescriptorHeap = device.device.CreateDescriptorHeap(&desc)?;
            Ok(Self {
                heap,
                capacity,
                cursor: 0,
            })
        }
    }
}

#[cfg(not(windows))]
pub struct GpuBuffer;

#[cfg(not(windows))]
pub struct PersistentVboPool;

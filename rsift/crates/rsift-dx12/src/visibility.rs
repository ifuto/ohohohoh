//! Phase 4 — Visibility Buffer (Triangle ID + Instance ID).

use crate::device::Dx12Device;
use crate::error::Dx12Result;

#[cfg(windows)]
use windows::Win32::Graphics::Direct3D12::*;

/// Represents the packed visibility buffer data.
pub struct VisibilityBuffer {
    #[cfg(windows)]
    pub resource: ID3D12Resource,
    pub width: u32,
    pub height: u32,
}

impl VisibilityBuffer {
    #[cfg(windows)]
    pub fn new(device: &Dx12Device, width: u32, height: u32) -> Dx12Result<Self> {
        unsafe {
            let heap = D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_DEFAULT,
                CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
                MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
                CreationNodeMask: 0,
                VisibleNodeMask: 0,
            };
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Alignment: 0,
                Width: width as u64,
                Height: height,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R32_UINT,
                SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
                Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET | D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            };

            let clear_value = D3D12_CLEAR_VALUE {
                Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R32_UINT,
                Anonymous: D3D12_CLEAR_VALUE_0 {
                    Color: [1.0, 1.0, 1.0, 1.0], // UINT max represents unwritten/sky
                },
            };

            let mut resource: Option<ID3D12Resource> = None;
            device.device.CreateCommittedResource(
                &heap,
                D3D12_HEAP_FLAG_NONE,
                &desc,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
                Some(&clear_value),
                &mut resource,
            )?;

            Ok(Self {
                resource: resource.unwrap(),
                width,
                height,
            })
        }
    }
}

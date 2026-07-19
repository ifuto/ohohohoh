//! Phase 1 — D3D12 device, command queue, allocator, fence.

use crate::agility::{bootstrap, AgilityConfig};
use crate::error::Dx12Result;
use rsift_api::engine_caps::{EngineCaps, ShaderModelTier};
use std::sync::Arc;
use tracing::info;

#[cfg(windows)]
use windows::Win32::Graphics::Direct3D::*;
#[cfg(windows)]
use windows::Win32::Graphics::Direct3D12::*;
#[cfg(windows)]
use windows::Win32::Graphics::Dxgi::*;

#[derive(Debug)]
pub struct Dx12Device {
    pub adapter_name: String,
    pub feature_level: u32,
    pub shader_model: String,
    pub device: Arc<ID3D12Device>,
    pub queue: ID3D12CommandQueue,
    pub allocator: ID3D12CommandAllocator,
    pub async_queue: ID3D12CommandQueue,
    pub async_allocator: ID3D12CommandAllocator,
    pub fence: ID3D12Fence,
    pub fence_value: u64,
    pub fence_event: windows::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
pub fn create_device(caps: &EngineCaps) -> Dx12Result<Dx12Device> {
    bootstrap(&AgilityConfig::default())?;

    unsafe {
        let factory: IDXGIFactory6 =
            CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
        let adapter = pick_adapter(&factory)?;
        let desc = adapter.GetDesc1()?;
        let adapter_name = String::from_utf16_lossy(
            &desc.Description[..desc
                .Description
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(desc.Description.len())],
        );

        let mut device: Option<ID3D12Device> = None;
        D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_12_1, &mut device)?;
        let device = device.ok_or_else(|| crate::error::Dx12Error::Msg("null device".into()))?;

        let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS::default();
        if device
            .CheckFeatureSupport(
                D3D12_FEATURE_D3D12_OPTIONS,
                &mut options as *mut _ as *mut _,
                std::mem::size_of::<D3D12_FEATURE_DATA_D3D12_OPTIONS>() as u32,
            )
            .is_ok()
        {
            info!(
                "[Dx12Device] tiled_resources={} conservative_raster={}",
                options.TiledResourcesTier.0,
                options.ConservativeRasterizationTier.0
            );
        }

        let mut sm = D3D12_FEATURE_DATA_SHADER_MODEL::default();
        sm.HighestShaderModel = match caps.shader_model {
            ShaderModelTier::Sm69 => D3D_SHADER_MODEL_6_9,
            ShaderModelTier::Sm66 => D3D_SHADER_MODEL_6_6,
        };
        let sm_ok = device
            .CheckFeatureSupport(
                D3D12_FEATURE_SHADER_MODEL,
                &mut sm as *mut _ as *mut _,
                std::mem::size_of::<D3D12_FEATURE_DATA_SHADER_MODEL>() as u32,
            )
            .is_ok();
        let shader_model = if sm_ok {
            format!(
                "{}.{}",
                sm.HighestShaderModel.0 >> 4,
                sm.HighestShaderModel.0 & 0xF
            )
        } else {
            caps.shader_model.as_str().to_string()
        };

        let queue = create_command_queue(&device, D3D12_COMMAND_LIST_TYPE_DIRECT)?;
        let allocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?;

        let async_queue = create_command_queue(&device, D3D12_COMMAND_LIST_TYPE_COMPUTE)?;
        let async_allocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COMPUTE)?;

        let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?;
        let fence_event = windows::Win32::System::Threading::CreateEventA(
            None,
            false,
            false,
            None,
        )?;

        info!(
            "[Dx12Device] adapter={} SM={} feature=12_1",
            adapter_name, shader_model
        );

        Ok(Dx12Device {
            adapter_name,
            feature_level: 0xC100,
            shader_model,
            device: Arc::new(device),
            queue,
            allocator,
            async_queue,
            async_allocator,
            fence,
            fence_value: 0,
            fence_event,
        })
    }
}

#[cfg(windows)]
unsafe fn pick_adapter(factory: &IDXGIFactory6) -> Dx12Result<IDXGIAdapter1> {
    for i in 0.. {
        match factory.EnumAdapterByGpuPreference::<IDXGIAdapter1>(
            i,
            DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
        ) {
            Ok(adapter) => {
                let desc = adapter.GetDesc1()?;
                if desc.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0 {
                    continue;
                }
                return Ok(adapter);
            }
            Err(_) => break,
        }
    }
    Err(crate::error::Dx12Error::Msg("no DXGI adapter".into()))
}

#[cfg(windows)]
unsafe fn create_command_queue(device: &ID3D12Device, queue_type: D3D12_COMMAND_LIST_TYPE) -> Dx12Result<ID3D12CommandQueue> {
    let desc = D3D12_COMMAND_QUEUE_DESC {
        Type: queue_type,
        Priority: D3D12_COMMAND_QUEUE_PRIORITY_NORMAL.0,
        Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
        NodeMask: 0,
    };
    Ok(device.CreateCommandQueue(&desc)?)
}

impl Dx12Device {
    pub fn wait_gpu(&mut self) -> Dx12Result<()> {
        #[cfg(windows)]
        unsafe {
            self.fence_value += 1;
            self.queue.Signal(&self.fence, self.fence_value)?;
            if self.fence.GetCompletedValue() < self.fence_value {
                self.fence
                    .SetEventOnCompletion(self.fence_value, self.fence_event)?;
                let _ = windows::Win32::System::Threading::WaitForSingleObject(
                    self.fence_event,
                    windows::Win32::System::Threading::INFINITE,
                );
            }
        }
        Ok(())
    }
}

#[cfg(not(windows))]
pub fn create_device(_caps: &EngineCaps) -> Dx12Result<Dx12Device> {
    Err(crate::error::Dx12Error::Msg("Windows only".into()))
}

#[cfg(not(windows))]
#[derive(Debug)]
pub struct Dx12Device;

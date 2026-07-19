//! Phase 1  EDXGI swap chain (flip-discard, HDR-ready).

use crate::device::Dx12Device;
use crate::error::Dx12Result;
use crate::win::{
    check_present, DXGI_ALPHA_MODE_UNSPECIFIED, DXGI_FORMAT, DXGI_FORMAT_R8G8B8A8_UNORM,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT;
use tracing::info;

#[cfg(windows)]
use windows::Win32::Graphics::Dxgi::*;
#[cfg(windows)]
use windows::Win32::Graphics::Direct3D12::*;
#[cfg(windows)]
use windows::core::Interface;

pub struct Dx12SwapChain {
    pub width: u32,
    pub height: u32,
    pub format: DXGI_FORMAT,
    #[cfg(windows)]
    pub swap_chain: IDXGISwapChain3,
    #[cfg(windows)]
    pub rtv_heap: ID3D12DescriptorHeap,
    #[cfg(windows)]
    pub waitable_object: windows::Win32::Foundation::HANDLE,
    #[cfg(windows)]
    pub back_buffers: Vec<ID3D12Resource>,
    pub frame_index: u32,
    pub buffer_count: u32,
}

#[cfg(windows)]
pub fn create_swap_chain(
    hwnd: windows::Win32::Foundation::HWND,
    device: &Dx12Device,
    width: u32,
    height: u32,
) -> Dx12Result<Dx12SwapChain> {
    unsafe {
        let factory: IDXGIFactory6 =
            CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
        let queue: ID3D12CommandQueue = device.queue.cast()?;

        let swap_desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_UNSPECIFIED,
            Flags: DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32,
        };

        let swap_chain1 = factory.CreateSwapChainForHwnd(&queue, hwnd, &swap_desc, None, None)?;
        let swap_chain: IDXGISwapChain3 = swap_chain1.cast()?;
        swap_chain.SetMaximumFrameLatency(1)?;
        let waitable_object = swap_chain.GetFrameLatencyWaitableObject();

        let rtv_heap: ID3D12DescriptorHeap = device.device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
            Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
            NumDescriptors: 2,
            Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
            NodeMask: 0,
        })?;

        let mut back_buffers = Vec::with_capacity(2);
        let rtv_size = device
            .device
            .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV);
        let mut rtv_handle = rtv_heap.GetCPUDescriptorHandleForHeapStart();

        for i in 0..2u32 {
            let buffer: ID3D12Resource = swap_chain.GetBuffer(i)?;
            device.device.CreateRenderTargetView(&buffer, None, rtv_handle);
            back_buffers.push(buffer);
            rtv_handle.ptr += rtv_size as usize;
        }

        info!("[Dx12SwapChain] {}x{} flip-discard x2", width, height);

        Ok(Dx12SwapChain {
            width,
            height,
            format: DXGI_FORMAT_R8G8B8A8_UNORM,
            swap_chain,
            rtv_heap,
            waitable_object,
            back_buffers,
            frame_index: 0,
            buffer_count: 2,
        })
    }
}

impl Dx12SwapChain {
    pub fn wait_for_frame(&self) {
        unsafe {
            windows::Win32::System::Threading::WaitForSingleObject(
                self.waitable_object,
                windows::Win32::System::Threading::INFINITE,
            );
        }
    }

    #[cfg(windows)]
    pub fn present(&mut self) -> Dx12Result<()> {
        unsafe {
            check_present(self.swap_chain.Present(1, DXGI_PRESENT(0)))?;
            self.frame_index = self.swap_chain.GetCurrentBackBufferIndex();
        }
        Ok(())
    }
}

#[cfg(not(windows))]
pub struct Dx12SwapChain;

#[cfg(not(windows))]
pub fn create_swap_chain(
    _hwnd: (),
    _device: &Dx12Device,
    _width: u32,
    _height: u32,
) -> Dx12Result<Dx12SwapChain> {
    Err(crate::error::Dx12Error::Msg("Windows only".into()))
}

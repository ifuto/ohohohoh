//! Phase 4 — Multi-View Rendering (stereo / shadow cascades).
//! Real path: upload ViewConstants CB + N DrawInstanced with per-view VP.
//! Hardware View Instancing PSO subobject is optional; looped draws satisfy Done.

use crate::device::Dx12Device;
use crate::error::Dx12Result;
use tracing::info;

#[cfg(windows)]
use windows::Win32::Graphics::Direct3D12::*;

/// Up to 4 views: main + shadow cascades or stereo pair.
pub const MAX_VIEWS: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ViewConstants {
    pub view_proj: [[f32; 4]; 4],
    pub view_index: u32,
    pub view_count: u32,
    pub _pad: [u32; 2],
}

pub struct MultiViewRenderer {
    pub view_count: u32,
    pub instancing_enabled: bool,
    #[cfg(windows)]
    pub view_cb: ID3D12Resource,
    pub views: Vec<ViewConstants>,
}

impl MultiViewRenderer {
    pub fn new(device: &Dx12Device, view_count: u32) -> Dx12Result<Self> {
        let view_count = view_count.clamp(1, MAX_VIEWS);
        let views = (0..view_count)
            .map(|i| ViewConstants {
                view_proj: identity_mat4(),
                view_index: i,
                view_count,
                _pad: [0; 2],
            })
            .collect();

        #[cfg(windows)]
        let view_cb = {
            let size = (std::mem::size_of::<ViewConstants>() * view_count as usize) as u64;
            crate::resources::create_upload_buffer(device, size, "multi_view_cb")?.resource
        };

        info!(
            "[MultiView] {} views (per-view DrawInstanced; CB upload live)",
            view_count
        );

        Ok(Self {
            view_count,
            instancing_enabled: view_count > 1,
            #[cfg(windows)]
            view_cb,
            views,
        })
    }

    pub fn set_view(&mut self, index: u32, view_proj: [[f32; 4]; 4]) {
        if let Some(v) = self.views.get_mut(index as usize) {
            v.view_proj = view_proj;
            v.view_index = index;
            v.view_count = self.view_count;
        }
    }

    /// Map view CB with current matrices (CPU → UPLOAD heap).
    #[cfg(windows)]
    pub fn upload_views(&self) -> Dx12Result<()> {
        unsafe {
            let mut ptr = std::ptr::null_mut();
            self.view_cb.Map(0, None, Some(&mut ptr))?;
            let bytes = bytemuck::cast_slice(self.views.as_slice());
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
            self.view_cb.Unmap(0, None);
        }
        Ok(())
    }

    /// Bind: upload CB. Draw loop is owned by terrain_pass (instance/views count).
    #[cfg(windows)]
    pub fn bind_view_instancing(&self, _cmd: &ID3D12GraphicsCommandList) -> Dx12Result<()> {
        self.upload_views()
    }

    pub fn view_proj(&self, index: u32) -> [[f32; 4]; 4] {
        self.views
            .get(index as usize)
            .map(|v| v.view_proj)
            .unwrap_or_else(identity_mat4)
    }
}

fn identity_mat4() -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

#[cfg(not(windows))]
impl MultiViewRenderer {
    pub fn new(_device: &Dx12Device, _view_count: u32) -> Dx12Result<Self> {
        Err(crate::error::Dx12Error::Msg("Windows only".into()))
    }
}

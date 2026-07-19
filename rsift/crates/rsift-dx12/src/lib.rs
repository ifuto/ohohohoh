//! # Rsift DirectX 12 Agility Engine (`rsift-dx12`)
//!
//! Native D3D12 Agility explicit rendering pipeline (Root Signature 1.1, Static Samplers,
//! Descriptor Heap Ring, DirectStorage, Work Graphs, and SFS).
//! On non-Windows platforms (`#[cfg(not(windows))]`), portable compatibility stubs are
//! exposed so workspace type-checking (`cargo check --workspace`) and cross-platform
//! hybrid fallback (`wgpu` / Vulkan) function seamlessly.

pub mod error;
pub mod phase;
pub mod win;

#[cfg(windows)]
pub mod frame;
#[cfg(windows)]
pub mod pso;
#[cfg(windows)]
pub mod resources;
#[cfg(windows)]
pub mod swapchain;
#[cfg(windows)]
pub mod visibility;
#[cfg(windows)]
pub mod culling;
#[cfg(windows)]
pub mod voxel;
#[cfg(windows)]
pub mod radiance;
#[cfg(windows)]
pub mod post_process;
#[cfg(windows)]
pub mod vertex;
#[cfg(windows)]
pub mod cpu_culling;
#[cfg(windows)]
pub mod tiled_resources;
#[cfg(windows)]
pub mod sfs;
#[cfg(windows)]
pub mod direct_storage;
#[cfg(windows)]
pub mod agility;
#[cfg(windows)]
pub mod device;
#[cfg(windows)]
pub mod dxc;
#[cfg(windows)]
pub mod multi_view;
#[cfg(windows)]
pub mod conservative_raster;
#[cfg(windows)]
pub mod work_graphs;
#[cfg(windows)]
pub mod terrain_pass;
#[cfg(windows)]
pub mod gpu_graph;

#[cfg(windows)]
pub mod engine;
#[cfg(windows)]
pub use engine::*;
#[cfg(windows)]
pub use frame::*;

pub use error::*;
pub use phase::*;

// ============================================================================
// Non-Windows Portable Compatibility Stubs (`#[cfg(not(windows))]`)
// ============================================================================

#[cfg(not(windows))]
pub struct Dx12Engine {
    pub phase: phase::PhaseStatus,
}

#[cfg(not(windows))]
impl Dx12Engine {
    pub fn create(_caps: rsift_api::engine_caps::EngineCaps) -> error::Dx12Result<Self> {
        Err(error::Dx12Error::FeatureUnavailable(
            "DirectX 12 Agility Explicit Engine is only available on Windows x86_64. Automatically falling back to Vulkan/wgpu.".into(),
        ))
    }

    pub fn phase_report(&self) -> String {
        "dx12=unsupported (fallback_to_vulkan_wgpu)".to_string()
    }
}

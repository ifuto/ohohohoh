//! Rsift DirectX 12 engine — native D3D12 Agility (no OpenGL / Vulkan / wgpu).
//!
//! # Phases
//! | Phase | Module | Description |
//! |-------|--------|-------------|
//! | 0 | `agility` | Agility SDK bootstrap (`D3D12SDKVersion`) |
//! | 1 | `device`, `swapchain` | DXGI adapter, D3D12 device, queues, fences |
//! | 2 | `dxc`, `resources`, `pso` | DXC SM 6.6/6.9, heaps, PSO, root signatures |
//! | 3 | `tiled_resources`, `sfs`, `direct_storage` | Reserved resources, SFS, DirectStorage |
//! | 4 | `multi_view`, `conservative_raster` | View instancing, conservative raster (SM 6.6+) |
//! | 5 | `work_graphs` | D3D12 Work Graphs (SM 6.8+, Agility latest) |
//! | 6 | `terrain_pass`, `engine` | Voxel terrain + frame orchestration |

pub mod error;
#[cfg(windows)]
pub mod win;
pub mod frame;
pub mod pso;
pub mod resources;
pub mod swapchain;
pub mod visibility;
pub mod culling;
pub mod voxel;
pub mod radiance;
pub mod post_process;
pub mod vertex;
pub mod cpu_culling;
pub mod tiled_resources;
pub mod sfs;
pub mod direct_storage;
pub mod phase;
pub mod agility;
pub mod device;
pub mod dxc;
pub mod multi_view;
pub mod conservative_raster;
pub mod work_graphs;
pub mod terrain_pass;
pub mod gpu_graph;
pub mod engine;

pub use error::*;
pub use phase::*;
pub use engine::*;
pub use frame::*;

#[cfg(not(windows))]
compile_error!("rsift-dx12 requires Windows (DirectX 12 Agility SDK).");

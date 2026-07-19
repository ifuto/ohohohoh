//! Engine bootstrap phases — strict ordering for DX12 feature bring-up.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum EnginePhase {
    /// Agility SDK + DXGI factory.
    #[default]
    Bootstrap = 0,
    /// Device, command queue, fence, swap chain.
    CoreDevice = 1,
    /// DXC, upload heaps, default PSO.
    ShadersAndResources = 2,
    /// Tiled/reserved resources + SFS + DirectStorage.
    StreamingResources = 3,
    /// Multi-view rendering + conservative rasterization.
    MultiViewRaster = 4,
    /// Work Graphs (SM 6.8+).
    WorkGraphs = 5,
    /// Terrain voxel pass wired.
    TerrainProduction = 6,
}

impl EnginePhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Bootstrap => "Phase 0: Agility bootstrap",
            Self::CoreDevice => "Phase 1: Core D3D12 device",
            Self::ShadersAndResources => "Phase 2: DXC + resources + PSO",
            Self::StreamingResources => "Phase 3: SFS + tiled + DirectStorage",
            Self::MultiViewRaster => "Phase 4: Multi-view + conservative raster",
            Self::WorkGraphs => "Phase 5: Work Graphs (SM 6.8+)",
            Self::TerrainProduction => "Phase 6: Terrain production",
        }
    }

    pub fn all() -> &'static [EnginePhase] {
        &[
            Self::Bootstrap,
            Self::CoreDevice,
            Self::ShadersAndResources,
            Self::StreamingResources,
            Self::MultiViewRaster,
            Self::WorkGraphs,
            Self::TerrainProduction,
        ]
    }
}

#[derive(Debug, Clone, Default)]
pub struct PhaseStatus {
    pub reached: EnginePhase,
    pub work_graphs_enabled: bool,
    pub sfs_enabled: bool,
    pub direct_storage_enabled: bool,
    pub multi_view_enabled: bool,
    pub conservative_raster_enabled: bool,
}

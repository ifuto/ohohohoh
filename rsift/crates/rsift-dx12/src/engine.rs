//! Rsift DX12 engine — phased bring-up orchestrator.

use crate::agility::AgilityConfig;
use crate::conservative_raster::ConservativeRasterState;
use crate::device::{create_device, Dx12Device};
use crate::direct_storage::DirectStorageQueue;
use crate::dxc::DxcCompiler;
use crate::error::{Dx12Error, Dx12Result};
use crate::multi_view::MultiViewRenderer;
use crate::phase::{EnginePhase, PhaseStatus};
use crate::sfs::{create_sfs, SamplerFeedbackStreaming};
use crate::swapchain::{create_swap_chain, Dx12SwapChain};
use crate::terrain_pass::TerrainRenderPass;
use crate::tiled_resources::{create_reserved_texture, TiledAtlas};
use crate::gpu_graph::FrameGpuGraph;
use crate::work_graphs::WorkGraphPipeline;
use rsift_api::engine_caps::{EngineCaps, RenderBackend};
use tracing::info;

pub struct Dx12Engine {
    pub caps: EngineCaps,
    pub phase: PhaseStatus,
    pub device: Dx12Device,
    pub dxc: DxcCompiler,
    pub swap_chain: Option<Dx12SwapChain>,
    pub tiled_atlas: Option<TiledAtlas>,
    pub sfs: Option<SamplerFeedbackStreaming>,
    pub direct_storage: DirectStorageQueue,
    pub multi_view: Option<MultiViewRenderer>,
    pub conservative: ConservativeRasterState,
    pub work_graphs: Option<WorkGraphPipeline>,
    pub terrain: Option<TerrainRenderPass>,
    pub gpu_graph: Option<FrameGpuGraph>,
}

impl Dx12Engine {
    /// Full phased initialization (Phases 0–6).
    pub fn create(caps: EngineCaps) -> Dx12Result<Self> {
        if caps.render_backend != RenderBackend::Dx12Agility {
            return Err(Dx12Error::Msg(format!(
                "Dx12Engine requires dx12_agility, got {}",
                caps.render_backend.as_str()
            )));
        }

        let mut phase = PhaseStatus::default();
        info!("{}", EnginePhase::Bootstrap.label());
        crate::agility::bootstrap(&AgilityConfig::default())?;
        phase.reached = EnginePhase::Bootstrap;

        info!("{}", EnginePhase::CoreDevice.label());
        let device = create_device(&caps)?;
        phase.reached = EnginePhase::CoreDevice;

        let dxc = DxcCompiler::new(caps.shader_model);

        info!("{}", EnginePhase::ShadersAndResources.label());
        phase.reached = EnginePhase::ShadersAndResources;

        info!("{}", EnginePhase::StreamingResources.label());
        let tiled_atlas = create_reserved_texture(&device, 4096, 4096, 8).ok();
        let sfs = tiled_atlas
            .as_ref()
            .and_then(|a| create_sfs(&device, a).ok());
        let direct_storage = DirectStorageQueue::new();
        phase.sfs_enabled = sfs.as_ref().map(|s| s.enabled).unwrap_or(false);
        phase.direct_storage_enabled = direct_storage.available;
        phase.reached = EnginePhase::StreamingResources;

        info!("{}", EnginePhase::MultiViewRaster.label());
        let multi_view = MultiViewRenderer::new(&device, 1).ok();
        let conservative = ConservativeRasterState::probe(&device)?;
        phase.multi_view_enabled = multi_view.as_ref().map(|m| m.instancing_enabled).unwrap_or(false);
        phase.conservative_raster_enabled = conservative.tier != crate::conservative_raster::ConservativeTier::Off;
        phase.reached = EnginePhase::MultiViewRaster;

        info!("{}", EnginePhase::WorkGraphs.label());
        let work_graphs = WorkGraphPipeline::create(&device, &dxc).ok();
        phase.work_graphs_enabled = work_graphs.as_ref().map(|w| w.enabled).unwrap_or(false);
        phase.reached = EnginePhase::WorkGraphs;

        info!("{}", EnginePhase::TerrainProduction.label());
        let terrain = TerrainRenderPass::new(&device, &dxc).ok();
        let gpu_graph = FrameGpuGraph::create(&device, &dxc).ok();
        if gpu_graph.is_some() {
            info!("[Dx12Engine] FrameGpuGraph live (Hi-Z / ExecuteIndirect / Vis / CMAA2 / RC)");
        }
        phase.reached = EnginePhase::TerrainProduction;

        info!(
            "[Dx12Engine] ready — SM={} phases through {:?}",
            device.shader_model,
            phase.reached
        );

        Ok(Self {
            caps,
            phase,
            device,
            dxc,
            swap_chain: None,
            tiled_atlas,
            sfs,
            direct_storage,
            multi_view,
            conservative,
            work_graphs,
            terrain,
            gpu_graph,
        })
    }

    pub fn from_jvm() -> Dx12Result<Self> {
        let caps = EngineCaps::from_jvm_props()
            .ok_or_else(|| Dx12Error::Msg("missing -Drsift.shader_model JVM property".into()))?;
        Self::create(caps)
    }

    #[cfg(windows)]
    pub fn attach_swap_chain(
        &mut self,
        hwnd: windows::Win32::Foundation::HWND,
        width: u32,
        height: u32,
    ) -> Dx12Result<()> {
        self.swap_chain = Some(create_swap_chain(hwnd, &self.device, width, height)?);
        Ok(())
    }

    pub fn stream_visible_tiles(&mut self, package: &std::path::Path) -> Dx12Result<u32> {
        // Always run ReadFile path; SFS selects tile indices when available.
        let tiles: Vec<u32> = if let Some(sfs) = &self.sfs {
            crate::sfs::tiles_to_stream(sfs, 64)
        } else {
            // Warm first few atlas pages so staging upload is exercised.
            (0..8).collect()
        };
        if tiles.is_empty() {
            return Ok(0);
        }
        self.direct_storage.enqueue_tiles(&tiles, package)?;
        let n = self.direct_storage.flush()?;
        #[cfg(windows)]
        {
            let dest = self
                .gpu_graph
                .as_ref()
                .and_then(|g| g.tile_gpu_dest.as_ref());
            let _ = self.direct_storage.upload_cached_tiles_to_gpu(
                &mut self.device,
                package,
                &tiles,
                dest,
            );
        }
        Ok(n)
    }

    pub fn phase_report(&self) -> String {
        format!(
            "phase={:?} wg={} sfs={} dstorage={} multiview={} consrv={} terrain={}",
            self.phase.reached,
            self.phase.work_graphs_enabled,
            self.phase.sfs_enabled,
            self.phase.direct_storage_enabled,
            self.phase.multi_view_enabled,
            self.phase.conservative_raster_enabled,
            self.terrain.is_some()
        )
    }
}

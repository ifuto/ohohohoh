//! Phase 6 — Voxel terrain render pass (vertex pull + MDI + work graphs).

use crate::conservative_raster::ConservativeRasterState;
use crate::device::Dx12Device;
use crate::dxc::DxcCompiler;
use crate::error::Dx12Result;
use crate::multi_view::MultiViewRenderer;
use crate::pso::{create_terrain_pso, TerrainPso};
use crate::resources::{GpuBuffer, PersistentVboPool};
use crate::win::DXGI_FORMAT_R8G8B8A8_UNORM;
use crate::work_graphs::WorkGraphPipeline;
use tracing::info;

#[cfg(windows)]
use windows::Win32::Graphics::Direct3D12::*;

/// Matches HLSL `FrameCB` / rsift-opt-gfx `TerrainFrameConstants` (20 × f32).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TerrainFrameCb {
    pub view_proj: [[f32; 4]; 4],
    pub chunk_origin: [f32; 4],
}

impl Default for TerrainFrameCb {
    fn default() -> Self {
        Self {
            view_proj: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            chunk_origin: [0.0; 4],
        }
    }
}

pub struct TerrainRenderPass {
    pub pso: TerrainPso,
    pub vbo_pool: PersistentVboPool,
    pub quad_ssbo: GpuBuffer,
    pub work_graphs: WorkGraphPipeline,
    pub multi_view: MultiViewRenderer,
    pub conservative: ConservativeRasterState,
    pub draw_vertex_count: u32,
    pub frame_cb: TerrainFrameCb,
}

impl TerrainRenderPass {
    pub fn new(device: &Dx12Device, dxc: &DxcCompiler) -> Dx12Result<Self> {
        let (vs, ps) = dxc.compile_embedded_terrain()?;
        #[cfg(windows)]
        let pso = create_terrain_pso(device, &vs, &ps, DXGI_FORMAT_R8G8B8A8_UNORM)?;
        #[cfg(not(windows))]
        let pso = create_terrain_pso(device, &vs, &ps, 0)?;

        let vbo_pool = PersistentVboPool::new(device)?;
        // UPLOAD heap so CPU Map works for per-frame quad ingest.
        let quad_ssbo = crate::resources::create_upload_buffer(
            device,
            64 * 1024 * 1024,
            "quad_pull_ssbo",
        )?;
        let work_graphs = WorkGraphPipeline::create(device, dxc)?;
        let multi_view = MultiViewRenderer::new(device, 1)?;
        let conservative = ConservativeRasterState::probe(device)?;

        info!(
            "[TerrainPass] pull SSBO + 1GB VBO + work graphs={}",
            work_graphs.enabled
        );

        Ok(Self {
            pso,
            vbo_pool,
            quad_ssbo,
            work_graphs,
            multi_view,
            conservative,
            draw_vertex_count: 0,
            frame_cb: TerrainFrameCb::default(),
        })
    }

    pub fn set_frame_cb(&mut self, cb: TerrainFrameCb) {
        self.frame_cb = cb;
    }

    pub fn upload_quads(&mut self, quad_bytes: &[u8]) -> Dx12Result<()> {
        crate::resources::upload_slice(&self.quad_ssbo, quad_bytes)?;
        self.draw_vertex_count = (quad_bytes.len() / 8 * 6) as u32;
        Ok(())
    }

    #[cfg(windows)]
    pub fn draw(
        &self,
        cmd: &ID3D12GraphicsCommandList,
        color_rtv: D3D12_CPU_DESCRIPTOR_HANDLE,
        vis_rtv: Option<D3D12_CPU_DESCRIPTOR_HANDLE>,
        dsv: Option<D3D12_CPU_DESCRIPTOR_HANDLE>,
        graph: Option<&crate::gpu_graph::FrameGpuGraph>,
    ) -> Dx12Result<()> {
        unsafe {
            let dsv_ptr = dsv
                .as_ref()
                .map(|h| h as *const D3D12_CPU_DESCRIPTOR_HANDLE);
            if let Some(vis) = vis_rtv {
                let rtvs = [color_rtv, vis];
                cmd.OMSetRenderTargets(2, Some(rtvs.as_ptr()), false, dsv_ptr);
            } else {
                cmd.OMSetRenderTargets(1, Some(&color_rtv), false, dsv_ptr);
            }

            // Viewport/scissor required for rasterization.
            if let Some(g) = graph {
                let vp = D3D12_VIEWPORT {
                    TopLeftX: 0.0,
                    TopLeftY: 0.0,
                    Width: g.width as f32,
                    Height: g.height as f32,
                    MinDepth: 0.0,
                    MaxDepth: 1.0,
                };
                let scissor = windows::Win32::Foundation::RECT {
                    left: 0,
                    top: 0,
                    right: g.width as i32,
                    bottom: g.height as i32,
                };
                cmd.RSSetViewports(&[vp]);
                cmd.RSSetScissorRects(&[scissor]);
            }

            let gpu_va = self.quad_ssbo.resource.GetGPUVirtualAddress();

            self.work_graphs
                .dispatch(cmd, &self.quad_ssbo, self.draw_vertex_count / 6)?;

            cmd.SetPipelineState(&self.pso.pso);
            cmd.SetGraphicsRootSignature(&self.pso.root_signature);
            cmd.SetGraphicsRootShaderResourceView(1, gpu_va);

            self.multi_view.upload_views()?;
            let views = self.multi_view.view_count.max(1);
            for vi in 0..views {
                let mut cb = self.frame_cb;
                if self.multi_view.instancing_enabled {
                    cb.view_proj = self.multi_view.view_proj(vi);
                }
                let constants: [f32; 20] = {
                    let mut out = [0.0f32; 20];
                    let flat: &[f32] = bytemuck::cast_slice(std::slice::from_ref(&cb));
                    out.copy_from_slice(&flat[..20]);
                    out
                };
                cmd.SetGraphicsRoot32BitConstants(0, 20, constants.as_ptr() as *const _, 0);
                if self.draw_vertex_count > 0 {
                    if let Some(g) = graph {
                        g.execute_indirect_draw(cmd)?;
                    } else {
                        cmd.DrawInstanced(self.draw_vertex_count, 1, 0, 0);
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(not(windows))]
pub struct TerrainRenderPass;

//! GPU vertex-pull pipeline — SSBO quads, draw without index buffer.
//!
//! Portable path: `@vertex` pull shader (`terrain_vertex_pull.wgsl`).
//! Meshlet カリング: `terrain_mesh_shader.wgsl` の compute エミュレーションを
//! `frame_worldgen::GpuMeshletCull` が実 dispatch する (mesh shader 自体は
//! WGSL 非対応のためソフトエミュレーションが正)。

use crate::packed4::PackedPullQuad;
use crate::pull_mesh::PullBuiltMesh;
use std::sync::Arc;
use tracing::{debug, info, warn};
use wgpu::util::DeviceExt;

pub const SHADER_VERTEX_PULL: &str = include_str!("../shaders/terrain_vertex_pull.wgsl");
pub const SHADER_MESH_SHADER: &str = include_str!("../shaders/terrain_mesh_shader.wgsl");

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FrameUniforms {
    pub view_proj: [[f32; 4]; 4],
    pub chunk_origin: [f32; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullRenderPath {
    /// SSBO + vertex_index pull (WebGPU / all backends).
    VertexPull,
    /// Task + mesh shader (Vulkan NV/EXT — full WGSL in terrain_mesh_shader.wgsl).
    /// Not selected on wgpu 0.20 / low-spec; VertexPull remains the default.
    MeshShader,
    /// Compute meshlet dispatch + vertex pull indirect.
    TaskEmulation,
}

pub struct GpuVertexPullEngine {
    pull_path: PullRenderPath,
    pull_pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    bind_group_layout: wgpu::BindGroupLayout,
    quads_uploaded: u64,
    draw_calls: u64,
}

impl GpuVertexPullEngine {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let pull_path = Self::detect_path(device);
        info!("[GpuPull] render path = {:?}", pull_path);

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Rsift Pull BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Rsift terrain_vertex_pull"),
            source: wgpu::ShaderSource::Wgsl(SHADER_VERTEX_PULL.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Rsift Pull PL"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pull_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Rsift Pull RP"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_pull",
                buffers: &[], // zero VBO — pure pull
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_pull",
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });

        // 注: 旧 task_pipeline (SHADER_TASK_EMULATION の ComputePipeline) は
        // エンジン初期化のたび実 WGSL コンパイルしながら一度も dispatch されない
        // デッドリソースだったため、フィールド・構築 fn (try_task_pipeline)・
        // 専用シェーダー (terrain_task_emulation.wgsl) ごと除去 (2026-07-21 監査。
        // meshlet カリングは frame_worldgen::GpuMeshletCull 側が別途実 dispatch
        // する設計は維持)。
        let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rsift Pull Uniforms"),
            contents: bytemuck::bytes_of(&FrameUniforms {
                view_proj: glam_like_identity(),
                chunk_origin: [0.0; 4],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        Self {
            pull_path,
            pull_pipeline,
            uniform_buf,
            bind_group_layout,
            quads_uploaded: 0,
            draw_calls: 0,
        }
    }

    fn detect_path(device: &wgpu::Device) -> PullRenderPath {
        let feats = device.features();
        // wgpu 0.20: mesh shaders not in stable Features — use vertex pull.
        // SHADER_MESH_SHADER は破棄せず frame_worldgen::GpuMeshletCull が
        // task/mesh 等価エミュレーションの meshlet カリング pass として実 dispatch
        // する (mesh shader 自体は WGSL 非対応のためソフトエミュレーションが正)。
        let _ = feats;
        PullRenderPath::VertexPull
    }

    pub fn path(&self) -> PullRenderPath {
        self.pull_path
    }

    pub fn stats(&self) -> (u64, u64) {
        (self.quads_uploaded, self.draw_calls)
    }

    /// Upload SSBO + draw without index buffer.
    pub fn draw_pull_mesh(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        mesh: &PullBuiltMesh,
        uniforms: &FrameUniforms,
    ) {
        if mesh.is_empty {
            return;
        }

        let quad_bytes = bytemuck::cast_slice(&mesh.quads);
        let ssbo = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rsift Pull SSBO"),
            contents: quad_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(uniforms));

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Rsift Pull BG"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: ssbo.as_entire_binding(),
                },
            ],
        });

        let vertex_count = mesh.pull_vertex_count();
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Rsift Pull Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pull_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..vertex_count, 0..1);
        }

        self.quads_uploaded += mesh.quads.len() as u64;
        self.draw_calls += 1;
        debug!(
            "[GpuPull] draw ({}, {}) quads={} verts={} path={:?} ssbo={}B ibo=0",
            mesh.chunk_x,
            mesh.chunk_z,
            mesh.quads.len(),
            vertex_count,
            self.pull_path,
            quad_bytes.len()
        );
    }
}

fn glam_like_identity() -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// Ring SSBO pool for pull quads (CPU bookkeeping; GPU upload in `GpuVertexPullEngine`).
#[derive(Debug)]
pub struct PullSsboPool {
    pub capacity_quads: usize,
    pub slots: std::collections::HashMap<(i32, i32), PullPoolSlot>,
    pub quad_cursor: u32,
    pub generation: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct PullPoolSlot {
    pub quad_offset: u32,
    pub quad_count: u32,
    pub pull_vertex_count: u32,
    pub generation: u32,
}

impl PullSsboPool {
    pub fn adaptive() -> Self {
        let hw = rsift_api::AdaptivePerfEngine::hardware();
        let rp = rsift_api::AdaptivePerfEngine::render_profile(hw);
        let mb = rp.bump_arena_mb.max(4);
        let quads = (mb * 1024 * 1024) / PackedPullQuad::memory_bytes();
        Self {
            capacity_quads: quads,
            slots: std::collections::HashMap::new(),
            quad_cursor: 0,
            generation: 0,
        }
    }

    pub fn upload_pull_mesh(&mut self, mesh: &PullBuiltMesh) -> Option<PullPoolSlot> {
        if mesh.is_empty {
            self.slots.remove(&(mesh.chunk_x, mesh.chunk_z));
            return None;
        }
        let qcount = mesh.quads.len() as u32;
        if qcount as usize > self.capacity_quads {
            warn!("[PullSsboPool] chunk ({}, {}) exceeds capacity", mesh.chunk_x, mesh.chunk_z);
            return None;
        }
        if self.quad_cursor + qcount > self.capacity_quads as u32 {
            self.reset_ring();
        }
        let slot = PullPoolSlot {
            quad_offset: self.quad_cursor,
            quad_count: qcount,
            pull_vertex_count: mesh.pull_vertex_count(),
            generation: self.generation,
        };
        self.quad_cursor += qcount;
        self.slots.insert((mesh.chunk_x, mesh.chunk_z), slot);
        Some(slot)
    }

    fn reset_ring(&mut self) {
        self.quad_cursor = 0;
        self.generation = self.generation.wrapping_add(1);
        self.slots.clear();
        debug!("[PullSsboPool] ring reset gen={}", self.generation);
    }
}

/// Lazy global pull engine (initialized when wgpu device is ready).
pub struct PullEngineHandle {
    inner: Option<GpuVertexPullEngine>,
}

impl PullEngineHandle {
    pub fn new() -> Self {
        Self { inner: None }
    }

    pub fn ensure(
        &mut self,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        format: wgpu::TextureFormat,
    ) -> &mut GpuVertexPullEngine {
        if self.inner.is_none() {
            self.inner = Some(GpuVertexPullEngine::new(&device, format));
            let _ = queue;
        }
        self.inner.as_mut().unwrap()
    }
}

impl Default for PullEngineHandle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary_greedy_meshing::{demo_palette, mesh_section_pull};

    #[test]
    fn pull_mesh_no_indices() {
        let p = demo_palette(1, 2);
        let mesh = mesh_section_pull(&p, 1, 2);
        assert!(!mesh.is_empty || mesh.quads.is_empty());
        assert_eq!(mesh.pull_vertex_count(), mesh.quads.len() as u32 * 6);
        if !mesh.quads.is_empty() {
            assert!(mesh.vram_ratio_vs_12b_indexed() > 2.0);
        }
    }
}

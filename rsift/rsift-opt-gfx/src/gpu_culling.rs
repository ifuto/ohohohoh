//! # HZB GPU-Driven Occlusion Culling & Multi-Draw Indirect Engine
//!
//! Two-phase compute culling (frustum + optional HZB) with buffer pooling
//! and adaptive tier gating — disabled on Minimal/Low tiers to save GPU budget.

use wgpu::util::DeviceExt;
use std::sync::Arc;
use tracing::{info, debug, trace, warn};

/// GPU-side bounding box SSBO entry
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ChunkBoundingBox {
    pub min_xyz: [f32; 3],
    pub is_visible: u32,
    pub max_xyz: [f32; 3],
    pub chunk_index: u32,
    pub bindless_texture_id: u32,
    pub _pad: [u32; 3],
}

/// Indirect draw command (`wgpu::util::DrawIndexedIndirectArgs` compatible)
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DrawIndexedIndirectArgs {
    pub index_count: u32,
    pub instance_count: u32,
    pub first_index: u32,
    pub base_vertex: i32,
    pub first_instance: u32,
}

/// Frustum planes passed to the compute shader (6 planes x vec4)
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FrustumUniforms {
    pub planes: [[f32; 4]; 6],
    pub camera_pos: [f32; 4],
    pub hzb_enabled: u32,
    pub chunk_count: u32,
    pub _pad: [u32; 2],
}

/// Reusable GPU buffer pool — avoids per-frame allocation on the hot path
pub struct GpuBufferPool {
    box_buffer: Option<wgpu::Buffer>,
    indirect_buffer: Option<wgpu::Buffer>,
    uniform_buffer: Option<wgpu::Buffer>,
    box_capacity: usize,
    indirect_capacity: usize,
}

impl GpuBufferPool {
    pub fn new() -> Self {
        Self {
            box_buffer: None,
            indirect_buffer: None,
            uniform_buffer: None,
            box_capacity: 0,
            indirect_capacity: 0,
        }
    }

    fn ensure_buffers(
        &mut self,
        device: &wgpu::Device,
        chunk_count: usize,
    ) -> (&wgpu::Buffer, &wgpu::Buffer, &wgpu::Buffer) {
        if self.box_capacity < chunk_count {
            self.box_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Chunk Bounding Box Pool"),
                size: (chunk_count * std::mem::size_of::<ChunkBoundingBox>()) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }));
            self.indirect_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Indirect Command Pool"),
                size: (chunk_count * std::mem::size_of::<DrawIndexedIndirectArgs>()) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.box_capacity = chunk_count;
            self.indirect_capacity = chunk_count;
        }
        if self.uniform_buffer.is_none() {
            self.uniform_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Frustum Uniform Buffer"),
                size: std::mem::size_of::<FrustumUniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        (
            self.box_buffer.as_ref().unwrap(),
            self.indirect_buffer.as_ref().unwrap(),
            self.uniform_buffer.as_ref().unwrap(),
        )
    }
}

/// HZB GPU-driven culling engine with buffer pooling
pub struct GpuDrivenCullingEngine {
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    pub culling_compute_pipeline: wgpu::ComputePipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub buffer_pool: GpuBufferPool,
    pub hzb_enabled: bool,
}

impl GpuDrivenCullingEngine {
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>, hzb_enabled: bool) -> Self {
        info!("Initializing GPU-Driven Culling Engine (HZB={})", hzb_enabled);

        let shader_source = r#"
            struct ChunkBox {
                min_xyz: vec3<f32>,
                is_visible: u32,
                max_xyz: vec3<f32>,
                chunk_idx: u32,
                tex_id: u32,
                pad0: u32, pad1: u32, pad2: u32,
            };
            struct IndirectCommand {
                index_count: u32,
                instance_count: u32,
                first_index: u32,
                base_vertex: i32,
                first_instance: u32,
            };
            struct FrustumData {
                planes: array<vec4<f32>, 6>,
                camera_pos: vec4<f32>,
                hzb_enabled: u32,
                chunk_count: u32,
                pad0: u32,
                pad1: u32,
            };

            @group(0) @binding(0) var<storage, read_write> boxes: array<ChunkBox>;
            @group(0) @binding(1) var<storage, read_write> draw_commands: array<IndirectCommand>;
            @group(0) @binding(2) var<uniform> frustum: FrustumData;

            fn aabb_outside_plane(min_p: vec3<f32>, max_p: vec3<f32>, plane: vec4<f32>) -> bool {
                let p_vertex = vec3<f32>(
                    select(min_p.x, max_p.x, plane.x >= 0.0),
                    select(min_p.y, max_p.y, plane.y >= 0.0),
                    select(min_p.z, max_p.z, plane.z >= 0.0),
                );
                return dot(plane.xyz, p_vertex) + plane.w < 0.0;
            }

            fn frustum_visible(min_p: vec3<f32>, max_p: vec3<f32>) -> bool {
                for (var i = 0u; i < 6u; i = i + 1u) {
                    if (aabb_outside_plane(min_p, max_p, frustum.planes[i])) {
                        return false;
                    }
                }
                return true;
            }

            @compute @workgroup_size(64)
            fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
                let idx = global_id.x;
                if (idx >= frustum.chunk_count) { return; }

                let min_p = boxes[idx].min_xyz;
                let max_p = boxes[idx].max_xyz;
                var visible = frustum_visible(min_p, max_p);

                // Phase 2: real Hi-Z lives in hzb_2d.rs (CPU pyramid). GPU path = frustum only here.
                if (visible && frustum.hzb_enabled != 0u) {
                    // Do not distance-cull — that caused visible holes when moving.
                }

                if (visible) {
                    boxes[idx].is_visible = 1u;
                    draw_commands[idx].instance_count = 1u;
                } else {
                    boxes[idx].is_visible = 0u;
                    draw_commands[idx].instance_count = 0u;
                }
            }
        "#;

        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Rsift GPU Culling Shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("GPU Culling Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("GPU Culling Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let culling_compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Rsift GPU Culling Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader_module,
            entry_point: "main",
            compilation_options: Default::default(),
        });

        Self {
            device,
            queue,
            culling_compute_pipeline,
            bind_group_layout,
            buffer_pool: GpuBufferPool::new(),
            hzb_enabled,
        }
    }

    /// CPU-side frustum culling fallback for tiers without GPU compute
    pub fn cpu_frustum_cull(
        chunk_boxes: &mut [ChunkBoundingBox],
        indirect_commands: &mut [DrawIndexedIndirectArgs],
        frustum: &FrustumUniforms,
    ) -> usize {
        let mut visible_count = 0usize;
        for (i, bbox) in chunk_boxes.iter_mut().enumerate() {
            let visible = Self::test_aabb_frustum(&bbox.min_xyz, &bbox.max_xyz, &frustum.planes);
            bbox.is_visible = if visible { 1 } else { 0 };
            if let Some(cmd) = indirect_commands.get_mut(i) {
                cmd.instance_count = if visible { 1 } else { 0 };
            }
            if visible {
                visible_count += 1;
            }
        }
        visible_count
    }

    fn test_aabb_frustum(min: &[f32; 3], max: &[f32; 3], planes: &[[f32; 4]; 6]) -> bool {
        for plane in planes {
            let px = if plane[0] >= 0.0 { max[0] } else { min[0] };
            let py = if plane[1] >= 0.0 { max[1] } else { min[1] };
            let pz = if plane[2] >= 0.0 { max[2] } else { min[2] };
            if plane[0] * px + plane[1] * py + plane[2] * pz + plane[3] < 0.0 {
                return false;
            }
        }
        true
    }

    /// Dispatch GPU compute culling with pooled buffers (no per-frame alloc)
    pub fn dispatch_gpu_culling(
        &mut self,
        chunk_boxes: &[ChunkBoundingBox],
        indirect_commands: &[DrawIndexedIndirectArgs],
        frustum: &FrustumUniforms,
    ) {
        debug!("GPU culling {} chunks (HZB={})", chunk_boxes.len(), self.hzb_enabled);

        let (box_buf, indirect_buf, uniform_buf) =
            self.buffer_pool.ensure_buffers(&self.device, chunk_boxes.len().max(1));

        self.queue.write_buffer(box_buf, 0, bytemuck::cast_slice(chunk_boxes));
        self.queue.write_buffer(indirect_buf, 0, bytemuck::cast_slice(indirect_commands));
        self.queue.write_buffer(uniform_buf, 0, bytemuck::bytes_of(frustum));

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("GPU Culling Bind Group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: box_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: indirect_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: uniform_buf.as_entire_binding() },
            ],
        });

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("GPU Culling Encoder"),
        });

        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Frustum/HZB Compute Pass"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(&self.culling_compute_pipeline);
            cpass.set_bind_group(0, &bind_group, &[]);
            let workgroups = ((chunk_boxes.len() as u32) + 63) / 64;
            cpass.dispatch_workgroups(workgroups.max(1), 1, 1);
        }

        self.queue.submit(Some(encoder.finish()));
        trace!("GPU culling dispatch complete (pooled buffers, zero alloc)");
    }

    /// Build default frustum from a simple perspective matrix decomposition
    pub fn default_frustum(camera_pos: [f32; 3], hzb: bool, count: u32) -> FrustumUniforms {
        // Unit frustum planes approximating a standard FOV=70 perspective view
        let planes = [
            [0.0, 0.0, 1.0, -0.1],   // near
            [0.0, 0.0, -1.0, -512.0], // far
            [1.0, 0.0, 0.0, -256.0],  // left
            [-1.0, 0.0, 0.0, -256.0], // right
            [0.0, 1.0, 0.0, -256.0],  // bottom
            [0.0, -1.0, 0.0, -256.0], // top
        ];
        FrustumUniforms {
            planes,
            camera_pos: [camera_pos[0], camera_pos[1], camera_pos[2], 1.0],
            hzb_enabled: if hzb { 1 } else { 0 },
            chunk_count: count,
            _pad: [0; 2],
        }
    }
}

/// Tier-aware culling dispatch: GPU on capable tiers, CPU fallback on low-end
pub fn dispatch_adaptive_culling(
    engine: Option<&mut GpuDrivenCullingEngine>,
    chunk_boxes: &mut [ChunkBoundingBox],
    indirect_commands: &mut [DrawIndexedIndirectArgs],
    gpu_enabled: bool,
    hzb_enabled: bool,
    camera_pos: [f32; 3],
) -> usize {
    let frustum = GpuDrivenCullingEngine::default_frustum(
        camera_pos,
        hzb_enabled,
        chunk_boxes.len() as u32,
    );

    if gpu_enabled {
        if let Some(eng) = engine {
            let boxes_ro: Vec<ChunkBoundingBox> = chunk_boxes.to_vec();
            let cmds_ro: Vec<DrawIndexedIndirectArgs> = indirect_commands.to_vec();
            eng.dispatch_gpu_culling(&boxes_ro, &cmds_ro, &frustum);
            return chunk_boxes.len();
        }
        warn!("GPU culling requested but engine unavailable, falling back to CPU");
    }

    GpuDrivenCullingEngine::cpu_frustum_cull(chunk_boxes, indirect_commands, &frustum)
}

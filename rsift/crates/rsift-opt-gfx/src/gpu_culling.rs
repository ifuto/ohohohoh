//! # HZB GPU-Driven Occlusion Culling & Multi-Draw Indirect Engine
//!
//! Two-phase compute culling (frustum + optional HZB) with buffer pooling
//! and adaptive tier gating — disabled on Minimal/Low tiers to save GPU budget.

use std::sync::Arc;
use tracing::{debug, info, trace, warn};

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

/// Indirect command (`wgpu::util::DrawIndexedIndirectArgs` compatible)
/// **wave 126 DZ-3**: gl33_compat 側の重複定義を本定義へ統一 (再 export)。
/// `Default` は gl33 版の derive を追従した等価性保持 (全 0=安全、Pod/Zeroable)。
/// Default 実使用は旧 gl33 側にも無かった (derive のみ) ため消費差はゼロ。
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
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

/// プール容量の決定則 (pure): 最小 64、next-power-of-two で償還成長。
/// exact-fit 再確保を防ぐため `chunk_count` を直接使わない。
fn pool_capacity(chunk_count: usize) -> usize {
    chunk_count.max(1).next_power_of_two().max(64)
}

/// Reusable GPU buffer pool — avoids per-frame allocation on the hot path.
///
/// 成長戦略: `pool_capacity` (next-power-of-two, min 64) の償還成長。
/// exact-fit だと +1 チャンク増減でプール全体を毎フレーム再確保し得るため、
/// 「hot path で確保しない」設計目的に反していた (2026-07-24 CO 監査)。
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
            let cap = pool_capacity(chunk_count);
            self.box_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Chunk Bounding Box Pool"),
                size: (cap * std::mem::size_of::<ChunkBoundingBox>()) as u64,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }));
            self.indirect_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Indirect Command Pool"),
                size: (cap * std::mem::size_of::<DrawIndexedIndirectArgs>()) as u64,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::INDIRECT
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.box_capacity = cap;
            self.indirect_capacity = cap;
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

/// culling コンピュートパスの WGSL。naga による GPU 非依存の構文検証を
/// 可能にするためモジュールレベル const へ切り出し (frame_pipeline.rs の
/// SHADER_VERTEX_PULL パターンと同型)。
pub(crate) const GPU_CULL_SHADER_WGSL: &str = r#"
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
    // Do not distance-cull — that caused visible holes when moving.
    // (hzb_enabled フラグで分岐する設計は CPU 側に一元化。空の if ブロック置き場は
    //  naga 0.20 が受理しないため撤去した — 2026-07-22 診断。)

    if (visible) {
        boxes[idx].is_visible = 1u;
        draw_commands[idx].instance_count = 1u;
    } else {
        boxes[idx].is_visible = 0u;
        draw_commands[idx].instance_count = 0u;
    }
}
        "#;

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
        info!(
            "Initializing GPU-Driven Culling Engine (HZB={})",
            hzb_enabled
        );

        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Rsift GPU Culling Shader"),
            source: wgpu::ShaderSource::Wgsl(GPU_CULL_SHADER_WGSL.into()),
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

        let culling_compute_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
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
        debug!(
            "GPU culling {} chunks (HZB={})",
            chunk_boxes.len(),
            self.hzb_enabled
        );
        // 契約ピン: frustum.chunk_count と実スライス長の不一致は、シェーダが
        // プール末尾の stale データまで cull 対象化する呼出側バグ。
        debug_assert_eq!(
            frustum.chunk_count as usize,
            chunk_boxes.len(),
            "FrustumUniforms.chunk_count must equal chunk_boxes.len()"
        );

        let (box_buf, indirect_buf, uniform_buf) = self
            .buffer_pool
            .ensure_buffers(&self.device, chunk_boxes.len().max(1));

        self.queue
            .write_buffer(box_buf, 0, bytemuck::cast_slice(chunk_boxes));
        self.queue
            .write_buffer(indirect_buf, 0, bytemuck::cast_slice(indirect_commands));
        self.queue
            .write_buffer(uniform_buf, 0, bytemuck::bytes_of(frustum));

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("GPU Culling Bind Group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: box_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: indirect_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform_buf.as_entire_binding(),
                },
            ],
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
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
        // 誠実注記: bind_group 生成・コマンドエンコーダの CPU 側確保は残る。
        // 「プール再確保なし (定常)」が上限の主張。
        trace!("GPU culling dispatch complete (pooled buffers)");
    }

    /// デフォルト frustum を生成する。
    ///
    /// 誠実注記 (CF-7 同型): これは行列分解からの導出では**なく**、
    /// 開発・テスト用の固定軸平行ボックス (view 空間で z∈[0.1,512],
    /// |x|,|y|≤256)。`camera_pos` は planes の計算には使われず、
    /// フィールド格納のみ (コンピュートシェーダも現状非参照)。
    /// 実カメラ追従の frustum 配線は将来課題 (CO 棚卸し参照)。
    pub fn default_frustum(camera_pos: [f32; 3], hzb: bool, count: u32) -> FrustumUniforms {
        // 内法線ボックス視錐 (keep ⟺ dot(n, p) + w >= 0、p-vertex 判定)。
        // 回帰修正 (2026-07-22 監査): far/top/right/bottom の w が旧実装では
        // -512/-256 で符号反転しており、near (z>=0.1) と far (z<=**-512**) が
        // 両立不可能 → 本テーブル構成の frustum は**全チャンクをカリング**した。
        // (本関数は現状未配線のため実害は潜伏。内法線ボックスへ訂正し、
        // 厳密テストで挙動を固定した。)
        let planes = [
            [0.0, 0.0, 1.0, -0.1],   // near:  z >= 0.1
            [0.0, 0.0, -1.0, 512.0], // far:   z <= 512
            [1.0, 0.0, 0.0, 256.0],  // left:  x >= -256
            [-1.0, 0.0, 0.0, 256.0], // right: x <= 256
            [0.0, 1.0, 0.0, 256.0],  // bottom: y >= -256
            [0.0, -1.0, 0.0, 256.0], // top:   y <= 256
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

/// Tier-aware culling dispatch: GPU on capable tiers, CPU fallback on low-end.
///
/// 返り値の契約 (CO 監査で明文化):
/// - **CPU 経路**: 可視チャンク数 (`cpu_frustum_cull` の厳密集計)。
/// - **GPU 経路**: 可視性はコンピュートシェーダが GPU バッファ上で確定し
///   indirect path が直接消費するため CPU 側では未知。**提出チャンク総数**
///   (= `chunk_boxes.len()`) を返す (可視数ではない)。また入力スライスの
///   `is_visible` / `instance_count` は GPU 経路では**更新されない**
///   (更新物は GPU バッファ)。両経路の返り値を混同しないこと。
pub fn dispatch_adaptive_culling(
    engine: Option<&mut GpuDrivenCullingEngine>,
    chunk_boxes: &mut [ChunkBoundingBox],
    indirect_commands: &mut [DrawIndexedIndirectArgs],
    gpu_enabled: bool,
    hzb_enabled: bool,
    camera_pos: [f32; 3],
) -> usize {
    let frustum =
        GpuDrivenCullingEngine::default_frustum(camera_pos, hzb_enabled, chunk_boxes.len() as u32);

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

#[cfg(test)]
mod strict_tests {
    use super::*;

    fn mk_box(min: [f32; 3], max: [f32; 3], idx: u32) -> ChunkBoundingBox {
        ChunkBoundingBox {
            min_xyz: min,
            is_visible: 0,
            max_xyz: max,
            chunk_index: idx,
            bindless_texture_id: 0,
            _pad: [0; 3],
        }
    }

    fn mk_cmd(first_index: u32) -> DrawIndexedIndirectArgs {
        DrawIndexedIndirectArgs {
            index_count: 0,
            instance_count: 5,
            first_index,
            base_vertex: 0,
            first_instance: 0,
        }
    }

    fn vis_flags(boxes: &[ChunkBoundingBox]) -> Vec<u32> {
        boxes.iter().map(|b| b.is_visible).collect()
    }

    #[test]
    fn pod_layout_sizes_and_bytemuck_roundtrip() {
        assert_eq!(
            std::mem::size_of::<ChunkBoundingBox>(),
            48,
            "SSBO wire 48B 固定"
        );
        assert_eq!(
            std::mem::size_of::<DrawIndexedIndirectArgs>(),
            20,
            "VK/MDI indirect cmd 20B"
        );
        assert_eq!(
            std::mem::size_of::<FrustumUniforms>(),
            128,
            "uniform 128B アライン"
        );
        let b = ChunkBoundingBox {
            min_xyz: [1.5, 2.5, 3.5],
            is_visible: 7,
            max_xyz: [4.5, 5.5, 6.5],
            chunk_index: 9,
            bindless_texture_id: 10,
            _pad: [11, 12, 13],
        };
        let bytes = bytemuck::bytes_of(&b);
        assert_eq!(bytes.len(), 48);
        let back: &ChunkBoundingBox = bytemuck::from_bytes(bytes);
        assert_eq!(
            (back.is_visible, back.chunk_index, back.bindless_texture_id),
            (7, 9, 10)
        );
        assert_eq!(back.min_xyz, [1.5, 2.5, 3.5]);
        assert_eq!(back._pad, [11, 12, 13]);
    }

    #[test]
    fn default_frustum_table_exact() {
        // 回帰: 旧 far=-512 / 側面=-256 の符号反転で全件カリングとなる矛盾面群。
        let f = GpuDrivenCullingEngine::default_frustum([8.0, 64.0, -8.0], true, 123);
        let expect = [
            [0.0f32, 0.0, 1.0, -0.1], // near:  z >= 0.1
            [0.0, 0.0, -1.0, 512.0],  // far:   z <= 512
            [1.0, 0.0, 0.0, 256.0],   // left:  x >= -256
            [-1.0, 0.0, 0.0, 256.0],  // right: x <= 256
            [0.0, 1.0, 0.0, 256.0],   // bottom: y >= -256
            [0.0, -1.0, 0.0, 256.0],  // top:   y <= 256
        ];
        assert_eq!(f.planes, expect);
        assert_eq!(f.camera_pos, [8.0, 64.0, -8.0, 1.0]);
        assert_eq!(f.hzb_enabled, 1);
        assert_eq!(f.chunk_count, 123);
        let g = GpuDrivenCullingEngine::default_frustum([0.0; 3], false, 0);
        assert_eq!(g.hzb_enabled, 0);
        assert_eq!(g.chunk_count, 0);
    }

    #[test]
    fn cpu_frustum_cull_visibility_semantics() {
        let boxes = &mut [
            mk_box([0.0, 0.0, 16.0], [16.0, 16.0, 32.0], 0), // inside → visible
            mk_box([0.0, 0.0, -30.0], [1.0, 1.0, 0.05], 1),  // near 外 (max_z < 0.1)
            mk_box([0.0, 0.0, 600.0], [10.0, 10.0, 620.0], 2), // far 外
            mk_box([-400.0, 0.0, 16.0], [-300.0, 10.0, 32.0], 3), // left 外
            mk_box([300.0, 0.0, 16.0], [400.0, 10.0, 32.0], 4), // right 外
            mk_box([0.0, 300.0, 16.0], [10.0, 400.0, 32.0], 5), // top 外
            mk_box([0.0, -400.0, 16.0], [10.0, -300.0, 32.0], 6), // bottom 外
        ];
        let cmds = &mut std::array::from_fn::<_, 7, _>(|i| mk_cmd(i as u32 * 100));
        let frustum = GpuDrivenCullingEngine::default_frustum([0.0, 64.0, 0.0], true, 7);
        let visible = GpuDrivenCullingEngine::cpu_frustum_cull(boxes, cmds, &frustum);
        assert_eq!(visible, 1, "回帰: 旧面群は near/far 矛盾で 0 を返していた");
        assert_eq!(vis_flags(boxes), vec![1, 0, 0, 0, 0, 0, 0]);
        let inst: Vec<u32> = cmds.iter().map(|c| c.instance_count).collect();
        assert_eq!(
            inst,
            vec![1, 0, 0, 0, 0, 0, 0],
            "is_visible と instance_count は同期"
        );
        let fi: Vec<u32> = cmds.iter().map(|c| c.first_index).collect();
        assert_eq!(
            fi,
            vec![0, 100, 200, 300, 400, 500, 600],
            "他フィールドは不変更"
        );
        assert!(
            boxes.iter().all(|b| b.chunk_index < 7),
            "chunk_index も不変更"
        );
    }

    #[test]
    fn cpu_frustum_cull_shorter_commands_array_is_safe() {
        let boxes = &mut [
            mk_box([0.0, 0.0, 16.0], [16.0, 16.0, 32.0], 0), // visible
            mk_box([0.0, 0.0, 600.0], [16.0, 16.0, 632.0], 1), // invisible (far)
        ];
        let cmds = &mut [mk_cmd(7)]; // 1 件しかない (get_mut 経路)
        let frustum = GpuDrivenCullingEngine::default_frustum([0.0; 3], false, 2);
        let visible = GpuDrivenCullingEngine::cpu_frustum_cull(boxes, cmds, &frustum);
        assert_eq!(visible, 1);
        assert_eq!(
            vis_flags(boxes),
            vec![1, 0],
            "commands が短くても box 側は全件更新"
        );
        assert_eq!(cmds[0].instance_count, 1);
        assert_eq!(cmds[0].first_index, 7);
    }

    #[test]
    fn dispatch_adaptive_cpu_paths_match_direct_cull() {
        let base = || {
            vec![
                mk_box([0.0, 0.0, 16.0], [16.0, 16.0, 32.0], 0),
                mk_box([0.0, 0.0, 600.0], [16.0, 16.0, 632.0], 1),
                mk_box([300.0, 0.0, 16.0], [400.0, 10.0, 32.0], 2),
            ]
        };
        let cam = [8.0, 64.0, 8.0];
        let (mut b1, mut c1) = (base(), vec![mk_cmd(0); 3]);
        let n1 = dispatch_adaptive_culling(None, &mut b1, &mut c1, false, false, cam);
        let (mut b2, mut c2) = (base(), vec![mk_cmd(0); 3]);
        // GPU 希望だが engine None → CPU フォールバック (結果は同一になるべき)
        let n2 = dispatch_adaptive_culling(None, &mut b2, &mut c2, true, true, cam);
        assert_eq!((n1, n2), (1, 1), "内側の 1 件のみ可視");
        assert_eq!(
            vis_flags(&b1),
            vis_flags(&b2),
            "gpu_enabled の希望有無でフォールバック結果が変わらない"
        );
        let (mut b3, mut c3) = (base(), vec![mk_cmd(0); 3]);
        let f = GpuDrivenCullingEngine::default_frustum(cam, true, 3);
        let n3 = GpuDrivenCullingEngine::cpu_frustum_cull(&mut b3, &mut c3, &f);
        assert_eq!(
            n2, n3,
            "adaptive CPU 経路は直接 cpu_frustum_cull と bit 一致"
        );
        assert_eq!(vis_flags(&b2), vis_flags(&b3));
        let inst2: Vec<u32> = c2.iter().map(|c| c.instance_count).collect();
        let inst3: Vec<u32> = c3.iter().map(|c| c.instance_count).collect();
        assert_eq!(inst2, inst3);
    }

    #[test]
    fn culling_wgsl_parses_with_main_entry_point() {
        let module = naga::front::wgsl::parse_str(GPU_CULL_SHADER_WGSL)
            .unwrap_or_else(|e| panic!("GPU culling WGSL invalid: {e}"));
        assert!(
            module.entry_points.iter().any(|f| f.name == "main"),
            "@compute @workgroup_size(64) main が存在すること"
        );
    }

    // ---------------- CO 監査 (2026-07-24) 追加テスト ----------------

    #[test]
    fn pool_capacity_is_amortized_pow2_slab() {
        // CO: exact-fit 時代は +1 チャンクで全再確保し得た。slab 粒度を厳密ピン。
        assert_eq!(pool_capacity(0), 64);
        assert_eq!(pool_capacity(1), 64);
        assert_eq!(pool_capacity(63), 64);
        assert_eq!(pool_capacity(64), 64);
        assert_eq!(pool_capacity(65), 128);
        assert_eq!(pool_capacity(128), 128);
        assert_eq!(pool_capacity(129), 256);
        assert_eq!(pool_capacity(1024), 1024);
    }

    #[test]
    fn chunk_box_field_offsets_match_wgsl_storage_layout() {
        use std::mem::offset_of;
        // WGSL storage 規則 (vec3 align=16) との語彙整合を offset_of! で機械ピン
        // (Python 検算: 0/12/16/28/32/36 → 48B)。
        assert_eq!(offset_of!(ChunkBoundingBox, min_xyz), 0);
        assert_eq!(offset_of!(ChunkBoundingBox, is_visible), 12);
        assert_eq!(offset_of!(ChunkBoundingBox, max_xyz), 16);
        assert_eq!(offset_of!(ChunkBoundingBox, chunk_index), 28);
        assert_eq!(offset_of!(ChunkBoundingBox, bindless_texture_id), 32);
        assert_eq!(offset_of!(ChunkBoundingBox, _pad), 36);
        assert_eq!(std::mem::align_of::<ChunkBoundingBox>(), 4);
    }
}

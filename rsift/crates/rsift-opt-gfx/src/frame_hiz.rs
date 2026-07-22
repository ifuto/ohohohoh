//! # GPU Hi-Z Occlusion Culling (`GpuHiz`) — RsGraphics 進化 Phase C
//!
//! 「前フレーム (またはフレーム中間) の深度」から **Hi-Z (64x64 保守的深度
//! ピラミッド)** を GPU 上で構築し、チャンクプロキシ (AABB) 群に対する
//! オクルージョンテストを実 compute dispatch で実行する統合層。
//!
//! ## パイプライン (実パス 3 本 + readback ≤16KB)
//! 1. **Downsample** (`shaders/depth_psychic.wgsl`): 主深度 `Depth32Float`
//!    (`texture_depth_2d` + `textureLoad`) を 64x64 へ **最遠 (max)** 集約。
//! 2. **Raster/Test** (`shaders/hiz_raster.wgsl`): AABB 群を実 `view_proj` で
//!    射影し、NDC 矩形の全セルで「ブロック最手前深度 <= セル最遠深度 + EPS」
//!    をテスト。coverage (可視セル数) / vis (0/1) をバッファに書き出す。
//! 3. **Debug** (`shaders/hiz_debug_view.wgsl`): Hi-Z を RGBA8 に可視化
//!    (`readback_debug` で BMP 化可能 — 検証の目視証拠)。
//! 4. **Resolve**: coverage ≤16KB を readback し、既存 `QueryCore`
//!    (2 フレームレイテンシ・ヒステリシス付き可視判定) に実データとして還元。
//!
//! ## iGPU (内蔵 GPU) 第一級設計 — WebGPU **コア機能のみ**:
//! - storage テクスチャは **write-only のみ** (storage-read/read_write は
//!   `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES` が要るため不使用)。
//! - アトミック / subgroup / bindless / mesh shader / 64bit 演算 不使用。
//! - `@workgroup_size(1)` per-block dispatch (最大 4096 wg) — ワークグループ
//!   共有メモリ不要、ドライバの並列化に素直に載る。
//! - 深度は `texture_depth_2d` + `textureLoad` のみ (サンプラー非使用)。
//!
//! 保守性: 判定不能 (カメラ後方・画面外) は常に「可視 = 描く」側に倒すので
//! 見えるものを誤って消すことはない (vis=1 側が安全側)。
//!
//! CPU 参照ミラー: `frame_reference::hiz_downsample_reference` /
//! `hiz_test_reference` が WGSL と同一規則で動く (GPU/CPU 交叉検証用)。

use crate::frame_pipeline::{read_rgba8, FrameImage};
use crate::occlusion_query::{OcclusionPolicy, QueryBox, QueryCore, VisState};
use crate::packed4::PackedPullQuad;
use crate::pull_mesh::PullBuiltMesh;
use tracing::debug;

/// Hi-Z ダウンサンプル WGSL (実 compute ソース)。
pub const DEPTH_PSYCHIC_WGSL: &str = include_str!("../shaders/depth_psychic.wgsl");
/// Hi-Z ラスタ (AABB テスト) WGSL。
pub const HIZ_RASTER_WGSL: &str = include_str!("../shaders/hiz_raster.wgsl");
/// Hi-Z カラー可視化 WGSL。
pub const HIZ_DEBUG_WGSL: &str = include_str!("../shaders/hiz_debug_view.wgsl");

/// Hi-Z マップの一辺 (64x64 = 4096 セル)。
pub const HIZ_DIM: u32 = 64;
/// 1 回の update で扱えるブロック数上限。coverage readback = 4096 * 4B =
/// **16KB** (iGPU の readback 帯域を圧迫しない実用上の上限)。
pub const MAX_BLOCKS: usize = 4096;

/// 画面寸法契約の純粋検査 (wgpu デバイス不要)。
/// 0 幅/高を許すと downsample の `sw = src/HIZ_DIM` が 0 となり、セル走査が
/// 全て空ループ → Hi-Z が全域 0.0 (最近深度) で埋まり、coverage=0 で
/// **画面が全カリング (真っ黒) になる fail-silent** を招くため明示拒否する。
/// w ≥ 1 かつ h ≥ 1 であれば x1−x0 ≥ 1 が証明できる
/// (`ceil((i+1)·sw) > floor(i·sw)` because `(i+1)·sw > i·sw`)。
pub fn validate_screen_dims(w: u32, h: u32) -> Result<(), String> {
    if w == 0 || h == 0 {
        return Err(format!("Hi-Z screen dims must be non-zero (got {w}x{h})"));
    }
    Ok(())
}

/// `hiz_raster.wgsl` `HizUniforms` と同一レイアウト (mat4x4 + vec2 + vec2 = 80B)。
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct HizUniforms {
    pub view_proj: [[f32; 4]; 4],
    pub hiz_dim: [f32; 2],
    pub _pad: [f32; 2],
}

/// `hiz_raster.wgsl` `BlockBox` と同一レイアウト (2 × vec4 = 32B)。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BlockBox {
    pub lo: [f32; 4],
    pub hi: [f32; 4],
}

impl From<&QueryBox> for BlockBox {
    fn from(b: &QueryBox) -> Self {
        Self {
            lo: [b.min[0], b.min[1], b.min[2], 0.0],
            hi: [b.max[0], b.max[1], b.max[2], 0.0],
        }
    }
}

/// GPU Hi-Z カリングパス。深度 → 64x64 Hi-Z → AABB coverage → `QueryCore`。
pub struct GpuHiz {
    pub screen_w: u32,
    pub screen_h: u32,
    hiz_tex: wgpu::Texture,
    hiz_view: wgpu::TextureView,
    debug_tex: wgpu::Texture,
    debug_view: wgpu::TextureView,
    debug_readback: wgpu::Buffer,
    blocks_buf: wgpu::Buffer,
    cov_buf: wgpu::Buffer,
    /// vis (coverage>0 の 0/1 判定) 出力バッファ。readback は coverage のみだが
    /// WGSL 契約 (binding 4) として実バッファを保持し raster パスに実バインドする。
    vis_buf: wgpu::Buffer,
    cov_staging: wgpu::Buffer,
    down_params: wgpu::Buffer,
    raster_params: wgpu::Buffer,
    dbg_params: wgpu::Buffer,
    down_pipeline: wgpu::ComputePipeline,
    down_bgl: wgpu::BindGroupLayout,
    raster_pipeline: wgpu::ComputePipeline,
    raster_bgl: wgpu::BindGroupLayout,
    dbg_pipeline: wgpu::ComputePipeline,
    dbg_bgl: wgpu::BindGroupLayout,
    core: QueryCore,
    last_coverage: Vec<u32>,
}

impl GpuHiz {
    /// 既定ポリシー (`OcclusionPolicy::default()`) で構築。
    pub fn new(device: &wgpu::Device, screen_w: u32, screen_h: u32) -> Self {
        Self::with_policy(device, screen_w, screen_h, OcclusionPolicy::default())
    }

    /// Hi-Z テクスチャ本体 (r32float 64x64)。`hiz_view` の元で、
    /// パイプライン実行中のライフタイム保持と検査のために公開する。
    pub fn hiz_texture(&self) -> &wgpu::Texture {
        &self.hiz_tex
    }

    pub fn with_policy(
        device: &wgpu::Device,
        screen_w: u32,
        screen_h: u32,
        policy: OcclusionPolicy,
    ) -> Self {
        validate_screen_dims(screen_w, screen_h).unwrap_or_else(|e| panic!("GpuHiz: {e}"));
        let dim = HIZ_DIM;
        let hiz_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Rsift Hi-Z 64x64"),
            size: wgpu::Extent3d {
                width: dim,
                height: dim,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            // downsample が write、raster/debug が textureLoad で読む (両者とも
            // コア機能)。COPY_SRC は将来の直接 readback 用ヘッドルーム。
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let debug_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Rsift Hi-Z DebugView"),
            size: wgpu::Extent3d {
                width: dim,
                height: dim,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let hiz_view = hiz_tex.create_view(&Default::default());
        let debug_view = debug_tex.create_view(&Default::default());
        let debug_readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift Hi-Z Debug Readback"),
            // 64 * 4B = 256B は既に 256B アライン済み
            size: (dim * dim * 4) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let blocks_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift Hi-Z Blocks"),
            size: (MAX_BLOCKS * std::mem::size_of::<BlockBox>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cov_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift Hi-Z Coverage"),
            size: (MAX_BLOCKS * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let vis_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift Hi-Z Vis"),
            size: (MAX_BLOCKS * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let cov_staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift Hi-Z Coverage Staging"),
            size: (MAX_BLOCKS * 4) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mk_uniform = |label: &str, size: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let down_params = mk_uniform("Rsift Hi-Z Down Params", 16); // src_dim + pad
        let raster_params = mk_uniform(
            "Rsift Hi-Z Raster Params",
            std::mem::size_of::<HizUniforms>() as u64,
        );
        let dbg_params = mk_uniform("Rsift Hi-Z Debug Params", 16); // tint

        let down_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Rsift Depth Psychic WGSL"),
            source: wgpu::ShaderSource::Wgsl(DEPTH_PSYCHIC_WGSL.into()),
        });
        let raster_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Rsift Hi-Z Raster WGSL"),
            source: wgpu::ShaderSource::Wgsl(HIZ_RASTER_WGSL.into()),
        });
        let dbg_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Rsift Hi-Z Debug WGSL"),
            source: wgpu::ShaderSource::Wgsl(HIZ_DEBUG_WGSL.into()),
        });

        let uniform_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };

        // --- downsample BGL: 0=Params / 1=src depth / 2=dst Hi-Z (write storage)
        let down_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Rsift Hi-Z Down BGL"),
            entries: &[
                uniform_entry(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    // texture_depth_2d に対応 (textureLoad のみ、サンプラー不要)
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::R32Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });

        // --- raster BGL: 0=Uniforms / 1=Hi-Z tex / 2=blocks RO / 3=coverage / 4=vis
        let raster_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Rsift Hi-Z Raster BGL"),
            entries: &[
                uniform_entry(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        // R32Float は非フィルタ → textureLoad 専用で filterable:false
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        // --- debug BGL: 0=Params(tint) / 1=Hi-Z tex / 2=out RGBA8 (write storage)
        let dbg_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Rsift Hi-Z Debug BGL"),
            entries: &[
                uniform_entry(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });

        let mk_pipeline = |label: &str,
                           bgl: &wgpu::BindGroupLayout,
                           module: &wgpu::ShaderModule|
         -> wgpu::ComputePipeline {
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[bgl],
                push_constant_ranges: &[],
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                module,
                entry_point: "main",
                compilation_options: Default::default(),
            })
        };
        let down_pipeline = mk_pipeline("Rsift Hi-Z Down Pipeline", &down_bgl, &down_module);
        let raster_pipeline =
            mk_pipeline("Rsift Hi-Z Raster Pipeline", &raster_bgl, &raster_module);
        let dbg_pipeline = mk_pipeline("Rsift Hi-Z Debug Pipeline", &dbg_bgl, &dbg_module);

        Self {
            screen_w,
            screen_h,
            hiz_tex,
            hiz_view,
            debug_tex,
            debug_view,
            debug_readback,
            blocks_buf,
            cov_buf,
            vis_buf,
            cov_staging,
            down_params,
            raster_params,
            dbg_params,
            down_pipeline,
            down_bgl,
            raster_pipeline,
            raster_bgl,
            dbg_pipeline,
            dbg_bgl,
            core: QueryCore::new(policy),
            last_coverage: Vec::new(),
        }
    }

    /// Hi-Z 構築 + AABB テスト + coverage readback + `QueryCore` 解決を
    /// 1 フレーム分まとめて実行 (実 3 dispatch + 実 readback)。
    ///
    /// `main_depth`: 深度構築元 (`GpuFramePipeline::depth_view` 等、
    /// Depth32Float / TEXTURE_BINDING usage 必須)。
    /// 戻り値は index 整列の per-box coverage (可視セル数)。
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        main_depth: &wgpu::TextureView,
        boxes: &[QueryBox],
        view_proj: [[f32; 4]; 4],
    ) -> Result<Vec<u32>, String> {
        if boxes.len() > MAX_BLOCKS {
            return Err(format!(
                "Hi-Z ブロック数 {} が上限 {} を超過 (readback 帯域保護)",
                boxes.len(),
                MAX_BLOCKS
            ));
        }
        self.core.set_boxes(boxes);

        // Params 実更新 (WGSL struct レイアウト一致)
        let down: [f32; 4] = [self.screen_w as f32, self.screen_h as f32, 0.0, 0.0];
        queue.write_buffer(&self.down_params, 0, bytemuck::cast_slice(&down));
        let uniforms = HizUniforms {
            view_proj,
            hiz_dim: [HIZ_DIM as f32, HIZ_DIM as f32],
            _pad: [0.0; 2],
        };
        queue.write_buffer(&self.raster_params, 0, bytemuck::bytes_of(&uniforms));
        let tint: [f32; 4] = [0.3, 0.9, 1.0, 1.0]; // シアン系
        queue.write_buffer(&self.dbg_params, 0, bytemuck::cast_slice(&tint));
        if !boxes.is_empty() {
            let packed: Vec<BlockBox> = boxes.iter().map(BlockBox::from).collect();
            queue.write_buffer(&self.blocks_buf, 0, bytemuck::cast_slice(&packed));
        }

        let down_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Rsift Hi-Z Down BG"),
            layout: &self.down_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.down_params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(main_depth),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&self.hiz_view),
                },
            ],
        });
        let dbg_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Rsift Hi-Z Debug BG"),
            layout: &self.dbg_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.dbg_params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.hiz_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&self.debug_view),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Rsift Hi-Z Encoder"),
        });
        {
            let mut cp = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Rsift Hi-Z Downsample Pass"),
                timestamp_writes: None,
            });
            cp.set_pipeline(&self.down_pipeline);
            cp.set_bind_group(0, &down_bg, &[]);
            cp.dispatch_workgroups(HIZ_DIM / 8, HIZ_DIM / 8, 1);
        }
        if !boxes.is_empty() {
            let raster_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Rsift Hi-Z Raster BG"),
                layout: &self.raster_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.raster_params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&self.hiz_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.blocks_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self.cov_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: self.vis_buf.as_entire_binding(),
                    },
                ],
            });
            {
                let mut cp = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("Rsift Hi-Z Raster Pass"),
                    timestamp_writes: None,
                });
                cp.set_pipeline(&self.raster_pipeline);
                cp.set_bind_group(0, &raster_bg, &[]);
                // 1 スレッド = 1 ブロック (@workgroup_size(1))
                cp.dispatch_workgroups(boxes.len() as u32, 1, 1);
            }
            encoder.copy_buffer_to_buffer(
                &self.cov_buf,
                0,
                &self.cov_staging,
                0,
                (boxes.len() * 4) as u64,
            );
        }
        {
            let mut cp = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Rsift Hi-Z Debug Pass"),
                timestamp_writes: None,
            });
            cp.set_pipeline(&self.dbg_pipeline);
            cp.set_bind_group(0, &dbg_bg, &[]);
            cp.dispatch_workgroups(HIZ_DIM / 8, HIZ_DIM / 8, 1);
        }
        queue.submit([encoder.finish()]);

        let covered = if boxes.is_empty() {
            Vec::new()
        } else {
            read_u32(device, &self.cov_staging, boxes.len())
        };
        let changed = self.core.resolve(&covered);
        debug!(
            "[Hi-Z] boxes={} changed={} coverage(first8)={:?}",
            boxes.len(),
            changed,
            &covered[..covered.len().min(8)]
        );
        self.last_coverage = covered.clone();
        Ok(covered)
    }

    /// Hi-Z カラー可視化を実画素 readback (検証の目視証拠: BMP 化に使う)。
    /// `update` 後に呼ぶこと (debug パスは update 内で実 dispatch 済み)。
    pub fn readback_debug(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> FrameImage {
        let dim = HIZ_DIM;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Rsift Hi-Z Debug Copy Encoder"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &self.debug_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &self.debug_readback,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(dim * 4), // 256B アライン済み
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: dim,
                height: dim,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        let pixels = read_rgba8(device, &self.debug_readback, dim * 4, dim, dim);
        FrameImage {
            width: dim,
            height: dim,
            pixels,
            draw_calls: 1, // debug dispatch
            quads: 0,
        }
    }

    // --- QueryCore への委譲 (カリング効果の実還元面) ---

    /// ブロック `id` をこのフレーム描くべきか (Unknown ⇒ 描く = 保守側)。
    pub fn should_draw(&self, id: usize) -> bool {
        self.core.should_draw(id)
    }

    pub fn state(&self, id: usize) -> VisState {
        self.core.state(id)
    }

    /// (draw 数, culled 数)。
    pub fn stats(&self) -> (usize, usize) {
        self.core.stats()
    }

    /// 直近 update の生 coverage (可視 Hi-Z セル数 / 箱)。
    pub fn last_coverage(&self) -> &[u32] {
        &self.last_coverage
    }
}

/// u32 バッファの実 readback (coverage ≤16KB 用)。`read_rgba8` と同一の
/// map + `Maintain::Wait` ポーリング規則。
fn read_u32(device: &wgpu::Device, buf: &wgpu::Buffer, count: usize) -> Vec<u32> {
    let slice = buf.slice(..(count * 4) as u64);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    while rx.try_recv().is_err() {
        let _ = device.poll(wgpu::Maintain::Wait);
    }
    let data = slice.get_mapped_range();
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let s = i * 4;
        out.push(u32::from_le_bytes([
            data[s],
            data[s + 1],
            data[s + 2],
            data[s + 3],
        ]));
    }
    drop(data);
    buf.unmap();
    out
}

/// メッシュ 1 個の実 quad 群からワールド AABB を厳密算出。
///
/// `corner_pos` (frame_reference / terrain_vertex_pull.wgsl と同一の face 別
/// 展開規則) に基づき、quad 1 枚の占有矩形を face 別に正確に範囲化する:
/// - ±X (0/1): [o ± 1, o] x [o.y, o.y+h] x [o.z, o.z+w]
/// - ±Y (2/3): [o.x, o.x+w] x [o ± 1, o] x [o.z, o.z+h]
/// - ±Z (4/5): [o.x, o.x+w] x [o.y, o.y+h] x [o ± 1, o]
///
/// (`unpack_width/height` は +1 デコード済みの実ブロック幅を返す)
///
/// `origin` は描画時の chunk_origin と同一値 (ワールド座標の基準点)。
pub fn aabb_from_mesh(mesh: &PullBuiltMesh, origin: [f32; 3]) -> QueryBox {
    if mesh.quads.is_empty() {
        // 体積ゼロ箱 (判定は保守的可視側に倒れるので害無し)
        return QueryBox {
            min: origin,
            max: origin,
        };
    }
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for q in &mesh.quads {
        let o = [
            PackedPullQuad::unpack_x(q.word0) as f32 + origin[0],
            PackedPullQuad::unpack_y(q.word0) as f32 + origin[1],
            PackedPullQuad::unpack_z(q.word0) as f32 + origin[2],
        ];
        let face = PackedPullQuad::unpack_face(q.word1);
        let w = PackedPullQuad::unpack_width(q.word1) as f32;
        let h = PackedPullQuad::unpack_height(q.word1) as f32;
        let (d0, d1) = match face {
            0 => ([1.0, 0.0, 0.0], [1.0, h, w]),
            1 => ([0.0, 0.0, 0.0], [0.0, h, w]),
            2 => ([0.0, 1.0, 0.0], [w, 1.0, h]),
            3 => ([0.0, 0.0, 0.0], [w, 0.0, h]),
            4 => ([0.0, 0.0, 1.0], [w, h, 1.0]),
            _ => ([0.0, 0.0, 0.0], [w, h, 0.0]),
        };
        for axis in 0..3 {
            lo[axis] = lo[axis].min(o[axis] + d0[axis]);
            hi[axis] = hi[axis].max(o[axis] + d1[axis]);
        }
    }
    QueryBox { min: lo, max: hi }
}

/// チャンク列 (mesh, origin) から index 整列の AABB 群を算出。
pub fn aabb_from_meshes(chunks: &[(PullBuiltMesh, [f32; 3])]) -> Vec<QueryBox> {
    chunks.iter().map(|(m, o)| aabb_from_mesh(m, *o)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Phase C WGSL 3 本の実妥当性 + エントリ実在 (naga、GPU 不要の真の検証)。
    #[test]
    fn hiz_shaders_parse() {
        for (name, src) in [
            ("depth_psychic", DEPTH_PSYCHIC_WGSL),
            ("hiz_raster", HIZ_RASTER_WGSL),
            ("hiz_debug_view", HIZ_DEBUG_WGSL),
        ] {
            let module = naga::front::wgsl::parse_str(src)
                .unwrap_or_else(|e| panic!("{name} WGSL invalid: {e}"));
            assert!(
                module.entry_points.iter().any(|f| f.name == "main"),
                "{name}: entry point 'main' not found"
            );
        }
    }

    /// WGSL struct とのレイアウト一致性 (uniform 80B / block 32B)。
    #[test]
    fn hiz_struct_layout_matches_wgsl() {
        assert_eq!(std::mem::size_of::<HizUniforms>(), 80);
        assert_eq!(std::mem::size_of::<BlockBox>(), 32);
        assert_eq!(MAX_BLOCKS * 4, 16 * 1024, "coverage readback must be 16KB");
    }

    /// naga 計算の WGSL offset が Rust repr(C) と逐語一致すること
    /// (size 一致のみでは検出できない member 順/pad 位置ドリフトを恒久検出)。
    #[test]
    fn hiz_struct_offsets_match_wgsl_exact() {
        use std::mem::offset_of;
        assert_eq!(offset_of!(HizUniforms, view_proj), 0);
        assert_eq!(offset_of!(HizUniforms, hiz_dim), 64);
        assert_eq!(offset_of!(HizUniforms, _pad), 72);
        assert_eq!(offset_of!(BlockBox, lo), 0);
        assert_eq!(offset_of!(BlockBox, hi), 16);

        let module = naga::front::wgsl::parse_str(HIZ_RASTER_WGSL)
            .unwrap_or_else(|e| panic!("hiz_raster WGSL invalid: {e}"));
        let find = |name: &str| {
            module
                .types
                .iter()
                .find_map(|(_, t)| {
                    if t.name.as_deref() == Some(name) {
                        let naga::TypeInner::Struct { members, span } = &t.inner else {
                            panic!("{name} must be struct");
                        };
                        Some((
                            members
                                .iter()
                                .map(|m| (m.name.clone().unwrap_or_default(), m.offset))
                                .collect::<Vec<_>>(),
                            *span,
                        ))
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| panic!("struct {name} must exist"))
        };
        let (u, uspan) = find("HizUniforms");
        assert_eq!(uspan, 80);
        assert_eq!(
            u,
            vec![
                ("view_proj".to_string(), 0),
                ("hiz_dim".to_string(), 64),
                ("_pad".to_string(), 72),
            ]
        );
        let (b, bspan) = find("BlockBox");
        assert_eq!(bspan, 32);
        assert_eq!(b, vec![("lo".to_string(), 0), ("hi".to_string(), 16)]);
    }

    /// 画面寸法契約: 受理/拒否の境界。
    #[test]
    fn validate_screen_dims_bounds() {
        assert!(super::validate_screen_dims(1, 1).is_ok());
        assert!(super::validate_screen_dims(640, 480).is_ok());
        assert!(super::validate_screen_dims(1, 4096).is_ok());
        assert!(super::validate_screen_dims(0, 480).is_err());
        assert!(super::validate_screen_dims(640, 0).is_err());
        assert!(super::validate_screen_dims(0, 0).is_err());
    }

    /// face 別厳密 AABB: +Y 上面 4x2 矩形の占位を実 quad から検証。
    #[test]
    fn aabb_face_ranges_are_exact() {
        let q = PackedPullQuad::new(3, 5, 7, 0, 0, 2, 4, 2); // +Y, w=4 h=2
        let mesh = PullBuiltMesh {
            chunk_x: 0,
            chunk_z: 0,
            quads: vec![q],
            is_empty: false,
        };
        let b = aabb_from_mesh(&mesh, [0.0, 0.0, 0.0]);
        assert_eq!(b.min, [3.0, 6.0, 7.0]);
        assert_eq!(b.max, [7.0, 6.0, 9.0]);
        // origin シフトがそのまま効くこと
        let b2 = aabb_from_mesh(&mesh, [16.0, 32.0, -8.0]);
        assert_eq!(b2.min, [19.0, 38.0, -1.0]);
        assert_eq!(b2.max, [23.0, 38.0, 1.0]);
    }

    /// -X 面の法線側固定面も厳密 (x は o.x..o.x の厚さゼロ)。
    #[test]
    fn aabb_negative_face_zero_thickness() {
        let q = PackedPullQuad::new(2, 0, 4, 0, 0, 1, 3, 3);
        let mesh = PullBuiltMesh {
            chunk_x: 0,
            chunk_z: 0,
            quads: vec![q],
            is_empty: false,
        };
        let b = aabb_from_mesh(&mesh, [0.0, 0.0, 0.0]);
        assert_eq!(b.min, [2.0, 0.0, 4.0]);
        assert_eq!(b.max, [2.0, 3.0, 7.0]);
    }
}

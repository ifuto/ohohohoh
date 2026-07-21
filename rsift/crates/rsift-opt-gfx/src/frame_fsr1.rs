//! # FSR1 GPU 実パス (`GpuFsr1Pass`) — RsGraphics 進化 Phase B
//!
//! 既存資産 `fsr1.rs` (CPU 参照) / `shaders/fsr1.wgsl` (実 compute シェーダ:
//! `fsr_easu` エッジ感知アップサンプル + `fsr_rcas` コントラスト適応シャープ化) を
//! **実 wgpu dispatch として配線**する (これまでは CPU チェーン内呼び出しのみで、
//! GPU では未実行だった)。
//!
//! ## チェーン (実パス駆動)
//! 1. 低解像度 (例 0.7x / 0.5x) で `GpuFramePipeline::record_to_ldr` →
//!    LDR `Rgba8Unorm` 低解像度テクスチャ。
//! 2. **EASU**: 低解像度 LDR をサンプルし、輝度勾配 (R チャンネル — WGSL 準拠) で
//!    エッジ方向を検出 → 全解像度の中間テクスチャ (`inter`) へ再構成。
//! 3. **RCAS**: `inter` に contrast-adaptive シャープを掛け最終 LDR へ。
//! 4. `copy_texture_to_buffer` + map で実画素 readback。
//!
//! WGSL は `fsr1.rs` の CPU 参照と同一数学 (EASU 勾配・位置寄せ・RCAS ラプラシアン)。
//! CPU 側ミラーは `frame_reference::fsr1_reference` — GPU/CPU 突合検証に使う。

use crate::frame_pipeline::{read_rgba8, FrameImage};
use tracing::debug;

/// 既定シャープネス (`full_graph_wiring` の `Fsr1 { sharpness: 0.2 }` と同値)。
pub const DEFAULT_SHARPNESS: f32 = 0.2;

/// FSR1 (EASU + RCAS) の実 GPU パス。低解像度 LDR → 全解像度 LDR + readback。
pub struct GpuFsr1Pass {
    pub full_w: u32,
    pub full_h: u32,
    pub low_w: u32,
    pub low_h: u32,
    pub sharpness: f32,
    // 注: 旧 `inter: wgpu::Texture` は view が内部参照を保持するため保持不要かつ
    // 一度も読まれないデッド状態だった (frame_pipeline hdr/depth と同根 — 2026-07-21 監査)。
    inter_view: wgpu::TextureView,
    final_tex: wgpu::Texture,
    final_view: wgpu::TextureView,
    readback: wgpu::Buffer,
    params_buf: wgpu::Buffer,
    sampler: wgpu::Sampler,
    easu_pipeline: wgpu::ComputePipeline,
    easu_bgl: wgpu::BindGroupLayout,
    rcas_pipeline: wgpu::ComputePipeline,
    rcas_bgl: wgpu::BindGroupLayout,
}

impl GpuFsr1Pass {
    pub fn new(device: &wgpu::Device, low_w: u32, low_h: u32, full_w: u32, full_h: u32) -> Self {
        Self::with_sharpness(device, low_w, low_h, full_w, full_h, DEFAULT_SHARPNESS)
    }

    pub fn with_sharpness(
        device: &wgpu::Device,
        low_w: u32,
        low_h: u32,
        full_w: u32,
        full_h: u32,
        sharpness: f32,
    ) -> Self {
        let mk = |format: wgpu::TextureFormat, usage: wgpu::TextureUsages, label: &str| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: full_w,
                    height: full_h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        // inter: EASU が storage-write → RCAS が texture-read (同一パス内で併用しないため合法)
        let inter = mk(
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            "Rsift FSR1 Inter",
        );
        let final_tex = mk(
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            "Rsift FSR1 Final",
        );
        let inter_view = inter.create_view(&Default::default());
        let final_view = final_tex.create_view(&Default::default());

        let padded = (full_w * 4).div_ceil(256) * 256;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift FSR1 Readback"),
            size: padded as u64 * full_h as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // WGSL `struct Params { inputSize, outputSize, sharpness, _pad }` = 24B
        let params_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift FSR1 Params"),
            size: 24,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Rsift FSR1 Sampler"),
            // EASU は (base+0.5)/inputSize の正確なテクセル中心を読むため Nearest。
            // (線形でも中心位置では同一値だが、規約を明示する)
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Rsift FSR1 WGSL"),
            source: wgpu::ShaderSource::Wgsl(crate::fsr1::FSR1_WGSL.into()),
        });

        let uniform_entry = |vis: wgpu::ShaderStages| wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: vis,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let tex_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let storage_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: wgpu::TextureFormat::Rgba8Unorm,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
            count: None,
        };
        let sampler_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };

        // EASU: 0=Params, 1=srcTex, 2=srcSamp, 3=dstTex(storage)
        let easu_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Rsift FSR1 EASU BGL"),
            entries: &[
                uniform_entry(wgpu::ShaderStages::COMPUTE),
                tex_entry(1),
                sampler_entry(2),
                storage_entry(3),
            ],
        });
        // RCAS: 0=Params, 4=casTex, 5=casOut(storage)
        let rcas_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Rsift FSR1 RCAS BGL"),
            entries: &[
                uniform_entry(wgpu::ShaderStages::COMPUTE),
                tex_entry(4),
                storage_entry(5),
            ],
        });
        let easu_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Rsift FSR1 EASU PL"),
            bind_group_layouts: &[&easu_bgl],
            push_constant_ranges: &[],
        });
        let rcas_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Rsift FSR1 RCAS PL"),
            bind_group_layouts: &[&rcas_bgl],
            push_constant_ranges: &[],
        });
        let easu_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Rsift FSR1 EASU Pipeline"),
            layout: Some(&easu_pl),
            module: &module,
            entry_point: "fsr_easu",
            compilation_options: Default::default(),
        });
        let rcas_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Rsift FSR1 RCAS Pipeline"),
            layout: Some(&rcas_pl),
            module: &module,
            entry_point: "fsr_rcas",
            compilation_options: Default::default(),
        });

        Self {
            full_w,
            full_h,
            low_w,
            low_h,
            sharpness,
            inter_view,
            final_tex,
            final_view,
            readback,
            params_buf,
            sampler,
            easu_pipeline,
            easu_bgl,
            rcas_pipeline,
            rcas_bgl,
        }
    }

    /// 低解像度 LDR (`src_low`) を全解像度へ EASU + RCAS で実拡大し、
    /// 実画素 (RGBA8, full_w x full_h) を返す。2 dispatch。
    pub fn render_full(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        src_low: &wgpu::TextureView,
    ) -> Result<FrameImage, String> {
        // Params ユニフォームを実値で更新 (WGSL struct レイアウト 24B)
        let params: [f32; 6] = [
            self.low_w as f32,
            self.low_h as f32,
            self.full_w as f32,
            self.full_h as f32,
            self.sharpness,
            0.0,
        ];
        queue.write_buffer(&self.params_buf, 0, bytemuck::cast_slice(&params));

        let easu_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Rsift FSR1 EASU BG"),
            layout: &self.easu_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(src_low),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&self.inter_view),
                },
            ],
        });
        let rcas_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Rsift FSR1 RCAS BG"),
            layout: &self.rcas_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&self.inter_view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&self.final_view),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Rsift FSR1 Encoder"),
        });
        {
            let mut cp = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Rsift FSR1 EASU Pass"),
                timestamp_writes: None,
            });
            cp.set_pipeline(&self.easu_pipeline);
            cp.set_bind_group(0, &easu_bg, &[]);
            cp.dispatch_workgroups(self.full_w.div_ceil(8), self.full_h.div_ceil(8), 1);
        }
        {
            let mut cp = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Rsift FSR1 RCAS Pass"),
                timestamp_writes: None,
            });
            cp.set_pipeline(&self.rcas_pipeline);
            cp.set_bind_group(0, &rcas_bg, &[]);
            cp.dispatch_workgroups(self.full_w.div_ceil(8), self.full_h.div_ceil(8), 1);
        }

        let padded = (self.full_w * 4).div_ceil(256) * 256;
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &self.final_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &self.readback,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: self.full_w,
                height: self.full_h,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        let pixels = read_rgba8(device, &self.readback, padded, self.full_w, self.full_h);
        debug!(
            "[FSR1] {}x{} -> {}x{} sharpness={} pixels={}B",
            self.low_w,
            self.low_h,
            self.full_w,
            self.full_h,
            self.sharpness,
            pixels.len()
        );
        Ok(FrameImage {
            width: self.full_w,
            height: self.full_h,
            pixels,
            draw_calls: 2, // EASU + RCAS の 2 compute dispatch
            quads: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    /// FSR1 WGSL の実妥当性 + 両エントリポイント実在 (naga、GPU 不要)。
    #[test]
    fn fsr1_wgsl_parses_with_entry_points() {
        let module = naga::front::wgsl::parse_str(crate::fsr1::FSR1_WGSL)
            .unwrap_or_else(|e| panic!("fsr1 WGSL invalid: {e}"));
        for ep in ["fsr_easu", "fsr_rcas"] {
            assert!(
                module.entry_points.iter().any(|f| f.name == ep),
                "entry point '{ep}' not found"
            );
        }
    }
}

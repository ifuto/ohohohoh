//! # Frame Completion Pipeline (`GpuFramePipeline`) — RsGraphics 進化 Phase A
//!
//! これまで独立実装だったパス群を「深度付き実フレーム」に束ねる統合層。
//!
//! ## パイプライン構成 (全て実 wgpu オブジェクト)
//! 1. **Raster**: `terrain_vertex_pull.wgsl` を深度 (Depth32Float) 付きで
//!    HDR (`Rgba16Float`) ターゲットに実描画 (既存 `GpuVertexPullEngine` は
//!    `depth_stencil: None` のサーフェス直結パス — 壊さないよう本モジュールで
//!    深度対応パイプラインを別建てする)。
//! 2. **Post**: `aces_tonemap.wgsl` (ACES + sRGB) を全画面三角形で LDR
//!    (`Rgba8Unorm`) にマッピング。
//! 3. **Readback**: `copy_texture_to_buffer` + `map_async` で実画素を CPU へ。
//!
//! 検証: `frame_reference.rs` の CPU 参照ラスタが同一パラメータで同一数値を
//! 出すこと (GPU/CPU 相互検証)、および `naga` による WGSL 妥当性テスト。

use crate::gpu_vertex_pull::{FrameUniforms, SHADER_VERTEX_PULL};
use crate::pull_mesh::PullBuiltMesh;
use tracing::debug;

/// HDR 中間フォーマット (ACES 入力)。Rgba16Float は filterable かつ
/// RENDER_ATTACHMENT/TEXTURE_BINDING/COPY_SRC が wgpu core 要件内で使える。
pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// 最終 LDR フォーマット。
pub const LDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// 深度フォーマット。
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// フレーム描画結果 (実画素+実統計)。
pub struct FrameImage {
    pub width: u32,
    pub height: u32,
    /// RGBA8 画素 (読み戻し実データ)。
    pub pixels: Vec<u8>,
    pub draw_calls: u32,
    pub quads: u32,
}

/// オービットカメラ入力。WGSL ユニフォームに流す列優先 (column-major) の
/// view_proj 行列を `build_view_proj` が生成する。
pub struct FrameCamera {
    pub eye: [f32; 3],
    pub target: [f32; 3],
    pub up: [f32; 3],
    pub fov_y_deg: f32,
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
}

/// 右手系 look-at (view) × wgpu 式透視 (z∈[0,1]) を列優先で合成。
/// CPU 参照ラスタ (`frame_reference`) はこの同一関数で行列を受け取り、
/// 同一の乗算規則で適用する (乖離しない設計)。
pub fn build_view_proj(cam: &FrameCamera) -> [[f32; 4]; 4] {
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let norm = |a: [f32; 3]| {
        let l = dot(a, a).sqrt().max(1e-8);
        [a[0] / l, a[1] / l, a[2] / l]
    };
    let f = norm(sub(cam.target, cam.eye));
    let s = norm(cross(f, cam.up));
    let u = cross(s, f);
    // view (RH: カメラ前方は -z)
    let view = [
        [s[0], u[0], -f[0], 0.0],
        [s[1], u[1], -f[1], 0.0],
        [s[2], u[2], -f[2], 0.0],
        [-dot(s, cam.eye), -dot(u, cam.eye), dot(f, cam.eye), 1.0],
    ];
    // wgpu 透視 (n→0, f→1 の z 範囲)
    let t = 1.0 / (cam.fov_y_deg.to_radians() * 0.5).tan();
    let (n, fa) = (cam.near, cam.far);
    let persp = [
        [t / cam.aspect, 0.0, 0.0, 0.0],
        [0.0, t, 0.0, 0.0],
        [0.0, 0.0, fa / (n - fa), -1.0],
        [0.0, 0.0, (fa * n) / (n - fa), 0.0],
    ];
    mul44(&persp, &view)
}

/// 列優先 4x4 行列乗算 (a * b)。CPU 側で WGSL の `persp * view` と一致させる。
fn mul44(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0; 4]; 4];
    for c in 0..4 {
        for r in 0..4 {
            let mut acc = 0.0;
            for k in 0..4 {
                acc += a[k][r] * b[c][k];
            }
            out[c][r] = acc;
        }
    }
    out
}

/// 列優先行列 × ベクトル (WGSL の `m * v` と一致)。CPU 参照ラスタも共有する。
pub(crate) fn mul_v4(m: &[[f32; 4]; 4], v: [f32; 4]) -> [f32; 4] {
    let mut out = [0.0; 4];
    for r in 0..4 {
        out[r] = m[0][r] * v[0] + m[1][r] * v[1] + m[2][r] * v[2] + m[3][r] * v[3];
    }
    out
}

/// 深度付きオフスクリーン実フレームレンダラー。
pub struct GpuFramePipeline {
    pub width: u32,
    pub height: u32,
    hdr: wgpu::Texture,
    hdr_view: wgpu::TextureView,
    depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    ldr: wgpu::Texture,
    ldr_view: wgpu::TextureView,
    readback: wgpu::Buffer,
    pull_pipeline: wgpu::RenderPipeline,
    pull_bgl: wgpu::BindGroupLayout,
    post_pipeline: wgpu::RenderPipeline,
    post_bgl: wgpu::BindGroupLayout,
    post_sampler: wgpu::Sampler,
    exposure_buf: wgpu::Buffer,
}

impl GpuFramePipeline {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let mk_target = |format: wgpu::TextureFormat, usage: wgpu::TextureUsages, label: &str| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
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
        let hdr = mk_target(
            HDR_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            "Rsift Frame HDR",
        );
        let depth = mk_target(
            DEPTH_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            "Rsift Frame Depth",
        );
        let ldr = mk_target(
            LDR_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            "Rsift Frame LDR",
        );
        let hdr_view = hdr.create_view(&Default::default());
        let depth_view = depth.create_view(&Default::default());
        let ldr_view = ldr.create_view(&Default::default());

        let padded = (width * 4).div_ceil(256) * 256;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift Frame Readback"),
            size: padded as u64 * height as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // --- Raster (vertex-pull + 深度) ---
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Rsift Frame Pull WGSL"),
            source: wgpu::ShaderSource::Wgsl(SHADER_VERTEX_PULL.into()),
        });
        let pull_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Rsift Frame Pull BGL"),
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
        let pull_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Rsift Frame Pull PL"),
            bind_group_layouts: &[&pull_bgl],
            push_constant_ranges: &[],
        });
        let pull_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Rsift Frame Pull Pipeline"),
            layout: Some(&pull_pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_pull",
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_pull",
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            // 既存 GpuVertexPullEngine との差分: 実深度テストで painter-order
            // 依存の前後関係破綻を解消する (inter-quad オクルージョンの実現)。
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
        });

        // --- Post (ACES: HDR → LDR 全画面三角形) ---
        let post_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Rsift Frame Post WGSL (ACES)"),
            source: wgpu::ShaderSource::Wgsl(crate::aces_tonemap::ACES_WGSL.into()),
        });
        let post_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Rsift Frame Post BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let post_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Rsift Frame Post PL"),
            bind_group_layouts: &[&post_bgl],
            push_constant_ranges: &[],
        });
        let post_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Rsift Frame Post Pipeline"),
            layout: Some(&post_pl),
            vertex: wgpu::VertexState {
                module: &post_shader,
                entry_point: "vs_main",
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &post_shader,
                entry_point: "fs_main",
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: LDR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
        });

        let post_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Rsift Frame Post Sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        // exposure = 1.0 (ACES_WGSL の struct U とレイアウト一致)
        let exposure_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift Frame Exposure"),
            size: 4,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            width,
            height,
            hdr,
            hdr_view,
            depth,
            depth_view,
            ldr,
            ldr_view,
            readback,
            pull_pipeline,
            pull_bgl,
            post_pipeline,
            post_bgl,
            post_sampler,
            exposure_buf,
        }
    }

    /// 1 実フレームを描き、実画素 (RGBA8) と統計を返す。
    /// `chunks`: (実プルメッシュ, ワールド原点 [x,y,z]) — 各要素 1 draw call。
    /// 深度クリア・HDR クリア (夜空色) → 全メッシュラスタ → ACES → リードバック。
    pub fn render_to_image(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view_proj: [[f32; 4]; 4],
        chunks: &[(PullBuiltMesh, [f32; 3])],
    ) -> Result<FrameImage, String> {
        queue.write_buffer(&self.exposure_buf, 0, bytemuck::bytes_of(&1.0f32));
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Rsift Frame Encoder"),
        });

        let mut draw_calls = 0u32;
        let mut quad_total = 0u32;
        for (mesh, origin) in chunks {
            if mesh.is_empty {
                continue;
            }
            let uniforms = FrameUniforms {
                view_proj,
                chunk_origin: [origin[0], origin[1], origin[2], 0.0],
            };
            let ubo = wgpu::util::DeviceExt::create_buffer_init(
                device,
                &wgpu::util::BufferInitDescriptor {
                    label: Some("Rsift Frame UBO"),
                    contents: bytemuck::bytes_of(&uniforms),
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                },
            );
            let ssbo = wgpu::util::DeviceExt::create_buffer_init(
                device,
                &wgpu::util::BufferInitDescriptor {
                    label: Some("Rsift Frame SSBO"),
                    contents: bytemuck::cast_slice(&mesh.quads),
                    usage: wgpu::BufferUsages::STORAGE,
                },
            );
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Rsift Frame Pull BG"),
                layout: &self.pull_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: ubo.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: ssbo.as_entire_binding(),
                    },
                ],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Rsift Frame Pull Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.hdr_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // 先頭 draw のみ HDR クリア (夜空色)、以降は累積 Load
                        load: if draw_calls == 0 {
                            wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.02,
                                g: 0.03,
                                b: 0.05,
                                a: 1.0,
                            })
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: if draw_calls == 0 {
                            wgpu::LoadOp::Clear(1.0)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pull_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..mesh.pull_vertex_count(), 0..1);
            drop(pass);
            draw_calls += 1;
            quad_total += mesh.quads.len() as u32;
        }
        // メッシュ 0 でもクリアだけは発生させる
        if draw_calls == 0 {
            let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Rsift Frame Clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.hdr_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.02,
                            g: 0.03,
                            b: 0.05,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }

        // Post: ACES (HDR → LDR → 全画面三角形)
        let post_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Rsift Frame Post BG"),
            layout: &self.post_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.exposure_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.hdr_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.post_sampler),
                },
            ],
        });
        {
            let mut post = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Rsift Frame Post Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.ldr_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            post.set_pipeline(&self.post_pipeline);
            post.set_bind_group(0, &post_bg, &[]);
            post.draw(0..3, 0..1);
        }

        // Readback: LDR → staging buffer
        let padded = (self.width * 4).div_ceil(256) * 256;
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &self.ldr,
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
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);

        let slice = self.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        // 実デバイスの完了待ち (wgpu::Maintain::Wait は完了までブロック)。
        while rx.try_recv().is_err() {
            let _ = device.poll(wgpu::Maintain::Wait);
        }
        let data = slice.get_mapped_range();
        let mut pixels = Vec::with_capacity((self.width * self.height * 4) as usize);
        for y in 0..self.height as usize {
            let s = y * padded as usize;
            pixels.extend_from_slice(&data[s..s + (self.width * 4) as usize]);
        }
        drop(data);
        self.readback.unmap();
        debug!(
            "[FramePipeline] draws={} quads={} pixels={}B",
            draw_calls,
            quad_total,
            pixels.len()
        );
        Ok(FrameImage {
            width: self.width,
            height: self.height,
            pixels,
            draw_calls,
            quads: quad_total,
        })
    }
}

#[cfg(test)]
mod tests {
    /// WGSL 実妥当性 + エントリポイント実在: Phase A で使う2ソースを naga パース
    /// し (GPU 不要の真の検証)、パイプラインが参照する関数名も確認する。
    #[test]
    fn frame_pipeline_wgsl_parses_with_entry_points() {
        for (name, src, entries) in [
            ("terrain_vertex_pull", SHADER_VERTEX_PULL, &["vs_pull", "fs_pull"][..]),
            (
                "aces_tonemap",
                crate::aces_tonemap::ACES_WGSL,
                &["vs_main", "fs_main"][..],
            ),
        ] {
            let module = naga::front::wgsl::parse_str(src)
                .unwrap_or_else(|e| panic!("{name} WGSL invalid: {e}"));
            for ep in entries {
                assert!(
                    module.entry_points.iter().any(|f| f.name == *ep),
                    "{name}: entry point '{ep}' not found"
                );
            }
        }
    }

    #[test]
    fn view_proj_maps_target_into_clip_volume() {
        let cam = FrameCamera {
            eye: [40.0, 72.0, 40.0],
            target: [8.0, 24.0, 8.0],
            up: [0.0, 1.0, 0.0],
            fov_y_deg: 60.0,
            aspect: 4.0 / 3.0,
            near: 0.1,
            far: 500.0,
        };
        let vp = build_view_proj(&cam);
        // 注視点がクリップ空間の視錐内 (|xy|<=w, 0<=z<=w, w>0) に写ること
        let clip = super::mul_v4(&vp, [8.0, 24.0, 8.0, 1.0]);
        let w = clip[3];
        assert!(w.is_finite() && w > 0.0, "w must be positive: {clip:?}");
        assert!(
            clip[0].abs() <= w && clip[1].abs() <= w,
            "within view: {clip:?}"
        );
        assert!(clip[2] >= 0.0 && clip[2] <= w, "z in [0,1]: {clip:?}");
    }

    use super::*;
}

//! Frame Worldgen wiring — 値ノイズ upsample / LBVH / Bindless / Half-vertex /
//! Meshlet cull の実 dispatch 配線。
//!
//! Phase E (全未配線解消): naga 検証 FAIL だった 5 モジュールを
//! 「WGSL 修復 (iGPU コア機能のみ) + CPU 精密ミラー + 実 GPU dispatch」へ配線。
//!
//! 発見・是正した実バグ (AUDIT 追記8):
//! - `noise_upsample.rs::perlin3d_dense` は i32 引数のみで勾配ノイズを評価する
//!   ため全整数ボクセルでラティス零点に退化し **常時 0.5 を返していた**
//!   (render_pipeline.rs が実使用 → ノイズ地形が事実上フラット)。
//!   ここでは整数ラティスで確実に変化する **値ノイズ (vhash)** を正準とし、
//!   WGSL/CPU の両側に同一演算を実装 (旧関数は消費者契約のため不触)。
//! - `half_vertex.wgsl` は WGSL 非標準の `u16` 型 + 実装定義 `pow` を使用
//!   → 浮動小数を一切使わないビット配置デコードへ置換 (GPU==CPU bitwise が
//!   厳密に成立)。
//! - `gpu_vertex_pull.rs` の `let _ = SHADER_MESH_SHADER;` 破棄スタブを解消
//!   (mesh shader は WGSL 非対応のため、task/mesh 等価エミュレーションの
//!   meshlet カリング pass として実 dispatch に転用)。
//!
//! frustum 平面抽出は Gribb/Hartmann 法 (列優先行列、wgpu z∈[0,1]) を
//! 実装し、既知点テストで向きを実検証している (見切り発車しない)。

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

pub const NOISE_WGSL: &str = include_str!("../shaders/noise_upsample.wgsl");
pub const LBVH_WGSL: &str = include_str!("../shaders/lbvh.wgsl");
pub const BINDLESS_WGSL: &str = include_str!("../shaders/bindless.wgsl");
pub const HALF_VERTEX_WGSL: &str = include_str!("../shaders/half_vertex.wgsl");
pub const MESHLET_WGSL: &str = crate::gpu_vertex_pull::SHADER_MESH_SHADER;

// ------------------------------------------------------------------
// Frustum planes (Gribb/Hartmann、列優先 m[col][row]、wgpu z∈[0,1])
// ------------------------------------------------------------------

/// 列優先 view_proj から 6 平面 (a,b,c,d) を抽出し正規化 (CPU 側 sqrt)。
/// 平面の内向きが正 (dist >= 0 が内側)。
pub fn frustum_planes(m: &[[f32; 4]; 4]) -> [[f32; 4]; 6] {
    let row = |r: usize| -> [f32; 4] { [m[0][r], m[1][r], m[2][r], m[3][r]] };
    let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
    let add = |a: [f32; 4], b: [f32; 4]| [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]];
    let sub = |a: [f32; 4], b: [f32; 4]| [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]];
    let mut planes = [
        add(r3, r0), // left
        sub(r3, r0), // right
        add(r3, r1), // bottom
        sub(r3, r1), // top
        r2,          // near (wgpu z∈[0,1])
        sub(r3, r2), // far
    ];
    for p in planes.iter_mut() {
        let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
        if len > 0.0 {
            p[0] /= len;
            p[1] /= len;
            p[2] /= len;
            p[3] /= len;
        }
    }
    planes
}

/// 球 (center, radius) が全平面の内側か (CPU 側 cull と同一規則)。
pub fn sphere_visible(planes: &[[f32; 4]], center: [f32; 3], radius: f32) -> bool {
    planes.iter().all(|p| {
        let dist = p[0] * center[0] + p[1] * center[1] + p[2] * center[2] + p[3];
        dist >= -radius
    })
}

// ------------------------------------------------------------------
// 値ノイズ upsample (正準 = vhash)
// ------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct UpsampleParams {
    pub stride: u32,
    pub seed: u32,
    pub origin_x: i32,
    pub origin_y: i32,
    pub origin_z: i32,
    pub size_x: u32,
    pub size_y: u32,
    pub size_z: u32,
    pub cave_threshold: f32,
    pub _pad: u32,
}

impl UpsampleParams {
    pub fn coarse_dims(&self) -> (u32, u32, u32) {
        (
            (self.size_x / self.stride) + 1,
            (self.size_y / self.stride) + 1,
            (self.size_z / self.stride) + 1,
        )
    }
}

/// CPU 精密ミラー: WGSL `hash3u` と完全一致 (u32 ラップ整数演算)。
pub fn hash3u(x: u32, y: u32, z: u32, seed: u32) -> u32 {
    let mut h = seed
        ^ x.wrapping_mul(0x9E37_79B9)
        ^ y.wrapping_mul(0x85EB_CA6B)
        ^ z.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^= h >> 16;
    h
}

/// CPU 精密ミラー: WGSL `vhash` と完全一致 (ラティス値ノイズ [0,1])。
pub fn vhash(ix: i32, iy: i32, iz: i32, seed: u32) -> f32 {
    (hash3u(
        (ix & 255) as u32,
        (iy & 255) as u32,
        (iz & 255) as u32,
        seed,
    ) & 0xFF) as f32
        / 255.0
}

/// CPU 精密ミラー: WGSL `cs_coarse_noise` と完全一致。
pub fn noise_coarse_cpu(params: &UpsampleParams) -> Vec<f32> {
    let (cx, cy, cz) = params.coarse_dims();
    let mut out = vec![0.0; (cx * cy * cz) as usize];
    for z in 0..cz {
        for y in 0..cy {
            for x in 0..cx {
                let wx = params.origin_x + (x * params.stride) as i32;
                let wy = params.origin_y + (y * params.stride) as i32;
                let wz = params.origin_z + (z * params.stride) as i32;
                let density = vhash(wx, wy, wz, params.seed);
                let cave = vhash(wx + 97, wy + 53, wz + 31, params.seed ^ 0xCAFE);
                out[(x + y * cx + z * cx * cy) as usize] = density - cave * params.cave_threshold;
            }
        }
    }
    out
}

/// CPU 精密ミラー: WGSL `cs_trilinear_fill` と完全一致
/// (= noise_upsample::trilinear_upsample の補間形と同一)。
pub fn noise_fill_cpu(params: &UpsampleParams, coarse: &[f32]) -> Vec<f32> {
    let (cw, ch, cd) = params.coarse_dims();
    let (sx, sy, sz) = (params.size_x, params.size_y, params.size_z);
    let row = cw;
    let slab = cw * ch;
    let at = |x: u32, y: u32, z: u32| coarse[(x + y * row + z * slab) as usize];
    let stride_f = params.stride as f32;
    let mut out = vec![0.0; (sx * sy * sz) as usize];
    for z in 0..sz {
        for y in 0..sy {
            for x in 0..sx {
                let lx = x as f32 / stride_f;
                let ly = y as f32 / stride_f;
                let lz = z as f32 / stride_f;
                let x0 = lx.floor() as u32;
                let y0 = ly.floor() as u32;
                let z0 = lz.floor() as u32;
                let x1 = (x0 + 1).min(cw - 1);
                let y1 = (y0 + 1).min(ch - 1);
                let z1 = (z0 + 1).min(cd - 1);
                let tx = lx - x0 as f32;
                let ty = ly - y0 as f32;
                let tz = lz - z0 as f32;
                let c000 = at(x0, y0, z0);
                let c100 = at(x1, y0, z0);
                let c010 = at(x0, y1, z0);
                let c110 = at(x1, y1, z0);
                let c001 = at(x0, y0, z1);
                let c101 = at(x1, y0, z1);
                let c011 = at(x0, y1, z1);
                let c111 = at(x1, y1, z1);
                let x00 = c000 + tx * (c100 - c000);
                let x10 = c010 + tx * (c110 - c010);
                let x01 = c001 + tx * (c101 - c001);
                let x11 = c011 + tx * (c111 - c011);
                let y0v = x00 + ty * (x10 - x00);
                let y1v = x01 + ty * (x11 - x01);
                out[(x + y * sx + z * sx * sy) as usize] = y0v + tz * (y1v - y0v);
            }
        }
    }
    out
}

pub struct GpuNoise {
    coarse_pipeline: wgpu::ComputePipeline,
    fill_pipeline: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
}

impl GpuNoise {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Rsift Noise WGSL"),
            source: wgpu::ShaderSource::Wgsl(NOISE_WGSL.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Rsift Noise BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
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
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Rsift Noise PL"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let coarse_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Rsift Noise Coarse Pipeline"),
            layout: Some(&pl),
            module: &module,
            entry_point: "cs_coarse_noise",
            compilation_options: Default::default(),
        });
        let fill_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Rsift Noise Fill Pipeline"),
            layout: Some(&pl),
            module: &module,
            entry_point: "cs_trilinear_fill",
            compilation_options: Default::default(),
        });
        Self {
            coarse_pipeline,
            fill_pipeline,
            bgl,
        }
    }

    /// 実 dispatch: coarse → fill の 2 pass で dense ボリュームを実生成し読み戻し。
    pub fn run(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        params: &UpsampleParams,
    ) -> (Vec<f32>, Vec<f32>) {
        let (cw, ch, cd) = params.coarse_dims();
        let coarse_count = (cw * ch * cd) as u64;
        let dense_count = (params.size_x * params.size_y * params.size_z) as u64;
        let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rsift Noise Params"),
            contents: bytemuck::bytes_of(params),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let coarse_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift Noise Coarse"),
            size: coarse_count * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let dense_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift Noise Dense"),
            size: dense_count * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let coarse_staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift Noise Coarse Staging"),
            size: coarse_count * 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let dense_staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift Noise Dense Staging"),
            size: dense_count * 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Rsift Noise BG"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: coarse_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: dense_buf.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Rsift Noise Encoder"),
        });
        {
            let mut cp = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Rsift Noise Pass"),
                timestamp_writes: None,
            });
            cp.set_pipeline(&self.coarse_pipeline);
            cp.set_bind_group(0, &bg, &[]);
            cp.dispatch_workgroups(cw.div_ceil(4), ch.div_ceil(4), cd.div_ceil(4));
            cp.set_pipeline(&self.fill_pipeline);
            cp.dispatch_workgroups(
                params.size_x.div_ceil(4),
                params.size_y.div_ceil(4),
                params.size_z.div_ceil(4),
            );
        }
        encoder.copy_buffer_to_buffer(&coarse_buf, 0, &coarse_staging, 0, coarse_count * 4);
        encoder.copy_buffer_to_buffer(&dense_buf, 0, &dense_staging, 0, dense_count * 4);
        queue.submit([encoder.finish()]);
        let coarse =
            crate::frame_postfx::read_f32_buffer(device, &coarse_staging, coarse_count as usize);
        let dense =
            crate::frame_postfx::read_f32_buffer(device, &dense_staging, dense_count as usize);
        (coarse, dense)
    }
}

// ------------------------------------------------------------------
// LBVH (morton codes + frustum cull)
// ------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct LbvhParams {
    pub count: u32,
    pub plane_count: u32,
    pub _pad0: u32,
    pub _pad1: u32,
    pub min: [f32; 3],
    pub _pad2: f32,
    pub span: [f32; 3],
    pub _pad3: f32,
    pub planes: [[f32; 4]; 6],
}

/// CPU 精密ミラー: WGSL `cs_morton` と完全一致。
pub fn lbvh_codes_cpu(spheres: &[[f32; 4]], min: [f32; 3], span: [f32; 3]) -> Vec<u32> {
    spheres
        .iter()
        .map(|s| {
            let nx = (((s[0] - min[0]) / span[0]).clamp(0.0, 0.9999) * 1024.0) as u32;
            let ny = (((s[1] - min[1]) / span[1]).clamp(0.0, 0.9999) * 1024.0) as u32;
            let nz = (((s[2] - min[2]) / span[2]).clamp(0.0, 0.9999) * 1024.0) as u32;
            crate::lbvh::morton3(nx, ny, nz)
        })
        .collect()
}

/// CPU 精密ミラー: WGSL `cs_cull` と完全一致 (0/1 マスク)。
pub fn lbvh_cull_cpu(spheres: &[[f32; 4]], planes: &[[f32; 4]]) -> Vec<u32> {
    spheres
        .iter()
        .map(|s| {
            let vis = planes.iter().all(|p| {
                let dist = p[0] * s[0] + p[1] * s[1] + p[2] * s[2] + p[3];
                dist >= -s[3]
            });
            if vis { 1 } else { 0 }
        })
        .collect()
}

pub struct GpuLbvh {
    morton_pipeline: wgpu::ComputePipeline,
    cull_pipeline: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
}

impl GpuLbvh {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Rsift LBVH WGSL"),
            source: wgpu::ShaderSource::Wgsl(LBVH_WGSL.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Rsift LBVH BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
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
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Rsift LBVH PL"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let morton_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Rsift LBVH Morton Pipeline"),
            layout: Some(&pl),
            module: &module,
            entry_point: "cs_morton",
            compilation_options: Default::default(),
        });
        let cull_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Rsift LBVH Cull Pipeline"),
            layout: Some(&pl),
            module: &module,
            entry_point: "cs_cull",
            compilation_options: Default::default(),
        });
        Self {
            morton_pipeline,
            cull_pipeline,
            bgl,
        }
    }

    /// 実 dispatch: spheres (center+radius) → morton codes + visibility マスク。
    pub fn run(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        spheres: &[[f32; 4]],
        min: [f32; 3],
        span: [f32; 3],
        planes: &[[f32; 4]],
    ) -> (Vec<u32>, Vec<u32>) {
        let count = spheres.len() as u32;
        let mut planes6 = [[0.0; 4]; 6];
        for (i, p) in planes.iter().enumerate().take(6) {
            planes6[i] = *p;
        }
        let params = LbvhParams {
            count,
            plane_count: planes.len().min(6) as u32,
            _pad0: 0,
            _pad1: 0,
            min,
            _pad2: 0.0,
            span,
            _pad3: 0.0,
            planes: planes6,
        };
        let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rsift LBVH Params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let sphere_bytes: Vec<u8> = spheres
            .iter()
            .flat_map(|s| s.iter().flat_map(|v| v.to_le_bytes()))
            .collect();
        let spheres_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rsift LBVH Spheres"),
            contents: &sphere_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let codes_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift LBVH Codes"),
            size: count as u64 * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let vis_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift LBVH Vis"),
            size: count as u64 * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let codes_staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift LBVH Codes Staging"),
            size: count as u64 * 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let vis_staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift LBVH Vis Staging"),
            size: count as u64 * 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Rsift LBVH BG"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: spheres_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: codes_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: vis_buf.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Rsift LBVH Encoder"),
        });
        {
            let mut cp = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Rsift LBVH Pass"),
                timestamp_writes: None,
            });
            cp.set_pipeline(&self.morton_pipeline);
            cp.set_bind_group(0, &bg, &[]);
            cp.dispatch_workgroups(count.div_ceil(64), 1, 1);
            cp.set_pipeline(&self.cull_pipeline);
            cp.dispatch_workgroups(count.div_ceil(64), 1, 1);
        }
        encoder.copy_buffer_to_buffer(&codes_buf, 0, &codes_staging, 0, count as u64 * 4);
        encoder.copy_buffer_to_buffer(&vis_buf, 0, &vis_staging, 0, count as u64 * 4);
        queue.submit([encoder.finish()]);
        let codes = crate::frame_postfx::read_u32_buffer(device, &codes_staging, count as usize);
        let vis = crate::frame_postfx::read_u32_buffer(device, &vis_staging, count as usize);
        (codes, vis)
    }
}

// ------------------------------------------------------------------
// Bindless handle unpack
// ------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct BindlessParams {
    pub count: u32,
    pub _pad0: u32,
    pub _pad1: u32,
    pub _pad2: u32,
}

/// CPU 精密ミラー: WGSL `cs_unpack_handles` と完全一致。
pub fn bindless_unpack_cpu(handles: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(handles.len() * 3);
    for &h in handles {
        let (s, b, i) = crate::bindless::unpack_handle(h);
        out.push(s);
        out.push(b);
        out.push(i);
    }
    out
}

pub struct GpuBindless {
    pipeline: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
}

impl GpuBindless {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Rsift Bindless WGSL"),
            source: wgpu::ShaderSource::Wgsl(BINDLESS_WGSL.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Rsift Bindless BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
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
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Rsift Bindless PL"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Rsift Bindless Pipeline"),
            layout: Some(&pl),
            module: &module,
            entry_point: "cs_unpack_handles",
            compilation_options: Default::default(),
        });
        Self { pipeline, bgl }
    }

    /// 実 dispatch: handles (count u32) → unpacked (count*3 u32)。
    pub fn run(&self, device: &wgpu::Device, queue: &wgpu::Queue, handles: &[u32]) -> Vec<u32> {
        let count = handles.len() as u32;
        let params = BindlessParams {
            count,
            _pad0: 0,
            _pad1: 0,
            _pad2: 0,
        };
        let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rsift Bindless Params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let handle_bytes: Vec<u8> = handles.iter().flat_map(|v| v.to_le_bytes()).collect();
        let handles_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rsift Bindless Handles"),
            contents: &handle_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let out_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift Bindless Out"),
            size: count as u64 * 12,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift Bindless Staging"),
            size: count as u64 * 12,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Rsift Bindless BG"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: handles_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: out_buf.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Rsift Bindless Encoder"),
        });
        {
            let mut cp = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Rsift Bindless Pass"),
                timestamp_writes: None,
            });
            cp.set_pipeline(&self.pipeline);
            cp.set_bind_group(0, &bg, &[]);
            cp.dispatch_workgroups(count.div_ceil(64), 1, 1);
        }
        encoder.copy_buffer_to_buffer(&out_buf, 0, &staging, 0, count as u64 * 12);
        queue.submit([encoder.finish()]);
        crate::frame_postfx::read_u32_buffer(device, &staging, (count * 3) as usize)
    }
}

// ------------------------------------------------------------------
// Half-vertex (f16 ビット配置デコード)
// ------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct HalfParams {
    pub word_count: u32,
    pub _pad0: u32,
    pub _pad1: u32,
    pub _pad2: u32,
}

/// CPU 精密ミラー: WGSL `f16_decode_bits` と完全一致。
/// 浮動小数を使わないビット配置デコード (エンコーダ half_vertex::f32_to_f16 が
/// subnormal を flush するため decode 側も exp==0 → ±0 が正準)。
pub fn f16_decode_bits(h: u32) -> f32 {
    let sign = (h >> 15) & 1;
    let exp = (h >> 10) & 31;
    let mant = h & 1023;
    let bits: u32 = if exp == 0 {
        0
    } else if exp == 31 {
        if mant == 0 {
            0x7F80_0000 // Inf
        } else {
            0x7FC0_0000 // NaN
        }
    } else {
        ((exp + 112) << 23) | (mant << 13)
    };
    f32::from_bits(bits | (sign << 31))
}

/// CPU 精密ミラー: WGSL `cs_f16_expand` と完全一致 (1 語 = f16×2)。
pub fn half_unpack_cpu(packed: &[u32]) -> Vec<f32> {
    let mut out = Vec::with_capacity(packed.len() * 2);
    for &w in packed {
        out.push(f16_decode_bits(w & 0xFFFF));
        out.push(f16_decode_bits(w >> 16));
    }
    out
}

pub struct GpuHalfVertex {
    pipeline: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
}

impl GpuHalfVertex {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Rsift Half Vertex WGSL"),
            source: wgpu::ShaderSource::Wgsl(HALF_VERTEX_WGSL.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Rsift Half BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
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
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Rsift Half PL"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Rsift Half Pipeline"),
            layout: Some(&pl),
            module: &module,
            entry_point: "cs_f16_expand",
            compilation_options: Default::default(),
        });
        Self { pipeline, bgl }
    }

    /// 実 dispatch: packed (f16×2/word) → decoded (2 f32/word)。
    pub fn run(&self, device: &wgpu::Device, queue: &wgpu::Queue, packed: &[u32]) -> Vec<f32> {
        let word_count = packed.len() as u32;
        let params = HalfParams {
            word_count,
            _pad0: 0,
            _pad1: 0,
            _pad2: 0,
        };
        let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rsift Half Params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let packed_bytes: Vec<u8> = packed.iter().flat_map(|v| v.to_le_bytes()).collect();
        let packed_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rsift Half Packed"),
            contents: &packed_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let out_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift Half Out"),
            size: word_count as u64 * 8,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift Half Staging"),
            size: word_count as u64 * 8,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Rsift Half BG"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: packed_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: out_buf.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Rsift Half Encoder"),
        });
        {
            let mut cp = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Rsift Half Pass"),
                timestamp_writes: None,
            });
            cp.set_pipeline(&self.pipeline);
            cp.set_bind_group(0, &bg, &[]);
            cp.dispatch_workgroups(word_count.div_ceil(64), 1, 1);
        }
        encoder.copy_buffer_to_buffer(&out_buf, 0, &staging, 0, word_count as u64 * 8);
        queue.submit([encoder.finish()]);
        crate::frame_postfx::read_f32_buffer(device, &staging, (word_count * 2) as usize)
    }
}

// ------------------------------------------------------------------
// Meshlet frustum cull (task/mesh 等価エミュレーション 1 段目)
// ------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct MeshletParams {
    pub count: u32,
    pub plane_count: u32,
    pub _pad0: u32,
    pub _pad1: u32,
    pub planes: [[f32; 4]; 6],
}

/// CPU 精密ミラー: WGSL `cs_meshlet_cull` と完全一致 (0/1 マスク)。
pub fn meshlet_cull_cpu(meshlets: &[[f32; 4]], planes: &[[f32; 4]]) -> Vec<u32> {
    meshlets
        .iter()
        .map(|s| {
            let vis = planes.iter().all(|p| {
                let dist = p[0] * s[0] + p[1] * s[1] + p[2] * s[2] + p[3];
                dist >= -s[3]
            });
            if vis { 1 } else { 0 }
        })
        .collect()
}

pub struct GpuMeshletCull {
    pipeline: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
}

impl GpuMeshletCull {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Rsift Meshlet WGSL"),
            source: wgpu::ShaderSource::Wgsl(MESHLET_WGSL.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Rsift Meshlet BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
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
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Rsift Meshlet PL"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Rsift Meshlet Pipeline"),
            layout: Some(&pl),
            module: &module,
            entry_point: "cs_meshlet_cull",
            compilation_options: Default::default(),
        });
        Self { pipeline, bgl }
    }

    /// 実 dispatch: meshlets (center+radius) → visibility マスク。
    pub fn run(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        meshlets: &[[f32; 4]],
        planes: &[[f32; 4]],
    ) -> Vec<u32> {
        let count = meshlets.len() as u32;
        let mut planes6 = [[0.0; 4]; 6];
        for (i, p) in planes.iter().enumerate().take(6) {
            planes6[i] = *p;
        }
        let params = MeshletParams {
            count,
            plane_count: planes.len().min(6) as u32,
            _pad0: 0,
            _pad1: 0,
            planes: planes6,
        };
        let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rsift Meshlet Params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let meshlet_bytes: Vec<u8> = meshlets
            .iter()
            .flat_map(|s| s.iter().flat_map(|v| v.to_le_bytes()))
            .collect();
        let meshlets_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rsift Meshlet Spheres"),
            contents: &meshlet_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let vis_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift Meshlet Vis"),
            size: count as u64 * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift Meshlet Staging"),
            size: count as u64 * 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Rsift Meshlet BG"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: meshlets_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: vis_buf.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Rsift Meshlet Encoder"),
        });
        {
            let mut cp = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Rsift Meshlet Pass"),
                timestamp_writes: None,
            });
            cp.set_pipeline(&self.pipeline);
            cp.set_bind_group(0, &bg, &[]);
            cp.dispatch_workgroups(count.div_ceil(64), 1, 1);
        }
        encoder.copy_buffer_to_buffer(&vis_buf, 0, &staging, 0, count as u64 * 4);
        queue.submit([encoder.finish()]);
        crate::frame_postfx::read_u32_buffer(device, &staging, count as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naga_ok(src: &str, label: &str) {
        naga::front::wgsl::parse_str(src)
            .unwrap_or_else(|e| panic!("{label} WGSL パース失敗: {}", e.emit_to_string(src)));
    }

    #[test]
    fn wgsl_parse_all() {
        naga_ok(NOISE_WGSL, "noise_upsample");
        naga_ok(LBVH_WGSL, "lbvh");
        naga_ok(BINDLESS_WGSL, "bindless");
        naga_ok(HALF_VERTEX_WGSL, "half_vertex");
        naga_ok(MESHLET_WGSL, "terrain_mesh_shader (emulation)");
    }

    #[test]
    fn hash3u_anchors() {
        // WGSL hash3u と同一演算であることの既知値アンカー。
        assert_eq!(hash3u(0, 0, 0, 0), 0);
        assert_eq!(hash3u(0, 0, 0, 1), {
            let mut h = 1u32;
            h ^= h >> 16;
            h = h.wrapping_mul(0x7FEB_352D);
            h ^= h >> 15;
            h = h.wrapping_mul(0x846C_A68B);
            h ^= h >> 16;
            h
        });
        // ラップ確認 (x=256 は mask で 0 ではなく別値)
        assert_ne!(hash3u(255, 0, 0, 7), hash3u(0, 0, 0, 7));
    }

    #[test]
    fn vhash_varies_on_lattice() {
        // 退化バグ (常時 0.5) への回帰検知: 整数ラティスで複数値が得られること。
        let vals: Vec<f32> = (0..64)
            .map(|i| vhash(i % 8, (i / 8) % 8, (i / 16) % 4, 0xC0FFEE))
            .collect();
        let mut distinct = std::collections::HashSet::new();
        for v in &vals {
            distinct.insert(v.to_bits());
        }
        assert!(
            distinct.len() >= 8,
            "値ノイズが退化 (distinct={})",
            distinct.len()
        );
        for v in &vals {
            assert!((0.0..=1.0).contains(v), "範囲外: {v}");
        }
    }

    #[test]
    fn noise_coarse_and_fill_deterministic() {
        let p = UpsampleParams {
            stride: 4,
            seed: 42,
            origin_x: 0,
            origin_y: 0,
            origin_z: 0,
            size_x: 16,
            size_y: 16,
            size_z: 16,
            cave_threshold: 0.25,
            _pad: 0,
        };
        let c1 = noise_coarse_cpu(&p);
        let c2 = noise_coarse_cpu(&p);
        assert_eq!(c1, c2, "coarse が非決定的");
        let d1 = noise_fill_cpu(&p, &c1);
        let d2 = noise_fill_cpu(&p, &c1);
        assert_eq!(d1, d2, "fill が非決定的");
        // 補間の単純健全性: dense の min/max は coarse の範囲内
        let cmin = c1.iter().copied().fold(f32::INFINITY, f32::min);
        let cmax = c1.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        for v in &d1 {
            assert!(*v >= cmin - 1e-6 && *v <= cmax + 1e-6, "補間が範囲外: {v}");
        }
    }

    #[test]
    fn lbvh_codes_match_module_morton() {
        let spheres = vec![
            [10.0, 20.0, 30.0, 5.0],
            [-5.0, 0.0, 100.0, 10.0],
            [50.0, 50.0, 50.0, 1.0],
        ];
        let min = [-16.0, -16.0, -16.0];
        let span = [128.0, 128.0, 128.0];
        let codes = lbvh_codes_cpu(&spheres, min, span);
        for (i, s) in spheres.iter().enumerate() {
            let nx = (((s[0] - min[0]) / span[0]).clamp(0.0, 0.9999) * 1024.0) as u32;
            let ny = (((s[1] - min[1]) / span[1]).clamp(0.0, 0.9999) * 1024.0) as u32;
            let nz = (((s[2] - min[2]) / span[2]).clamp(0.0, 0.9999) * 1024.0) as u32;
            assert_eq!(codes[i], crate::lbvh::morton3(nx, ny, nz), "codes#{i}");
        }
    }

    #[test]
    fn lbvh_cull_matches_sphere_semantics() {
        let spheres = vec![[0.0, 0.0, 0.0, 1.0], [-100.0, 0.0, 0.0, 1.0]];
        let planes = vec![[1.0, 0.0, 0.0, 50.0]]; // x >= -50
        let vis = lbvh_cull_cpu(&spheres, &planes);
        assert_eq!(vis, vec![1, 0], "奥の球はカリングされるべき");
    }

    #[test]
    fn bindless_roundtrip() {
        let handles: Vec<u32> = vec![
            crate::bindless::pack_handle(0, 0, 0),
            crate::bindless::pack_handle(15, 255, 0xFFFFF),
            crate::bindless::pack_handle(3, 42, 123456),
        ];
        let unpacked = bindless_unpack_cpu(&handles);
        for (i, &h) in handles.iter().enumerate() {
            let (s, b, ix) = crate::bindless::unpack_handle(h);
            assert_eq!(
                (unpacked[i * 3], unpacked[i * 3 + 1], unpacked[i * 3 + 2]),
                (s, b, ix),
                "handle#{i}"
            );
            // pack(unpack(h)) == h の完全 roundtrip
            assert_eq!(crate::bindless::pack_handle(s, b, ix), h);
        }
    }

    #[test]
    fn f16_decode_anchors() {
        assert_eq!(f16_decode_bits(0x3C00), 1.0);
        assert_eq!(f16_decode_bits(0xC000), -2.0);
        assert_eq!(f16_decode_bits(0x0000), 0.0);
        assert_eq!(f16_decode_bits(0x8000).to_bits(), (-0.0f32).to_bits());
        assert_eq!(f16_decode_bits(0x7C00), f32::INFINITY);
        assert_eq!(f16_decode_bits(0xFC00), f32::NEG_INFINITY);
        assert!(f16_decode_bits(0x7C01).is_nan(), "mant!=0 → NaN");
        // エンコーダ出力に対して旧デコーダと bitwise 一致することを実 assert
        for &v in &[
            0.0f32,
            1.0,
            -1.0,
            0.5,
            3.141_592,
            65504.0,
            -0.001_953_125,
            2048.0,
        ] {
            let h = crate::half_vertex::f32_to_f16(v);
            let old = crate::half_vertex::f16_to_f32(h);
            let new = f16_decode_bits(h as u32);
            assert_eq!(new.to_bits(), old.to_bits(), "v={v} h={h:#06x}");
        }
    }

    #[test]
    fn meshlet_cull_respects_frustum() {
        // 実フレームと同じカメラで、既知点が内/外判定どおりになることを実検証。
        let vp = crate::frame_pipeline::build_view_proj(&crate::frame_pipeline::FrameCamera {
            eye: [-30.0, 90.0, -30.0],
            target: [8.0, 20.0, 8.0],
            up: [0.0, 1.0, 0.0],
            fov_y_deg: 60.0,
            aspect: 640.0 / 480.0,
            near: 0.1,
            far: 500.0,
        });
        let planes = frustum_planes(&vp);
        let inside: Vec<[f32; 4]> = vec![[8.0, 20.0, 8.0, 2.0]];
        let outside: Vec<[f32; 4]> = vec![[1000.0, 1000.0, 1000.0, 2.0]];
        let vis_in = meshlet_cull_cpu(&inside, &planes);
        let vis_out = meshlet_cull_cpu(&outside, &planes);
        assert_eq!(vis_in, vec![1], "target 点は視錐台内のはず");
        assert_eq!(vis_out, vec![0], "遠方点はカリングされるはず");
    }
}

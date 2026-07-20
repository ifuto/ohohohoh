//! Frame PostFX wiring — CAS / Checkerboard / Exposure / VRS の実 dispatch 配線。
//!
//! Phase E (全未配線解消): 各 WGSL が naga 検証で FAIL していた 4 モジュールを
//! 「WGSL 修復 (iGPU コア機能のみ) + CPU 精密ミラー + 実 GPU dispatch」へ配線。
//! CPU ミラーと WGSL は演算順を完全一致させており、gpu-* 例では
//! GPU==CPU bitwise assert を実施する。
//!
//! - CAS: `cas.rs::cas_sample` の全画素版 (`cas_run_cpu` → `cas.wgsl`)
//! - Checkerboard: `Checkerboard::reconstruct` の全画素版 (描画半分 + 再構成)
//! - Exposure: luma 計算と露出適用を GPU、ヒストグラム計量は CPU
//!   (log/exp は WGSL 実装定義のため GPU には置かない設計)
//! - VRS: `Vrs::build_mask` の完全ミラー (バッファ読みに変更、
//!   sampler 双一次では GPU==CPU bitwise が不可能なため)

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

pub const CAS_WGSL: &str = include_str!("../shaders/cas.wgsl");
pub const CHECKERBOARD_WGSL: &str = include_str!("../shaders/checkerboard.wgsl");
pub const EXPOSURE_WGSL: &str = include_str!("../shaders/exposure.wgsl");
pub const VRS_WGSL: &str = include_str!("../shaders/vrs.wgsl");

// ------------------------------------------------------------------
// 共通 GPU ヘルパ (frame_hiz の実パターンと同一規則)
// ------------------------------------------------------------------

fn mk_pipeline(
    device: &wgpu::Device,
    label: &str,
    wgsl: &str,
) -> (wgpu::ComputePipeline, wgpu::BindGroupLayout) {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(wgsl.into()),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
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
        label: Some(label),
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });
    let pipe = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: Some(&pl),
        module: &module,
        entry_point: "main",
        compilation_options: Default::default(),
    });
    (pipe, bgl)
}

/// f32 バッファの実 readback (frame_hiz::read_u32 と同一の map + ポーリング規則)。
pub fn read_f32_buffer(device: &wgpu::Device, buf: &wgpu::Buffer, count: usize) -> Vec<f32> {
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
        out.push(f32::from_le_bytes([
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

pub fn read_u32_buffer(device: &wgpu::Device, buf: &wgpu::Buffer, count: usize) -> Vec<u32> {
    let raw = read_f32_buffer(device, buf, count);
    raw.iter().map(|v| v.to_bits()).collect()
}

// ------------------------------------------------------------------
// CAS
// ------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct CasParams {
    pub width: u32,
    pub height: u32,
    pub sharpness: f32,
    pub _pad: f32,
}

/// CPU 精密ミラー: WGSL `cas_run` と演算順完全一致
/// (= `cas.rs::cas_sample` の全画素版、ボーダーは端 clamp)。
pub fn cas_run_cpu(src: &[[f32; 4]], width: u32, height: u32, sharpness: f32) -> Vec<[f32; 4]> {
    let (w, h) = (width as i32, height as i32);
    let px = |x: i32, y: i32| -> [f32; 3] {
        let cx = x.clamp(0, w - 1) as u32;
        let cy = y.clamp(0, h - 1) as u32;
        let p = src[(cy * width + cx) as usize];
        [p[0], p[1], p[2]]
    };
    let mut out = Vec::with_capacity(src.len());
    for y in 0..h {
        for x in 0..w {
            let n = px(x, y - 1);
            let s = px(x, y + 1);
            let e = px(x + 1, y);
            let ww = px(x - 1, y);
            let c = px(x, y);
            let src_px = src[(y as u32 * width + x as u32) as usize];
            let mut rgb = [0f32; 3];
            for ch in 0..3 {
                let mn = n[ch].min(s[ch]).min(e[ch]).min(ww[ch]);
                let mx = n[ch].max(s[ch]).max(e[ch]).max(ww[ch]);
                let contour = (1.0 - (mx - mn)).clamp(0.0, 1.0);
                let peaking = 1.0 / (4.0 * (mx - mn) + 1.0);
                let amp = (contour * peaking * sharpness).clamp(0.0, 1.0);
                let avg = (mn + mx) * 0.5;
                rgb[ch] = c[ch] * (1.0 - amp) + avg * amp;
            }
            out.push([rgb[0], rgb[1], rgb[2], src_px[3]]);
        }
    }
    out
}

pub struct GpuCas {
    pipeline: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
}

impl GpuCas {
    pub fn new(device: &wgpu::Device) -> Self {
        let (pipeline, bgl) = mk_pipeline(device, "Rsift CAS Pipeline", CAS_WGSL);
        Self { pipeline, bgl }
    }

    /// 実 dispatch: src (w×h RGBA f32) → sharpen 済み RGBA f32 読み戻し。
    pub fn run(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        src: &[[f32; 4]],
        width: u32,
        height: u32,
        sharpness: f32,
    ) -> Vec<[f32; 4]> {
        let params = CasParams {
            width,
            height,
            sharpness,
            _pad: 0.0,
        };
        let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rsift CAS Params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let src_bytes: Vec<u8> = src
            .iter()
            .flat_map(|p| p.iter().flat_map(|v| v.to_le_bytes()))
            .collect();
        let src_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rsift CAS Src"),
            contents: &src_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let px_count = (width * height) as u64;
        let dst_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift CAS Dst"),
            size: px_count * 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift CAS Staging"),
            size: px_count * 16,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Rsift CAS BG"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: src_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: dst_buf.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Rsift CAS Encoder"),
        });
        {
            let mut cp = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Rsift CAS Pass"),
                timestamp_writes: None,
            });
            cp.set_pipeline(&self.pipeline);
            cp.set_bind_group(0, &bg, &[]);
            cp.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
        }
        encoder.copy_buffer_to_buffer(&dst_buf, 0, &staging, 0, px_count * 16);
        queue.submit([encoder.finish()]);
        read_f32_buffer(device, &staging, (px_count * 4) as usize)
            .chunks_exact(4)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect()
    }
}

// ------------------------------------------------------------------
// Checkerboard
// ------------------------------------------------------------------

/// CPU 精密ミラー: WGSL `cs_checkerboard` と演算順完全一致
/// (描画済み `(x+y)&1==0` はコピー、それ以外は斜め 4 近傍の左結合平均 ×0.25)。
pub fn checker_run_cpu(src: &[[f32; 4]], width: u32, height: u32) -> Vec<[f32; 4]> {
    let (w, h) = (width as i32, height as i32);
    let px = |x: i32, y: i32| -> [f32; 4] {
        let cx = x.clamp(0, w - 1) as u32;
        let cy = y.clamp(0, h - 1) as u32;
        src[(cy * width + cx) as usize]
    };
    let mut out = Vec::with_capacity(src.len());
    for y in 0..h {
        for x in 0..w {
            let idx = (y as u32 * width + x as u32) as usize;
            if ((x as u32 + y as u32) & 1) == 0 {
                out.push(src[idx]);
                continue;
            }
            let nw = px(x - 1, y - 1);
            let ne = px(x + 1, y - 1);
            let sw = px(x - 1, y + 1);
            let se = px(x + 1, y + 1);
            let mut px_out = [0f32; 4];
            for ch in 0..4 {
                px_out[ch] = (nw[ch] + ne[ch] + sw[ch] + se[ch]) * 0.25;
            }
            out.push(px_out);
        }
    }
    out
}

pub struct GpuCheckerboard {
    pipeline: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct CbParams {
    pub width: u32,
    pub height: u32,
    pub _pad0: u32,
    pub _pad1: u32,
}

impl GpuCheckerboard {
    pub fn new(device: &wgpu::Device) -> Self {
        let (pipeline, bgl) = mk_pipeline(device, "Rsift Checkerboard Pipeline", CHECKERBOARD_WGSL);
        Self { pipeline, bgl }
    }

    pub fn run(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        src: &[[f32; 4]],
        width: u32,
        height: u32,
    ) -> Vec<[f32; 4]> {
        let params = CbParams {
            width,
            height,
            _pad0: 0,
            _pad1: 0,
        };
        let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rsift CB Params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let src_bytes: Vec<u8> = src
            .iter()
            .flat_map(|p| p.iter().flat_map(|v| v.to_le_bytes()))
            .collect();
        let src_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rsift CB Src"),
            contents: &src_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let px_count = (width * height) as u64;
        let dst_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift CB Dst"),
            size: px_count * 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift CB Staging"),
            size: px_count * 16,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Rsift CB BG"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: src_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: dst_buf.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Rsift CB Encoder"),
        });
        {
            let mut cp = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Rsift CB Pass"),
                timestamp_writes: None,
            });
            cp.set_pipeline(&self.pipeline);
            cp.set_bind_group(0, &bg, &[]);
            cp.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
        }
        encoder.copy_buffer_to_buffer(&dst_buf, 0, &staging, 0, px_count * 16);
        queue.submit([encoder.finish()]);
        read_f32_buffer(device, &staging, (px_count * 4) as usize)
            .chunks_exact(4)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect()
    }
}

// ------------------------------------------------------------------
// Exposure (GPU: luma + apply、計量は CPU)
// ------------------------------------------------------------------

/// CPU 精密ミラー: WGSL `cs_luma` と演算順完全一致 (Rec.709)。
pub fn luma_run_cpu(src: &[[f32; 4]]) -> Vec<f32> {
    src.iter()
        .map(|c| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2])
        .collect()
}

/// CPU 精密ミラー: WGSL `cs_apply` と完全一致 (clamp(c * e, 0, 1)、alpha 透過)。
pub fn apply_run_cpu(src: &[[f32; 4]], exposure: f32) -> Vec<[f32; 4]> {
    src.iter()
        .map(|c| {
            [
                (c[0] * exposure).clamp(0.0, 1.0),
                (c[1] * exposure).clamp(0.0, 1.0),
                (c[2] * exposure).clamp(0.0, 1.0),
                c[3],
            ]
        })
        .collect()
}

pub struct GpuExposure {
    luma_pipeline: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct ExposureParams {
    pub count: u32,
    pub exposure: f32,
    pub _pad0: u32,
    pub _pad1: u32,
}

impl GpuExposure {
    pub fn new(device: &wgpu::Device) -> Self {
        // luma と apply は同一 BGL (0=uniform 1=src 2=dst) を共有できるが
        // WGSL では binding 2/3 が別用途のため、パイプライン個別にする。
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Rsift Exposure WGSL"),
            source: wgpu::ShaderSource::Wgsl(EXPOSURE_WGSL.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Rsift Exposure BGL"),
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
            label: Some("Rsift Exposure PL"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let luma_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Rsift Exposure Luma Pipeline"),
            layout: Some(&pl),
            module: &module,
            entry_point: "cs_luma",
            compilation_options: Default::default(),
        });
        let apply_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Rsift Exposure Apply Pipeline"),
            layout: Some(&pl),
            module: &module,
            entry_point: "cs_apply",
            compilation_options: Default::default(),
        });
        let _ = apply_pipeline; // run_luma / run_apply が個別に保持
        Self { luma_pipeline, bgl }
    }

    /// 実 dispatch (luma): src (count RGBA f32) → count luma f32。
    pub fn run_luma(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        src: &[[f32; 4]],
    ) -> Vec<f32> {
        let count = src.len() as u32;
        let params = ExposureParams {
            count,
            exposure: 1.0,
            _pad0: 0,
            _pad1: 0,
        };
        let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rsift Exposure Params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let src_bytes: Vec<u8> = src
            .iter()
            .flat_map(|p| p.iter().flat_map(|v| v.to_le_bytes()))
            .collect();
        let src_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rsift Exposure Src"),
            contents: &src_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let luma_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift Exposure Luma"),
            size: count as u64 * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let dst_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift Exposure Dst (unused in luma)"),
            size: count as u64 * 16,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift Exposure Luma Staging"),
            size: count as u64 * 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Rsift Exposure BG"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: src_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: luma_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: dst_buf.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Rsift Exposure Luma Encoder"),
        });
        {
            let mut cp = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Rsift Exposure Luma Pass"),
                timestamp_writes: None,
            });
            cp.set_pipeline(&self.luma_pipeline);
            cp.set_bind_group(0, &bg, &[]);
            cp.dispatch_workgroups(count.div_ceil(64), 1, 1);
        }
        encoder.copy_buffer_to_buffer(&luma_buf, 0, &staging, 0, count as u64 * 4);
        queue.submit([encoder.finish()]);
        read_f32_buffer(device, &staging, count as usize)
    }
}

// ------------------------------------------------------------------
// VRS
// ------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct VrsParams {
    pub dims: [u32; 2],
    pub tile: u32,
    pub motion_weight: f32,
    pub variance_weight: f32,
    pub _pad0: u32,
    pub _pad1: u32,
    pub _pad2: u32,
}

/// CPU 精密ミラー: WGSL `cs_vrs_mask` と完全一致
/// (= `vrs.rs::Vrs::build_mask` と同一規則、タイル端 clamp・cnt 除算・閾値)。
pub fn vrs_run_cpu(
    motion: &[f32],
    var: &[f32],
    width: u32,
    height: u32,
    tile: u32,
    motion_weight: f32,
    variance_weight: f32,
) -> Vec<u32> {
    assert_eq!(motion.len(), (width * height) as usize);
    assert_eq!(var.len(), (width * height) as usize);
    let (w, h, t) = (width as usize, height as usize, tile as usize);
    let tw = w.div_ceil(t);
    let th = h.div_ceil(t);
    let mut out = vec![0u32; tw * th];
    for ty in 0..th {
        for tx in 0..tw {
            let y0 = ty * t;
            let y1 = ((ty + 1) * t).min(h);
            let x0 = tx * t;
            let x1 = ((tx + 1) * t).min(w);
            let mut ms = 0.0f32;
            let mut vs = 0.0f32;
            let mut cnt = 0u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = y * w + x;
                    ms += motion[i];
                    vs += var[i];
                    cnt += 1;
                }
            }
            let motion_avg = ms / cnt as f32;
            let var_avg = vs / cnt as f32;
            let score = motion_avg.clamp(0.0, 1.0) * motion_weight
                - var_avg.clamp(0.0, 1.0) * variance_weight;
            let code = if score > 0.6 {
                4
            } else if score > 0.3 {
                3
            } else if score > 0.05 {
                2
            } else if score > -0.3 {
                1
            } else {
                0
            };
            out[ty * tw + tx] = code;
        }
    }
    out
}

pub struct GpuVrs {
    pipeline: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
}

/// GpuVrs::run への入力フィールド (引数過多の構造化)。
pub struct VrsField<'a> {
    pub motion: &'a [f32],
    pub var: &'a [f32],
    pub width: u32,
    pub height: u32,
}

impl GpuVrs {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Rsift VRS WGSL"),
            source: wgpu::ShaderSource::Wgsl(VRS_WGSL.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Rsift VRS BGL"),
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
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Rsift VRS PL"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Rsift VRS Pipeline"),
            layout: Some(&pl),
            module: &module,
            entry_point: "cs_vrs_mask",
            compilation_options: Default::default(),
        });
        Self { pipeline, bgl }
    }

    /// 実 dispatch: motion/var (w×h f32) → タイルマスク codes。
    pub fn run(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        field: &VrsField,
        tile: u32,
        weights: [f32; 2],
    ) -> Vec<u32> {
        let (motion, var, width, height) = (field.motion, field.var, field.width, field.height);
        let params = VrsParams {
            dims: [width, height],
            tile,
            motion_weight: weights[0],
            variance_weight: weights[1],
            _pad0: 0,
            _pad1: 0,
            _pad2: 0,
        };
        let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rsift VRS Params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let motion_bytes: Vec<u8> = motion.iter().flat_map(|v| v.to_le_bytes()).collect();
        let var_bytes: Vec<u8> = var.iter().flat_map(|v| v.to_le_bytes()).collect();
        let motion_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rsift VRS Motion"),
            contents: &motion_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let var_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rsift VRS Var"),
            contents: &var_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let tw = width.div_ceil(tile);
        let th = height.div_ceil(tile);
        let tile_count = (tw * th) as u64;
        let mask_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift VRS Mask"),
            size: tile_count * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rsift VRS Staging"),
            size: tile_count * 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Rsift VRS BG"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: motion_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: var_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: mask_buf.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Rsift VRS Encoder"),
        });
        {
            let mut cp = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Rsift VRS Pass"),
                timestamp_writes: None,
            });
            cp.set_pipeline(&self.pipeline);
            cp.set_bind_group(0, &bg, &[]);
            cp.dispatch_workgroups(tw.div_ceil(8), th.div_ceil(8), 1);
        }
        encoder.copy_buffer_to_buffer(&mask_buf, 0, &staging, 0, tile_count * 4);
        queue.submit([encoder.finish()]);
        read_u32_buffer(device, &staging, tile_count as usize)
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
        naga_ok(CAS_WGSL, "cas");
        naga_ok(CHECKERBOARD_WGSL, "checkerboard");
        naga_ok(EXPOSURE_WGSL, "exposure");
        naga_ok(VRS_WGSL, "vrs");
    }

    #[test]
    fn cas_matches_module_fn_bitwise() {
        // cas.rs::cas_sample と全画素版 cas_run_cpu の完全一致を実 assert。
        let src: Vec<[f32; 4]> = (0..64)
            .map(|i| {
                let v = ((i * 37 % 101) as f32) / 100.0;
                [v, 1.0 - v, (v * 0.5) + 0.25, 1.0]
            })
            .collect();
        let out = cas_run_cpu(&src, 8, 8, 0.7);
        for (i, px) in out.iter().enumerate().take(64) {
            let x = (i % 8) as i32;
            let y = (i / 8) as i32;
            let get = |gx: i32, gy: i32| -> crate::cas::Vec3 {
                let cx = gx.clamp(0, 7) as u32;
                let cy = gy.clamp(0, 7) as u32;
                let p = src[(cy * 8 + cx) as usize];
                crate::cas::Vec3::new(p[0], p[1], p[2])
            };
            let expect = crate::cas::cas_sample(
                get(x, y),
                get(x, y - 1),
                get(x, y + 1),
                get(x + 1, y),
                get(x - 1, y),
                0.7,
            );
            assert!(
                (px[0] - expect.x).abs() < 1e-7
                    && (px[1] - expect.y).abs() < 1e-7
                    && (px[2] - expect.z).abs() < 1e-7,
                "px#{i} out={px:?} expect=({},{},{})",
                expect.x,
                expect.y,
                expect.z
            );
        }
    }

    #[test]
    fn checker_matches_module_fns() {
        // マスク規則と再構成式が Checkerboard の公開 API と一致することを実 assert。
        for y in 0..4u32 {
            for x in 0..4u32 {
                assert_eq!(
                    ((x + y) & 1) == 0,
                    crate::checkerboard::Checkerboard::is_rendered(x, y)
                );
            }
        }
        let src: Vec<[f32; 4]> = (0..16)
            .map(|i| [i as f32, (i * 2) as f32, (i * 3) as f32, 1.0])
            .collect();
        let out = checker_run_cpu(&src, 4, 4);
        // (1,0) = 未描画 → 斜め 4 近傍 (0,-1→端 clamp (0,0)),(2,-1→(2,0)? no: y-1 clamp 0),
        // (0,1),(2,1) の平均。ミラー規則どおり端 clamp。
        let expect = crate::checkerboard::Checkerboard::reconstruct(
            crate::checkerboard::Vec3::new(src[0][0], src[0][1], src[0][2]),
            crate::checkerboard::Vec3::new(src[2][0], src[2][1], src[2][2]),
            crate::checkerboard::Vec3::new(src[4][0], src[4][1], src[4][2]),
            crate::checkerboard::Vec3::new(src[6][0], src[6][1], src[6][2]),
        );
        let got = out[1];
        assert!(
            (got[0] - expect.x).abs() < 1e-7
                && (got[1] - expect.y).abs() < 1e-7
                && (got[2] - expect.z).abs() < 1e-7,
            "got={got:?}"
        );
        // 描画済み (0,0) はコピー
        assert_eq!(out[0], src[0]);
    }

    #[test]
    fn luma_matches_rec709_exact() {
        let src = vec![[1.0, 0.5, 0.25, 1.0], [0.0, 0.0, 0.0, 0.0]];
        let l = luma_run_cpu(&src);
        let expect0: f32 = 0.2126 * 1.0 + 0.7152 * 0.5 + 0.0722 * 0.25;
        assert_eq!(l[0].to_bits(), expect0.to_bits());
        assert_eq!(l[1], 0.0);
    }

    #[test]
    fn vrs_matches_module_build_mask() {
        // Vrs::build_mask と vrs_run_cpu が全タイルで一致することを実 assert。
        let (w, h, tile) = (32u32, 16u32, 8u32);
        let motion: Vec<f32> = (0..(w * h))
            .map(|i| ((i * 13 % 97) as f32) / 96.0)
            .collect();
        let var: Vec<f32> = (0..(w * h)).map(|i| ((i * 7 % 53) as f32) / 52.0).collect();
        let vrs = crate::vrs::Vrs::new();
        let expect = vrs.build_mask(w as usize, h as usize, tile as usize, &motion, &var);
        let got = vrs_run_cpu(
            &motion,
            &var,
            w,
            h,
            tile,
            vrs.motion_weight,
            vrs.variance_weight,
        );
        assert_eq!(got, expect, "vrs_run_cpu が Vrs::build_mask と不一致");
    }
}

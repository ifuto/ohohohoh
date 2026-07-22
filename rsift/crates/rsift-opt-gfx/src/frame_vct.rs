//! # RsGraphics 進化 Phase D1 — Voxel Cone Tracing 実 GPU dispatch (`GpuVct`)
//!
//! 実 SVO (`svo.rs` from_column、修正後の軸別半分割ツリー) を GPU バッファに
//! 語列化 (`to_gpu_words`) してアップロードし、`voxel_cone_tracing.wgsl` で
//! **実コーン毎に実走査・実蓄積** して readback する。
//!
//! ## データ駆動経路 (フェイク無し)
//! 1. 実 SectionPalette → `SparseVoxelOctree::from_column` (CPU、既存実コード)
//! 2. `to_gpu_words` → RO storage buffer (10 u32/node、子並び dz*4+dy*2+dx)
//! 3. コーン列 (`ConeWgsl` 32B) → RO storage buffer
//! 4. compute dispatch (64x1x1, 1 invocation = 1 コーン) → vec4<f32> 出力
//! 5. readback (16B × コーン数) → 呼び出し側が GI/AO として実消費できる
//!
//! ## CPU 参照との一致保証
//! `trace_cone_words` は WGSL と同一演算順・同一 lod ビット抽出の Rust 精密
//! ミラー (GPU 不要で検証可能)。テストではオブジェクトツリー経路
//! (`VoxelConeTracing::trace_diffuse_cone` + `sample_lod`) との **bitwise 一致**
//! を実 assert する (log2 を libm に依存しない lod ビット抽出により、
//! 実装定義丸め差を排除した設計)。

use crate::svo::{SparseVoxelOctree, GPU_NODE_STRIDE};

pub const VCT_WGSL: &str = include_str!("../shaders/voxel_cone_tracing.wgsl");

/// WGSL `VctParams` と一致 (32B)。
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct VctParams {
    pub bounds: [f32; 3],
    pub root: u32,
    pub cap: u32,
    pub node_count: u32,
    pub cone_count: u32,
    pub _pad: u32,
}

/// WGSL `ConeWgsl` と一致 (32B)。`d` は正規化済みで送ること
/// (CPU 参照 `ConeRay` も正規化前提で dist 意味が一致する)。
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ConeWgsl {
    pub o: [f32; 3],
    pub aperture: f32,
    pub d: [f32; 3],
    pub max_dist: f32,
}

impl ConeWgsl {
    /// 方向を正規化して構築。
    /// ゼロベクトルは**そのまま**残る (除算 NaN を避ける防御)。この場合
    /// コーンは退化して「原点 o 自身のセルを max_dist まで繰り返しサンプルする」
    /// 定義になる — 体積内部ではヒットし得る (旧 doc の「命中しない」は
    /// 原点が体積外の場合に限られるため訂正。走査規則は WGSL/ミラーとも同一)。
    pub fn new_normalized(o: [f32; 3], d: [f32; 3], aperture: f32, max_dist: f32) -> Self {
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let d = if l > 1e-8 {
            [d[0] / l, d[1] / l, d[2] / l]
        } else {
            d
        };
        Self {
            o,
            aperture,
            d,
            max_dist,
        }
    }
}

/// WGSL `lod_from_diameter` と一致する lod 算出。
/// 実体は `voxel_cone_tracing::lod_from_diameter` (一元化: CPU 実経路・
/// 本ミラー・WGSL の 3 者が同一規則を共有し、LOD 選択が構造的に一致する)。
pub fn lod_from_diameter_wgsl(d: f32) -> u32 {
    crate::voxel_cone_tracing::lod_from_diameter(d)
}

/// WGSL `svo_sample_lod` の Rust 精密ミラー (語列版)。
/// 戻り値は `sample_lod` と同規約: `Some((color, alpha))` / `None`。
pub fn sample_lod_words(
    words: &[u32],
    bounds: [f32; 3],
    root: u32,
    cap: u32,
    p: [f32; 3],
    lod: u32,
) -> Option<([f32; 3], f32)> {
    const ALBEDO: [f32; 3] = [0.5, 0.5, 0.5];
    // NaN でも WGSL の !(...) → NO_HIT と同じ結果になる否定形
    let inside = p[0] >= 0.0
        && p[1] >= 0.0
        && p[2] >= 0.0
        && p[0] < bounds[0]
        && p[1] < bounds[1]
        && p[2] < bounds[2];
    if !inside {
        return None;
    }
    let budget = cap.saturating_sub(lod.min(cap));
    let mut node_idx = root as usize;
    let mut org = [0.0f32; 3];
    let mut size = bounds;
    let mut depth = 0u32;
    loop {
        let base = node_idx * GPU_NODE_STRIDE;
        if base + 9 >= words.len() {
            return None; // アクセスガード
        }
        let w0 = words[base];
        if w0 == 0 {
            return None; // Empty
        }
        if w0 >= 2 {
            return Some((ALBEDO, 1.0)); // Uniform(block = w0-2)
        }
        if depth >= budget {
            let mut solid = 0u32;
            for i in 0..8 {
                let c = words[base + 1 + i] as usize;
                let cbase = c * GPU_NODE_STRIDE;
                if cbase + 9 < words.len() && words[cbase] != 0 {
                    solid += 1;
                }
            }
            return if solid > 0 {
                Some((ALBEDO, solid as f32 / 8.0))
            } else {
                None
            };
        }
        let half = [size[0] * 0.5, size[1] * 0.5, size[2] * 0.5];
        let dx = (p[0] >= org[0] + half[0]) as usize;
        let dy = (p[1] >= org[1] + half[1]) as usize;
        let dz = (p[2] >= org[2] + half[2]) as usize;
        let ci = dx + dy * 2 + dz * 4;
        org[0] += half[0] * dx as f32;
        org[1] += half[1] * dy as f32;
        org[2] += half[2] * dz as f32;
        size = half;
        node_idx = words[base + 1 + ci] as usize;
        depth += 1;
        if depth > 32 {
            return None; // 安全弁 (WGSL と同一)
        }
    }
}

/// WGSL `main` 1 コーンぶんの Rust 精密ミラー (同一演算順)。
/// 戻り値 `[r, g, b, alpha]` (alpha は 0..1 clamp 済)。
pub fn trace_cone_words(
    words: &[u32],
    bounds: [f32; 3],
    root: u32,
    cap: u32,
    cone: &ConeWgsl,
) -> [f32; 4] {
    let mut accum = [0.0f32; 3];
    let mut aa = 0.0f32;
    let mut dist = 0.5f32;
    loop {
        if !(dist < cone.max_dist && aa < 0.99) {
            break;
        }
        let pos = [
            cone.o[0] + cone.d[0] * dist,
            cone.o[1] + cone.d[1] * dist,
            cone.o[2] + cone.d[2] * dist,
        ];
        let diameter = (2.0 * cone.aperture * dist).max(1.0);
        let lod = lod_from_diameter_wgsl(diameter);
        if let Some((col, a)) = sample_lod_words(words, bounds, root, cap, pos, lod) {
            if a > 0.001 {
                let weight = a * (1.0 - aa);
                accum[0] += col[0] * weight;
                accum[1] += col[1] * weight;
                accum[2] += col[2] * weight;
                aa += weight;
            }
        }
        dist += diameter * 0.75;
    }
    [accum[0], accum[1], accum[2], aa.clamp(0.0, 1.0)]
}

/// シーン (SVO) の GPU ミラー一式。CPU 参照と GPU dispatch の両方が
/// この同一データを使う (語列一元化でミラー対ミラーの比較となる)。
pub struct VctScene {
    pub words: Vec<u32>,
    pub bounds: [f32; 3],
    pub root: u32,
    pub cap: u32,
}

impl VctScene {
    pub fn from_svo(svo: &SparseVoxelOctree) -> Self {
        Self {
            words: svo.to_gpu_words(),
            bounds: [
                svo.bounds[0] as f32,
                svo.bounds[1] as f32,
                svo.bounds[2] as f32,
            ],
            root: svo.root,
            cap: svo.max_depth,
        }
    }

    /// CPU 参照実行 (WGSL 精密ミラー)。GPU 非依存。
    pub fn trace_cpu(&self, cones: &[ConeWgsl]) -> Vec<[f32; 4]> {
        cones
            .iter()
            .map(|c| trace_cone_words(&self.words, self.bounds, self.root, self.cap, c))
            .collect()
    }
}

/// GPU VCT パス (実 dispatch 1 本 + readback)。iGPU コア機能のみ。
pub struct GpuVct {
    params_buf: wgpu::Buffer,
    nodes_buf: wgpu::Buffer,
    cones_buf: wgpu::Buffer,
    out_buf: wgpu::Buffer,
    out_staging: wgpu::Buffer,
    pipeline: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
    nodes_capacity: usize, // u32 数
    cones_capacity: usize, // コーン数
    scene: Option<VctParams>,
}

impl GpuVct {
    /// 初期容量 (コーン 65,536・ノード 262,144 語 = 2.6 万ノード)。超過時は
    /// set_scene / update が自動で拡張する。
    pub fn new(device: &wgpu::Device) -> Self {
        let mk_buf = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size.max(16),
                usage,
                mapped_at_creation: false,
            })
        };
        let params_buf = mk_buf(
            "Rsift VCT Params",
            std::mem::size_of::<VctParams>() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let nodes_capacity = 262_144usize;
        let nodes_buf = mk_buf(
            "Rsift VCT SVO Nodes",
            (nodes_capacity * 4) as u64,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let cones_capacity = 65_536usize;
        let cones_buf = mk_buf(
            "Rsift VCT Cones",
            (cones_capacity * 32) as u64,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let out_buf = mk_buf(
            "Rsift VCT Radiance Out",
            (cones_capacity * 16) as u64,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let out_staging = mk_buf(
            "Rsift VCT Radiance Staging",
            (cones_capacity * 16) as u64,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Rsift VCT WGSL"),
            source: wgpu::ShaderSource::Wgsl(VCT_WGSL.into()),
        });

        let buf_entry = |binding: u32, ty: wgpu::BufferBindingType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Rsift VCT BGL"),
            entries: &[
                buf_entry(0, wgpu::BufferBindingType::Uniform),
                buf_entry(1, wgpu::BufferBindingType::Storage { read_only: true }),
                buf_entry(2, wgpu::BufferBindingType::Storage { read_only: true }),
                buf_entry(3, wgpu::BufferBindingType::Storage { read_only: false }),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Rsift VCT Pipeline Layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Rsift VCT Pipeline"),
            layout: Some(&layout),
            module: &module,
            entry_point: "main",
            compilation_options: Default::default(),
        });

        Self {
            params_buf,
            nodes_buf,
            cones_buf,
            out_buf,
            out_staging,
            pipeline,
            bgl,
            nodes_capacity,
            cones_capacity,
            scene: None,
        }
    }

    fn grow(
        device: &wgpu::Device,
        label: &str,
        size: u64,
        usage: wgpu::BufferUsages,
    ) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: size.max(16),
            usage,
            mapped_at_creation: false,
        })
    }

    /// 実シーン (SVO 語列) をアップロード。容量超過時はバッファ再確保。
    pub fn set_scene(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, scene: &VctScene) {
        if scene.words.len() > self.nodes_capacity {
            self.nodes_capacity = scene.words.len().next_power_of_two();
            self.nodes_buf = Self::grow(
                device,
                "Rsift VCT SVO Nodes",
                (self.nodes_capacity * 4) as u64,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            );
        }
        queue.write_buffer(&self.nodes_buf, 0, bytemuck::cast_slice(&scene.words));
        self.scene = Some(VctParams {
            bounds: scene.bounds,
            root: scene.root,
            cap: scene.cap,
            node_count: (scene.words.len() / GPU_NODE_STRIDE) as u32,
            cone_count: 0,
            _pad: 0,
        });
    }

    /// 実コーン列を dispatch して readback する (実実行、結果は実バッファ由来)。
    /// `set_scene` 済みであること。戻り値はコーン列と同順の [r,g,b,alpha]。
    /// 空コーン列は GPU 演算を行わず空を返す (0 サイズ dispatch/copy/map の
    /// ドライバ厳格性エッジを回避。結果は dispatch 経路と決定的に同値)。
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cones: &[ConeWgsl],
    ) -> Result<Vec<[f32; 4]>, String> {
        let mut params = self
            .scene
            .ok_or_else(|| "GpuVct: set_scene 未呼び出し".to_string())?;
        if cones.is_empty() {
            return Ok(Vec::new());
        }
        if cones.len() > self.cones_capacity {
            self.cones_capacity = cones.len().next_power_of_two();
            self.cones_buf = Self::grow(
                device,
                "Rsift VCT Cones",
                (self.cones_capacity * 32) as u64,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            );
            self.out_buf = Self::grow(
                device,
                "Rsift VCT Radiance Out",
                (self.cones_capacity * 16) as u64,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            );
            self.out_staging = Self::grow(
                device,
                "Rsift VCT Radiance Staging",
                (self.cones_capacity * 16) as u64,
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            );
        }
        params.cone_count = cones.len() as u32;
        queue.write_buffer(&self.params_buf, 0, bytemuck::bytes_of(&params));
        if !cones.is_empty() {
            queue.write_buffer(&self.cones_buf, 0, bytemuck::cast_slice(cones));
        }

        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Rsift VCT BG"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.nodes_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.cones_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.out_buf.as_entire_binding(),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Rsift VCT Encoder"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Rsift VCT Pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups(cones.len().div_ceil(64) as u32, 1, 1);
        }
        encoder.copy_buffer_to_buffer(
            &self.out_buf,
            0,
            &self.out_staging,
            0,
            (cones.len() * 16) as u64,
        );
        queue.submit([encoder.finish()]);

        // readback (frame_hiz::read_u32 と同一の map+Wait 実パターン)
        let n = cones.len() * 16;
        let slice = self.out_staging.slice(..n as u64);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        while rx.try_recv().is_err() {
            let _ = device.poll(wgpu::Maintain::Wait);
        }
        let data = slice.get_mapped_range();
        let mut out = Vec::with_capacity(cones.len());
        for i in 0..cones.len() {
            let s = i * 16;
            let mut px = [0.0f32; 4];
            for (j, v) in px.iter_mut().enumerate() {
                let b = s + j * 4;
                *v = f32::from_le_bytes([data[b], data[b + 1], data[b + 2], data[b + 3]]);
            }
            out.push(px);
        }
        drop(data);
        self.out_staging.unmap();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary_greedy_meshing::demo_column_palettes;
    use crate::svo::GPU_NODE_STRIDE as STRIDE;
    use crate::voxel_cone_tracing::{ConeRay, VoxelConeTracing};

    /// WGSL 実妥当性 + エントリ実在 (naga、GPU 不要の真の検証)。
    #[test]
    fn vct_wgsl_parse() {
        let module = naga::front::wgsl::parse_str(VCT_WGSL)
            .unwrap_or_else(|e| panic!("voxel_cone_tracing WGSL invalid: {e}"));
        assert!(
            module.entry_points.iter().any(|f| f.name == "main"),
            "entry point 'main' not found"
        );
    }

    /// WGSL struct とのレイアウト一致性 (params 32B / cone 32B)。
    #[test]
    fn vct_struct_layout_matches_wgsl() {
        assert_eq!(std::mem::size_of::<VctParams>(), 32);
        assert_eq!(std::mem::size_of::<ConeWgsl>(), 32);
        assert_eq!(STRIDE, 10);
    }

    /// naga が計算する WGSL `VctParams` (uniform) / `ConeWgsl` (storage) の
    /// メンバ offset が Rust repr(C) と逐語一致すること。
    /// (size 一致だけでは member 順の入替や pad 位置の違いを検出できない —
    ///  vec3<f32> の align 16 規則で uniform/storage 両空間とも同じ並び
    ///  になることを offset まで固定する。GPU 無しでドリフト検出可能)
    #[test]
    fn vct_struct_offsets_match_wgsl_exact() {
        use std::mem::offset_of;
        // Rust 側 repr(C) offset
        assert_eq!(offset_of!(VctParams, bounds), 0);
        assert_eq!(offset_of!(VctParams, root), 12);
        assert_eq!(offset_of!(VctParams, cap), 16);
        assert_eq!(offset_of!(VctParams, node_count), 20);
        assert_eq!(offset_of!(VctParams, cone_count), 24);
        assert_eq!(offset_of!(VctParams, _pad), 28);
        assert_eq!(offset_of!(ConeWgsl, o), 0);
        assert_eq!(offset_of!(ConeWgsl, aperture), 12);
        assert_eq!(offset_of!(ConeWgsl, d), 16);
        assert_eq!(offset_of!(ConeWgsl, max_dist), 28);

        // WGSL 側 (naga 計算 offset)
        let module = naga::front::wgsl::parse_str(VCT_WGSL)
            .unwrap_or_else(|e| panic!("vct WGSL invalid: {e}"));
        let find = |name: &str| {
            module
                .types
                .iter()
                .find_map(|(_, t)| {
                    if t.name.as_deref() == Some(name) {
                        let naga::TypeInner::Struct { members, span } = &t.inner else {
                            panic!("{name} must be struct");
                        };
                        let m: Vec<(String, u32)> = members
                            .iter()
                            .map(|m| (m.name.clone().unwrap_or_default(), m.offset))
                            .collect();
                        Some((m, *span))
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| panic!("struct {name} must exist"))
        };
        let (params, pspan) = find("VctParams");
        assert_eq!(pspan, 32);
        assert_eq!(
            params,
            vec![
                ("bounds".to_string(), 0),
                ("root".to_string(), 12),
                ("cap".to_string(), 16),
                ("node_count".to_string(), 20),
                ("cone_count".to_string(), 24),
                ("_pad".to_string(), 28),
            ]
        );
        let (cone, cspan) = find("ConeWgsl");
        assert_eq!(cspan, 32);
        assert_eq!(
            cone,
            vec![
                ("o".to_string(), 0),
                ("aperture".to_string(), 12),
                ("d".to_string(), 16),
                ("max_dist".to_string(), 28),
            ]
        );
    }

    /// ゼロ方向コーンの定義: 包含セルを max_dist まで繰り返しサンプルする
    /// (doc 明記の退化意味論)。体積内では飽和ヒット、体積外ではミス。
    #[test]
    fn zero_direction_cone_samples_containing_cell() {
        // 全面占有カラム: 内部の退化コーンは alpha 飽和 (containing cell ヒット)
        let solid = [[1u16; 4096]; 4];
        let svo = SparseVoxelOctree::from_column(&solid);
        let scene = VctScene::from_svo(&svo);
        let inside = ConeWgsl::new_normalized([8.0, 32.0, 8.0], [0.0, 0.0, 0.0], 0.577, 24.0);
        let r = trace_cone_words(&scene.words, scene.bounds, scene.root, scene.cap, &inside);
        assert!(r[3] >= 0.99, "zero-dir inside solid must saturate: {r:?}");
        // 体積外の原点では何にも当たらない
        let outside = ConeWgsl::new_normalized([100.0, 32.0, 8.0], [0.0, 0.0, 0.0], 0.577, 24.0);
        let r = trace_cone_words(&scene.words, scene.bounds, scene.root, scene.cap, &outside);
        assert_eq!(r[3], 0.0, "zero-dir outside must miss: {r:?}");
        // 正規化が非ゼロ単位長を維持すること (bit 安定)
        let n = ConeWgsl::new_normalized([0.0; 3], [3.0, 0.0, 4.0], 0.5, 10.0);
        let len = (n.d[0] * n.d[0] + n.d[1] * n.d[1] + n.d[2] * n.d[2]).sqrt();
        assert!((len - 1.0).abs() < 1e-6, "normalized dir length: {len}");
    }

    /// CPU 参照の空コーン列は空結果 (`GpuVct::update` の空早期 return と同値)。
    #[test]
    fn empty_cone_list_is_empty_result() {
        let (_svo, scene) = demo_scene();
        let out = scene.trace_cpu(&[]);
        assert!(out.is_empty());
    }

    /// lod の**真の契約** (bucket 意味論): 2^lod ≤ diameter < 2^(lod+1)。
    /// 2 冪境界 ±ulp でも厳密に成立する (旧 log2f 丸めアーティファクトは
    /// この契約を境界 ulp で破っていた → 共有 helper 化で解消済)。
    /// WGSL も同一コード規則なので、GPU/CPU の LOD は構造的に bitwise 一致。
    #[test]
    fn lod_bucket_semantics_exact() {
        let mut cases: Vec<f32> = vec![0.0, f32::MIN_POSITIVE, 1.0, f32::MAX, f32::NAN];
        for k in -20i32..30 {
            let p = (2.0f32).powi(k);
            cases.push(p);
            cases.push(f32::from_bits(p.to_bits().wrapping_sub(1))); // p の 1ulp 下
            cases.push(f32::from_bits(p.to_bits() + 1)); // p の 1ulp 上
            cases.push(p * 1.5);
            cases.push(p * 1.999);
        }
        for d in cases {
            let lod = lod_from_diameter_wgsl(d);
            if d > 1.0 && d.is_finite() {
                let lo = (2.0f32).powi(lod as i32);
                let hi = (2.0f32).powi(lod as i32 + 1);
                assert!(
                    d >= lo && (lod >= 10 || d < hi),
                    "bucket 契約違反 d={d:e}: 2^{lod}={lo:e} ≤ d < 2^{}={hi:e}",
                    lod + 1
                );
            } else {
                assert_eq!(lod, 0, "非正規/NaN/<=1.0 は lod 0: d={d:e}");
            }
        }
        // 境界の具体的回帰値: 8.0 の 1ulp 下は 2 (旧 log2 経路では 3 に跳んだ)
        let below8 = f32::from_bits(0x40ff_ffff);
        assert_eq!(lod_from_diameter_wgsl(below8), 2);
        assert_eq!(lod_from_diameter_wgsl(8.0), 3);
    }

    fn demo_scene() -> (SparseVoxelOctree, VctScene) {
        let palettes = demo_column_palettes(0, 0);
        let svo = SparseVoxelOctree::from_column(&palettes);
        let scene = VctScene::from_svo(&svo);
        (svo, scene)
    }

    /// 決定的 LCG (外部乱数依存を避ける)。
    struct Lcg(u64);
    impl Lcg {
        fn next_f32(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 40) as f32) / ((1u64 << 24) as f32)
        }
    }

    /// **最重要**: 語列 WGSL ミラー (`trace_cone_words`) がオブジェクトツリー
    /// 実経路 (`trace_diffuse_cone` + `sample_lod`) と **bitwise 一致** すること。
    /// GPU dispatch は語列ミラーと同一規則なので、この一致が GPU==CPU の根拠。
    #[test]
    fn words_mirror_matches_object_tree_bitexact() {
        let (svo, scene) = demo_scene();
        let mut rng = Lcg(0x5eed_cafe_d15e_u64);
        let mut checked = 0u32;
        for _ in 0..128 {
            let o = [
                rng.next_f32() * 28.0 - 6.0,
                rng.next_f32() * 80.0 - 8.0,
                rng.next_f32() * 28.0 - 6.0,
            ];
            let d = [
                rng.next_f32() * 2.0 - 1.0,
                rng.next_f32() * 2.0 - 1.0,
                rng.next_f32() * 2.0 - 1.0,
            ];
            let cone = ConeWgsl::new_normalized(o, d, 0.577, 24.0);
            let got = trace_cone_words(&scene.words, scene.bounds, scene.root, scene.cap, &cone);
            let (col, alpha) = VoxelConeTracing::trace_diffuse_cone(
                &svo,
                &ConeRay {
                    origin: cone.o,
                    dir: cone.d,
                    aperture: cone.aperture,
                    max_dist: cone.max_dist,
                },
            );
            let expect = [col[0], col[1], col[2], alpha];
            assert_eq!(
                got.map(f32::to_bits),
                expect.map(f32::to_bits),
                "cone o={:?} d={:?}: words ミラーと実ツリー経路が bitwise 不一致",
                cone.o,
                cone.d
            );
            checked += 1;
        }
        assert_eq!(checked, 128);
    }

    /// 実シーンでの意味論: 空コーン alpha=0 / 地中コーン alpha>0・色=中立 albedo。
    #[test]
    fn sky_and_ground_cones_semantics() {
        let (_svo, scene) = demo_scene();
        // 上空から真上 (空): 何にも当たらない
        let sky = ConeWgsl::new_normalized([8.0, 70.0, 8.0], [0.0, 1.0, 0.0], 0.577, 24.0);
        let r = trace_cone_words(&scene.words, scene.bounds, scene.root, scene.cap, &sky);
        assert_eq!(r[3], 0.0, "sky cone alpha must be 0: {r:?}");
        // 上空から真下 (地形): 占有に到達して alpha>0、色は中立 albedo 0.5
        let ground = ConeWgsl::new_normalized([8.0, 70.0, 8.0], [0.0, -1.0, 0.0], 0.577, 64.0);
        let r = trace_cone_words(&scene.words, scene.bounds, scene.root, scene.cap, &ground);
        assert!(r[3] > 0.0, "ground cone alpha must be > 0: {r:?}");
        assert!(
            (r[0] - 0.5).abs() < 1e-6 && (r[1] - 0.5).abs() < 1e-6 && (r[2] - 0.5).abs() < 1e-6,
            "hit color must be neutral albedo 0.5: {r:?}"
        );
    }

    /// コーンループが alpha>=0.99 で早期終了する規則 (蓄積が 1.0 に近づく経路)。
    #[test]
    fn solid_world_saturates_alpha() {
        // 全面占有カラム (実 builder 経由)
        let solid = [[1u16; 4096]; 4];
        let svo = SparseVoxelOctree::from_column(&solid);
        let scene = VctScene::from_svo(&svo);
        let cone = ConeWgsl::new_normalized([8.0, 32.0, 8.0], [1.0, 0.0, 0.0], 0.577, 64.0);
        let r = trace_cone_words(&scene.words, scene.bounds, scene.root, scene.cap, &cone);
        assert!(r[3] >= 0.99, "solid 内コーンは alpha 飽和: {r:?}");
        assert!((r[0] - 0.5).abs() < 1e-6);
    }
}

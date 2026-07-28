//! # RsGraphics 進化 Phase D2 — DDGI probe volume 実 GPU dispatch (`GpuDdgi`)
//!
//! 既存 `ddgi.rs` の octahedral encode/decode + Chebyshev 可視率のアイデアを、
//! **実 SVO データ + 実 probe ray-march + 実 octahedral atlas blend** の
//! 2 compute pass で GPU 実行し、atlas を readback して GI 可視率として
//! 実消費 (実フレーム画素の変調) できる状態にする。
//!
//! ## データ駆動経路 (フェイク無し)
//! 1. 実 SVO (`svo.rs` 修正後ツリー) → 語列 (D1 の `to_gpu_words` と同一)
//! 2. ray 方向 (`fibonacci_dirs`、CPU 生成 f32 → GPU アップロード、両者同一ビット)
//! 3. Pass 1 `probe_rays`: 各プローブ×ray を SVO 占有 march (0.5 刻み、
//!    `svo_sample_lod` 共有規則 = `voxel_cone_tracing.wgsl` と共有領域バイト同一)
//! 4. Pass 2 `atlas_blend`: octahedral texel 毎に dot^4 重みで
//!    sky 見通し irradiance + Chebyshev 用 moments を蓄積
//! 5. readback → CPU 精密ミラーと bitwise 検証 → `DdgiAtlas::sample_visibility`
//!    (トライリニア + Chebyshev) で実画素の GI 可視率として還元
//!
//! 決定性設計 (iGPU コア): trig/pow 不使用 (方向は CPU 供給・二乗×2)、
//! sqrt/除算は IEEE 厳密丸め (naga WGSL 規格)、蓄積順固定。
//!
//! ## 体積定義の契約 (`DdgiVolumeDef::validate` で GPU/CPU 両入口が強制)
//! - `oct_w ≥ 1` (texel 0 の atlas は定義不能、sample_visibility で OOB に化ける)
//! - `ray_count ∈ 1..=64` (GPU rays_buf はプローブ毎 64 スロット固定設計)
//! - `0 < max_dist ≤ 65.0` (probe_rays march は t=0.5 から 0.5 刻み 129 開始値、
//!   t < max_dist の全格子点を完全走査できる限界)
//! - `sky` 全成分 > 0 (可視率正規化の分母)、`dims` 各成分 ≥ 1 かつ積は usize 内

use crate::frame_vct::sample_lod_words;

pub const DDGI_WGSL: &str = include_str!("../shaders/ddgi.wgsl");

/// WGSL `DdgiParams` と一致 (64B)。
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DdgiParams {
    pub origin: [f32; 3],
    pub max_dist: f32,
    pub cell: [f32; 3],
    pub probe_count: u32,
    pub dims: [u32; 3],
    pub ray_count: u32,
    pub sky: [f32; 3],
    pub oct_w: u32,
}

/// DDGI プローブ体積の定義 (CPU 側設定 + params 生成)。
#[derive(Clone, Copy, Debug)]
pub struct DdgiVolumeDef {
    pub origin: [f32; 3],
    pub cell: [f32; 3],
    pub dims: [u32; 3],
    pub max_dist: f32,
    pub sky: [f32; 3],
    pub oct_w: u32,
    pub ray_count: u32,
}

impl DdgiVolumeDef {
    pub fn probe_count(&self) -> usize {
        (self.dims[0] * self.dims[1] * self.dims[2]) as usize
    }
    pub fn texel_count(&self) -> usize {
        (self.oct_w * self.oct_w) as usize
    }
    pub fn params(&self) -> DdgiParams {
        DdgiParams {
            origin: self.origin,
            max_dist: self.max_dist,
            cell: self.cell,
            probe_count: self.probe_count() as u32,
            dims: self.dims,
            ray_count: self.ray_count,
            sky: self.sky,
            oct_w: self.oct_w,
        }
    }
    /// プローブの SVO ローカル中心座標 (WGSL `probe_rays` 内の base と同一演算順)。
    pub fn probe_base(&self, p: u32) -> [f32; 3] {
        let dx = p % self.dims[0];
        let dy = (p / self.dims[0]) % self.dims[1];
        let dz = p / (self.dims[0] * self.dims[1]);
        [
            self.origin[0] + (dx as f32 + 0.5) * self.cell[0],
            self.origin[1] + (dy as f32 + 0.5) * self.cell[1],
            self.origin[2] + (dz as f32 + 0.5) * self.cell[2],
        ]
    }

    /// 体積定義の契約検証 (GPU `set_inputs` / CPU `ddgi_update_cpu` の両入口で
    /// 強制する純粋関数 — GPU なしの環境でも全契約をテスト可能にするため
    /// 検証ロジックはここに一元化する)。
    ///
    /// 契約 (全てモジュール固定セマンティクスから導出):
    /// - `oct_w ≥ 1`: texel 0 の atlas は定義不能。`sample_visibility` では
    ///   `oct_w - 1` の u32 アンダーフロー経由で mom/irr の OOB パニックに化ける。
    /// - `ray_count ∈ 1..=64`: GPU `rays_buf` はプローブ毎 **64 スロット固定**
    ///   設計のため、65 以上は storage 配列の静かな OOB 書き込みになる。
    /// - `0 < max_dist ≤ 65.0`: march は t=0.5 から 0.5 刻み・129 開始値
    ///   (`0..=128`)。t < max_dist の全格子点を完全走査できるのは
    ///   max_dist ≤ 65.0 のときのみ (それ超過は打ち切り漏れ = 偽の sky)。
    /// - `dims` 各成分 ≥ 1 かつ積が usize 内 (probe_count オーバーフロー拒否)。
    /// - `sky` 全成分 > 0: `sample_visibility` の正規化分母
    ///   (CPU 直構築経路でも NaN 伝播を防ぐ)。
    pub fn validate(&self) -> Result<(), String> {
        if self.oct_w == 0 {
            return Err("oct_w は 1 以上必須 (texel 0 の atlas は定義不能)".to_string());
        }
        if self.ray_count == 0 || self.ray_count > 64 {
            return Err(format!(
                "ray_count は 1..=64 必須 (rays_buf はプローブ毎 64 スロット固定): {}",
                self.ray_count
            ));
        }
        if !(self.max_dist > 0.0 && self.max_dist <= 65.0) {
            return Err(format!(
                "max_dist は (0, 65.0] 必須 (march 129 開始値格子の完全走査条件): {}",
                self.max_dist
            ));
        }
        if self.dims.iter().any(|&d| d == 0) {
            return Err("dims の各成分は 1 以上必須 (probe 0 の体積は無意味)".to_string());
        }
        if self
            .dims
            .iter()
            .try_fold(1usize, |a, &d| a.checked_mul(d as usize))
            .is_none()
        {
            return Err("dims の積が usize を超過 (probe_count オーバーフロー)".to_string());
        }
        for (i, c) in self.sky.iter().enumerate() {
            if !(*c > 0.0) {
                return Err(format!("sky[{i}] は > 0 必須 (正規化分母): {c}"));
            }
        }
        Ok(())
    }
}

/// 実 ray 方向列 (Fibonacci sphere、f64 生成 → f32)。
/// CPU ミラーと GPU の双方が**この同一ビット列**を使う (trig を WGSL に
/// 入れない設計: GPU 実装定義誤差の排除)。
pub fn fibonacci_dirs(n: usize) -> Vec<[f32; 3]> {
    let ga = std::f64::consts::PI * (3.0 - 5.0f64.sqrt());
    (0..n)
        .map(|i| {
            let t = (i as f64 + 0.5) / n as f64;
            let y = 1.0 - 2.0 * t;
            let r = (1.0 - y * y).max(0.0).sqrt();
            let phi = i as f64 * ga;
            [r * phi.cos(), y, r * phi.sin()].map(|v| v as f32)
        })
        .collect()
}

// ---------------- WGSL 精密ミラー (CPU 参照実装) ----------------

fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}
fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// WGSL `octEncode` と同一演算 (標準 diamond wrap)。
pub fn oct_encode_wgsl(n: [f32; 3]) -> [f32; 2] {
    // 【wave 160 FF 捕捉 87】WGSL 語彙の唯一 CPU 参照 `ddgi::oct_encode_unit`
    // へ委譲 (式ツリー同一のため corpus 全域で旧複製実装と bit 同一 —
    // probe: enc 8/8・dec 14/14 bit 一致、ddgi tests の bit 同一 pin で
    // 恒常監視)。旧来は二重実装のまま ddgi 本家が非標準 wrap のまま
    // 腐っていた (捕捉 86) のを単一真実へ統合。
    let (x, y) = crate::ddgi::oct_encode_unit(crate::ddgi::Vec3::new(n[0], n[1], n[2]));
    [x, y]
}

/// WGSL `octDecode` と同一演算 (normalize = IEEE sqrt 厳密)。
pub fn oct_decode_wgsl(f: [f32; 2]) -> [f32; 3] {
    // 【wave 160 FF 捕捉 87】同上: `ddgi::oct_decode_unit` へ委譲
    // (bit 同一、ddgi tests pin)。
    let v = crate::ddgi::oct_decode_unit((f[0], f[1]));
    [v.x, v.y, v.z]
}

/// WGSL 版の uv → 最近傍 texel (blend で書いた atlas の参照逆変換)。
pub fn oct_texel_from_dir_wgsl(dir: [f32; 3], oct_w: u32) -> (u32, u32) {
    let uv = oct_encode_wgsl(dir);
    let conv = |u: f32| -> u32 {
        let t = ((u + 1.0) * 0.5 * oct_w as f32).floor();
        // partial_cmp 版: NaN → 0 (WGSL の !(t > 0.0) と同じ規則)
        if !matches!(t.partial_cmp(&0.0), Some(std::cmp::Ordering::Greater)) {
            0
        } else if t >= oct_w as f32 {
            oct_w - 1
        } else {
            t as u32
        }
    };
    (conv(uv[0]), conv(uv[1]))
}

/// Pass 1 `probe_rays` の Rust 精密ミラー (1 ray ぶん)。命中距離/miss 時 max_dist。
pub fn probe_march_cpu(
    words: &[u32],
    bounds: [f32; 3],
    root: u32,
    cap: u32,
    base: [f32; 3],
    dir: [f32; 3],
    max_dist: f32,
) -> f32 {
    let mut t = 0.5f32;
    let mut hit = max_dist;
    for _step in 0..=128u32 {
        if t >= max_dist {
            break;
        }
        let pos = [
            base[0] + dir[0] * t,
            base[1] + dir[1] * t,
            base[2] + dir[2] * t,
        ];
        if let Some((_, a)) = sample_lod_words(words, bounds, root, cap, pos, 0) {
            if a > 0.001 {
                hit = t;
                break;
            }
        }
        t += 0.5;
    }
    hit
}

/// Pass 2 `atlas_blend` の Rust 精密ミラー (1 texel ぶん)。
/// `rays` は当該プローブの ray スライス (ray_count 個)。
pub fn atlas_blend_cpu(
    dirs: &[[f32; 3]],
    rays: &[f32],
    max_dist: f32,
    sky: [f32; 3],
    oct_w: u32,
    texel: u32,
) -> ([f32; 4], [f32; 2]) {
    let tx = texel % oct_w;
    let ty = texel / oct_w;
    let uv = [
        (tx as f32 + 0.5) / oct_w as f32 * 2.0 - 1.0,
        (ty as f32 + 0.5) / oct_w as f32 * 2.0 - 1.0,
    ];
    let dir = oct_decode_wgsl(uv);
    let mut irr = [0.0f32; 3];
    let mut wm = 0.0f32;
    let mut m1 = 0.0f32;
    let mut m2 = 0.0f32;
    for (r, rd) in dirs.iter().enumerate() {
        let mut w = clamp01(dot3(dir, *rd));
        w *= w;
        w *= w; // dot^4
        let d = rays[r];
        let sky_w = if d >= max_dist { 1.0f32 } else { 0.0f32 };
        irr[0] += sky[0] * (sky_w * w);
        irr[1] += sky[1] * (sky_w * w);
        irr[2] += sky[2] * (sky_w * w);
        wm += w;
        m1 += d * w;
        m2 += d * d * w;
    }
    if wm > 0.0 {
        irr[0] /= wm;
        irr[1] /= wm;
        irr[2] /= wm;
        m1 /= wm;
        m2 /= wm;
    }
    ([irr[0], irr[1], irr[2], 1.0], [m1, m2])
}

/// DDGI 更新結果 (irradiance atlas, moments atlas)。GPU readback と同順。
pub type DdgiUpdateOutput = (Vec<[f32; 4]>, Vec<[f32; 2]>);

/// DDGI 更新 1 サイクルの CPU 参照 (WGSL 精密ミラー駆動)。
/// 戻り値は GPU readback と同順。
pub fn ddgi_update_cpu(
    words: &[u32],
    bounds: [f32; 3],
    root: u32,
    cap: u32,
    vol: &DdgiVolumeDef,
    dirs: &[[f32; 3]],
) -> DdgiUpdateOutput {
    // GPU set_inputs と同一契約を CPU 経路でも強制 (rays_buf 64 スロット設計と
    // march 129 開始値・sky 正規化分母は CPU 側にも等しく効く制約)。
    vol.validate()
        .expect("DdgiVolumeDef::validate の契約違反");
    assert_eq!(
        dirs.len(),
        vol.ray_count as usize,
        "ray 方向数が defs と不一致"
    );
    let probes = vol.probe_count();
    let tex = vol.texel_count();
    let mut irr = vec![[0.0; 4]; probes * tex];
    let mut mom = vec![[0.0; 2]; probes * tex];
    let mut rays = vec![0.0f32; vol.ray_count as usize];
    for p in 0..probes as u32 {
        let base = vol.probe_base(p);
        for (r, rd) in dirs.iter().enumerate() {
            rays[r] = probe_march_cpu(words, bounds, root, cap, base, *rd, vol.max_dist);
        }
        for t in 0..tex as u32 {
            let (i4, m2v) = atlas_blend_cpu(dirs, &rays, vol.max_dist, vol.sky, vol.oct_w, t);
            irr[p as usize * tex + t as usize] = i4;
            mom[p as usize * tex + t as usize] = m2v;
        }
    }
    (irr, mom)
}

/// probe irradiance/moments atlas (CPU 計算 or GPU readback、同一レイアウト)。
pub struct DdgiAtlas {
    pub vol: DdgiVolumeDef,
    pub irr: Vec<[f32; 4]>,
    pub mom: Vec<[f32; 2]>,
}

impl DdgiAtlas {
    /// 実サンプラ: トライリニア 8 プローブ × Chebyshev 浅埋め漏れ抑制 ×
    /// octahedral 最近傍 texel で、ワールド点の sky 可視率 v ∈ [0,1] を返す。
    /// (frame_proof cpu-ddgi/gpu-ddgi が実フレーム画素へ適用する実消費口)
    ///
    /// 前提: `vol` は `DdgiVolumeDef::validate` 適合であること
    /// (`ddgi_update_cpu` 経由で構築した atlas は保証済み)。違反体積では
    /// texel 索引が定義不能になる (oct_w=0 の u32 アンダーフロー等)。
    pub fn sample_visibility(&self, world: [f32; 3]) -> f32 {
        let vol = &self.vol;
        let tex = vol.texel_count();
        // probe 格子座標 (cell 中心基準に 0.5 shift)
        let t = [
            (world[0] - vol.origin[0]) / vol.cell[0] - 0.5,
            (world[1] - vol.origin[1]) / vol.cell[1] - 0.5,
            (world[2] - vol.origin[2]) / vol.cell[2] - 0.5,
        ];
        let mut acc = 0.0f32;
        let mut wsum = 0.0f32;
        for dz in 0..2u32 {
            for dy in 0..2u32 {
                for dx in 0..2u32 {
                    let mut w = 1.0f32;
                    let mut idx = [0u32; 3];
                    let mut ok = true;
                    for (ax, d) in [dx, dy, dz].iter().enumerate() {
                        let base = t[ax].floor();
                        // 端は単側 (境界外 → 欠落として寄与 0)
                        let i = base as i32 + *d as i32;
                        if i < 0 || i >= vol.dims[ax] as i32 {
                            ok = false;
                            break;
                        }
                        idx[ax] = i as u32;
                        w *= if *d == 0 {
                            (base + 1.0) - t[ax]
                        } else {
                            t[ax] - base
                        };
                    }
                    if !ok || w <= 0.0 {
                        continue;
                    }
                    let p = (idx[0] + vol.dims[0] * idx[1] + vol.dims[0] * vol.dims[1] * idx[2])
                        as usize;
                    let pb = vol.probe_base(p as u32);
                    let dv = [world[0] - pb[0], world[1] - pb[1], world[2] - pb[2]];
                    let dist = dot3(dv, dv).sqrt();
                    let dir = if dist > 1e-6 {
                        [dv[0] / dist, dv[1] / dist, dv[2] / dist]
                    } else {
                        [0.0, 1.0, 0.0]
                    };
                    let (tx0, ty0) = oct_texel_from_dir_wgsl(dir, vol.oct_w);
                    let tidx = (ty0 * vol.oct_w + tx0) as usize;
                    let m = self.mom[p * tex + tidx];
                    let cheb = crate::ddgi::chebyshev_visibility(m[0], m[1], dist);
                    let ir = self.irr[p * tex + tidx];
                    // sky 正規化可視率 (各 channels 平均)
                    let v_p = ((ir[0] / vol.sky[0]) + (ir[1] / vol.sky[1]) + (ir[2] / vol.sky[2]))
                        * (1.0 / 3.0);
                    acc += w * cheb * v_p;
                    wsum += w;
                }
            }
        }
        if wsum > 1e-8 {
            acc / wsum
        } else {
            0.0
        }
    }
}

// ---------------- GPU 実 dispatch ----------------

/// GPU DDGI パス (probe_rays + atlas_blend の 2 実 pipeline)。
pub struct GpuDdgi {
    meta_buf: wgpu::Buffer,
    params_buf: wgpu::Buffer,
    nodes_buf: wgpu::Buffer,
    dirs_buf: wgpu::Buffer,
    rays_buf: wgpu::Buffer,
    irr_buf: wgpu::Buffer,
    mom_buf: wgpu::Buffer,
    irr_staging: wgpu::Buffer,
    mom_staging: wgpu::Buffer,
    rays_pipeline: wgpu::ComputePipeline,
    blend_pipeline: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
    nodes_capacity: usize,
    probes_capacity: usize,
    tex_capacity: usize,
    vol: Option<DdgiVolumeDef>,
    scene: Option<crate::frame_vct::VctParams>,
}

impl GpuDdgi {
    pub fn new(device: &wgpu::Device) -> Self {
        let mk = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size.max(16),
                usage,
                mapped_at_creation: false,
            })
        };
        let meta_buf = mk(
            "Rsift DDGI SVO Meta",
            32,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let params_buf = mk(
            "Rsift DDGI Params",
            std::mem::size_of::<DdgiParams>() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let nodes_capacity = 262_144usize;
        let nodes_buf = mk(
            "Rsift DDGI SVO Nodes",
            (nodes_capacity * 4) as u64,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let dirs_buf = mk(
            "Rsift DDGI Ray Dirs",
            4096 * 16,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let probes_capacity = 4096usize;
        let rays_buf = mk(
            "Rsift DDGI Rays Out",
            (probes_capacity * 64 * 4) as u64,
            wgpu::BufferUsages::STORAGE,
        );
        let tex_capacity = 4096 * 64;
        let irr_buf = mk(
            "Rsift DDGI Atlas Irr",
            (tex_capacity * 16) as u64,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let mom_buf = mk(
            "Rsift DDGI Atlas Mom",
            (tex_capacity * 8) as u64,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let irr_staging = mk(
            "Rsift DDGI Irr Staging",
            (tex_capacity * 16) as u64,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );
        let mom_staging = mk(
            "Rsift DDGI Mom Staging",
            (tex_capacity * 8) as u64,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Rsift DDGI WGSL"),
            source: wgpu::ShaderSource::Wgsl(DDGI_WGSL.into()),
        });
        let entry = |binding: u32, ty: wgpu::BufferBindingType| wgpu::BindGroupLayoutEntry {
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
            label: Some("Rsift DDGI BGL"),
            entries: &[
                entry(0, wgpu::BufferBindingType::Uniform),
                entry(1, wgpu::BufferBindingType::Storage { read_only: true }),
                entry(2, wgpu::BufferBindingType::Uniform),
                entry(3, wgpu::BufferBindingType::Storage { read_only: true }),
                entry(4, wgpu::BufferBindingType::Storage { read_only: false }),
                entry(5, wgpu::BufferBindingType::Storage { read_only: false }),
                entry(6, wgpu::BufferBindingType::Storage { read_only: false }),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Rsift DDGI Pipeline Layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let mkpl = |name: &str, ep: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(name),
                layout: Some(&layout),
                module: &module,
                entry_point: ep,
                compilation_options: Default::default(),
            })
        };
        let rays_pipeline = mkpl("Rsift DDGI Probe Rays", "probe_rays");
        let blend_pipeline = mkpl("Rsift DDGI Atlas Blend", "atlas_blend");
        Self {
            meta_buf,
            params_buf,
            nodes_buf,
            dirs_buf,
            rays_buf,
            irr_buf,
            mom_buf,
            irr_staging,
            mom_staging,
            rays_pipeline,
            blend_pipeline,
            bgl,
            nodes_capacity,
            probes_capacity,
            tex_capacity,
            vol: None,
            scene: None,
        }
    }

    fn grow(&mut self, device: &wgpu::Device, vol: &DdgiVolumeDef) {
        let probes = vol.probe_count();
        let tex = vol.texel_count();
        if probes * 64 > self.probes_capacity * 64 || probes > self.probes_capacity {
            self.probes_capacity = probes.next_power_of_two();
            self.rays_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Rsift DDGI Rays Out"),
                size: (self.probes_capacity * 64 * 4) as u64,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            });
        }
        if probes * tex > self.tex_capacity {
            self.tex_capacity = (probes * tex).next_power_of_two();
            let mk = |label: &str, bytes: usize, usage: wgpu::BufferUsages| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: bytes as u64,
                    usage,
                    mapped_at_creation: false,
                })
            };
            self.irr_buf = mk(
                "Rsift DDGI Atlas Irr",
                self.tex_capacity * 16,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            );
            self.mom_buf = mk(
                "Rsift DDGI Atlas Mom",
                self.tex_capacity * 8,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            );
            self.irr_staging = mk(
                "Rsift DDGI Irr Staging",
                self.tex_capacity * 16,
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            );
            self.mom_staging = mk(
                "Rsift DDGI Mom Staging",
                self.tex_capacity * 8,
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            );
        }
    }

    /// 実 SVO 語列 + 体積定義 + ray 方向をアップロード。
    pub fn set_inputs(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &crate::frame_vct::VctScene,
        vol: &DdgiVolumeDef,
        dirs: &[[f32; 3]],
    ) -> Result<(), String> {
        vol.validate()?;
        if dirs.len() != vol.ray_count as usize {
            return Err("ray 方向数が ray_count と不一致".to_string());
        }
        if dirs.len() > 4096 {
            return Err("ray_count > 4096 は dirs_buf 初期容量超過".to_string());
        }
        if scene.words.len() > self.nodes_capacity {
            self.nodes_capacity = scene.words.len().next_power_of_two();
            self.nodes_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Rsift DDGI SVO Nodes"),
                size: (self.nodes_capacity * 4) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        self.grow(device, vol);
        queue.write_buffer(&self.nodes_buf, 0, bytemuck::cast_slice(&scene.words));
        let mut packed_dirs = Vec::with_capacity(dirs.len() * 4);
        for d in dirs {
            packed_dirs.extend_from_slice(&[d[0], d[1], d[2], 0.0]);
        }
        queue.write_buffer(&self.dirs_buf, 0, bytemuck::cast_slice(&packed_dirs));
        queue.write_buffer(&self.params_buf, 0, bytemuck::bytes_of(&vol.params()));
        self.scene = Some(crate::frame_vct::VctParams {
            bounds: scene.bounds,
            root: scene.root,
            cap: scene.cap,
            node_count: (scene.words.len() / crate::svo::GPU_NODE_STRIDE) as u32,
            cone_count: 0,
            _pad: 0,
        });
        self.vol = Some(*vol);
        Ok(())
    }

    /// 2 pass 実 dispatch + readback。(irr atlas, mom atlas) を返す。
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<DdgiUpdateOutput, String> {
        let meta = self
            .scene
            .ok_or_else(|| "GpuDdgi: set_inputs 未呼び出し".to_string())?;
        let vol = self.vol.unwrap();
        queue.write_buffer(&self.meta_buf, 0, bytemuck::bytes_of(&meta));
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Rsift DDGI BG"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.meta_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.nodes_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.dirs_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.rays_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: self.irr_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: self.mom_buf.as_entire_binding(),
                },
            ],
        });
        let probes = vol.probe_count();
        let tex = vol.texel_count();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Rsift DDGI Encoder"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Rsift DDGI Pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.rays_pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups((probes * vol.ray_count as usize).div_ceil(64) as u32, 1, 1);
            pass.set_pipeline(&self.blend_pipeline);
            pass.dispatch_workgroups((probes * tex).div_ceil(64) as u32, 1, 1);
        }
        encoder.copy_buffer_to_buffer(
            &self.irr_buf,
            0,
            &self.irr_staging,
            0,
            (probes * tex * 16) as u64,
        );
        encoder.copy_buffer_to_buffer(
            &self.mom_buf,
            0,
            &self.mom_staging,
            0,
            (probes * tex * 8) as u64,
        );
        queue.submit([encoder.finish()]);

        let irr_raw = read_f32(device, &self.irr_staging, probes * tex * 4);
        let mom_raw = read_f32(device, &self.mom_staging, probes * tex * 2);
        let irr: Vec<[f32; 4]> = irr_raw
            .chunks_exact(4)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect();
        let mom: Vec<[f32; 2]> = mom_raw.chunks_exact(2).map(|c| [c[0], c[1]]).collect();
        Ok((irr, mom))
    }
}

/// staging buffer から f32 列を読み戻す (frame_hiz::read_u32 と同一 map+Wait 実パターン)。
fn read_f32(device: &wgpu::Device, buf: &wgpu::Buffer, count: usize) -> Vec<f32> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary_greedy_meshing::demo_column_palettes;
    use crate::frame_vct::VctScene;
    use crate::svo::SparseVoxelOctree;

    fn demo() -> (SparseVoxelOctree, VctScene) {
        let palettes = demo_column_palettes(0, 0);
        let svo = SparseVoxelOctree::from_column(&palettes);
        let scene = VctScene::from_svo(&svo);
        (svo, scene)
    }

    fn demo_vol() -> DdgiVolumeDef {
        DdgiVolumeDef {
            origin: [0.0, 0.0, 0.0],
            cell: [4.0, 4.0, 4.0],
            dims: [4, 16, 4],
            max_dist: 24.0,
            sky: [0.55, 0.75, 0.95],
            oct_w: 8,
            ray_count: 32,
        }
    }

    /// WGSL 実妥当性 + 2 エントリ実在 (naga、GPU 不要の真の検証)。
    #[test]
    fn ddgi_wgsl_parse_both_entries() {
        let module = naga::front::wgsl::parse_str(DDGI_WGSL)
            .unwrap_or_else(|e| panic!("ddgi WGSL invalid: {e}"));
        for ep in ["probe_rays", "atlas_blend"] {
            assert!(
                module.entry_points.iter().any(|f| f.name == ep),
                "entry point '{ep}' not found"
            );
        }
    }

    /// **共有領域のドリフト防止**: VCT/DDGI 両 WGSL の SVO-TRACE SHARED REGION が
    /// バイト同一であること (片方だけの編集はこのテストで検出される)。
    #[test]
    fn shared_svo_region_byte_identical() {
        let extract = |src: &str| -> String {
            let b = src
                .find("// ==== SVO-TRACE SHARED REGION BEGIN ====")
                .expect("BEGIN marker 無し");
            let e = src
                .find("// ==== SVO-TRACE SHARED REGION END ====")
                .expect("END marker 無し")
                + "// ==== SVO-TRACE SHARED REGION END ====".len();
            src[b..e].to_string()
        };
        let a = extract(crate::frame_vct::VCT_WGSL);
        let b = extract(DDGI_WGSL);
        assert_eq!(a, b, "SVO 走査共有領域が VCT/DDGI で乖離している");
        assert!(
            a.contains("fn svo_sample_lod"),
            "共有領域に走査関数が含まれること"
        );
    }

    /// WGSL struct とのレイアウト一致性 (ddgi params 64B)。
    #[test]
    fn ddgi_params_layout() {
        assert_eq!(std::mem::size_of::<DdgiParams>(), 64);
    }

    /// oct wgsl ミラー組の roundtrip。
    #[test]
    fn oct_wgsl_mirror_roundtrip() {
        let dirs = [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.577, 0.577, 0.577],
            [-0.3, 0.8, -0.5],
        ];
        for d in dirs {
            let l = dot3(d, d).sqrt();
            let dn = [d[0] / l, d[1] / l, d[2] / l];
            let r = oct_decode_wgsl(oct_encode_wgsl(dn));
            assert!(
                dot3(r, dn) > 0.999,
                "oct wgsl roundtrip dot = {}",
                dot3(r, dn)
            );
        }
    }

    /// **解析的真値 (壁の内側)**: 全面占有 SVO では全 ray が t=0.5 で命中、
    /// 全 texel の moments が (0.5, 0.25)、irradiance が 0 になる。
    /// (x*A/A == x の IEEE 性質上、moments は厳密一致を assert できる)
    #[test]
    fn buried_probe_analytic_exact() {
        let solid = [[1u16; 4096]; 4];
        let svo = SparseVoxelOctree::from_column(&solid);
        let scene = VctScene::from_svo(&svo);
        let vol = demo_vol();
        let dirs = fibonacci_dirs(vol.ray_count as usize);
        let words = scene.words.clone();
        let base = vol.probe_base(0);
        // pass1: 全 ray が 0.5 命中
        let mut rays = Vec::new();
        for rd in &dirs {
            let d = probe_march_cpu(
                &words,
                scene.bounds,
                scene.root,
                scene.cap,
                base,
                *rd,
                vol.max_dist,
            );
            assert_eq!(d, 0.5, "buried probe の ray は t=0.5 即命中");
            rays.push(d);
        }
        // pass2: wm>0 の texel は irr=0 / m1=0.5 / m2=0.25 (厳密)
        let mut checked = 0;
        for t in 0..vol.texel_count() as u32 {
            let (i4, m) = atlas_blend_cpu(&dirs, &rays, vol.max_dist, vol.sky, vol.oct_w, t);
            // atlas_blend は wm==0 の時 0 を返す設計: いずれにせよ irr==0
            assert_eq!(i4, [0.0, 0.0, 0.0, 1.0], "buried irr must be 0 (t={t})");
            if m[0] != 0.0 {
                assert_eq!(m, [0.5, 0.25], "buried moments analytic (t={t})");
                checked += 1;
            }
        }
        assert!(checked > 0, "wm>0 の texel が存在すること");
    }

    /// **解析的真値 (空)**: 空 SVO では全 ray miss → irradiance ≈ sky、
    /// moments ≈ (max_dist, max_dist²) (A/Α=1 の除算で 1ulp 未満許容)。
    #[test]
    fn empty_volume_full_sky() {
        let svo = SparseVoxelOctree::empty();
        let scene = VctScene::from_svo(&svo);
        let vol = demo_vol();
        let dirs = fibonacci_dirs(vol.ray_count as usize);
        let words = scene.words.clone();
        let base = vol.probe_base(0);
        let rays: Vec<f32> = dirs
            .iter()
            .map(|rd| {
                probe_march_cpu(
                    &words,
                    scene.bounds,
                    scene.root,
                    scene.cap,
                    base,
                    *rd,
                    vol.max_dist,
                )
            })
            .collect();
        assert!(rays.iter().all(|&d| d == vol.max_dist), "empty は全 miss");
        let mut checked = 0;
        for t in 0..vol.texel_count() as u32 {
            let (i4, m) = atlas_blend_cpu(&dirs, &rays, vol.max_dist, vol.sky, vol.oct_w, t);
            if m[0] != 0.0 {
                for (c, (iv, sv)) in i4.iter().take(3).zip(vol.sky.iter()).enumerate() {
                    assert!(
                        (iv - sv).abs() < 1e-5,
                        "sky irr ≈ sky 値: ch{c} t={t}: {iv} vs {sv}"
                    );
                }
                assert!((m[0] - vol.max_dist).abs() < 1e-4);
                assert!((m[1] - vol.max_dist * vol.max_dist).abs() < 1e-3);
                checked += 1;
            }
        }
        assert!(checked > 0);
    }

    /// **交差アンカー**: DDGI march (語列ミラー) がオブジェクトツリー実
    /// `sample_lod` を直接使った march と bitwise 一致すること。
    #[test]
    fn march_anchors_object_tree_bitexact() {
        let (svo, scene) = demo();
        let vol = demo_vol();
        let dirs = fibonacci_dirs(vol.ray_count as usize);
        let mut checked = 0;
        for p in [0u32, 17, 63, 100, 200, 255] {
            let base = vol.probe_base(p);
            for (ri, rd) in dirs.iter().take(4).enumerate() {
                // オブジェクトツリー直接 march (同一ループ構造)
                let mut t = 0.5f32;
                let mut expect = vol.max_dist;
                for _ in 0..=128 {
                    if t >= vol.max_dist {
                        break;
                    }
                    let pos = [
                        base[0] + rd[0] * t,
                        base[1] + rd[1] * t,
                        base[2] + rd[2] * t,
                    ];
                    if let Some((_, a)) = svo.sample_lod(pos[0], pos[1], pos[2], 0) {
                        if a > 0.001 {
                            expect = t;
                            break;
                        }
                    }
                    t += 0.5;
                }
                let got = probe_march_cpu(
                    &scene.words,
                    scene.bounds,
                    scene.root,
                    scene.cap,
                    base,
                    *rd,
                    vol.max_dist,
                );
                assert_eq!(
                    got.to_bits(),
                    expect.to_bits(),
                    "p={p} ray{ri}: march が実ツリーと bitwise 不一致"
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 24);
    }

    /// サンプラの意味論: 空気中の点は高 v、地中の点は低 v、[0,1] 内。
    #[test]
    fn sampler_semantics_air_vs_ground() {
        let (_svo, scene) = demo();
        let vol = demo_vol();
        let dirs = fibonacci_dirs(vol.ray_count as usize);
        let (irr, mom) = ddgi_update_cpu(
            &scene.words,
            scene.bounds,
            scene.root,
            scene.cap,
            &vol,
            &dirs,
        );
        let atlas = DdgiAtlas { vol, irr, mom };
        let v_air = atlas.sample_visibility([8.0, 60.0, 8.0]);
        let v_ground = atlas.sample_visibility([8.0, 4.0, 8.0]);
        for (name, v) in [("air", v_air), ("ground", v_ground)] {
            assert!((0.0..=1.0).contains(&v), "{name} v in [0,1]: {v}");
        }
        assert!(
            v_air > v_ground,
            "空気 ({v_air}) > 地中 ({v_ground}) の順序: 実遮蔽の方向性が正しいこと"
        );
        // 実測値: demo 地形の slab 3 (y48..59) の真上で v_air≈0.45
        // (下半球が直近の slab 地形に遮られるため — 絶対閾値ではなく
        //  地形構造に基づく実信号)。v_ground は内部埋め込みで ≈0。
        assert!(v_ground < 0.3, "地中は相当に暗いこと: {v_ground}");
        assert!(
            v_air > 0.3,
            "空気は slab 直上でも半球分は明るいこと: {v_air}"
        );
        assert!(v_air - v_ground > 0.1, "遮蔽差が実信号として十分あること");
    }

    /// atlas 決定性 (同入力 → 同ビット列)。
    #[test]
    fn atlas_deterministic() {
        let (_svo, scene) = demo();
        let vol = demo_vol();
        let dirs = fibonacci_dirs(vol.ray_count as usize);
        let a = ddgi_update_cpu(
            &scene.words,
            scene.bounds,
            scene.root,
            scene.cap,
            &vol,
            &dirs,
        );
        let b = ddgi_update_cpu(
            &scene.words,
            scene.bounds,
            scene.root,
            scene.cap,
            &vol,
            &dirs,
        );
        assert_eq!(a.0.len(), b.0.len());
        for (x, y) in a.0.iter().zip(b.0.iter()) {
            assert_eq!(x.map(f32::to_bits), y.map(f32::to_bits));
        }
    }

    /// demo 体積は契約適合であること、および max_dist=65.0 ちょうどが
    /// 境界として受理されること (march 129 開始値で t<65.0 の全格子点を
    /// 走査できる上限)。
    #[test]
    fn demo_volume_passes_validation() {
        demo_vol().validate().expect("demo volume は契約適合");
        let mut v = demo_vol();
        v.max_dist = 65.0;
        v.validate().expect("max_dist=65.0 境界は受理");
    }

    /// 契約違反を全項目で拒否すること (エラーメッセージの識別子で分野を固定)。
    /// 旧実装は ray_count>64 (GPU storage OOB)・oct_w=0 (sample OOB 化) を
    /// 何も検査せず受け入れていた。
    #[test]
    fn validate_rejects_contract_violations() {
        let mut v = demo_vol();
        v.sky = [0.0, 0.7, 0.9];
        assert!(v.validate().unwrap_err().contains("sky"));

        let mut v = demo_vol();
        v.ray_count = 65; // rays_buf(64/probe 固定) 超過 → GPU storage OOB
        assert!(v.validate().unwrap_err().contains("ray_count"));

        let mut v = demo_vol();
        v.ray_count = 0;
        assert!(v.validate().unwrap_err().contains("ray_count"));

        let mut v = demo_vol();
        v.oct_w = 0; // texel 0 → sample_visibility の索引が定義不能
        assert!(v.validate().unwrap_err().contains("oct_w"));

        let mut v = demo_vol();
        v.max_dist = 65.5; // march 129 開始値の打ち切りで偽 sky が混入する領域
        assert!(v.validate().unwrap_err().contains("max_dist"));

        let mut v = demo_vol();
        v.max_dist = 0.0;
        assert!(v.validate().unwrap_err().contains("max_dist"));

        let mut v = demo_vol();
        v.dims = [0, 4, 4];
        assert!(v.validate().unwrap_err().contains("dims"));
    }

    /// CPU ミラーも GPU set_inputs と同一契約で入口拒否されること
    /// (rays_buf 設計制約は CPU 側の行列レイアウトにも効くため)。
    #[test]
    #[should_panic(expected = "DdgiVolumeDef::validate")]
    fn ddgi_update_cpu_rejects_invalid_volume() {
        let svo = SparseVoxelOctree::empty();
        let scene = VctScene::from_svo(&svo);
        let mut vol = demo_vol();
        vol.ray_count = 100; // rays_buf(64/probe) 超過 → 拒否
        let dirs = fibonacci_dirs(64);
        let _ = ddgi_update_cpu(
            &scene.words,
            scene.bounds,
            scene.root,
            scene.cap,
            &vol,
            &dirs,
        );
    }
}

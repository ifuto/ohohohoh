//! # Real wgpu Runtime — 実デバイス生成 + 全 WGSL シェーダーの実コンパイル検証
//!
//! これまで opt-gfx には `wgpu::Instance` / `Device` を生成するコードが
//! **全ワークスペースのどこにも存在しなかった** (監査 B3 指摘)。本モジュールは
//! 実際にアダプタ/デバイスを遅延生成し、全モジュールが埋め込む WGSL を
//! naga 検証付きで実コンパイルする。GPU が無い環境では安全に `None` になる。
//!
//! 依存クレート追加なし (Cargo.lock 不変): 手製 `block_on` で request_adapter を待機。

use std::sync::{Arc, OnceLock};
use tracing::{info, warn};

/// GPU ランタイム (1 プロセス 1 個)。
pub struct GpuRuntime {
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    pub adapter_name: String,
    pub backend: String,
    /// 実コンパイルに成功したシェーダー数。
    pub shaders_compiled: u32,
    /// コンパイル失敗したシェーダー名 (検証された障害 = 修正対象)。
    pub shaders_failed: Vec<String>,
}

static RUNTIME: OnceLock<Option<GpuRuntime>> = OnceLock::new();

/// crate 依存を増やさない最小 block_on (pollster 相当)。
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn no_op(_: *const ()) {}
    fn clone_fn(_: *const ()) -> RawWaker {
        RawWaker::new(
            std::ptr::null(),
            &RawWakerVTable::new(clone_fn, no_op, no_op, no_op),
        )
    }
    let raw = RawWaker::new(
        std::ptr::null(),
        &RawWakerVTable::new(clone_fn, no_op, no_op, no_op),
    );
    let waker = unsafe { Waker::from_raw(raw) };
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        // pin! マクロの戻り値は既に Pin<&mut F>。as_mut() で再借用して poll。
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

/// (name, WGSL source) 一覧 — 全 Wave モジュールの実シェーダー。
pub fn all_wgsl_sources() -> Vec<(&'static str, &'static str)> {
    vec![
        ("async_compute", crate::async_compute::wgsl_source()),
        ("lbvh", crate::lbvh::wgsl_source()),
        ("restir", crate::restir::wgsl_source()),
        ("ddgi", crate::ddgi::wgsl_source()),
        ("ssr", crate::ssr::wgsl_source()),
        ("bloom", crate::bloom::wgsl_source()),
        ("cas", crate::cas::wgsl_source()),
        ("exposure", crate::exposure::wgsl_source()),
        ("atmospheric", crate::atmospheric::wgsl_source()),
        ("volumetric_fog", crate::volumetric_fog::wgsl_source()),
        ("taa_ycocg", crate::taa_ycocg::wgsl_source()),
        (
            "screen_space_shadow",
            crate::screen_space_shadow::wgsl_source(),
        ),
        ("motion_blur", crate::motion_blur::wgsl_source()),
        ("depth_of_field", crate::depth_of_field::wgsl_source()),
        ("parallax", crate::parallax::wgsl_source()),
        ("ibl_sh", crate::ibl_sh::wgsl_source()),
        ("decals", crate::decals::wgsl_source()),
        ("subgroup", crate::subgroup::wgsl_source()),
        ("bindless", crate::bindless::wgsl_source()),
        ("foveated", crate::foveated::wgsl_source()),
        (
            "visibility_buffer",
            crate::visibility_buffer::visibility_buffer_wgsl(),
        ),
        ("meshlet_cone", crate::meshlet_cone::meshlet_cone_wgsl()),
        ("checkerboard", crate::checkerboard::CHECKERBOARD_WGSL),
        ("half_vertex", crate::half_vertex::HALF_VERTEX_WGSL),
        ("overdraw_sort", crate::overdraw_sort::OVERDRAW_SORT_WGSL),
        (
            "vertex_cache_opt",
            crate::vertex_cache_opt::VERTEX_CACHE_OPT_WGSL,
        ),
        ("mip_streaming", crate::mip_streaming::MIP_STREAMING_WGSL),
        ("tbdr_hints", crate::tbdr_hints::wgsl_source()),
        ("simd_frustum", crate::simd_frustum::SIMD_FRUSTUM_WGSL),
        ("frame_pacing", crate::frame_pacing::wgsl_source()),
        (
            "clustered_lighting",
            crate::clustered_lighting::CLUSTERED_LIGHTING_WGSL,
        ),
        ("sparse_texture", crate::sparse_texture::SPARSE_TEXTURE_WGSL),
        ("shadow_lod", crate::shadow_lod::wgsl_source()),
        ("fsr1", crate::fsr1::FSR1_WGSL),
        ("fsr2", crate::fsr2::FSR2_WGSL),
        ("fxaa", crate::fxaa::FXAA_WGSL),
        ("smaa", crate::smaa::SMAA_WGSL),
        ("vrs", crate::vrs::VRS_WGSL),
        ("gtao", crate::gtao::GTAO_WGSL),
        ("fsr3_fg", crate::fsr3_fg::FSR3_FG_WGSL),
        ("taa", crate::taa::TAA_WGSL),
        ("aces_tonemap", crate::aces_tonemap::ACES_WGSL),
        ("wboit", crate::wboit::WBOIT_WGSL),
        ("fragment_ray_box", crate::fragment_ray_box::wgsl_source()),
        (
            "compute_light_prop",
            crate::compute_light_prop::LIGHT_PROP_WGSL,
        ),
        ("mesh_compactor", crate::mesh_compactor::COMPACT_WGSL),
    ]
}

/// 実デバイスを遅延生成し全 WGSL を naga 検証付き実コンパイル。
/// GPU 非搭載・ドライバー異常・仮想環境では `None` (安全側)。
pub fn runtime() -> Option<&'static GpuRuntime> {
    RUNTIME
        .get_or_init(|| {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
            let created = block_on(async {
                let adapter = instance
                    .request_adapter(&wgpu::RequestAdapterOptions::default())
                    .await?;
                let info = adapter.get_info();
                let limits = wgpu::Limits::downlevel_defaults();
                let desc = wgpu::DeviceDescriptor {
                    label: Some("rsift-opt-gfx runtime"),
                    required_features: wgpu::Features::empty(),
                    required_limits: limits,
                };
                let (device, queue) = adapter.request_device(&desc, None).await.ok()?;
                Some((adapter, info, device, queue))
            });
            let Some((_adapter, info, device, queue)) = created else {
                warn!("[GpuRuntime] no wgpu adapter/device available — CPU fallback mode");
                return None;
            };
            let device = Arc::new(device);
            let queue = Arc::new(queue);

            // 全 WGSL を実コンパイル (create_shader_module は検証失敗で panic するため
            // catch_unwind で個別回収 — 失敗は「発見された検証済みバグ」として記録)。
            let mut compiled = 0u32;
            let mut failed = Vec::new();
            for (name, src) in all_wgsl_sources() {
                let dev = Arc::clone(&device);
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                    dev.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some(name),
                        source: wgpu::ShaderSource::Wgsl(src.into()),
                    })
                }));
                match res {
                    Ok(_module) => compiled += 1,
                    Err(_) => failed.push(name.to_string()),
                }
            }
            info!(
                "[GpuRuntime] device ready: {} ({:?}) — {} WGSL compiled, {} failed",
                info.name,
                info.backend,
                compiled,
                failed.len()
            );
            for f in &failed {
                warn!("[GpuRuntime] WGSL validation FAILED: {}", f);
            }
            Some(GpuRuntime {
                device,
                queue,
                adapter_name: info.name.clone(),
                backend: format!("{:?}", info.backend),
                shaders_compiled: compiled,
                shaders_failed: failed,
            })
        })
        .as_ref()
}

/// GPU 利用可否 (デバイス生成に成功した場合のみ true)。
pub fn is_available() -> bool {
    runtime().is_some()
}

/// `LIGHT_PROP_WGSL` (ブロック光伝播 BFS) を **実デバイス上で実ディスパッチ** し、
/// 実チャンク opacity + 実発光種の伝播結果を CPU へ読み戻す。
///
/// - `opacity_words`: 4096 ボクセル分の不透明ビットボード (128 word)。線形 index は
///   WGSL と同一規約 `idx = x | (y << 4) | (z << 8)`。
/// - `seeds`: (voxel_idx, level 0..=15) の実発光ボクセル。
/// - `iterations`: Jacobi 反復回数 (1 反復 = 1 dispatch, 伝播半径 16 なら 16)。
///
/// GPU 非搭載・検証失敗・ドライバーパニック時は `None` (catch_unwind で隔離)。
pub fn dispatch_light_propagation(
    opacity_words: &[u32; 128],
    seeds: &[(u32, u8)],
    iterations: u32,
) -> Option<Vec<u32>> {
    let rt = runtime()?;
    let device = std::sync::Arc::clone(&rt.device);
    let queue = std::sync::Arc::clone(&rt.queue);
    let opacity = *opacity_words;
    let seeds_vec: Vec<(u32, u8)> = seeds.to_vec();
    let iters = iterations.max(1).min(32);
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        dispatch_light_inner(&device, &queue, &opacity, &seeds_vec, iters)
    }));
    match res {
        Ok(out) => out,
        Err(_) => {
            warn!(
                "[GpuRuntime] CLP dispatch panicked — GPU light propagation disabled for this run"
            );
            None
        }
    }
}

fn dispatch_light_inner(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    opacity: &[u32; 128],
    seeds: &[(u32, u8)],
    iterations: u32,
) -> Option<Vec<u32>> {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("rsift-clp-dispatch"),
        source: wgpu::ShaderSource::Wgsl(crate::compute_light_prop::LIGHT_PROP_WGSL.into()),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("rsift-clp-bgl"),
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
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("rsift-clp-layout"),
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("rsift-clp-pipeline"),
        layout: Some(&layout),
        module: &module,
        entry_point: "cs_light_propagate",
        compilation_options: wgpu::PipelineCompilationOptions::default(),
    });

    // light_levels 初期値 = 実発光種 (low nibble = block 光レベル)。
    let mut light = vec![0u32; 4096];
    for (idx, lvl) in seeds.iter().take(4096) {
        let i = (*idx as usize).min(4095);
        light[i] = (*lvl as u32).min(15);
    }
    let light_size = (light.len() * 4) as u64;
    let light_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rsift-clp-light"),
        size: light_size,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    queue.write_buffer(&light_buf, 0, bytemuck::cast_slice(&light));
    let opacity_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rsift-clp-opacity"),
        size: (opacity.len() * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&opacity_buf, 0, bytemuck::cast_slice(opacity));
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("rsift-clp-bg"),
        layout: &bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: light_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: opacity_buf.as_entire_binding(),
            },
        ],
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("rsift-clp-enc"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("rsift-clp-pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        for _ in 0..iterations {
            pass.dispatch_workgroups(64, 1, 1); // 64 * 64 = 4096 ボクセル
        }
    }
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rsift-clp-staging"),
        size: light_size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    encoder.copy_buffer_to_buffer(&light_buf, 0, &staging, 0, light_size);
    queue.submit(Some(encoder.finish()));

    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::Maintain::Wait);
    match rx.recv() {
        Ok(Ok(())) => {
            let view = slice.get_mapped_range();
            let out: Vec<u32> = bytemuck::cast_slice(&view[..]).to_vec();
            drop(view);
            staging.unmap();
            Some(out)
        }
        Ok(Err(e)) => {
            warn!("[GpuRuntime] CLP map_async failed: {:?}", e);
            None
        }
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    /// naga による WGSL 検証 (パース + 全セマンティクス検証)。GPU 非依存の純 CPU
    /// 検査で、シェーダー破壊を実行時ではなくテスト時に捕捉する恒久ガード。
    fn naga_validate(src: &str) -> Result<(), String> {
        let module = naga::front::wgsl::parse_str(src).map_err(|e| e.emit_to_string(src))?;
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator.validate(&module).map_err(|e| format!("{e:?}"))?;
        Ok(())
    }

    /// shaders/ ディレクトリの全 .wgsl をスイープ。新しいシェーダーファイルは
    /// 自動的に検査対象に乗る (CARGO_MANIFEST_DIR 経由の列挙)。
    #[test]
    fn naga_sweep_all_shader_files() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders");
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .expect("shaders dir")
            .map(|e| e.expect("entry").file_name().into_string().expect("utf8"))
            .filter(|n| n.ends_with(".wgsl"))
            .collect();
        names.sort();
        let mut failures: Vec<(String, String)> = Vec::new();
        for name in &names {
            let src = std::fs::read_to_string(dir.join(name)).expect("read wgsl");
            if let Err(e) = naga_validate(&src) {
                failures.push((name.clone(), e));
            }
        }
        for (n, e) in &failures {
            let head: String = e.chars().take(200).collect();
            eprintln!("[naga-sweep] FAIL {n}: {head}");
        }
        eprintln!(
            "[naga-sweep] {} files: {} pass / {} fail",
            names.len(),
            names.len() - failures.len(),
            failures.len()
        );
        assert!(
            failures.is_empty(),
            "WGSL 検証失敗: {:?}",
            failures.iter().map(|(n, _)| n).collect::<Vec<_>>()
        );
    }

    /// 実 dispatch 経路に載る WGSL (all_wgsl_sources) は個別に検証失敗を報告。
    #[test]
    fn naga_all_runtime_dispatched_wgsl_validate() {
        for (name, src) in super::all_wgsl_sources() {
            naga_validate(src).unwrap_or_else(|e| panic!("runtime WGSL 検証失敗 [{name}]: {e}"));
        }
    }
}

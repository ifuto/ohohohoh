//! 実フレーム描画証明 (Frame Proof of Render) — RsGraphics 進化 Phase A
//!
//! これまで独立実装だったパス群 (vertex-pull ラスタ → 深度 → ACES ポスト) を
//! 「深度付き実フレーム」に束ねた `GpuFramePipeline` の実動作証明 + GPU 非依存の
//! CPU 参照ラスタによる同一画像の検証。
//!
//! 使い方:
//!   cargo run --release -p rsift-opt-gfx --example frame_proof            # CPU 参照 (GPU 不要)
//!   cargo run --release -p rsift-opt-gfx --example frame_proof -- cpu-fsr # CPU 参照 + FSR1 拡大 (Phase B)
//!   cargo run --release -p rsift-opt-gfx --example frame_proof -- cpu-hiz # CPU 参照 + Hi-Z カリング (Phase C)
//!   cargo run --release -p rsift-opt-gfx --example frame_proof -- gpu     # 実 GPU (wgpu adapter 必須)
//!   cargo run --release -p rsift-opt-gfx --example frame_proof -- gpu-fsr # 実 GPU + FSR1 実 dispatch
//!   cargo run --release -p rsift-opt-gfx --example frame_proof -- gpu-hiz # 実 GPU + Hi-Z 実 dispatch (Phase C)
//!
//! 出力: 実行ディレクトリに frame_proof_{cpu,cpu_fsr,cpu_hiz_*,gpu,gpu_fsr,gpu_hiz_*}.bmp (実画素)。
//! GPU 環境が無い場合 `gpu*` モードはアダプタ無しとして失敗を明示する (fail-loud)。

use rsift_opt_gfx::binary_greedy_meshing::{demo_column_palettes, mesh_chunk_column_pull_world};
use rsift_opt_gfx::frame_hiz::aabb_from_meshes;
use rsift_opt_gfx::frame_pipeline::{build_view_proj, FrameCamera};
use rsift_opt_gfx::frame_reference::{
    hiz_downsample_reference, hiz_test_reference, render_reference, render_reference_fsr, write_bmp,
};
use rsift_opt_gfx::occlusion_query::{OcclusionPolicy, QueryCore};

const WIDTH: u32 = 640;
const HEIGHT: u32 = 480;
// Phase B (FSR1): 低解像度レンダ → 全解像度へ拡大
// (0.5x で効果が分かりやすい実スケール。チューニングで 0.7x 等へ変更可能)
const LOW_W: u32 = WIDTH / 2;
const LOW_H: u32 = HEIGHT / 2;

/// 単一チャンクカラム (チャンク (0,0)・4 セクション) を実テラス仕様で生成し、
/// 実メッシュ化。origin は (0,0,0) の単一ウインドウ。
fn demo_chunks() -> Vec<(rsift_opt_gfx::pull_mesh::PullBuiltMesh, [f32; 3])> {
    let palettes = demo_column_palettes(0, 0);
    let mesh = mesh_chunk_column_pull_world(&palettes, 0, 0, 0, 0, 0, 0, true);
    vec![(mesh, [0.0, 0.0, 0.0])]
}

fn camera() -> FrameCamera {
    // チャンク (0..16, 0..64, 0..16) を南西上空から見下ろす構図。
    FrameCamera {
        eye: [-30.0, 90.0, -30.0],
        target: [8.0, 20.0, 8.0],
        up: [0.0, 1.0, 0.0],
        fov_y_deg: 60.0,
        aspect: WIDTH as f32 / HEIGHT as f32,
        near: 0.1,
        far: 500.0,
    }
}

fn run_cpu(chunks: &[(rsift_opt_gfx::pull_mesh::PullBuiltMesh, [f32; 3])]) {
    let vp = build_view_proj(&camera());
    let frame = render_reference(chunks, &vp, WIDTH, HEIGHT);
    let out = std::env::current_dir()
        .unwrap_or_default()
        .join("frame_proof_cpu.bmp");
    write_bmp(&out, frame.width, frame.height, &frame.pixels)
        .unwrap_or_else(|e| panic!("BMP 書き出し失敗: {e}"));
    println!("[FrameProof/cpu] 実参照ラスタ完了");
    println!("  quads        : {}", frame.quads);
    println!("  covered_px   : {} / {}", frame.covered_px, WIDTH * HEIGHT);
    println!("  avg_lum      : {:.4}", frame.avg_lum);
    println!("  output       : {}", out.display());
    assert!(frame.covered_px > 0, "被覆ピクセルゼロ — 描画成立せず");
}

fn run_cpu_fsr(chunks: &[(rsift_opt_gfx::pull_mesh::PullBuiltMesh, [f32; 3])]) {
    let vp = build_view_proj(&camera());
    let frame = render_reference_fsr(chunks, &vp, LOW_W, LOW_H, WIDTH, HEIGHT, 0.2);
    let out = std::env::current_dir()
        .unwrap_or_default()
        .join("frame_proof_cpu_fsr.bmp");
    write_bmp(&out, frame.width, frame.height, &frame.pixels)
        .unwrap_or_else(|e| panic!("BMP 書き出し失敗: {e}"));
    println!("[FrameProof/cpu-fsr] 実参照ラスタ (低解像度) + FSR1 (WGSL ミラー) 完了");
    println!("  chain        : {LOW_W}x{LOW_H} → EASU → RCAS → {WIDTH}x{HEIGHT}");
    println!("  quads        : {}", frame.quads);
    println!("  covered_px   : {} / {}", frame.covered_px, WIDTH * HEIGHT);
    println!("  avg_lum      : {:.4}", frame.avg_lum);
    println!("  output       : {}", out.display());
    assert!(
        frame.covered_px > 0,
        "被覆ピクセルゼロ — 拡大後の描画成立せず"
    );
}

fn run_gpu(chunks: &[(rsift_opt_gfx::pull_mesh::PullBuiltMesh, [f32; 3])]) -> Result<(), String> {
    let rt = rsift_opt_gfx::gpu_runtime::runtime()
        .ok_or_else(|| "wgpu adapter 無し — GPU パスは実行不可 (fail-loud)".to_string())?;
    let mut pipe = rsift_opt_gfx::frame_pipeline::GpuFramePipeline::new(&rt.device, WIDTH, HEIGHT);
    let vp = build_view_proj(&camera());
    let img = pipe.render_to_image(&rt.device, &rt.queue, vp, chunks)?;
    let out = std::env::current_dir()
        .unwrap_or_default()
        .join("frame_proof_gpu.bmp");
    write_bmp(&out, img.width, img.height, &img.pixels)
        .map_err(|e| format!("BMP 書き出し失敗: {e}"))?;
    println!("[FrameProof/gpu] 実 GPU フレーム完了");
    println!("  draw_calls   : {}", img.draw_calls);
    println!("  quads        : {}", img.quads);
    println!("  pixels       : {} B", img.pixels.len());
    println!("  output       : {}", out.display());
    Ok(())
}

fn run_gpu_fsr(
    chunks: &[(rsift_opt_gfx::pull_mesh::PullBuiltMesh, [f32; 3])],
) -> Result<(), String> {
    let rt = rsift_opt_gfx::gpu_runtime::runtime()
        .ok_or_else(|| "wgpu adapter 無し — GPU パスは実行不可 (fail-loud)".to_string())?;
    // 低解像度で実フレームを LDR まで録画
    let mut low = rsift_opt_gfx::frame_pipeline::GpuFramePipeline::new(&rt.device, LOW_W, LOW_H);
    let vp = build_view_proj(&camera());
    let (draws, quads) = low.record_to_ldr(&rt.device, &rt.queue, vp, chunks)?;
    println!("[FrameProof/gpu-fsr] 低解像度録画 draws={draws} quads={quads}");
    // FSR1: 実 compute dispatch (EASU + RCAS) で全解像度へ
    let mut fsr =
        rsift_opt_gfx::frame_fsr1::GpuFsr1Pass::new(&rt.device, LOW_W, LOW_H, WIDTH, HEIGHT);
    let img = fsr.render_full(&rt.device, &rt.queue, low.ldr_view())?;
    let out = std::env::current_dir()
        .unwrap_or_default()
        .join("frame_proof_gpu_fsr.bmp");
    write_bmp(&out, img.width, img.height, &img.pixels)
        .map_err(|e| format!("BMP 書き出し失敗: {e}"))?;
    println!("[FrameProof/gpu-fsr] 実 GPU FSR1 完了");
    println!("  dispatches   : {}", img.draw_calls);
    println!(
        "  pixels       : {} B ({}x{})",
        img.pixels.len(),
        img.width,
        img.height
    );
    println!("  output       : {}", out.display());
    Ok(())
}

// ---------- Phase C: Hi-Z 遮蔽シーン (front が back を完全に遮蔽) ----------

type HizSceneChunks = Vec<(rsift_opt_gfx::pull_mesh::PullBuiltMesh, [f32; 3])>;
type HizSceneBoxes = Vec<rsift_opt_gfx::occlusion_query::QueryBox>;
type HizScene = (HizSceneChunks, HizSceneBoxes, FrameCamera);

/// 2 カラム遮蔽シーン:
/// - front = chunk (0,0) **全面占有の壁** (16x64x16、全 4 セクション)
/// - back  = chunk (0,2) 実 demo 地形の最下 1 セクション (高さ ~2..11, z 32..48)
///
/// 水平カメラから back は壁に完全遮蔽される (demo 地形のセクション間空隙で
/// 遮蔽が破綻することを設計検証で確認済のため、front は空隙の無い壁を使う。
/// 生成は実メッシュルート mesh_chunk_column_pull_world 経由 = 実データ)。
/// 戻り値: (実チャンク列, index 整列の実 AABB 列, カメラ)
fn hiz_scene() -> HizScene {
    use rsift_opt_gfx::binary_greedy_meshing::SectionPalette;
    let wall: Vec<SectionPalette> = vec![[1u16; 16 * 16 * 16]; 4];
    let front = mesh_chunk_column_pull_world(&wall, 0, 0, 0, 0, 0, 0, true);
    let back_p = demo_column_palettes(0, 2);
    let back = mesh_chunk_column_pull_world(&back_p[..1], 0, 2, 0, 0, 0, 0, true);
    let chunks = vec![(front, [0.0, 0.0, 0.0]), (back, [0.0, 0.0, 0.0])];
    let boxes = aabb_from_meshes(&chunks);
    // 遮蔽関係の前提を実測確認 (front が back より十分高いこと)
    assert!(
        boxes[0].max[1] > boxes[1].max[1],
        "front must tower over back for a deterministic occlusion scene: {boxes:?}"
    );
    let cam = FrameCamera {
        eye: [8.0, 24.0, -30.0],
        target: [8.0, 24.0, 16.0],
        up: [0.0, 1.0, 0.0],
        fov_y_deg: 60.0,
        aspect: WIDTH as f32 / HEIGHT as f32,
        near: 0.1,
        far: 500.0,
    };
    (chunks, boxes, cam)
}

fn run_cpu_hiz() {
    let (chunks, boxes, cam) = hiz_scene();
    let vp = build_view_proj(&cam);
    let dim = rsift_opt_gfx::frame_hiz::HIZ_DIM;
    let mut core = QueryCore::new(OcclusionPolicy::default());

    // 3 フレームの実運用ループ: 描画 (culled 適用) → CPU 参照深度 →
    // Hi-Z ダウンサンプル → AABB テスト → QueryCore 解決。
    // (occlude_after_frames=2 で back は 2 フレーム連続 coverage=0 後に culled)
    let mut coverage = Vec::new();
    for frame in 0..3 {
        let mask: Vec<bool> = (0..chunks.len()).map(|i| core.should_draw(i)).collect();
        let drawn: Vec<_> = chunks
            .iter()
            .zip(mask.iter())
            .filter(|(_, m)| **m)
            .map(|(c, _)| c.clone())
            .collect();
        let f = render_reference(&drawn, &vp, WIDTH, HEIGHT);
        let hiz = hiz_downsample_reference(&f.depth, WIDTH, HEIGHT, dim);
        coverage = hiz_test_reference(&hiz, dim, &boxes, &vp);
        core.set_boxes(&boxes);
        core.resolve(&coverage);
        let (draw, culled) = core.stats();
        println!(
            "[FrameProof/cpu-hiz] frame={frame} coverage={coverage:?} → draw={draw} culled={culled}"
        );
    }
    assert!(coverage[0] > 0, "front must stay visible");
    assert_eq!(coverage[1], 0, "back must be fully occluded (実遮蔽)");
    assert!(
        !core.should_draw(1),
        "back must be culled after grace frames"
    );

    // 実効果還元の実証: culled 描画の結果を実画素で確認する。
    // back は壁に完全遮蔽されているので、culled 版は全描画版と
    // **全ピクセル一致** するはず (深度テストで back は一切書き込まれない)。
    let full = render_reference(&chunks, &vp, WIDTH, HEIGHT);
    let culled = render_reference(&chunks[..1], &vp, WIDTH, HEIGHT);
    let cwd = std::env::current_dir().unwrap_or_default();
    let out_full = cwd.join("frame_proof_cpu_hiz_full.bmp");
    let out_culled = cwd.join("frame_proof_cpu_hiz_culled.bmp");
    write_bmp(&out_full, full.width, full.height, &full.pixels)
        .unwrap_or_else(|e| panic!("BMP 書き出し失敗: {e}"));
    write_bmp(&out_culled, culled.width, culled.height, &culled.pixels)
        .unwrap_or_else(|e| panic!("BMP 書き出し失敗: {e}"));
    assert_eq!(
        full.pixels, culled.pixels,
        "culled render must be pixel-identical (back contributes nothing = culling is safe)"
    );
    println!("[FrameProof/cpu-hiz] カリングループ実証完了");
    println!("  boxes        : {} (front AABB / back AABB)", boxes.len());
    println!(
        "  culled_px比較: full={} culled={} (一致=カリング安全)",
        full.covered_px, culled.covered_px
    );
    println!(
        "  output       : {} / {}",
        out_full.display(),
        out_culled.display()
    );
}

fn run_gpu_hiz() -> Result<(), String> {
    let rt = rsift_opt_gfx::gpu_runtime::runtime()
        .ok_or_else(|| "wgpu adapter 無し — GPU パスは実行不可 (fail-loud)".to_string())?;
    let (chunks, boxes, cam) = hiz_scene();
    let vp = build_view_proj(&cam);
    let mut pipe = rsift_opt_gfx::frame_pipeline::GpuFramePipeline::new(&rt.device, WIDTH, HEIGHT);
    let mut hiz = rsift_opt_gfx::frame_hiz::GpuHiz::new(&rt.device, WIDTH, HEIGHT);

    // 3 フレーム実ループ: culled 適用描画 (実深度構築) → Hi-Z 実 dispatch →
    // coverage 実 readback → QueryCore 解決。
    let mut coverage = Vec::new();
    for frame in 0..3 {
        let mask: Vec<bool> = (0..chunks.len()).map(|i| hiz.should_draw(i)).collect();
        let (draws, quads) =
            pipe.record_to_ldr_gated(&rt.device, &rt.queue, vp, &chunks, Some(&mask))?;
        coverage = hiz.update(&rt.device, &rt.queue, pipe.depth_view(), &boxes, vp)?;
        let (draw, culled) = hiz.stats();
        println!(
            "[FrameProof/gpu-hiz] frame={frame} draws={draws} quads={quads} coverage={coverage:?} → draw={draw} culled={culled}"
        );
    }
    assert!(coverage[0] > 0, "front must stay visible (GPU readback)");
    assert_eq!(coverage[1], 0, "back must be occluded (GPU readback)");
    assert!(hiz.stats().1 > 0, "culled>0 must be achieved");

    // 最終フレーム: culled 適用で実フレーム画像 + Hi-Z 可視化を出力
    let mask: Vec<bool> = (0..chunks.len()).map(|i| hiz.should_draw(i)).collect();
    let img = pipe.render_to_image_gated(&rt.device, &rt.queue, vp, &chunks, Some(&mask))?;
    let cwd = std::env::current_dir().unwrap_or_default();
    let out = cwd.join("frame_proof_gpu_hiz_culled.bmp");
    write_bmp(&out, img.width, img.height, &img.pixels)
        .map_err(|e| format!("BMP 書き出し失敗: {e}"))?;
    let dbg = hiz.readback_debug(&rt.device, &rt.queue);
    let out_dbg = cwd.join("frame_proof_gpu_hiz_debug.bmp");
    write_bmp(&out_dbg, dbg.width, dbg.height, &dbg.pixels)
        .map_err(|e| format!("BMP 書き出し失敗: {e}"))?;
    println!("[FrameProof/gpu-hiz] 実 GPU Hi-Z カリング実証完了");
    println!("  output       : {} / {}", out.display(), out_dbg.display());
    Ok(())
}

fn main() {
    tracing_subscriber::fmt::try_init().ok();
    let mode = std::env::args().nth(1).unwrap_or_else(|| "cpu".to_string());
    // hiz 系モードは独自の 2 カラム遮蔽シーン (hiz_scene) を使う
    let demo_modes = matches!(mode.as_str(), "cpu" | "cpu-fsr" | "gpu" | "gpu-fsr");
    let chunks = if demo_modes { demo_chunks() } else { vec![] };
    let quads: usize = chunks.iter().map(|(m, _)| m.quads.len()).sum();
    println!("=== Frame Proof of Render (Phase A+B+C) ===");
    if demo_modes {
        println!("mode={mode}  input: demo column chunk(0,0) → {quads} packed quads (実データ)");
    } else {
        println!("mode={mode}  input: 2-column occlusion scene (Phase C)");
    }
    match mode.as_str() {
        "cpu" => run_cpu(&chunks),
        "cpu-fsr" => run_cpu_fsr(&chunks),
        "cpu-hiz" => run_cpu_hiz(),
        "gpu" => {
            if let Err(e) = run_gpu(&chunks) {
                eprintln!("[FrameProof/gpu] {e}");
                std::process::exit(2);
            }
        }
        "gpu-fsr" => {
            if let Err(e) = run_gpu_fsr(&chunks) {
                eprintln!("[FrameProof/gpu-fsr] {e}");
                std::process::exit(2);
            }
        }
        "gpu-hiz" => {
            if let Err(e) = run_gpu_hiz() {
                eprintln!("[FrameProof/gpu-hiz] {e}");
                std::process::exit(2);
            }
        }
        other => {
            eprintln!(
                "unknown mode '{other}' — 'cpu' (既定) / 'cpu-fsr' / 'cpu-hiz' / 'gpu' / 'gpu-fsr' / 'gpu-hiz' を指定"
            );
            std::process::exit(2);
        }
    }
}

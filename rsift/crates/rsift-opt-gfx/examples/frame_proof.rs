//! 実フレーム描画証明 (Frame Proof of Render) — RsGraphics 進化 Phase A
//!
//! これまで独立実装だったパス群 (vertex-pull ラスタ → 深度 → ACES ポスト) を
//! 「深度付き実フレーム」に束ねた `GpuFramePipeline` の実動作証明 + GPU 非依存の
//! CPU 参照ラスタによる同一画像の検証。
//!
//! 使い方:
//!   cargo run --release -p rsift-opt-gfx --example frame_proof            # CPU 参照 (GPU 不要)
//!   cargo run --release -p rsift-opt-gfx --example frame_proof -- cpu-fsr # CPU 参照 + FSR1 拡大 (Phase B)
//!   cargo run --release -p rsift-opt-gfx --example frame_proof -- gpu     # 実 GPU (wgpu adapter 必須)
//!   cargo run --release -p rsift-opt-gfx --example frame_proof -- gpu-fsr # 実 GPU + FSR1 実 dispatch
//!
//! 出力: 実行ディレクトリに frame_proof_{cpu,cpu_fsr,gpu,gpu_fsr}.bmp (実画素)。
//! GPU 環境が無い場合 `gpu*` モードはアダプタ無しとして失敗を明示する (fail-loud)。

use rsift_opt_gfx::binary_greedy_meshing::{demo_column_palettes, mesh_chunk_column_pull_world};
use rsift_opt_gfx::frame_pipeline::{build_view_proj, FrameCamera};
use rsift_opt_gfx::frame_reference::{render_reference, render_reference_fsr, write_bmp};

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

fn main() {
    tracing_subscriber::fmt::try_init().ok();
    let mode = std::env::args().nth(1).unwrap_or_else(|| "cpu".to_string());
    let chunks = demo_chunks();
    let quads: usize = chunks.iter().map(|(m, _)| m.quads.len()).sum();
    println!("=== Frame Proof of Render (Phase A+B) ===");
    println!("mode={mode}  input: demo column chunk(0,0) → {quads} packed quads (実データ)");
    match mode.as_str() {
        "cpu" => run_cpu(&chunks),
        "cpu-fsr" => run_cpu_fsr(&chunks),
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
        other => {
            eprintln!(
                "unknown mode '{other}' — 'cpu' (既定) / 'cpu-fsr' / 'gpu' / 'gpu-fsr' を指定"
            );
            std::process::exit(2);
        }
    }
}

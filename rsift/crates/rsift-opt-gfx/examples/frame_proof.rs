//! 実フレーム描画証明 (Frame Proof of Render) — RsGraphics 進化 Phase A
//!
//! これまで独立実装だったパス群 (vertex-pull ラスタ → 深度 → ACES ポスト) を
//! 「深度付き実フレーム」に束ねた `GpuFramePipeline` の実動作証明 + GPU 非依存の
//! CPU 参照ラスタによる同一画像の検証。
//!
//! 使い方:
//!   cargo run --release -p rsift-opt-gfx --example frame_proof            # CPU 参照 (GPU 不要)
//!   cargo run --release -p rsift-opt-gfx --example frame_proof -- gpu     # 実 GPU (wgpu adapter 必須)
//!
//! 出力: 実行ディレクトリに frame_proof_cpu.bmp / frame_proof_gpu.bmp (実画素)。
//! GPU 環境が無い場合 `gpu` モードはアダプタ無しとして失敗を明示する (fail-loud)。

use rsift_opt_gfx::binary_greedy_meshing::{demo_column_palettes, mesh_chunk_column_pull_world};
use rsift_opt_gfx::frame_pipeline::{build_view_proj, FrameCamera};
use rsift_opt_gfx::frame_reference::{render_reference, write_bmp};

const WIDTH: u32 = 640;
const HEIGHT: u32 = 480;

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

fn main() {
    tracing_subscriber::fmt::try_init().ok();
    let mode = std::env::args().nth(1).unwrap_or_else(|| "cpu".to_string());
    let chunks = demo_chunks();
    let quads: usize = chunks.iter().map(|(m, _)| m.quads.len()).sum();
    println!("=== Frame Proof of Render (Phase A) ===");
    println!("mode={mode}  input: demo column chunk(0,0) → {quads} packed quads (実データ)");
    match mode.as_str() {
        "cpu" => run_cpu(&chunks),
        "gpu" => {
            if let Err(e) = run_gpu(&chunks) {
                eprintln!("[FrameProof/gpu] {e}");
                std::process::exit(2);
            }
        }
        other => {
            eprintln!("unknown mode '{other}' — 'cpu' (既定) か 'gpu' を指定");
            std::process::exit(2);
        }
    }
}

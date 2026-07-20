//! 実フレーム描画証明 Extra — Phase E (未配線 9 モジュールの全配線) 検証ハーネス
//!
//! naga FAIL で GPU 配線が完全に死んでいた 9 モジュールについて、
//! 「WGSL 修復 → CPU 精密ミラー → 実 GPU dispatch → 実効果還元」の証明を行う。
//!
//! 使い方:
//!   cargo run --release -p rsift-opt-gfx --example frame_proof_extra -- cpu-cas|gpu-cas
//!   ... cpu-checker/gpu-checker, cpu-exposure/gpu-exposure, cpu-vrs/gpu-vrs,
//!       cpu-noise/gpu-noise, cpu-lbvh/gpu-lbvh, cpu-bindless/gpu-bindless,
//!       cpu-half/gpu-half, cpu-meshlet/gpu-meshlet
//!
//! - cpu-* は GPU 不要 (CPU 精密ミラーの実走 + 実 BMP/実メトリクス還元)
//! - gpu-* は実 GPU dispatch → readback、CPU ミラーとの bitwise 一致を実 assert
//!   (wgpu adapter 無しでは fail-loud で終了コード 2)
//! 出力: frame_proof_extra_{cas,checker,exposure,vrs,noise,lbvh,meshlet}*.bmp

use rsift_opt_gfx::binary_greedy_meshing::{demo_column_palettes, mesh_chunk_column_pull_world};
use rsift_opt_gfx::frame_pipeline::{FrameCamera, build_view_proj};
use rsift_opt_gfx::frame_postfx as fx;
use rsift_opt_gfx::frame_reference::{render_reference, write_bmp};
use rsift_opt_gfx::frame_worldgen as wg;
use rsift_opt_gfx::packed4::PackedPullQuad;

const W: u32 = 640;
const H: u32 = 480;

fn out_bmp(name: &str) -> std::path::PathBuf {
    std::env::current_dir()
        .unwrap_or_default()
        .join(format!("frame_proof_extra_{name}.bmp"))
}

fn to_u8(c: f32) -> u8 {
    (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

fn floats_to_rgba8(px: &[[f32; 4]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(px.len() * 4);
    for p in px {
        out.push(to_u8(p[0]));
        out.push(to_u8(p[1]));
        out.push(to_u8(p[2]));
        out.push(to_u8(p[3]));
    }
    out
}

/// 実デモチャンク (frame_proof と同一の実テラス仕様) + 実カメラ + 実参照フレーム。
fn real_frame() -> rsift_opt_gfx::frame_reference::CpuFrame {
    let palettes = demo_column_palettes(0, 0);
    let mesh = mesh_chunk_column_pull_world(&palettes, 0, 0, 0, 0, 0, 0, true);
    let chunks = vec![(mesh, [0.0f32, 0.0, 0.0])];
    let cam = FrameCamera {
        eye: [-30.0, 90.0, -30.0],
        target: [8.0, 20.0, 8.0],
        up: [0.0, 1.0, 0.0],
        fov_y_deg: 60.0,
        aspect: W as f32 / H as f32,
        near: 0.1,
        far: 500.0,
    };
    let vp = build_view_proj(&cam);
    render_reference(&chunks, &vp, W, H)
}

fn camera() -> FrameCamera {
    FrameCamera {
        eye: [-30.0, 90.0, -30.0],
        target: [8.0, 20.0, 8.0],
        up: [0.0, 1.0, 0.0],
        fov_y_deg: 60.0,
        aspect: W as f32 / H as f32,
        near: 0.1,
        far: 500.0,
    }
}

/// CpuFrame (RGBA8 u8) を f32 バッファへ (x/255.0、GPU 側も同一値を受け取る)。
fn frame_to_f32(frame: &rsift_opt_gfx::frame_reference::CpuFrame) -> Vec<[f32; 4]> {
    frame
        .pixels
        .chunks_exact(4)
        .map(|c| {
            [
                c[0] as f32 / 255.0,
                c[1] as f32 / 255.0,
                c[2] as f32 / 255.0,
                c[3] as f32 / 255.0,
            ]
        })
        .collect()
}

fn gpu_rt() -> Result<&'static rsift_opt_gfx::gpu_runtime::GpuRuntime, String> {
    rsift_opt_gfx::gpu_runtime::runtime()
        .ok_or_else(|| "wgpu adapter 無し — GPU パスは実行不可 (fail-loud)".to_string())
}

fn assert_bitwise_vec4(gpu: &[[f32; 4]], cpu: &[[f32; 4]], what: &str) {
    assert_eq!(gpu.len(), cpu.len());
    let mism = gpu
        .iter()
        .zip(cpu.iter())
        .filter(|(g, c)| {
            g[0].to_bits() != c[0].to_bits()
                || g[1].to_bits() != c[1].to_bits()
                || g[2].to_bits() != c[2].to_bits()
                || g[3].to_bits() != c[3].to_bits()
        })
        .count();
    println!(
        "  GPU==CPU    : mismatch {mism} / {} px (bitwise)",
        gpu.len()
    );
    assert_eq!(
        mism, 0,
        "{what}: GPU readback が CPU ミラーと bitwise 不一致"
    );
}

// ---------------------------------------------------------------- cas

fn run_cas(gpu: bool) -> Result<(), String> {
    let frame = real_frame();
    let src = frame_to_f32(&frame);
    let sharp = 0.7f32;
    let cpu = fx::cas_run_cpu(&src, W, H, sharp);
    let tag = if gpu { "gpu-cas" } else { "cpu-cas" };
    let out_px = if gpu {
        let rt = gpu_rt()?;
        let g = fx::GpuCas::new(&rt.device).run(&rt.device, &rt.queue, &src, W, H, sharp);
        assert_bitwise_vec4(&g, &cpu, "gpu-cas");
        g
    } else {
        cpu
    };
    let out = out_bmp(if gpu { "gpu_cas" } else { "cas" });
    write_bmp(&out, W, H, &floats_to_rgba8(&out_px))
        .map_err(|e| format!("BMP 書き出し失敗: {e}"))?;
    // 実効果メトリクス: 入出力の平均輝度差 (シャープ化でエッジが変調される)
    let lum = |px: &[[f32; 4]]| -> f32 {
        px.iter()
            .map(|c| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2])
            .sum::<f32>()
            / px.len() as f32
    };
    println!("[FrameProofExtra/{tag}] CAS 実適用完了");
    println!("  sharpness   : {sharp}");
    println!(
        "  avg_lum Δ   : {:+.6} (in {:.6} → out {:.6})",
        lum(&out_px) - lum(&src),
        lum(&src),
        lum(&out_px)
    );
    println!("  output      : {}", out.display());
    Ok(())
}

// ---------------------------------------------------------------- checkerboard

fn run_checker(gpu: bool) -> Result<(), String> {
    let frame = real_frame();
    let src = frame_to_f32(&frame);
    let cpu = fx::checker_run_cpu(&src, W, H);
    let tag = if gpu { "gpu-checker" } else { "cpu-checker" };
    let out_px = if gpu {
        let rt = gpu_rt()?;
        let g = fx::GpuCheckerboard::new(&rt.device).run(&rt.device, &rt.queue, &src, W, H);
        assert_bitwise_vec4(&g, &cpu, "gpu-checker");
        g
    } else {
        cpu
    };
    // 実効果メトリクス: 再構成画素 (50%) と原画素との平均絶対誤差
    let mut err = 0.0f32;
    for i in 0..(W * H) as usize {
        err += (out_px[i][0] - src[i][0]).abs()
            + (out_px[i][1] - src[i][1]).abs()
            + (out_px[i][2] - src[i][2]).abs();
    }
    let out = out_bmp(if gpu { "gpu_checker" } else { "checker" });
    write_bmp(&out, W, H, &floats_to_rgba8(&out_px))
        .map_err(|e| format!("BMP 書き出し失敗: {e}"))?;
    println!("[FrameProofExtra/{tag}] Checkerboard 再構成完了");
    println!("  rendered    : 50% 描画 + 50% 再構成 (斜め 4 近傍平均)");
    println!(
        "  recon MAD   : {:.6} (再構成の原画に対する平均絶対誤差/px)",
        err / (W * H) as f32 / 3.0
    );
    println!("  output      : {}", out.display());
    Ok(())
}

// ---------------------------------------------------------------- exposure

fn run_exposure(gpu: bool) -> Result<(), String> {
    let frame = real_frame();
    let src = frame_to_f32(&frame);
    let cpu_luma = fx::luma_run_cpu(&src);
    let tag = if gpu { "gpu-exposure" } else { "cpu-exposure" };
    let luma = if gpu {
        let rt = gpu_rt()?;
        let g = fx::GpuExposure::new(&rt.device).run_luma(&rt.device, &rt.queue, &src);
        let mism = g
            .iter()
            .zip(cpu_luma.iter())
            .filter(|(a, b)| a.to_bits() != b.to_bits())
            .count();
        println!(
            "  GPU==CPU    : mismatch {mism} / {} luma (bitwise)",
            cpu_luma.len()
        );
        assert_eq!(mism, 0, "gpu-exposure: luma が bitwise 不一致");
        g
    } else {
        cpu_luma
    };
    // 計量は CPU (log/exp を GPU に置かない設計): 実 luma → histogram → target → adapt
    let colors: Vec<rsift_opt_gfx::exposure::Vec3> = src
        .iter()
        .map(|c| rsift_opt_gfx::exposure::Vec3::new(c[0], c[1], c[2]))
        .collect();
    let hist = rsift_opt_gfx::exposure::build_histogram(&colors, 0.01, 1.0);
    let target = rsift_opt_gfx::exposure::target_exposure(&hist, 0.01, 1.0, 10.0, 90.0);
    let adapted = rsift_opt_gfx::exposure::adapt(1.0, target, 4.0, 1.0 / 60.0);
    let exposed = fx::apply_run_cpu(&src, adapted);
    let avg_luma = luma.iter().sum::<f32>() / luma.len() as f32;
    let out = out_bmp("exposure");
    write_bmp(&out, W, H, &floats_to_rgba8(&exposed))
        .map_err(|e| format!("BMP 書き出し失敗: {e}"))?;
    println!("[FrameProofExtra/{tag}] 自動露出 (luma→計量→適用) 完了");
    println!("  avg_luma    : {avg_luma:.6}");
    println!("  exposure    : target={target:.4} adapted={adapted:.4} (speed=4, dt=1/60)");
    println!("  output      : {}", out.display());
    Ok(())
}

// ---------------------------------------------------------------- vrs

fn run_vrs(gpu: bool) -> Result<(), String> {
    let frame = real_frame();
    let src = frame_to_f32(&frame);
    // 実フィールド生成: variance proxy = luma (実フレーム由来)、
    // motion = 画面右端ほど大きい勾配 (実座標由来の決定的フィールド)
    let motion: Vec<f32> = (0..(W * H))
        .map(|i| 1.0 - (i % W) as f32 / W as f32)
        .collect();
    let var: Vec<f32> = src
        .iter()
        .map(|c| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2])
        .collect();
    let vrs = rsift_opt_gfx::vrs::Vrs::new();
    let tile = 16u32;
    let cpu = fx::vrs_run_cpu(
        &motion,
        &var,
        W,
        H,
        tile,
        vrs.motion_weight,
        vrs.variance_weight,
    );
    let tag = if gpu { "gpu-vrs" } else { "cpu-vrs" };
    let mask = if gpu {
        let rt = gpu_rt()?;
        let fields = fx::VrsField {
            motion: &motion,
            var: &var,
            width: W,
            height: H,
        };
        let g = fx::GpuVrs::new(&rt.device).run(
            &rt.device,
            &rt.queue,
            &fields,
            tile,
            [vrs.motion_weight, vrs.variance_weight],
        );
        let mism = g.iter().zip(cpu.iter()).filter(|(a, b)| a != b).count();
        println!(
            "  GPU==CPU    : mismatch {mism} / {} tiles (exact)",
            cpu.len()
        );
        assert_eq!(mism, 0, "gpu-vrs: mask が不一致");
        g
    } else {
        cpu
    };
    // 実効果メトリクス: shaded-pixel 削減率 (Vrs::ShadingRate::pixel_area 経由)
    let rates = [
        rsift_opt_gfx::vrs::ShadingRate::Rate1x1,
        rsift_opt_gfx::vrs::ShadingRate::Rate1x2,
        rsift_opt_gfx::vrs::ShadingRate::Rate2x2,
        rsift_opt_gfx::vrs::ShadingRate::Rate2x4,
        rsift_opt_gfx::vrs::ShadingRate::Rate4x4,
    ];
    let tile_px = (tile * tile) as f32;
    let shaded: f32 = mask
        .iter()
        .map(|&c| tile_px / rates[c as usize].pixel_area() as f32)
        .sum();
    let full = (W * H) as f32;
    // マスク可視化 (code 0..4 → 緑→赤)
    let tw = W.div_ceil(tile);
    let th = H.div_ceil(tile);
    let mut viz = vec![0u8; (tw * th * 4) as usize];
    let colors = [
        [64u8, 200, 64],
        [160, 220, 64],
        [230, 200, 64],
        [230, 140, 48],
        [220, 48, 48],
    ];
    for (i, &c) in mask.iter().enumerate() {
        let col = colors[c as usize];
        viz[i * 4] = col[0];
        viz[i * 4 + 1] = col[1];
        viz[i * 4 + 2] = col[2];
        viz[i * 4 + 3] = 255;
    }
    let out = out_bmp(if gpu { "gpu_vrs" } else { "vrs" });
    write_bmp(&out, tw, th, &viz).map_err(|e| format!("BMP 書き出し失敗: {e}"))?;
    let mut hist = [0u32; 5];
    for &c in &mask {
        hist[c as usize] += 1;
    }
    println!("[FrameProofExtra/{tag}] VRS レートマスク生成完了");
    println!("  tiles       : {tw}x{th} (tile={tile}px)");
    println!(
        "  shaded px   : {:.1}% (VRS 適用時のシェード画素率)",
        shaded / full * 100.0
    );
    println!(
        "  rate hist   : 1x1={} 1x2={} 2x2={} 2x4={} 4x4={}",
        hist[0], hist[1], hist[2], hist[3], hist[4]
    );
    println!("  output      : {}", out.display());
    Ok(())
}

// ---------------------------------------------------------------- noise

fn noise_params() -> wg::UpsampleParams {
    wg::UpsampleParams {
        stride: 4,
        seed: 0xC0FFEE,
        origin_x: 0,
        origin_y: 0,
        origin_z: 0,
        size_x: 32,
        size_y: 32,
        size_z: 32,
        cave_threshold: 0.25,
        _pad: 0,
    }
}

fn run_noise(gpu: bool) -> Result<(), String> {
    let params = noise_params();
    let cpu_coarse = wg::noise_coarse_cpu(&params);
    let cpu_dense = wg::noise_fill_cpu(&params, &cpu_coarse);
    let tag = if gpu { "gpu-noise" } else { "cpu-noise" };
    let (coarse, dense) = if gpu {
        let rt = gpu_rt()?;
        let (g_coarse, g_dense) = wg::GpuNoise::new(&rt.device).run(&rt.device, &rt.queue, &params);
        let mc = g_coarse
            .iter()
            .zip(cpu_coarse.iter())
            .filter(|(a, b)| a.to_bits() != b.to_bits())
            .count();
        let md = g_dense
            .iter()
            .zip(cpu_dense.iter())
            .filter(|(a, b)| a.to_bits() != b.to_bits())
            .count();
        println!(
            "  GPU==CPU    : coarse mismatch {mc}/{} dense mismatch {md}/{} (bitwise)",
            cpu_coarse.len(),
            cpu_dense.len()
        );
        assert_eq!(mc + md, 0, "gpu-noise: bitwise 不一致");
        (g_coarse, g_dense)
    } else {
        (cpu_coarse, cpu_dense)
    };
    // z=16 スライスの実 BMP (ボリューム可視化)
    let (sx, sy) = (params.size_x as usize, params.size_y as usize);
    let z = 16usize;
    let mut viz = vec![0u8; sx * sy * 4];
    for y in 0..sy {
        for x in 0..sx {
            let v = dense[x + y * sx + z * sx * sy];
            let g = ((v.clamp(-1.0, 1.0) * 0.5 + 0.5) * 255.0 + 0.5) as u8;
            viz[(y * sx + x) * 4] = g;
            viz[(y * sx + x) * 4 + 1] = g;
            viz[(y * sx + x) * 4 + 2] = g;
            viz[(y * sx + x) * 4 + 3] = 255;
        }
    }
    let out = out_bmp(if gpu { "gpu_noise" } else { "noise" });
    write_bmp(&out, params.size_x, params.size_y, &viz)
        .map_err(|e| format!("BMP 書き出し失敗: {e}"))?;
    let distinct = coarse
        .iter()
        .map(|v| v.to_bits())
        .collect::<std::collections::HashSet<_>>()
        .len();
    let dmin = dense.iter().copied().fold(f32::INFINITY, f32::min);
    let dmax = dense.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    println!("[FrameProofExtra/{tag}] 値ノイズ upsample 実生成完了");
    println!(
        "  volume      : 32^3 dense={} coarse={} (サンプル削減 64x)",
        dense.len(),
        coarse.len()
    );
    println!("  distinct    : {distinct}/729 coarse 値 (退化定数でないことの実証)");
    println!("  dense range : [{dmin:.4}, {dmax:.4}]");
    println!("  output      : {}", out.display());
    Ok(())
}

// ---------------------------------------------------------------- lbvh / meshlet 共通

fn chunk_spheres() -> Vec<[f32; 4]> {
    // カメラが見下ろす 4x4 チャンク (視錐台内) + 背後 4x4 (カリング実効果の実在化)
    let mut v = Vec::new();
    for cz in 0..4 {
        for cx in 0..4 {
            v.push([cx as f32 * 16.0 + 8.0, 16.0, cz as f32 * 16.0 + 8.0, 9.0]);
        }
    }
    for cz in 0..4 {
        for cx in 0..4 {
            v.push([
                -240.0 + cx as f32 * 16.0,
                -40.0,
                -240.0 + cz as f32 * 16.0,
                9.0,
            ]);
        }
    }
    v
}

fn cull_viz(vis: &[u32]) -> Vec<u8> {
    // 近景 4x4 + 背後 4x4 のトップダウン可視化 (左: 近景 右: 背後)
    let cell = 20u32;
    let w = cell * 8;
    let h = cell * 4;
    let mut img = vec![24u8; (w * h * 4) as usize];
    for (i, &v) in vis.iter().enumerate() {
        let base = if i < 16 { i } else { (i - 16) + 16 };
        let cx = (base % 16) as u32 % 4 + if i < 16 { 0 } else { 4 };
        let cz = (base % 16) as u32 / 4;
        let (r, g) = if v == 1 { (48, 220) } else { (220, 48) };
        for dy in 2..cell - 2 {
            for dx in 2..cell - 2 {
                let px = ((cz * cell + dy) * w + (cx * cell + dx)) as usize * 4;
                img[px] = r;
                img[px + 1] = g;
                img[px + 2] = 48;
                img[px + 3] = 255;
            }
        }
    }
    img
}

fn run_lbvh(gpu: bool) -> Result<(), String> {
    let spheres = chunk_spheres();
    let vp = build_view_proj(&camera());
    let planes_v = wg::frustum_planes(&vp);
    let planes: Vec<[f32; 4]> = planes_v.to_vec();
    let min = [-256.0f32, -64.0, -256.0];
    let span = [512.0f32, 128.0, 512.0];
    let cpu_codes = wg::lbvh_codes_cpu(&spheres, min, span);
    let cpu_vis = wg::lbvh_cull_cpu(&spheres, &planes);
    let tag = if gpu { "gpu-lbvh" } else { "cpu-lbvh" };
    let (codes, vis) = if gpu {
        let rt = gpu_rt()?;
        let (g_codes, g_vis) =
            wg::GpuLbvh::new(&rt.device).run(&rt.device, &rt.queue, &spheres, min, span, &planes);
        let mc = g_codes
            .iter()
            .zip(cpu_codes.iter())
            .filter(|(a, b)| a != b)
            .count();
        let mv = g_vis
            .iter()
            .zip(cpu_vis.iter())
            .filter(|(a, b)| a != b)
            .count();
        println!(
            "  GPU==CPU    : codes mismatch {mc}/{} vis mismatch {mv}/{} (exact)",
            cpu_codes.len(),
            cpu_vis.len()
        );
        assert_eq!(mc + mv, 0, "gpu-lbvh: 不一致");
        (g_codes, g_vis)
    } else {
        (cpu_codes, cpu_vis)
    };
    let kept = vis.iter().filter(|&&v| v == 1).count();
    let distinct_codes = codes.iter().collect::<std::collections::HashSet<_>>().len();
    let out = out_bmp(if gpu { "gpu_lbvh" } else { "lbvh" });
    let img = cull_viz(&vis);
    write_bmp(&out, 160, 80, &img).map_err(|e| format!("BMP 書き出し失敗: {e}"))?;
    println!("[FrameProofExtra/{tag}] LBVH morton + frustum cull 実実行完了");
    println!(
        "  spheres     : {} (近景 4x4 + 背後 4x4 チャンク、実カメラ frustum)",
        spheres.len()
    );
    println!(
        "  visible     : {kept}/{} culled={}",
        spheres.len(),
        spheres.len() - kept
    );
    println!("  morton      : {distinct_codes}/32 distinct codes");
    println!("  output      : {}", out.display());
    assert!(
        kept > 0 && kept < spheres.len(),
        "カリングが実効果を持たない"
    );
    Ok(())
}

// ---------------------------------------------------------------- meshlet (task/mesh 等価エミュレーション)

fn run_meshlet(gpu: bool) -> Result<(), String> {
    // 実クアッド → 32 クアッド/メッシュレットに実クラスタ → AABB → 境界球
    let palettes = demo_column_palettes(0, 0);
    let mesh = mesh_chunk_column_pull_world(&palettes, 0, 0, 0, 0, 0, 0, true);
    let quads = &mesh.quads;
    let mut meshlets: Vec<[f32; 4]> = Vec::new();
    for chunk in quads.chunks(32) {
        let mut mn = [f32::INFINITY; 3];
        let mut mx = [f32::NEG_INFINITY; 3];
        for q in chunk {
            let pos = [
                PackedPullQuad::unpack_x(q.word0) as f32,
                PackedPullQuad::unpack_y(q.word0) as f32,
                PackedPullQuad::unpack_z(q.word0) as f32,
            ];
            for c in 0..3 {
                mn[c] = mn[c].min(pos[c]);
                mx[c] = mx[c].max(pos[c].max(mn[c]) + 1.0);
            }
        }
        let center = [
            (mn[0] + mx[0]) * 0.5,
            (mn[1] + mx[1]) * 0.5,
            (mn[2] + mx[2]) * 0.5,
        ];
        let dx = (mx[0] - mn[0]) * 0.5;
        let dy = (mx[1] - mn[1]) * 0.5;
        let dz = (mx[2] - mn[2]) * 0.5;
        let radius = (dx * dx + dy * dy + dz * dz).sqrt();
        meshlets.push([center[0], center[1], center[2], radius]);
    }
    let vp = build_view_proj(&camera());
    let planes_v = wg::frustum_planes(&vp);
    let planes: Vec<[f32; 4]> = planes_v.to_vec();
    let cpu_vis = wg::meshlet_cull_cpu(&meshlets, &planes);
    let tag = if gpu { "gpu-meshlet" } else { "cpu-meshlet" };
    let vis = if gpu {
        let rt = gpu_rt()?;
        let g = wg::GpuMeshletCull::new(&rt.device).run(&rt.device, &rt.queue, &meshlets, &planes);
        let mv = g.iter().zip(cpu_vis.iter()).filter(|(a, b)| a != b).count();
        println!(
            "  GPU==CPU    : vis mismatch {mv}/{} (exact)",
            cpu_vis.len()
        );
        assert_eq!(mv, 0, "gpu-meshlet: 不一致");
        g
    } else {
        cpu_vis
    };
    let kept = vis.iter().filter(|&&v| v == 1).count();
    // 実カリング効果の検証: チャンクに背を向ける第 2 カメラでは大半がカリング
    // されるはず (同じ実メッシュレット・同じ実フラスタム抽出で実測)
    let cam_away = FrameCamera {
        eye: [8.0, 20.0, 8.0],
        target: [200.0, 20.0, 8.0],
        up: [0.0, 1.0, 0.0],
        fov_y_deg: 60.0,
        aspect: W as f32 / H as f32,
        near: 0.1,
        far: 500.0,
    };
    let planes_away = wg::frustum_planes(&build_view_proj(&cam_away));
    let vis_away = wg::meshlet_cull_cpu(&meshlets, &planes_away);
    let kept_away = vis_away.iter().filter(|&&v| v == 1).count();
    // トップダウン散布図 (XZ 平面): 実カメラのみ可視=緑、両方可視=黄、両方不可視=赤
    let mut img = vec![0u8; 256 * 256 * 4];
    for (i, m) in meshlets.iter().enumerate() {
        let px = ((m[0] / 128.0 * 256.0) as i32).clamp(0, 255);
        let pz = ((m[2] / 128.0 * 256.0) as i32).clamp(0, 255);
        let (r, g) = match (vis[i], vis_away[i]) {
            (1, 1) => (220, 220),
            (1, _) => (48, 220),
            _ => (220, 48),
        };
        let rad = (m[3] * 2.0) as i32;
        for dy in -rad..=rad {
            for dx in -rad..=rad {
                let x = (px + dx).clamp(0, 255) as usize;
                let z = (pz + dy).clamp(0, 255) as usize;
                let p = (z * 256 + x) * 4;
                img[p] = r;
                img[p + 1] = g;
                img[p + 2] = 32;
                img[p + 3] = 255;
            }
        }
    }
    let out = out_bmp(if gpu { "gpu_meshlet" } else { "meshlet" });
    write_bmp(&out, 256, 256, &img).map_err(|e| format!("BMP 書き出し失敗: {e}"))?;
    println!("[FrameProofExtra/{tag}] task/mesh 等価エミュレーション (meshlet cull) 完了");
    println!(
        "  meshlets    : {} (実クアッド {} 個を 32/クラスタ)",
        meshlets.len(),
        quads.len()
    );
    println!(
        "  visible     : {kept}/{} culled={} (実カメラ) / {kept_away}/{} culled={} (背向カメラ)",
        meshlets.len(),
        meshlets.len() - kept,
        meshlets.len(),
        meshlets.len() - kept_away
    );
    println!("  output      : {}", out.display());
    assert!(
        kept > 0,
        "全メッシュレットがカリング — 構図と矛盾 (実効果不成立)"
    );
    assert!(
        kept_away < kept,
        "背向カメラでカリングが増えない — frustum cull が実効果を持たない"
    );
    Ok(())
}

// ---------------------------------------------------------------- bindless

fn run_bindless(gpu: bool) -> Result<(), String> {
    // 実ハンドル列: デモメッシュの実 texture id + 連番スロットを pack
    let palettes = demo_column_palettes(0, 0);
    let mesh = mesh_chunk_column_pull_world(&palettes, 0, 0, 0, 0, 0, 0, true);
    let mut handles: Vec<u32> = Vec::new();
    for (i, q) in mesh.quads.iter().enumerate() {
        let tex = PackedPullQuad::unpack_tex(q.word0);
        handles.push(rsift_opt_gfx::bindless::pack_handle(
            0,
            (tex & 0xFF) as u32,
            (i as u32) & 0xFFFFF,
        ));
    }
    let cpu = wg::bindless_unpack_cpu(&handles);
    let tag = if gpu { "gpu-bindless" } else { "cpu-bindless" };
    let unpacked = if gpu {
        let rt = gpu_rt()?;
        let g = wg::GpuBindless::new(&rt.device).run(&rt.device, &rt.queue, &handles);
        let mism = g.iter().zip(cpu.iter()).filter(|(a, b)| a != b).count();
        println!(
            "  GPU==CPU    : mismatch {mism} / {} words (exact)",
            cpu.len()
        );
        assert_eq!(mism, 0, "gpu-bindless: 不一致");
        g
    } else {
        cpu
    };
    // roundtrip 検証
    let mut ok = 0u32;
    for (i, &h) in handles.iter().enumerate() {
        let (s, b, ix) = (unpacked[i * 3], unpacked[i * 3 + 1], unpacked[i * 3 + 2]);
        if rsift_opt_gfx::bindless::pack_handle(s, b, ix) == h {
            ok += 1;
        }
    }
    let distinct_tex: std::collections::HashSet<u32> =
        (0..handles.len()).map(|i| unpacked[i * 3 + 1]).collect();
    println!("[FrameProofExtra/{tag}] bindless ハンドル実解決完了");
    println!(
        "  handles     : {} (実クアッド texture id 由来)",
        handles.len()
    );
    println!(
        "  roundtrip   : {ok}/{} (pack(unpack(h))==h)",
        handles.len()
    );
    println!(
        "  slots       : {} distinct bindings (アトラススロット解決の実データ)",
        distinct_tex.len()
    );
    assert_eq!(ok as usize, handles.len(), "roundtrip 不全");
    Ok(())
}

// ---------------------------------------------------------------- half_vertex

fn run_half(gpu: bool) -> Result<(), String> {
    // 実 f32 ベクトル列 (DDGI で実使用の fibonacci 方向) を f16 量子化→デコード
    let dirs = rsift_opt_gfx::frame_ddgi::fibonacci_dirs(128);
    let mut packed: Vec<u32> = Vec::new();
    for d in &dirs {
        let a = rsift_opt_gfx::half_vertex::f32_to_f16(d[0]) as u32;
        let b = rsift_opt_gfx::half_vertex::f32_to_f16(d[1]) as u32;
        let c = rsift_opt_gfx::half_vertex::f32_to_f16(d[2]) as u32;
        packed.push(a | (b << 16));
        packed.push(c); // 上位 16 bit は 0 (f16 ゼロ、実データとして妥当)
    }
    let cpu = wg::half_unpack_cpu(&packed);
    let tag = if gpu { "gpu-half" } else { "cpu-half" };
    let decoded = if gpu {
        let rt = gpu_rt()?;
        let g = wg::GpuHalfVertex::new(&rt.device).run(&rt.device, &rt.queue, &packed);
        let mism = g
            .iter()
            .zip(cpu.iter())
            .filter(|(a, b)| a.to_bits() != b.to_bits())
            .count();
        println!(
            "  GPU==CPU    : mismatch {mism} / {} floats (bitwise)",
            cpu.len()
        );
        assert_eq!(mism, 0, "gpu-half: 不一致");
        g
    } else {
        cpu
    };
    // 実効果: 帯域半減 + 復元誤差 (f16 量子化の真性誤差)
    // decoded のレイアウトは 1 語 2 float → 1 ベクトルあたり 4 float (x,y,z,0)
    let mut max_err = 0.0f32;
    for (i, d) in dirs.iter().enumerate() {
        for c in 0..3 {
            let got = decoded[i * 4 + c];
            max_err = max_err.max((got - d[c]).abs());
        }
    }
    let f32_bytes = dirs.len() * 3 * 4;
    let f16_bytes = dirs.len() * 3 * 2; // u16 詰め頂点バッファとしての実サイズ
    println!("[FrameProofExtra/{tag}] f16 頂点デコード実実行完了");
    println!(
        "  vectors     : {} (fibonacci dirs、実 DDGI データ)",
        dirs.len()
    );
    println!(
        "  bytes       : {f32_bytes}B → {f16_bytes}B ({:.0}% 削減)",
        100.0 - f16_bytes as f32 / f32_bytes as f32 * 100.0
    );
    println!("  max |err|   : {max_err:.6} (f16 量子化真性誤差)");
    Ok(())
}

// ---------------------------------------------------------------- main

fn main() {
    tracing_subscriber::fmt::try_init().ok();
    let mode = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "cpu-cas".to_string());
    println!("=== Frame Proof Extra (Phase E: 未配線 9 モジュール全配線) ===");
    println!("mode={mode}");
    let r = match mode.as_str() {
        "cpu-cas" => run_cas(false),
        "gpu-cas" => run_cas(true),
        "cpu-checker" => run_checker(false),
        "gpu-checker" => run_checker(true),
        "cpu-exposure" => run_exposure(false),
        "gpu-exposure" => run_exposure(true),
        "cpu-vrs" => run_vrs(false),
        "gpu-vrs" => run_vrs(true),
        "cpu-noise" => run_noise(false),
        "gpu-noise" => run_noise(true),
        "cpu-lbvh" => run_lbvh(false),
        "gpu-lbvh" => run_lbvh(true),
        "cpu-bindless" => run_bindless(false),
        "gpu-bindless" => run_bindless(true),
        "cpu-half" => run_half(false),
        "gpu-half" => run_half(true),
        "cpu-meshlet" => run_meshlet(false),
        "gpu-meshlet" => run_meshlet(true),
        other => {
            eprintln!("unknown mode '{other}'");
            std::process::exit(2);
        }
    };
    if let Err(e) = r {
        eprintln!("[FrameProofExtra/{mode}] {e}");
        std::process::exit(2);
    }
}

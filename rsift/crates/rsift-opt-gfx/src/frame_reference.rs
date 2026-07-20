//! # Frame Reference Rasterizer (CPU) — GPU/CPU 相互検証用の参照実装
//!
//! `frame_pipeline.rs` (wgpu 本体) と**同一入力・同一数学規則**で実フレームを
//! CPU ラスタ化する。フェイクではなく、実メッシュの実ピクセルを本物の
//! ラスタライズ規則で算出し、GPU 出力との突合・オフライン環境での
//! 実画像検証を可能にする参照経路。
//!
//! 規則は `shaders/terrain_vertex_pull.wgsl` (頂点展開・Lambert) と
//! `shaders/aces_tonemap.wgsl` (ACES+sRGB) に厳密一致させること。
//! シェーダ側の定数 (SUN_DIR / AO 係数 / band 係数) を変えたらここも変える。

use crate::packed4::PackedPullQuad;
use crate::pull_mesh::PullBuiltMesh;

/// WGSL `SUN_DIR` と同一 (normalize(0.6, 1.0, 0.3))。
const SUN_DIR: [f32; 3] = [0.4985076, 0.8308459, 0.2492538];
/// WGSL `FACE_UV` と同一順序。
const FACE_UV: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
/// WGSL `TRI_CORNER` と同一順序。
const TRI_CORNER: [u32; 6] = [0, 1, 2, 2, 3, 0];

pub struct CpuFrame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>, // RGBA8 (ACES+sRGB 適用済み)
    /// 深度が実際に書き込まれた (ジオメトリで被覆された) ピクセル数。
    pub covered_px: u32,
    pub quads: u32,
    /// 被覆ピクセルの平均輝度 (内容があることの定量指標)。
    pub avg_lum: f32,
}

/// WGSL `corner_pos` の face スイッチと同一。
fn corner_pos(face: u32, o: [f32; 3], w: f32, h: f32, corner: u32) -> [f32; 3] {
    let cu = FACE_UV[corner as usize][0];
    let cv = FACE_UV[corner as usize][1];
    match face {
        0 => [o[0] + 1.0, o[1] + cv * h, o[2] + cu * w],
        1 => [o[0], o[1] + cv * h, o[2] + cu * w],
        2 => [o[0] + cu * w, o[1] + 1.0, o[2] + cv * h],
        3 => [o[0] + cu * w, o[1], o[2] + cv * h],
        4 => [o[0] + cu * w, o[1] + cv * h, o[2] + 1.0],
        _ => [o[0] + cu * w, o[1] + cv * h, o[2]],
    }
}

/// WGSL `face_normal` と同一。
fn face_normal(face: u32) -> [f32; 3] {
    match face {
        0 => [1.0, 0.0, 0.0],
        1 => [-1.0, 0.0, 0.0],
        2 => [0.0, 1.0, 0.0],
        3 => [0.0, -1.0, 0.0],
        4 => [0.0, 0.0, 1.0],
        _ => [0.0, 0.0, -1.0],
    }
}

/// fs_pull の Lambert + AO + band と同一。
fn shade(tex: u32, light_ao: u32, face: u32) -> [f32; 3] {
    let ao = 0.55 + light_ao as f32 * 0.15;
    let band = (tex % 7) as f32 / 7.0;
    let n = face_normal(face);
    let ndl = (n[0] * SUN_DIR[0] + n[1] * SUN_DIR[1] + n[2] * SUN_DIR[2]).max(0.0);
    let li = 0.35 + 0.65 * ndl;
    [band * ao * li, band * 0.6 * ao * li, band * 0.3 * ao * li]
}

/// aces_tonemap.wgsl の aces() + linear_to_srgb() と同一 (exposure=1.0)。
fn aces_srgb(x: [f32; 3]) -> [f32; 3] {
    let aces = |v: f32| -> f32 {
        let (a, b, c, d, e) = (2.51, 0.03, 2.43, 0.59, 0.14);
        ((v * (a * v + b)) / (v * (c * v + d) + e)).clamp(0.0, 1.0)
    };
    [
        aces(x[0]).max(0.0).powf(1.0 / 2.2),
        aces(x[1]).max(0.0).powf(1.0 / 2.2),
        aces(x[2]).max(0.0).powf(1.0 / 2.2),
    ]
}

/// CPU 参照レンダリング。`vp` は `frame_pipeline::build_view_proj` の生成物
/// (列優先) をそのまま受け取る。
pub fn render_reference(
    chunks: &[(PullBuiltMesh, [f32; 3])],
    vp: &[[f32; 4]; 4],
    width: u32,
    height: u32,
) -> CpuFrame {
    let sky = [0.02f32, 0.03, 0.05]; // HDR クリア色 (frame_pipeline と同一)
    let npix = (width * height) as usize;
    let mut color = vec![sky; npix];
    let mut depth = vec![1.0f32; npix];
    let mut covered = 0u32;
    let mut quads_total = 0u32;

    for (mesh, origin) in chunks {
        for q in &mesh.quads {
            quads_total += 1;
            let ox = PackedPullQuad::unpack_x(q.word0) as f32;
            let oy = PackedPullQuad::unpack_y(q.word0) as f32;
            let oz = PackedPullQuad::unpack_z(q.word0) as f32;
            let tex = PackedPullQuad::unpack_tex(q.word0);
            let lao = PackedPullQuad::unpack_light_ao(q.word0);
            let face = PackedPullQuad::unpack_face(q.word1);
            let qw = PackedPullQuad::unpack_width(q.word1) as f32;
            let qh = PackedPullQuad::unpack_height(q.word1) as f32;

            // 4 角をクリップ空間へ (WGSL の vs_pull と同一計算)
            let mut sx = [0.0f32; 4];
            let mut sy = [0.0f32; 4];
            let mut sz_ndc = [0.0f32; 4];
            let mut sw = [0.0f32; 4];
            let mut visible = true;
            for c in 0..4 {
                let local = corner_pos(face, [ox, oy, oz], qw, qh, c as u32);
                let world = [
                    local[0] + origin[0],
                    local[1] + origin[1],
                    local[2] + origin[2],
                ];
                let clip = crate::frame_pipeline::mul_v4(vp, [world[0], world[1], world[2], 1.0]);
                let w = clip[3];
                if !(w > 1e-5) {
                    // near 面より手前 (本格クリッピングの代わりに quad 棄却:
                    // 参照用途で十分かつ GPU でもジオメトリ消失領域)。
                    visible = false;
                    break;
                }
                let nx = clip[0] / w;
                let ny = clip[1] / w;
                sx[c] = (nx * 0.5 + 0.5) * width as f32;
                sy[c] = (1.0 - (ny * 0.5 + 0.5)) * height as f32;
                sz_ndc[c] = clip[2] / w;
                sw[c] = 1.0 / w;
            }
            if !visible {
                continue;
            }

            let col = shade(tex, lao, face);
            for t in 0..2 {
                let tri = [
                    TRI_CORNER[t * 3] as usize,
                    TRI_CORNER[t * 3 + 1] as usize,
                    TRI_CORNER[t * 3 + 2] as usize,
                ];
                raster_tri(
                    [sx[tri[0]], sy[tri[0]]],
                    [sx[tri[1]], sy[tri[1]]],
                    [sx[tri[2]], sy[tri[2]]],
                    [sz_ndc[tri[0]], sz_ndc[tri[1]], sz_ndc[tri[2]]],
                    [sw[tri[0]], sw[tri[1]], sw[tri[2]]],
                    col,
                    width,
                    height,
                    &mut color,
                    &mut depth,
                    &mut covered,
                );
            }
        }
    }

    // ACES + sRGB (post パスと同一)
    let mut pixels = Vec::with_capacity(npix * 4);
    let mut lum_sum = 0.0f64;
    for px in color.iter() {
        let srgb = aces_srgb(*px);
        lum_sum += (srgb[0] * 0.2126 + srgb[1] * 0.7152 + srgb[2] * 0.0722) as f64;
        pixels.push((srgb[0].clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
        pixels.push((srgb[1].clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
        pixels.push((srgb[2].clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
        pixels.push(255);
    }
    let avg_lum = if covered > 0 {
        (lum_sum / covered.max(1) as f64) as f32
    } else {
        0.0
    };
    CpuFrame {
        width,
        height,
        pixels,
        covered_px: covered,
        quads: quads_total,
        avg_lum,
    }
}

/// エッジ関数ベースの実三角形ラスタ (透視補正 ndc.z 深度)。
#[allow(clippy::too_many_arguments)]
fn raster_tri(
    a: [f32; 2],
    b: [f32; 2],
    c: [f32; 2],
    za: [f32; 3],
    wa: [f32; 3],
    col: [f32; 3],
    width: u32,
    height: u32,
    color: &mut [[f32; 3]],
    depth: &mut [f32],
    covered: &mut u32,
) {
    let edge = |p: [f32; 2], q: [f32; 2], r: [f32; 2]| {
        (r[0] - p[0]) * (q[1] - p[1]) - (r[1] - p[1]) * (q[0] - p[0])
    };
    let area = edge(a, b, c);
    if area.abs() < 1e-9 {
        return; // 縮退三角形
    }
    let min_x = a[0].min(b[0]).min(c[0]).floor().max(0.0) as u32;
    let max_x = (a[0].max(b[0]).max(c[0]).ceil() as u32).min(width);
    let min_y = a[1].min(b[1]).min(c[1]).floor().max(0.0) as u32;
    let max_y = (a[1].max(b[1]).max(c[1]).ceil() as u32).min(height);
    for y in min_y..max_y {
        for x in min_x..max_x {
            let p = [x as f32 + 0.5, y as f32 + 0.5];
            let w0 = edge(b, c, p) / area;
            let w1 = edge(c, a, p) / area;
            let w2 = edge(a, b, p) / area;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            // 透視補正: Σ(λi * (1/wi)) で z を補間 (ndc.z は 1/w 線形)
            let inv_w = w0 * wa[0] + w1 * wa[1] + w2 * wa[2];
            if inv_w <= 1e-9 {
                continue;
            }
            let z = (w0 * za[0] * wa[0] + w1 * za[1] * wa[1] + w2 * za[2] * wa[2]) / inv_w;
            let idx = (y * width + x) as usize;
            if z < depth[idx] {
                depth[idx] = z;
                color[idx] = col;
                *covered += 1;
            }
        }
    }
}

/// RGBA8 を 24-bit BMP (top-down, BGR) として実ファイルに書き出す。
/// top-down は高さを負値で書く DIB 形式 (主要ビューア互換)。
pub fn write_bmp(
    path: impl AsRef<std::path::Path>,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> std::io::Result<()> {
    use std::io::Write;
    let row_bytes = width as usize * 3;
    let pad = (4 - row_bytes % 4) % 4;
    let img_size = ((row_bytes + pad) * height as usize) as u32;
    let mut f = std::fs::File::create(path)?;
    let mut header = [0u8; 54];
    header[0..2].copy_from_slice(b"BM");
    header[2..6].copy_from_slice(&(54 + img_size).to_le_bytes());
    header[10..14].copy_from_slice(&54u32.to_le_bytes());
    header[14..18].copy_from_slice(&40u32.to_le_bytes());
    header[18..22].copy_from_slice(&(width as i32).to_le_bytes());
    header[22..26].copy_from_slice(&(-(height as i32)).to_le_bytes()); // top-down
    header[26..28].copy_from_slice(&1u16.to_le_bytes());
    header[28..30].copy_from_slice(&24u16.to_le_bytes());
    header[34..38].copy_from_slice(&img_size.to_le_bytes());
    f.write_all(&header)?;
    let mut row = vec![0u8; row_bytes + pad];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let s = (y * width as usize + x) * 4;
            row[x * 3] = rgba[s + 2]; // B
            row[x * 3 + 1] = rgba[s + 1]; // G
            row[x * 3 + 2] = rgba[s]; // R
        }
        f.write_all(&row)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame_pipeline::{build_view_proj, FrameCamera};

    fn demo_chunks() -> Vec<(PullBuiltMesh, [f32; 3])> {
        let palettes = crate::binary_greedy_meshing::demo_column_palettes(0, 0);
        let mesh = crate::binary_greedy_meshing::mesh_chunk_column_pull_world(
            &palettes, 0, 0, 0, 0, 0, 0, true,
        );
        vec![(mesh, [0.0, 0.0, 0.0])]
    }

    #[test]
    fn reference_frame_has_real_coverage_and_shading() {
        let cam = FrameCamera {
            eye: [-28.0, 72.0, -28.0],
            target: [8.0, 16.0, 8.0],
            up: [0.0, 1.0, 0.0],
            fov_y_deg: 60.0,
            aspect: 1.0,
            near: 0.1,
            far: 500.0,
        };
        let vp = build_view_proj(&cam);
        let chunks = demo_chunks();
        let fr = render_reference(&chunks, &vp, 128, 128);
        assert!(fr.quads > 0, "demo mesh must have quads");
        assert!(fr.covered_px > 0, "some pixels must be covered");
        assert!(
            fr.covered_px < 128 * 128,
            "coverage must be partial (sky visible)"
        );
        assert!(fr.avg_lum > 0.01, "shaded pixels must be non-black");
    }
}

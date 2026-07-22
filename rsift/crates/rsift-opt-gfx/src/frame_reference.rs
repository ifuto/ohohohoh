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

/// WGSL `SUN_DIR` と同一値 (normalize(0.6, 1.0, 0.3) を f32 演算で評価した値)。
/// 旧値 [0.4985076, 0.8308459, 0.2492538] は |v|≈1.00047 の非単位ベクトルで
/// doc の「normalize(0.6,1,0.3)」と約 2.4e-4 (各成分) ずれており、かつ WGSL 側に
/// SUN_DIR が存在していなかった (2026-07-22 wave 21 監査で厳密再導出値に両側同時訂正)。
/// `sun_dir_is_f32_normalized_direction` テストが再導出一致を機械保証する。
const SUN_DIR: [f32; 3] = [0.49827290, 0.83045477, 0.24913645];
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
    /// 全ピクセルの NDC z 深度 (1.0 = 未被覆 / far)。補間は**スクリーン空間
    /// 線形** — GPU 固定機能ラスタライザ (Depth32Float アタッチメント) と
    /// 同一規則 (wave 17 で透視補正補間から訂正: z_ndc = clip.z/clip.w は
    /// 画面重心座標の厳密なアフィン関数であり、透視補正式
    /// Σλ(z/w)/Σλ(1/w) とは数学的に一致しない)。
    pub depth: Vec<f32>,
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
        depth,
    }
}

/// エッジ関数ベースの実三角形ラスタ (深度は画面重心座標の線形補間)。
///
/// z_ndc = clip.z/clip.w はスクリーン空間で厳密にアフィン (z_ndc = A + B/w と
/// 1/w が画面アフィンであることから導かれる定石結果) であり、GPU 固定機能
/// ラスタライザはこれを画面線形で補間する。旧実装は透視補正式
/// Σλ(z/w)/Σλ(1/w) を採っていたが、これは z_ndc に対しては
/// 同一三角形内で w が変化する限り GPU 値と一致しない (誤差は λ 加重の
/// 1/w の分散に比例)。wave 17 でハードウェア規則の画面線形に訂正。
/// ただし z 比較は全角 w>0 (quad 棄却で保証) の正深度領域でのみ行う。
#[allow(clippy::too_many_arguments)]
fn raster_tri(
    a: [f32; 2],
    b: [f32; 2],
    c: [f32; 2],
    za: [f32; 3],
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
            // z_ndc の画面線形補間 (GPU 深度アタッチメントと同一規則)。
            let z = w0 * za[0] + w1 * za[1] + w2 * za[2];
            let idx = (y * width + x) as usize;
            if z < depth[idx] {
                depth[idx] = z;
                color[idx] = col;
                *covered += 1;
            }
        }
    }
}

// ---------- Phase B: FSR1 (EASU + RCAS) の WGSL 精密ミラー ----------

/// `shaders/fsr1.wgsl` の fsr_easu + fsr_rcas と同一数学で RGBA8 を拡大する。
/// 入力は低解像度 RGBA8 (ACES 後 LDR)。出力は全解像度 RGBA8。
/// 差異は float→unorm 格納の丸めのみ (WGSL は round-to-nearest、こちらは +0.5 切捨て。
/// 境界での ±1LSB は統計比較の許容内 — ビット等値は要求しない)。
pub fn fsr1_reference(
    low: &[u8],
    low_w: u32,
    low_h: u32,
    full_w: u32,
    full_h: u32,
    sharpness: f32,
) -> Vec<u8> {
    assert_eq!(low.len(), (low_w * low_h * 4) as usize, "low px size");
    let px = |x: i64, y: i64| -> [f32; 3] {
        // sampler ClampToEdge 相当
        let cx = x.clamp(0, low_w as i64 - 1) as usize;
        let cy = y.clamp(0, low_h as i64 - 1) as usize;
        let s = (cy * low_w as usize + cx) * 4;
        [
            low[s] as f32 / 255.0,
            low[s + 1] as f32 / 255.0,
            low[s + 2] as f32 / 255.0,
        ]
    };

    // --- fsr_easu (勾配は R チャンネルのみ — WGSL/fsr1.rs 3連鎖規約) ---
    let mut inter = vec![[0.0f32; 3]; (full_w * full_h) as usize];
    for gy in 0..full_h {
        for gx in 0..full_w {
            // uv = (gid+0.5)/outputSize; lr = uv*inputSize - 0.5
            let ux = (gx as f32 + 0.5) / full_w as f32;
            let uy = (gy as f32 + 0.5) / full_h as f32;
            let lx = ux * low_w as f32 - 0.5;
            let ly = uy * low_h as f32 - 0.5;
            let bx = lx.floor();
            let by = ly.floor();
            let fx = lx - bx;
            let fy = ly - by;
            let (bx, by) = (bx as i64, by as i64);
            let p00 = px(bx, by);
            let p10 = px(bx + 1, by);
            let p01 = px(bx, by + 1);
            let p11 = px(bx + 1, by + 1);
            let grx = ((p10[0] + p11[0]) - (p00[0] + p01[0])).abs();
            let gry = ((p00[0] + p10[0]) - (p01[0] + p11[0])).abs();
            let ex = grx / (grx + 0.5);
            let ey = gry / (gry + 0.5);
            let fx2 = fx + (0.5 - fx) * ex;
            let fy2 = fy + (0.5 - fy) * ey;
            let mut outc = [0.0f32; 3];
            for ch in 0..3 {
                let top = p00[ch] + (p10[ch] - p00[ch]) * fx2;
                let bot = p01[ch] + (p11[ch] - p01[ch]) * fx2;
                outc[ch] = top + (bot - top) * fy2;
            }
            inter[(gy * full_w + gx) as usize] = outc;
        }
    }

    // --- fsr_rcas (OOB textureLoad = 0、WGSL 準拠) ---
    // 符号規約: `c - lap * sharpness` (中心を近傍平均から遠ざける = 鮮鋭化)。
    // `+` は「ぼかし」であり fsr1.rs::rcas / cas.rs::cas_sample と矛盾する
    // (wave 17 で WGSL 側と同時に修正、3連鎖は常に同符号)。
    let load = |x: i64, y: i64| -> [f32; 3] {
        if x < 0 || y < 0 || x >= full_w as i64 || y >= full_h as i64 {
            [0.0; 3]
        } else {
            inter[(y as u32 * full_w + x as u32) as usize]
        }
    };
    let mut out = Vec::with_capacity((full_w * full_h * 4) as usize);
    for gy in 0..full_h as i64 {
        for gx in 0..full_w as i64 {
            let c = load(gx, gy);
            let n = load(gx, gy + 1);
            let s = load(gx, gy - 1);
            let e = load(gx + 1, gy);
            let w = load(gx - 1, gy);
            for ch in 0..3 {
                let lap = (n[ch] + s[ch] + e[ch] + w[ch]) * 0.25 - c[ch];
                let v = (c[ch] - lap * sharpness).clamp(0.0, 1.0);
                out.push((v * 255.0 + 0.5) as u8);
            }
            out.push(255);
        }
    }
    out
}

/// 低解像度で CPU 参照レンダリング → FSR1 (WGSL ミラー) で全解像度へ。
/// 被覆統計は空色 (ACES エンコード済み) と異なる画素を全解像度側で計測。
pub fn render_reference_fsr(
    chunks: &[(PullBuiltMesh, [f32; 3])],
    vp: &[[f32; 4]; 4],
    low_w: u32,
    low_h: u32,
    full_w: u32,
    full_h: u32,
    sharpness: f32,
) -> CpuFrame {
    let lo = render_reference(chunks, vp, low_w, low_h);
    let pixels = fsr1_reference(&lo.pixels, low_w, low_h, full_w, full_h, sharpness);
    let sky_f = aces_srgb([0.02, 0.03, 0.05]);
    let sky = [
        (sky_f[0].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        (sky_f[1].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        (sky_f[2].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
    ];
    let mut covered = 0u32;
    let mut lum_sum = 0.0f64;
    for px in pixels.chunks_exact(4) {
        if px[0] != sky[0] || px[1] != sky[1] || px[2] != sky[2] {
            covered += 1;
            lum_sum +=
                (px[0] as f64 * 0.2126 + px[1] as f64 * 0.7152 + px[2] as f64 * 0.0722) / 255.0;
        }
    }
    // 深度も画素と同一の位置対応で nearest 拡大 (Hi-Z 参照経路の整合のため)
    let mut depth = vec![1.0f32; (full_w * full_h) as usize];
    for y in 0..full_h {
        let ly = ((y as u64 * low_h as u64) / full_h as u64).min(low_h as u64 - 1) as u32;
        for x in 0..full_w {
            let lx = ((x as u64 * low_w as u64) / full_w as u64).min(low_w as u64 - 1) as u32;
            depth[(y * full_w + x) as usize] = lo.depth[(ly * low_w + lx) as usize];
        }
    }
    CpuFrame {
        width: full_w,
        height: full_h,
        pixels,
        covered_px: covered,
        quads: lo.quads,
        avg_lum: if covered > 0 {
            (lum_sum / covered as f64) as f32
        } else {
            0.0
        },
        depth,
    }
}

// ---------- Phase C: Hi-Z (保守的深度ダウンサンプル + AABB テスト) WGSL 精密ミラー ----------

/// `depth_psychic.wgsl` と同一規則: 主深度を dim x dim の Hi-Z マップへ
/// **セル内最遠深度 (max)** 集約でダウンサンプルする。floor/ceil の範囲決め、
/// max の冪等性まで WGSL と一致 (GPU/CPU 交叉検証の参照面)。
pub fn hiz_downsample_reference(depth: &[f32], width: u32, height: u32, dim: u32) -> Vec<f32> {
    assert_eq!(
        depth.len(),
        (width * height) as usize,
        "depth buffer size mismatch"
    );
    let mut out = vec![0.0f32; (dim * dim) as usize];
    let sw = width as f32 / dim as f32;
    let sh = height as f32 / dim as f32;
    for gy in 0..dim {
        for gx in 0..dim {
            let x0 = (gx as f32 * sw).floor() as u32;
            let y0 = (gy as f32 * sh).floor() as u32;
            let x1 = ((gx + 1) as f32 * sw).ceil().min(width as f32) as u32;
            let y1 = ((gy + 1) as f32 * sh).ceil().min(height as f32) as u32;
            let mut m = 0.0f32;
            for y in y0..y1 {
                for x in x0..x1 {
                    m = m.max(depth[(y * width + x) as usize]);
                }
            }
            out[(gy * dim + gx) as usize] = m;
        }
    }
    out
}

/// `hiz_raster.wgsl` と同一規則の AABB オクルージョンカバレッジ。
///
/// 各箱の 8 角を `vp` で射影 → NDC bbox → Hi-Z セル矩形を全走査し、
/// 「箱の最手前深度 z_min <= セル最遠深度 + DEPTH_EPS」のセル数を返す。
/// カメラ後方 / 画面外 / far 越えは **保守的可視 (coverage=1)**。
/// 返り値は `QueryCore::resolve` にそのまま渡せる per-box coverage。
pub fn hiz_test_reference(
    hiz: &[f32],
    dim: u32,
    boxes: &[crate::occlusion_query::QueryBox],
    vp: &[[f32; 4]; 4],
) -> Vec<u32> {
    const DEPTH_EPS: f32 = 1e-4; // hiz_raster.wgsl と同値
    assert_eq!(hiz.len(), (dim * dim) as usize, "hi-z map size mismatch");
    let h = dim as f32;
    boxes
        .iter()
        .map(|b| {
            let mut ndc_min = [1e9f32; 2];
            let mut ndc_max = [-1e9f32; 2];
            let mut z_min = 1e9f32;
            let mut any_invalid = false;
            for i in 0..8u32 {
                let px = if i & 1 != 0 { b.max[0] } else { b.min[0] };
                let py = if i & 2 != 0 { b.max[1] } else { b.min[1] };
                let pz = if i & 4 != 0 { b.max[2] } else { b.min[2] };
                let clip = crate::frame_pipeline::mul_v4(vp, [px, py, pz, 1.0]);
                let w = clip[3];
                // near 面より手前の角が 1 つでもある = near 跨ぎ/後方 (WGSL と同一:
                // z_min 偏りによる誤カリング防止のため全て判定不能=保守的可視)。
                // partial_cmp 版は NaN も「不明」として保守的可視に倒れる (WGSL `w > 1e-5`
                // の否定と完全一致)。
                if w.partial_cmp(&1e-5) != Some(std::cmp::Ordering::Greater) {
                    any_invalid = true;
                    continue;
                }
                let (nx, ny, nz) = (clip[0] / w, clip[1] / w, clip[2] / w);
                ndc_min[0] = ndc_min[0].min(nx);
                ndc_min[1] = ndc_min[1].min(ny);
                ndc_max[0] = ndc_max[0].max(nx);
                ndc_max[1] = ndc_max[1].max(ny);
                z_min = z_min.min(nz);
            }
            let offscreen = ndc_max[0] < -1.0
                || ndc_min[0] > 1.0
                || ndc_max[1] < -1.0
                || ndc_min[1] > 1.0
                || z_min > 1.0;
            if any_invalid || offscreen {
                return 1; // 保守的可視 (誤カリング絶対防止)
            }
            let cx0 = ((ndc_min[0] * 0.5 + 0.5) * h).floor().clamp(0.0, h - 1.0) as u32;
            let cy0 = ((0.5 - ndc_max[1] * 0.5) * h).floor().clamp(0.0, h - 1.0) as u32;
            let cx1 = (((ndc_max[0] * 0.5 + 0.5) * h).ceil().clamp(1.0, h) as u32).max(cx0 + 1);
            let cy1 = (((0.5 - ndc_min[1] * 0.5) * h).ceil().clamp(1.0, h) as u32).max(cy0 + 1);
            let mut cov = 0u32;
            for cy in cy0..cy1 {
                for cx in cx0..cx1 {
                    let farthest = hiz[(cy * dim + cx) as usize];
                    if z_min <= farthest + DEPTH_EPS {
                        cov += 1;
                    }
                }
            }
            cov
        })
        .collect()
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

    /// 全面占有の壁カラム (16x64x16)。実メッシュ生成ルート経由の決定的データ。
    /// (demo 地形はセクション間に水平空隙があり遮蔽が破綻する — 設計検証で
    /// 実測済のため、遮蔽成立テストでは空隙の無い壁を用いる)
    fn wall_chunks() -> Vec<(PullBuiltMesh, [f32; 3])> {
        let wall: Vec<crate::binary_greedy_meshing::SectionPalette> = vec![[1u16; 16 * 16 * 16]; 4];
        let mesh = crate::binary_greedy_meshing::mesh_chunk_column_pull_world(
            &wall, 0, 0, 0, 0, 0, 0, true,
        );
        vec![(mesh, [0.0, 0.0, 0.0])]
    }

    #[test]
    fn fsr1_flat_region_is_identity() {
        // 全面同一色の低解像度入力 → 内部は EASU+RCAS を通っても完全同一色
        // (勾配ゼロ、lap=0)。
        // 外周は WGSL の OOB textureLoad=0 規則により近傍ゼロが混入する。
        // RCAS は鮮鋭化 (中心を近傍平均から遠ざける: out = c - lap·s) なので、
        // 近傍平均がゼロ混入で低い → lap<0 → 境界画素は規則的に「明転」する。
        //   辺   : 平均 = 3c/4 → lap = -c/4 → c·(1 + s/4)  = 120·1.05 = 126
        //   角   : 平均 = c/2  → lap = -c/2 → c·(1 + s/2)  = 120·1.10 = 132
        // (旧ぼかし符号では暗転方向に振れていた — wave 17 RCAS 符号修正の回帰固定。
        //  GPU も WGSL 経由でこの同一規則に従う)
        let lo = vec![120u8; 4 * 4 * 4];
        let out = fsr1_reference(&lo, 4, 4, 8, 8, 0.2);
        assert_eq!(out.len(), 8 * 8 * 4);
        for y in 0..8usize {
            for x in 0..8usize {
                let v = out[(y * 8 + x) * 4];
                let border_x = x == 0 || x == 7;
                let border_y = y == 0 || y == 7;
                match (border_x, border_y) {
                    (false, false) => assert_eq!(v, 120, "interior must stay flat"),
                    (true, true) => assert_eq!(v, 132, "corner: 2 OOB taps → ×1.10 halo"),
                    _ => assert_eq!(v, 126, "edge: 1 OOB tap → ×1.05 halo"),
                }
            }
        }
    }

    #[test]
    fn fsr1_rcas_sharpens_valley_and_peak() {
        // 谷底 (周囲より暗い中心) は鮮鋭化でさらに深く、峰はさらに高くなる。
        // sharpness=0 (RCAS 無効 = EASU のみ) との差分で検証するため、
        // EASU のエッジ位置寄せの中間値には一切依存しない
        // (符号の向きそのものの直接固定。旧ぼかし符号 c + lap·s では
        //  本テストは両分岐とも決定的に失敗する)。
        let mut lo = vec![128u8; 3 * 3 * 4];
        for ch in 0..3 {
            lo[(1 * 3 + 1) * 4 + ch] = 64; // 谷底
        }
        let easu_only = fsr1_reference(&lo, 3, 3, 3, 3, 0.0);
        let sharpened = fsr1_reference(&lo, 3, 3, 3, 3, 0.2);
        let c0 = easu_only[(1 * 3 + 1) * 4];
        let c1 = sharpened[(1 * 3 + 1) * 4];
        assert!(
            c1 < c0,
            "valley must deepen under RCAS: sharp {c1} vs easu-only {c0}"
        );

        for ch in 0..3 {
            lo[(1 * 3 + 1) * 4 + ch] = 255; // 峰
        }
        let easu_only = fsr1_reference(&lo, 3, 3, 3, 3, 0.0);
        let sharpened = fsr1_reference(&lo, 3, 3, 3, 3, 0.2);
        let c0 = easu_only[(1 * 3 + 1) * 4];
        let c1 = sharpened[(1 * 3 + 1) * 4];
        assert!(
            c1 > c0,
            "peak must rise under RCAS: sharp {c1} vs easu-only {c0}"
        );
    }

    #[test]
    fn fsr1_preserves_hard_edge_and_differs_from_plain_bilinear() {
        // 垂直ハードエッジ (左=暗 0, 右=明 255) 4x4 → 8x8
        let mut lo = vec![0u8; 4 * 4 * 4];
        for y in 0..4usize {
            for x in 2..4usize {
                for ch in 0..3 {
                    lo[(y * 4 + x) * 4 + ch] = 255;
                }
            }
        }
        let out = fsr1_reference(&lo, 4, 4, 8, 8, 0.2);
        let get = |x: usize, y: usize| out[(y * 8 + x) * 4];
        // 明部の内側は明のまま、暗部の内側は暗のまま
        assert!(get(6, 4) >= 200, "bright interior preserved: {}", get(6, 4));
        assert!(get(0, 4) <= 20, "dark interior preserved: {}", get(0, 4));
        // ただのバイリニアとの差分が非ゼロ (EASU の位置寄せ + RCAS が実効果あり)
        let mut bil = vec![0u8; 8 * 8 * 4];
        for y in 0..8usize {
            for x in 0..8usize {
                let lx = ((x as f32 + 0.5) * 4.0 / 8.0 - 0.5).clamp(0.0, 3.0);
                let ly = ((y as f32 + 0.5) * 4.0 / 8.0 - 0.5).clamp(0.0, 3.0);
                let (x0, y0) = (lx as usize, ly as usize);
                let (x1, y1) = ((x0 + 1).min(3), (y0 + 1).min(3));
                let (fx, fy) = (lx.fract(), ly.fract());
                for ch in 0..3 {
                    let a = lo[(y0 * 4 + x0) * 4 + ch] as f32;
                    let b = lo[(y0 * 4 + x1) * 4 + ch] as f32;
                    let c = lo[(y1 * 4 + x0) * 4 + ch] as f32;
                    let d = lo[(y1 * 4 + x1) * 4 + ch] as f32;
                    let v = (a + (b - a) * fx) + ((c + (d - c) * fx) - (a + (b - a) * fx)) * fy;
                    bil[(y * 8 + x) * 4 + ch] = v as u8;
                }
            }
        }
        let mad: u64 = out
            .iter()
            .zip(bil.iter())
            .map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs() as u64)
            .sum();
        assert!(
            mad > 0,
            "FSR1 must differ from plain bilinear (edge-aware + sharpen)"
        );
    }

    #[test]
    fn fsr1_reference_e2e_real_mesh() {
        let cam = FrameCamera {
            eye: [-28.0, 72.0, -28.0],
            target: [8.0, 16.0, 8.0],
            up: [0.0, 1.0, 0.0],
            fov_y_deg: 60.0,
            aspect: 192.0 / 144.0,
            near: 0.1,
            far: 500.0,
        };
        let vp = build_view_proj(&cam);
        let chunks = demo_chunks();
        let fr = render_reference_fsr(&chunks, &vp, 192, 144, 384, 288, 0.2);
        assert!(fr.quads > 0);
        assert!(fr.covered_px > 0, "upscaled frame must show terrain");
        assert!(fr.covered_px < 384 * 288, "sky must remain");
        assert!(fr.avg_lum > 0.01);
    }

    // ---------- Phase C: Hi-Z 参照ミラーの実検証 ----------

    #[test]
    fn hiz_downsample_takes_cell_max() {
        // 4x4 → 2x2: 各セルの max が取られること (手計算一致)
        let depth = [
            0.1, 0.5, 0.7, 0.2, //
            0.9, 0.3, 0.4, 0.6, //
            0.2, 0.8, 0.3, 0.9, //
            0.4, 0.6, 0.1, 0.5,
        ];
        let hiz = hiz_downsample_reference(&depth, 4, 4, 2);
        assert_eq!(hiz, vec![0.9, 0.7, 0.8, 0.9]);
    }

    #[test]
    fn hiz_back_box_fully_occluded_by_wall_depth() {
        // 実メッシュ深度 (全面占有の壁) で Hi-Z 構築 → 手前 AABB は coverage>0、
        // 完全に背後の箱は coverage==0 (実遮蔽の実証、決定的)。
        let chunks = wall_chunks(); // chunk (0,0): x 0..16, y 0..64, z 0..16 の実壁
        let cam = FrameCamera {
            eye: [8.0, 24.0, -30.0],
            target: [8.0, 24.0, 16.0],
            up: [0.0, 1.0, 0.0],
            fov_y_deg: 60.0,
            aspect: 640.0 / 480.0,
            near: 0.1,
            far: 500.0,
        };
        let vp = build_view_proj(&cam);
        let frame = render_reference(&chunks, &vp, 640, 480);
        assert!(
            frame.covered_px > 0,
            "wall must cover pixels for a meaningful test"
        );
        let dim = crate::frame_hiz::HIZ_DIM;
        let hiz = hiz_downsample_reference(&frame.depth, 640, 480, dim);
        let front = crate::frame_hiz::aabb_from_mesh(&chunks[0].0, chunks[0].1);
        // 壁の完全に背後 (z 32..46) の箱
        let back = crate::occlusion_query::QueryBox {
            min: [2.0, 0.0, 32.0],
            max: [14.0, 40.0, 46.0],
        };
        let cov = hiz_test_reference(&hiz, dim, &[front, back], &vp);
        assert!(cov[0] > 0, "front wall box must be visible: {cov:?}");
        assert_eq!(cov[1], 0, "box behind wall must be fully occluded: {cov:?}");
    }

    #[test]
    fn hiz_conservative_when_offscreen_or_behind_camera() {
        let cam = FrameCamera {
            eye: [0.0, 0.0, 0.0],
            target: [0.0, 0.0, 1.0],
            up: [0.0, 1.0, 0.0],
            fov_y_deg: 60.0,
            aspect: 1.0,
            near: 0.1,
            far: 100.0,
        };
        let vp = build_view_proj(&cam);
        let hiz = vec![0.5f32; 64 * 64]; // 全域に深度あるが関係しないはず
        let far_side = crate::occlusion_query::QueryBox {
            min: [1000.0, -1.0, 10.0],
            max: [1002.0, 1.0, 12.0],
        };
        let behind = crate::occlusion_query::QueryBox {
            min: [-1.0, -1.0, -20.0],
            max: [1.0, 1.0, -10.0],
        };
        let cov = hiz_test_reference(&hiz, 64, &[far_side, behind], &vp);
        assert_eq!(cov, vec![1, 1], "判定不能は保守的可視 (coverage=1)");
    }

    #[test]
    fn hiz_self_depth_does_not_self_cull() {
        // 箱自身から作った深度に対して自分をテスト → EPS で可視 (誤カリング防止)
        let cam = FrameCamera {
            eye: [8.0, 24.0, -30.0],
            target: [8.0, 24.0, 8.0],
            up: [0.0, 1.0, 0.0],
            fov_y_deg: 60.0,
            aspect: 1.0,
            near: 0.1,
            far: 500.0,
        };
        let vp = build_view_proj(&cam);
        let chunks = wall_chunks();
        let frame = render_reference(&chunks, &vp, 320, 240);
        let dim = 64u32;
        let hiz = hiz_downsample_reference(&frame.depth, 320, 240, dim);
        let this = crate::frame_hiz::aabb_from_mesh(&chunks[0].0, chunks[0].1);
        let cov = hiz_test_reference(&hiz, dim, &[this], &vp);
        assert!(cov[0] > 0, "self depth must not self-cull: {cov:?}");
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

    /// wave 21-1: SUN_DIR は normalize(0.6, 1.0, 0.3) の f32 演算評価と bit 一致し、
    /// 単位長であること。旧値 [0.4985076, 0.8308459, 0.2492538] は |v|≈1.00047 の
    /// 非単位ベクトルであり doc と乖離していた。
    #[test]
    fn sun_dir_is_f32_normalized_direction() {
        let s = 0.6f32 * 0.6 + 1.0 * 1.0 + 0.3f32 * 0.3;
        let n = s.sqrt();
        let want = [0.6f32 / n, 1.0 / n, 0.3 / n];
        for i in 0..3 {
            assert_eq!(
                SUN_DIR[i].to_bits(),
                want[i].to_bits(),
                "SUN_DIR[{i}] must be bit-equal to f32 normalize(0.6, 1, 0.3)"
            );
        }
        let len2 = SUN_DIR[0] * SUN_DIR[0] + SUN_DIR[1] * SUN_DIR[1] + SUN_DIR[2] * SUN_DIR[2];
        assert!(
            (len2 - 1.0).abs() <= 2.0 * f32::EPSILON,
            "SUN_DIR must be unit length: |v|^2={len2}"
        );
    }

    /// wave 21-2: WGSL fs_pull が SUN_DIR リテラル同一表記で Lambert を実装している
    /// こと (GPU/CPU 相互検証の成立条件)。除去・退行した場合に fail-loud。
    #[test]
    fn wgsl_fs_pull_implements_mirror_lambert() {
        const WGSL: &str = include_str!("../shaders/terrain_vertex_pull.wgsl");
        for lit in ["0.49827290", "0.83045477", "0.24913645"] {
            assert!(
                WGSL.contains(lit),
                "WGSL SUN_DIR literal {lit} must mirror frame_reference.rs"
            );
        }
        let body = WGSL
            .split("fn fs_pull")
            .nth(1)
            .expect("terrain_vertex_pull.wgsl must define fs_pull");
        for tok in ["dot(", "max(", "SUN_DIR", "in.normal", "0.35", "0.65"] {
            assert!(
                body.contains(tok),
                "fs_pull must compute Lambert shading ({tok} missing)"
            );
        }
    }

    /// wave 21-3: Lambert が面方位を実際に識別し、AO/band 係数との合成出力が
    /// WGSL/Rust 同一演算順の f32 厳密導出値と bit 一致すること。
    /// 期待値はモジュール固定演算順 (band * ao * li, ((band*0.6)*ao)*li, ...) から
    /// f32 エミュレーションで厳密導出 (直感値禁止: K-6 教訓)。
    #[test]
    fn shade_exact_bits_matching_wgsl_eval_order() {
        // shade(tex=4, light_ao=3, face): band=4/7, ao=1.0
        let want: [[u32; 3]; 6] = [
            [0x3ec52843, 0x3e6c96b8, 0x3dec96b8], // face0 +X (ndl = SUN_DIR.x)
            [0x3e4ccccd, 0x3df5c291, 0x3d75c291], // face1 -X 陰 (li=0.35)
            [0x3f022a15, 0x3e9c3280, 0x3e1c3280], // face2 +Y 天面 (最明)
            [0x3e4ccccd, 0x3df5c291, 0x3d75c291], // face3 -Y 陰
            [0x3e95c755, 0x3e33bc00, 0x3db3bc00], // face4 +Z (ndl = SUN_DIR.z)
            [0x3e4ccccd, 0x3df5c291, 0x3d75c291], // face5 -Z 陰
        ];
        for f in 0..6u32 {
            let got = shade(4, 3, f);
            for c in 0..3 {
                assert_eq!(
                    got[c].to_bits(),
                    want[f as usize][c],
                    "shade(4,3,{f})[{c}] bit mismatch"
                );
            }
        }
        // 面方位の明度順序: 天面 > +X > +Z > 陰面 (3 陰面は bit 同一)
        assert!(shade(4, 3, 2)[0] > shade(4, 3, 0)[0]);
        assert!(shade(4, 3, 0)[0] > shade(4, 3, 4)[0]);
        assert!(shade(4, 3, 4)[0] > shade(4, 3, 1)[0]);
        for f in [1u32, 3, 5] {
            assert_eq!(
                shade(4, 3, f)[0].to_bits(),
                want[1][0],
                "shadowed faces identical"
            );
        }
        // band=0 (tex%7==0) は全 face で厳密 +0.0 (AO/Lambert は band の乗数)
        for f in 0..6u32 {
            for c in 0..3 {
                assert_eq!(shade(7, 0, f)[c].to_bits(), 0, "band zero must be black");
            }
        }
    }
}

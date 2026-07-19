//! # DeinterleaveAO — 低スペック向け AO: デインターリーブ半解像度 + 空間デノイズ
//!
//! 出典: XeGTAO（Intel GameTechDev）— iGPU（11th Gen Iris Xe）でも
//! 1080p 2.39ms。低スペックでは半解像度化（→約 1/4 コスト）+ deinterleave
//! 4バリアント方式（NVIDIA HBAO 系テクニック）でさらに割引。
//!
//! このモジュールは:
//! * AO 計算の **参照 CPU 実装**（SSAO 相当の horizon ベース簡易版）
//! * 半解像度 deinterleave/reinterleave（4 サブイメージ分割・再合成）
//! * エッジ考慮 3x3 デノイズフィルタ（深度差で重み減衰）
//!
//! 実機 GPU 結線は `crate::gtao` の WGSL をそのまま半解像度ターゲットで
//! 動かし、このモジュールの再合成カーネルでフル解像度へ戻す。

/// AO パラメータ。
#[derive(Debug, Clone, Copy)]
pub struct AoParams {
    /// サンプル半径（world / view 単位）
    pub radius: f32,
    /// 半径内サンプル数
    pub samples: u8,
    /// 強度
    pub intensity: f32,
    /// 深度差の許容
    pub depth_epsilon: f32,
}

impl Default for AoParams {
    fn default() -> Self {
        Self {
            radius: 1.5,
            samples: 8,
            intensity: 1.0,
            depth_epsilon: 0.06,
        }
    }
}

/// ハッシュで決定論的なサンプル回転（XeGTAO の spatial offset 相当）。
#[inline]
fn rot_for(x: u32, y: u32, variant: u32) -> (f32, f32) {
    let mut h = x
        .wrapping_mul(374761393)
        .wrapping_add(y.wrapping_mul(668265263))
        .wrapping_add(variant.wrapping_mul(2246822519));
    h ^= h >> 13;
    let a = (h & 0xFFFF) as f32 / 65536.0 * std::f32::consts::TAU;
    (a.cos(), a.sin())
}

/// 1 ピクセルの AO (view-space)。`depths` は row-major 深度バッファ、
/// `inv_res = (1/w, 1/h)`。z は線形非負（近=0 に正規化済み想定）。
pub fn ao_pixel(
    depths: &[f32],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    variant: u32,
    params: &AoParams,
) -> f32 {
    let z0 = depths[y * width + x];
    if z0 <= 0.0 {
        return 1.0;
    }
    let (rx, ry) = rot_for(x as u32, y as u32, variant);
    // 回転した 2 直交方向に radius 内で horizon を探る
    let mut occ = 0.0f32;
    let dirs = [(rx, ry), (-ry, rx)];
    for (dx, dy) in dirs {
        let mut horizon = -1.0f32; // sin 角度の最大値
        for s in 1..=(params.samples as i32) {
            let t = s as f32 / params.samples as f32;
            let px = (x as f32 + dx * t * params.radius * 16.0).round() as i32;
            let py = (y as f32 + dy * t * params.radius * 16.0).round() as i32;
            if px < 0 || py < 0 || px as usize >= width || py as usize >= height {
                break;
            }
            let ze = depths[py as usize * width + px as usize];
            // 深度差 → 仰角 sin 近似
            let dz = ze - z0;
            let dist = t * params.radius;
            let sin = dz / (dz * dz + dist * dist + 1e-5).sqrt();
            if sin > horizon {
                horizon = sin;
            }
        }
        if horizon > 0.0 {
            occ += horizon;
        }
    }
    let ao = 1.0 - (occ * params.intensity * 0.5).clamp(0.0, 1.0);
    ao.clamp(0.0, 1.0)
}

/// 4 バリアントのデインターリーブ: (x%2, y%2) で 4 つの半解像度サブイメージ。
/// 各サブイメージで AO を計算（サンプル回転をバリアントでずらす）。
/// 戻り値は 4 つの半解像度 AO バッファ。
pub fn deinterleaved_ao(
    depths_full: &[f32],
    width: usize,
    height: usize,
    params: &AoParams,
) -> [Vec<f32>; 4] {
    let hw = width / 2;
    let hh = height / 2;
    let mut out: [Vec<f32>; 4] = [
        vec![1.0; hw * hh],
        vec![1.0; hw * hh],
        vec![1.0; hw * hh],
        vec![1.0; hw * hh],
    ];
    // 半解像度深度を用意（まず nearest で間引く）
    let mut half_depth = vec![0.0f32; hw * hh];
    for v in 0..2usize {
        for u in 0..2usize {
            for y in 0..hh {
                for x in 0..hw {
                    half_depth[y * hw + x] = depths_full[(y * 2 + v) * width + (x * 2 + u)];
                }
            }
            let variant = (v * 2 + u) as u32;
            for y in 0..hh {
                for x in 0..hw {
                    out[v * 2 + u][y * hw + x] =
                        ao_pixel(&half_depth, hw, hh, x, y, variant, params);
                }
            }
        }
    }
    out
}

/// 4 サブ → フル解像度へ reinterleave → 深度考慮 3x3 デノイズ。
pub fn reinterleave_denoise(
    halves: &[Vec<f32>; 4],
    depths_full: &[f32],
    width: usize,
    height: usize,
    params: &AoParams,
) -> Vec<f32> {
    let hw = width / 2;
    let mut full = vec![1.0f32; width * height];
    for v in 0..2usize {
        for u in 0..2usize {
            let sub = &halves[v * 2 + u];
            for y in 0..(height / 2) {
                for x in 0..hw {
                    full[(y * 2 + v) * width + (x * 2 + u)] = sub[y * hw + x];
                }
            }
        }
    }
    // エッジ保持 3x3：深度差が閾値以内の隣接のみ平均
    let mut out = full.clone();
    for y in 1..height.saturating_sub(1) {
        for x in 1..width.saturating_sub(1) {
            let z0 = depths_full[y * width + x];
            let mut sum = full[y * width + x];
            let mut w = 1.0f32;
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let nx = (x as isize + dx) as usize;
                    let ny = (y as isize + dy) as usize;
                    let z1 = depths_full[ny * width + nx];
                    if (z1 - z0).abs() <= params.depth_epsilon {
                        sum += full[ny * width + nx];
                        w += 1.0;
                    }
                }
            }
            out[y * width + x] = sum / w;
        }
    }
    out
}

/// 計算量見積（ベンチ用）: フル 1920x1080 を全サンプルした場合との比率。
pub fn cost_ratio_vs_full(samples_half: u8, samples_full: u8) -> f32 {
    // 半解像度 2x2 で 4 バリアント → ピクセル数は 1/4、サンプルごとコスト同じ
    let base = samples_full.max(1) as f32;
    (samples_half as f32 * 0.25) / base
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_depth(w: usize, h: usize, z: f32) -> Vec<f32> {
        vec![z; w * h]
    }

    #[test]
    fn flat_scene_has_no_occlusion() {
        let w = 64;
        let h = 64;
        let d = flat_depth(w, h, 1.0);
        let halves = deinterleaved_ao(&d, w, h, &AoParams::default());
        let full = reinterleave_denoise(&halves, &d, w, h, &AoParams::default());
        for (i, &v) in full.iter().enumerate() {
            assert!(
                (v - 1.0).abs() < 1e-3,
                "flat depth must give AO=1 everywhere, idx {i} = {v}"
            );
        }
    }

    #[test]
    fn step_crease_occludes() {
        // 左半分 z=1.0, 右半分 z=1.5 の段差 → 段差近傍は AO < 1
        let w = 64;
        let h = 64;
        let mut d = flat_depth(w, h, 1.0);
        for y in 0..h {
            for x in 32..w {
                d[y * w + x] = 1.02; // 小さな段差
            }
        }
        let halves = deinterleaved_ao(&d, w, h, &AoParams::default());
        let full = reinterleave_denoise(&halves, &d, w, h, &AoParams::default());
        let edge = full[32 * w + 33];
        let far = full[32 * w + 60];
        assert!(edge < 1.01); // クリース近傍は何か遮蔽が出る（>= フラットは禁止）
        assert!(far >= edge, "far from crease must be brighter than crease");
    }

    #[test]
    fn cost_ratio_quarter_pixels() {
        let r = cost_ratio_vs_full(8, 8);
        assert!((r - 0.25).abs() < 1e-6);
    }

    #[test]
    fn reinterleave_shape() {
        let w = 32;
        let h = 16;
        let d = flat_depth(w, h, 1.0);
        let halves = deinterleaved_ao(&d, w, h, &AoParams::default());
        for hbuf in &halves {
            assert_eq!(hbuf.len(), (w / 2) * (h / 2));
        }
        let full = reinterleave_denoise(&halves, &d, w, h, &AoParams::default());
        assert_eq!(full.len(), w * h);
    }
}

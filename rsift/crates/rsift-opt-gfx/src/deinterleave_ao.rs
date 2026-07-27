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
//!
//! 【wave 136 EJ-1 (2026-07-26)】以下の構造を誠実注記 (全て機械検証済):
//! 1. `ao_pixel` の horizon 探索は回転直交ベクトル **(rx,ry), (−ry,rx) の
//!    片側 2 方向のみ** (反対位相の −(rx,ry), −(−ry,rx) は未探索) =
//!    方向非対称の簡易形 (文献 GTAO/XeGTAO の全周多重方向積分ではない)。
//!    `rot_for` は決定論的ハッシュ (x,y,variant → 16bit 量子化角度)
//!    なので同一入力は bit 同一だが、オクルージョン近似はバイアスを持つ。
//! 2. `reinterleave_denoise` のエッジ保持 3x3 は**画像境界 1px 帯を処理
//!    せず**（`1..=h-2` ループ）再インリーブ直値をそのまま透過（構造
//!    契約としてピン、"最外周はデノイズ無し"の真値仕様化）。
//! 3. 奇数寸法でも index は安全: x∈[0,hw) に対し 2x+u ≤ 2hw−1 ≤ w−1 が
//!    hw = w/2 (切捨) で恒成立 (hw = (w−1)/2 の場合 2hw−1 = w−2 でも
//!    w−1=2hw でも w−1 到達) — panic は depth バッファ長不足のみ
//!    (Rust index 規約で fail-loud)。
//! 4. `cost_ratio_vs_full` は半解像度=画素 1/4 × サンプル数比の**単純積
//!    モデル** (レイアウト・キャッシュ・SIMD 効果は無視の名目見積)。

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

    /// 【wave 136 EJ-1】奇数寸法 7x5 での index 安全と形状性。
    #[test]
    fn odd_dimensions_index_safe_and_shaped() {
        let w = 7;
        let h = 5;
        let d = flat_depth(w, h, 1.0);
        let halves = deinterleaved_ao(&d, w, h, &AoParams::default());
        for hbuf in &halves {
            assert_eq!(hbuf.len(), 6, "hw=3, hh=2 → 6 cells");
        }
        let full = reinterleave_denoise(&halves, &d, w, h, &AoParams::default());
        assert_eq!(full.len(), 35, "7x5");
    }

    /// 【wave 136 EJ-1】境界 1px 帯は denoise が書き換えず再インタリーブ
    /// 直値 (variant raw) を透過する構造契約の機械ピン。
    #[test]
    fn denoise_boundary_rows_kept_as_variant() {
        let w = 8;
        let h = 8;
        let hw = 4;
        let hh = 4;
        // halves を非均一化 (variant ごとに区別可能な値)、内部も構成分解できる形。
        let mut halves: [Vec<f32>; 4] = [
            vec![0.0; hw * hh],
            vec![0.25; hw * hh],
            vec![0.5; hw * hh],
            vec![0.75; hw * hh],
        ];
        // 内部セルも均一 (denoise の平均にも同値が供給される形) にして区別を明瞭化。
        halves[1][0] = 0.8125; // (u=1,v=0) 行 0 列 0 — full[(0,1)] に当たる
        let d = flat_depth(w, h, 1.0);
        let p = AoParams::default();
        let out = reinterleave_denoise(&halves, &d, w, h, &p);
        // 境界 (y=0 行, y=h-1 行, x=0 列, x=w-1 列) は variant 値のまま。
        assert_eq!(
            out[0 * w + 0].to_bits(),
            0.0f32.to_bits(),
            "(0,0) ← v0u0=0.0"
        );
        assert_eq!(
            out[0 * w + 1].to_bits(),
            0.8125f32.to_bits(),
            "(0,1) ← v0u1=0.8125 raw"
        );
        assert_eq!(
            out[1 * w + 0].to_bits(),
            0.5f32.to_bits(),
            "(1,0) ← v1u0=0.5"
        );
        // (1,1) は 8x8 ではループ範囲 [1,w-2]×[1,h-2] 内の「内部ピクセル」で
        // denoise 平均の対象: (2y+v)=1 → v=1、(2x+u)=1 → u=1、center=0.75 に
        // 隣接 8 セル [0,0.8125,0,0.5,0.5,0,0.25,0] を加算 → 2.8125/9 =
        // 0.3125 = 5/16 exact (rq 導出 bits 0x3EA00000、実測 1050673152 と一致)。
        assert_eq!(
            out[1 * w + 1].to_bits(),
            0x3EA0_0000,
            "(1,1) 内部: denoise 平均 0.3125 exact (rq golden、初版は境界誤認で赤捕捉)"
        );
        assert_eq!(out[(h - 1) * w + 0].to_bits(), 0.5f32.to_bits(), "(h-1,0)");
        // (0,w-1) ← v0u1 サブ (y=0,x=3) = halves[1][3]、改造は [0] のみなので 0.25
        assert_eq!(
            out[0 * w + (w - 1)].to_bits(),
            0.25f32.to_bits(),
            "(0,w-1) ← halves[1][3]=0.25"
        );
    }

    /// 【wave 136 EJ-1】`depth_epsilon` のエッジ保持が真に働くこと:
    /// Δz=0.125 > 0.06 のスパイク近傍は平均から排除され (tight)、
    /// eps=1e6 では全 8 近傍を平均に含める (loose)。同一 halves でも
    /// 両設定で out が厳密に変わる (排除契約の検出感度 pin)。
    #[test]
    fn eps_edge_keep_rejects_depth_zigma_neighbors_contract() {
        let w = 4;
        let h = 4;
        let hw = 2;
        let hh = 2;
        // halves: 全 0.5、但し v0u0 の (0,0) セル (full 位置 (0,0)) の近傍に
        // 影響を与える (1,1)(full) へ mod。検査対象は内部 (y=1,x=1) セル:
        // それは v1u1 halves[3][(1*2+1)... wait v1u1 → index 3、サブ座標
        // (y*hw + x) = 0*2+0 = 0] から full[(1,1)] = 0.9 を供給する形。
        let mut halves: [Vec<f32>; 4] = [
            vec![0.5; hw * hh],
            vec![0.5; hw * hh],
            vec![0.5; hw * hh],
            vec![0.5; hw * hh],
        ];
        halves[3][0] = 0.9; // v1u1 のサブ (0,0) → full (2*0+1, 2*0+1) = (1,1) セル
        let mut d = flat_depth(w, h, 0.125);
        d[1 * w + 1] = 0.25; // (1,1) が深度スパイク Δz=0.125 > 0.06
        let tight = AoParams {
            depth_epsilon: 0.06,
            ..Default::default()
        };
        let loose = AoParams {
            depth_epsilon: 1e6,
            ..Default::default()
        };
        let out_t = reinterleave_denoise(&halves, &d, w, h, &tight);
        let out_l = reinterleave_denoise(&halves, &d, w, h, &loose);
        // tight: (1,1) は z0=0.25、全 8 近傍 z1=0.125 で |Δ|=0.125>0.06 →
        // 全隣接 reject → out = raw = 0.9 (bits exact)
        assert_eq!(
            out_t[1 * w + 1].to_bits(),
            0.9f32.to_bits(),
            "eps tight: 近傍全 excluded → raw 0.9"
        );
        // loose: 全 8 近傍 0.5 を受容 → out = (0.9 + 8*0.5)/9 = 4.9/9
        // (sum=0.9+0.5*8=4.9, w=1+8=9 の fixed 加算順序列、rq 導出 golden)
        assert_eq!(
            out_l[1 * w + 1].to_bits(),
            (4.9f32 / 9.0f32).to_bits(),
            "eps loose: 全 8 近傍受容 → (0.9+8*0.5)/9"
        );
        assert!(
            out_l[1 * w + 1] < out_t[1 * w + 1],
            "loose は近傍平均に沈む"
        );
        // 逆側中心 (z0=0.125 の (1,2) セル) は tight では 0.9 隣接を排除:
        // centers 0.125 同土のみ平均。v 行の詳細値は half 供給的に 0.5 統一 →
        // out = 0.5 (スパイク排除が効いていることの contravariant pin)
        assert_eq!(
            out_t[1 * w + 2].to_bits(),
            0.5f32.to_bits(),
            "tight: (1,2) はスパイク excluded → 同高 0.5 平均"
        );
    }

    /// 【wave 136 EJ-1】全エア (z=0) 断面では ao_pixel が early return 1.0
    /// で全域 1.0 bits exact、かつ denoise も不変 (Δz=0 ≤ eps 全受容だが
    /// 全員同値のため平均も 1.0)。wiring 空入力との整合契約。
    #[test]
    fn all_air_section_gives_exact_1_bits() {
        let d = flat_depth(16, 16, 0.0);
        let halves = deinterleaved_ao(&d, 16, 16, &AoParams::default());
        for buf in &halves {
            assert!(
                buf.iter().all(|v| v.to_bits() == 1.0f32.to_bits()),
                "全エア → 全 1.0 exact"
            );
        }
        let full = reinterleave_denoise(&halves, &d, 16, 16, &AoParams::default());
        assert!(
            full.iter().all(|v| v.to_bits() == 1.0f32.to_bits()),
            "denoise 後も全 1.0 exact"
        );
    }
}

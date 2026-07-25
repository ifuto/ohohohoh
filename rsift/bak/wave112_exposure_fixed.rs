//! Auto-exposure (eye adaptation) via a log-luminance histogram for
//! `rsift-opt-gfx`.
//!
//! Real logic (no stubs): build a 256-bin histogram of scene log-luminance,
//! compute a weighted average exposure (discarding the brightest/darkest
//! percentiles via `low_percent`/`high_percent`), and temporally adapt the
//! exposure toward that target.
//!
//! ## メータリング規約 (監査 2026-07-25 DL-1、Epic 一次情報照合済)
//! Unreal の Auto Exposure Histogram 同型: **log 領域**の輝度ヒストグラムの
//! 加重平均 (= 幾何平均輝度) を計量し、その逆数を目標露出とする
//! (Epic「Auto Exposure in Unreal Engine」: Histogram は log 輝度の
//! ヒストグラムを解析して平均輝度を決める)。旧実装は線形領域の算術平均の
//! 逆数 1/E[L] で、これは Jensen 不等式 E[ln L] ≤ ln E[L] により
//! **常に Unreal 式以下の値** (= 実シーンで暗め、2 段実測シーンで比 0.837、
//! 逆数比は新/旧 = 1.1948) にずれていた — 「same metering used by Unreal」
//! という旧ヘッダ記載は数学的に虚偽だったため、log 領域計量に修正した
//! (定数シーンでは両式が厳密一致するため挙動不変、変動シーンでのみ
//! 新 = 旧 × 幾何/算術比で明るくなる)。Unreal との残差異 (誠実注記):
//! bin 数 (Unreal 64 / 本実装 256) と較正規約 (Unreal は平均を 18% 中間グレー
//! 扱い K 較正 / 本実装は 1/幾何平均輝度の独自規約) — どちらも意図的な
//! プロジェクト規約として維持する。

use std::ops::{Add, Mul, Sub};

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
}
impl Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f32) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}
impl Vec4 {
    pub fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }
}
impl Add for Vec4 {
    type Output = Vec4;
    fn add(self, o: Vec4) -> Vec4 {
        Vec4::new(self.x + o.x, self.y + o.y, self.z + o.z, self.w + o.w)
    }
}
impl Sub for Vec4 {
    type Output = Vec4;
    fn sub(self, o: Vec4) -> Vec4 {
        Vec4::new(self.x - o.x, self.y - o.y, self.z - o.z, self.w - o.w)
    }
}
impl Mul<f32> for Vec4 {
    type Output = Vec4;
    fn mul(self, s: f32) -> Vec4 {
        Vec4::new(self.x * s, self.y * s, self.z * s, self.w * s)
    }
}

pub const HIST_BINS: usize = 256;

/// Build a log-luminance histogram from linear HDR colours.
///
/// ## 契約 (監査 2026-07-25 DL-2)
/// * 輝度は Rec.709 (0.2126/0.7152/0.0722) の線形加重、下限 1e-4 に張付け。
/// * `t = (ln l - log_min) / range` を 0.9999 に clamp し `* 256` で bin 化。
/// * **NaN/inf 入力は決定的**: Rust `f32::max` は非 NaN 側を返すため、
///   NaN 色は l=1e-4 (bin 0 側)、+inf は bin 255、-inf も l=1e-4 (bin 0)。
/// * **退化範囲は静寂崩壊せず静寂確定**: min_lum ≤ 0 かつ max_lum ≤ 0 では
///   log_max が NaN となり `range` は 1e-6 床に張付き、全サンプルが bin 255
///   に確定する (panic も Err も出さない、adversarial 耐性は pin で固定)。
pub fn build_histogram(colors: &[Vec3], min_lum: f32, max_lum: f32) -> [u32; HIST_BINS] {
    let mut hist = [0u32; HIST_BINS];
    let log_min = min_lum.max(1e-4).ln();
    let log_max = max_lum.max(min_lum * 1.001).ln();
    let range = (log_max - log_min).max(1e-6);
    for c in colors {
        let l = (0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z).max(1e-4);
        let t = (l.ln() - log_min) / range;
        let bin = (t.clamp(0.0, 0.9999) * HIST_BINS as f32) as usize;
        hist[bin] += 1;
    }
    hist
}

/// Compute target exposure (Unreal Histogram 同型の log 領域加重平均の逆数)
/// discarding extreme percentiles.
/// `low_percent`/`high_percent` are percentile thresholds in [0,100]; only the
/// central mass whose cumulative sample position lies within `[lo, hi]` is used.
///
/// ## 契約 (監査 2026-07-25 DL-1/DL-4)
/// * 計量は **log 領域**: `avg_log = Σ v·(log_min + t·range) / included`、
///   露出 = `exp(-avg_log)` を 0.05..=20 に clamp (旧線形 1/E[L] から DL-1
///   で修正、定数シーンは不変・変動シーンは幾何平均側へ)。
/// * 分位境界は **切捨て u32** (`(total * pct / 100) as u32`) で、包含判定は
///   bin 累積区間の整数中点 `(cstart+cend)/2` に依存する (bin 粒度依存)。
/// * `low >= hi`、空ヒスト、全 bin 除外のいずれでも 1.0 を返す。
pub fn target_exposure(
    hist: &[u32; HIST_BINS],
    min_lum: f32,
    max_lum: f32,
    low_percent: f32,
    high_percent: f32,
) -> f32 {
    let total: u32 = hist.iter().sum();
    if total == 0 {
        return 1.0;
    }
    let lo = (total as f32 * low_percent / 100.0) as u32;
    let hi = (total as f32 * high_percent / 100.0) as u32;
    let log_min = min_lum.max(1e-4).ln();
    let log_max = max_lum.max(min_lum * 1.001).ln();
    let range = (log_max - log_min).max(1e-6);
    let mut cum = 0u32;
    let mut log_weighted = 0.0f32;
    let mut included = 0u32;
    for (b, &v) in hist.iter().enumerate() {
        let cstart = cum;
        let cend = cum + v;
        cum = cend;
        let center = (cstart + cend) / 2;
        if center >= lo && center <= hi {
            let t = (b as f32 + 0.5) / HIST_BINS as f32;
            log_weighted += (log_min + t * range) * v as f32;
            included += v;
        }
    }
    if included == 0 {
        return 1.0;
    }
    let avg_log = log_weighted / included as f32;
    (-avg_log).exp().clamp(0.05, 20.0)
}

/// Exponential (smooth) temporal adaptation toward the target.
///
/// ## 契約 (監査 2026-07-25 DL-3)
/// * `k = 1 - exp(-speed·dt)` は 微分方程式 de/dt = speed·(target − e) の
///   **厳密な離散解** (e_new = target + (prev−target)·exp(−speed·dt) と一致) で、
///   単純 lerp ではない。speed=0 なら恒等。
/// * speed < 0 または dt < 0 は k < 0 の**反適応** (target から遠ざかる) で、
///   結果は 0.05 側 clamp になりうる (呼出側契約: 非負のみ渡す)。
/// * NaN は **伝播する** (0.05 への静寂崩落ではない): 出力 NaN のまま。
pub fn adapt(prev: f32, target: f32, speed: f32, dt: f32) -> f32 {
    let k = 1.0 - (-speed * dt).exp();
    (prev + (target - prev) * k).clamp(0.05, 20.0)
}

pub fn wgsl_source() -> &'static str {
    EXPOSURE_WGSL
}

pub const EXPOSURE_WGSL: &str = include_str!("../shaders/exposure.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn constant_scene_gives_stable_exposure() {
        let colors: Vec<Vec3> = (0..1000).map(|_| Vec3::new(0.5, 0.5, 0.5)).collect();
        let h = build_histogram(&colors, 0.01, 100.0);
        let e = target_exposure(&h, 0.01, 100.0, 10.0, 90.0);
        // avg lum ~0.5 -> exposure ~ 1/0.5 = 2.0
        assert!((e - 2.0).abs() < 0.2, "exposure = {}", e);
    }
    #[test]
    fn adaptation_moves_toward_target() {
        // Scene of luminance 0.25 -> target exposure ~ 1/0.25 = 4.0.
        let t = target_exposure(
            &build_histogram(&[Vec3::new(0.25, 0.25, 0.25); 500], 0.01, 100.0),
            0.01,
            100.0,
            10.0,
            90.0,
        );
        assert!((t - 4.0).abs() < 0.2, "target = {}", t);
        let a = adapt(1.0, t, 4.0, 0.1);
        assert!(a > 1.0 && a < t + 1e-3, "adapted = {}", a);
    }
    #[test]
    fn adaptation_converges() {
        let mut e = 1.0;
        for _ in 0..60 {
            e = adapt(e, 3.0, 4.0, 1.0 / 60.0);
        }
        assert!((e - 3.0).abs() < 0.1, "converged to {}", e);
    }

    // ------------------------------------------------------------ 監査 2026-07-25 DL追加分

    /// DL-1: 2 段シーンの bins と log 領域計量の厳密値 pin。
    /// Python 独立シム確定値: bins=(170→900, 230→100)、新=8.543594451、
    /// 旧 (線形 1/E[L])=7.150503027、新/旧=1.194824 (Jensen 方向 新>旧)。
    #[test]
    fn two_level_scene_matches_geometric_mean_exactly() {
        let mut colors = vec![Vec3::new(0.1, 0.1, 0.1); 900];
        colors.extend(vec![Vec3::new(0.5, 0.5, 0.5); 100]);
        let h = build_histogram(&colors, 1e-3, 1.0);
        assert_eq!(h.iter().sum::<u32>(), 1000);
        assert_eq!(h[170], 900, "lum 0.1 帯");
        assert_eq!(h[230], 100, "lum 0.5 帯");
        let e = target_exposure(&h, 1e-3, 1.0, 0.0, 100.0);
        assert!(
            (e - 8.5435945).abs() < 0.01,
            "幾何平均計量の厳密値 (旧線形式は 7.1505030 で 1.39 乖離 = 逆戻し検出域内): {e}"
        );
    }

    /// DL-2: NaN/inf の決定的 bin 契約 pin (Python 機械検算と一致)。
    #[test]
    fn nan_and_inf_inputs_land_in_deterministic_bins() {
        let h = build_histogram(&[Vec3::new(f32::NAN, 0.0, 0.0)], 0.01, 1.0);
        assert_eq!(h[0], 1, "NaN 色は l=1e-4 で bin 0 (f32::max 非 NaN 優先)");
        assert_eq!(h.iter().sum::<u32>(), 1);
        let h = build_histogram(&[Vec3::new(f32::INFINITY, 0.0, 0.0)], 0.01, 1.0);
        assert_eq!(h[255], 1, "+inf は bin 255");
        let h = build_histogram(&[Vec3::new(f32::NEG_INFINITY, 0.0, 0.0)], 0.01, 1.0);
        assert_eq!(h[0], 1, "-inf も l=1e-4 で bin 0");
    }

    /// DL-2: 退化範囲 (min≤0 かつ max≤0) の静寂確定 pin (panic ではない)。
    #[test]
    fn degenerate_range_collapses_deterministically() {
        let colors = vec![Vec3::new(1.0, 1.0, 1.0); 3];
        let h = build_histogram(&colors, -2.0, -1.0);
        assert_eq!(h[255], 3, "range 床張付きで全サンプル bin 255");
        let e = target_exposure(&h, -2.0, -1.0, 0.0, 100.0);
        assert_eq!(e, 20.0, "exp(9.21…)≈1e4 → clamp 20 に確定");
    }

    /// DL-3: adapt の厳密値・契約 pin (Python 機械検算: 1.659359908)。
    #[test]
    fn adapt_exactness_overflow_and_nan_contract() {
        let a = adapt(1.0, 3.0, 4.0, 0.1);
        assert!((a - 1.6593599).abs() < 1e-5, "厳密離散解: {a}");
        // speed=0 は恒等 (k=0)。
        assert_eq!(adapt(5.0, 3.0, 0.0, 0.1), 5.0);
        // 負 speed は反適応 → 0.05 clamp (1+2·(1-e^{0.4})=0.0163…)。
        assert_eq!(adapt(1.0, 3.0, -4.0, 0.1), 0.05);
        // NaN は伝播 (0.05 静寂崩落ではない fail-visible 契約)。
        assert!(adapt(f32::NAN, 1.0, 4.0, 0.1).is_nan());
        assert!(adapt(1.0, f32::NAN, 4.0, 0.1).is_nan());
    }

    /// DL-4: 分位境界契約 pin (中点整数包含・low>hi/空ヒスト → 1.0)。
    #[test]
    fn percentile_boundary_contract() {
        let colors: Vec<Vec3> = (0..1000).map(|_| Vec3::new(0.5, 0.5, 0.5)).collect();
        let h = build_histogram(&colors, 0.01, 100.0);
        // 単一 bin (total=1000, center=500): [500,500] は包含、low=50/high=49 は除外。
        let inclusive = target_exposure(&h, 0.01, 100.0, 50.0, 50.0);
        assert!((inclusive - 1.0).abs() > 0.5, "50/50 境界は包含する");
        let exclusive = target_exposure(&h, 0.01, 100.0, 50.0, 49.0);
        assert_eq!(exclusive, 1.0, "low>hi → 全除外 → 1.0");
        let empty: [u32; HIST_BINS] = [0; HIST_BINS];
        assert_eq!(target_exposure(&empty, 0.01, 100.0, 0.0, 100.0), 1.0);
    }

    /// DL-1: 定数シーンの計量不変性 (旧線形式とも厳密一致) を厳密 pin。
    /// 両式とも 2.0169146 / 3.9954206 (bin-center 量子化込み) に確定する。
    #[test]
    fn constant_scene_metering_is_invariant_under_dl1() {
        let h = build_histogram(&[Vec3::new(0.5, 0.5, 0.5); 1000], 0.01, 100.0);
        let e = target_exposure(&h, 0.01, 100.0, 10.0, 90.0);
        assert!((e - 2.0169146).abs() < 1e-3, "L=0.5 幾何=算術: {e}");
        let h = build_histogram(&[Vec3::new(0.25, 0.25, 0.25); 500], 0.01, 100.0);
        let e = target_exposure(&h, 0.01, 100.0, 10.0, 90.0);
        assert!((e - 3.9954206).abs() < 1e-3, "L=0.25 幾何=算術: {e}");
    }
}

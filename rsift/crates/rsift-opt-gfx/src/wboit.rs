//! Weighted Blended Order-Independent Transparency (McGuire & Bavoil 2013).
//!
//! Renders transparent surfaces in a single geometry pass with no sorting.
//! Each fragment blends `(color.rgb * color.a * weight, color.a * weight)`; a
//! final resolve divides by the summed alpha. A depth-based weight biases toward
//! nearer surfaces, keeping smoke/glass plausible while avoiding sort stalls —
//! a good fit when transparent foliage/water is the bottleneck on low-spec GPUs.
//!
//! ## wave 68 BR-1: McGuire 原論文との差分 (誠実な明記)
//! 本実装は原論文の**簡約単一ターゲット形**である。原論文 (McGuire & Bavoil
//! 2013, "Weighted Blended Order-Independent Transparency", JCGT) は解決に
//! 2 つのターゲットを使う: 加重色 accum (rgb·a·w, a·w) と**別ターゲットの
//! revealage `Π(1 − a_i)`** (乗算ブレンド)。最終合成は
//! `C = (accum.rgb / accum.a)·(1 − reveal) + C_opaque·reveal`。
//! 本実装は revealage を持たず、`resolve` の返り alpha は正規化されていない
//! **Σ(a_i·w_i)** — 近接フラグメントが重なると 1.0 を超える
//! (例: 深度 ≤ near の a=0.5 フラグメント 3 枚で厳密 1.5)。
//! 消費側はこれを「正規化された光学 alpha」ではなく非負の合成度として扱う
//! こと (現在の配線は実生成物として非消費: full_graph_wiring :1568 で
//! 計算→破棄の状態連鎖)。本格導入時は revealage 第二ターゲットを実装する
//! こと — 挙動変更は実機 GPU 検証なしに行わない方針のため本 wave は
//! ドキュメント側を真実に合わせた (WGSL ミラー wboit.wgsl も同簡約形で
//! 2 連鎖一致)。

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec4 {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Vec4 {
    pub fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Wboit {
    /// `near` depth used to normalize the depth weight.
    pub near: f32,
    /// Higher `k` => nearer fragments dominate more strongly.
    pub k: f32,
}
impl Default for Wboit {
    fn default() -> Self {
        Self { near: 0.1, k: 8.0 }
    }
}
impl Wboit {
    pub fn new() -> Self {
        Self::default()
    }

    /// Per-fragment weight: nearer (smaller depth) => larger weight.
    ///
    /// **契約 (wave 68 BR-2)**: depth は有限値必須。NaN を渡すと Rust
    /// `f32::max(NaN, 0.0)` が 0.0 を返す仕様のため、**深度不明が静寂に
    /// 最近接 weight 1.0 へ化ける** (= 最前面扱いの誤合成) ハザードがあった。
    /// NaN=観測欠測は拒否 の哲学通り入口で fail-loud に遮断する
    /// (WGSL の max は片側 NaN で不定値返却が規格上許容されるため、
    /// GPU/CPU のどちらでも NaN depth を決定的に扱う経路は持てない)。
    pub fn weight(&self, depth: f32) -> f32 {
        assert!(
            depth.is_finite(),
            "wboit weight 契約違反: depth={depth} 非有限 (NaN depth の静寂な最近接化を遮断)"
        );
        let d = (depth - self.near).max(0.0);
        1.0 / (1.0 + d * self.k)
    }

    /// Accumulate one fragment into the running sums. `sum` holds
    /// `(r*a*w, g*a*w, b*a*w, a*w)` and `depth` is the fragment's view depth.
    /// color の各成分も有限必須 (weight と同契約 — NaN 色の累積汚染を遮断)。
    pub fn accumulate(&self, sum: &mut Vec4, color: Vec4, depth: f32) {
        assert!(
            color.r.is_finite()
                && color.g.is_finite()
                && color.b.is_finite()
                && color.a.is_finite(),
            "wboit accumulate 契約違反: 非有限な色成分 {color:?}"
        );
        let w = self.weight(depth) * color.a;
        sum.r += color.r * w;
        sum.g += color.g * w;
        sum.b += color.b * w;
        sum.a += w;
    }

    /// Resolve the accumulated buffer to a final color (alpha-premultiplied).
    pub fn resolve(&self, sum: Vec4) -> Vec4 {
        if sum.a <= 1e-5 {
            return Vec4::default();
        }
        Vec4 {
            r: sum.r / sum.a,
            g: sum.g / sum.a,
            b: sum.b / sum.a,
            a: sum.a,
        }
    }

    pub fn wgsl_source(&self) -> &'static str {
        WBOIT_WGSL
    }
}

pub const WBOIT_WGSL: &str = include_str!("../shaders/wboit.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nearer_has_higher_weight() {
        let w = Wboit::new();
        assert!(w.weight(0.1) > w.weight(5.0));
    }
    #[test]
    fn front_biased_result() {
        let w = Wboit::new();
        let mut sum = Vec4::default();
        // far red, near blue
        w.accumulate(&mut sum, Vec4::new(1.0, 0.0, 0.0, 0.5), 5.0);
        w.accumulate(&mut sum, Vec4::new(0.0, 0.0, 1.0, 0.5), 0.1);
        let r = w.resolve(sum);
        // blue (near) should dominate red
        assert!(r.b > r.r, "near color should dominate: {:?}", r);
    }
    #[test]
    fn empty_resolves_transparent() {
        let w = Wboit::new();
        let r = w.resolve(Vec4::default());
        assert_eq!(r.a, 0.0);
    }
    #[test]
    fn single_fragment_normalizes_to_itself() {
        let w = Wboit::new();
        let mut sum = Vec4::default();
        w.accumulate(&mut sum, Vec4::new(0.2, 0.4, 0.6, 0.8), 1.0);
        let r = w.resolve(sum);
        // resolve normalizes rgb by sum.a; with one fragment a = w, rgb = color*w/w = color
        // (bit 保証ではない: (x*s)/s は浮動少数では恒等とは限らないため許容差)
        assert!((r.r - 0.2).abs() < 1e-5);
        assert!((r.g - 0.4).abs() < 1e-5);
        assert!((r.b - 0.6).abs() < 1e-5);
    }

    /// wave 68 BR-3: weight の厳密ビット固定 (f32 エミュレーション独立導出)。
    /// depth ≤ near は d=0 クランプで厳密 1.0 (最前面優位の不変条件)。
    /// weight(1.1)=1/9、weight(2.1)=1/17 (near=0.1, k=8.0)。
    #[test]
    fn weight_exact_bits_canonical() {
        let w = Wboit::new();
        assert_eq!(w.weight(0.1).to_bits(), 0x3f800000, "at near → 1.0");
        assert_eq!(
            w.weight(0.05).to_bits(),
            0x3f800000,
            "below near clamps→1.0"
        );
        assert_eq!(w.weight(1.1).to_bits(), 0x3de38e39, "1/9");
        assert_eq!(w.weight(2.1).to_bits(), 0x3d70f0f1, "1/17");
    }

    /// wave 68 BR-3: accumulate → resolve 全経路の厳密ビット固定
    /// (far red + near blue、front-bias 比を定量: 青 0.83 対 赤 0.17)。
    #[test]
    fn accumulate_resolve_exact_bits_canonical() {
        let w = Wboit::new();
        let mut sum = Vec4::default();
        w.accumulate(&mut sum, Vec4::new(1.0, 0.0, 0.0, 0.5), 5.0);
        w.accumulate(&mut sum, Vec4::new(0.0, 0.0, 1.0, 0.5), 1.0);
        assert_eq!(
            [
                sum.r.to_bits(),
                sum.g.to_bits(),
                sum.b.to_bits(),
                sum.a.to_bits()
            ],
            [0x3c4bc7f6, 0x00000000, 0x3d79c190, 0x3d9659c7],
            "accumulation bit drift"
        );
        let r = w.resolve(sum);
        assert_eq!(
            [r.r.to_bits(), r.g.to_bits(), r.b.to_bits()],
            [0x3e2d7cd3, 0x00000000, 0x3f54a0cb],
            "resolve bit drift: {r:?}"
        );
        assert!(r.b > r.r, "near must dominate");
    }

    /// wave 68 BR-1: resolve 返り alpha は正規化されない Σ(a·w) であること
    /// (1.0 を超え得る簡約形の動作) を決定的に固定する回帰ピン。
    /// depth ≤ near・a=0.5 の 3 フラグメントで厳密 1.5。
    #[test]
    fn resolve_alpha_is_unnormalized_sum_documented_behavior() {
        let w = Wboit::new();
        let mut sum = Vec4::default();
        for _ in 0..3 {
            w.accumulate(&mut sum, Vec4::new(0.8, 0.8, 0.8, 0.5), 0.1);
        }
        let r = w.resolve(sum);
        assert_eq!(sum.a.to_bits(), 0x3fc00000, "Σ(a·w) = 1.5 exactly");
        assert_eq!(
            r.a.to_bits(),
            0x3fc00000,
            "resolve alpha is the unnormalized sum (doc BR-1) — \
             McGuire revealage Π(1-a) ではない簡約形である"
        );
        // 正規化 rgb は一括加重平均で保存される (同色なので色値そのまま)
        assert_eq!(r.r.to_bits(), 0x3f4ccccd);
    }

    /// wave 68 BR-2: NaN depth は Rust f32::max で 0.0 (最近接 weight 1.0) に
    /// 静寂化するハザードを入口 assert で遮断 — fail-loud 契約の回帰ピン。
    #[test]
    #[should_panic(expected = "wboit weight 契約違反")]
    fn nan_depth_is_rejected() {
        let w = Wboit::new();
        let _ = w.weight(f32::NAN);
    }

    /// wave 68 BR-2: 非有限色成分の累積混入を入口 assert で遮断。
    #[test]
    #[should_panic(expected = "wboit accumulate 契約違反")]
    fn nan_color_is_rejected() {
        let w = Wboit::new();
        let mut sum = Vec4::default();
        w.accumulate(&mut sum, Vec4::new(f32::NAN, 0.0, 0.0, 0.5), 1.0);
    }
}

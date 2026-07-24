//! FXAA — Fast Approximate Anti-Aliasing (Timothy Lottes). Post-process pass
//! that smooths jaggies in the final color buffer using only luma gradients.
//!
//! Cheap (one full-screen pass, no depth/geometry needed), so it is a good
//! default on low-spec GPUs where MSAA/TAA are too expensive. The Rust side
//! here is a faithful CPU reference used by the tests; the WGSL is the GPU pass.

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec3 {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}
impl Vec3 {
    pub fn new(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b }
    }
}
impl std::ops::Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.r + o.r, self.g + o.g, self.b + o.b)
    }
}
impl std::ops::Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f32) -> Vec3 {
        Vec3::new(self.r * s, self.g * s, self.b * s)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Fxaa {
    /// Absolute contrast floor (JND). Below this, no AA is applied.
    /// **CPU reference (この構造体の [`Self::shade`]) にのみ効く**。
    /// GPU pass (fxaa.wgsl) は 1.0/256.0 をハードコードしており、本フィールド
    /// の変更は WGSL へ伝播しない (uniform は invRes のみのため)。
    /// 既定値の一致は wgsl_threshold_constants_match_rust_default_bits が
    /// bit 単位で機械担保する。
    pub contrast_base: f32,
    /// Relative contrast threshold as a fraction of local max luma.
    /// 上記と同じく CPU reference のみ有効 (WGSL は 0.166667 固定)。
    pub relative_threshold: f32,
}
impl Default for Fxaa {
    fn default() -> Self {
        Self {
            contrast_base: 1.0 / 256.0,
            relative_threshold: 0.166667,
        }
    }
}
impl Fxaa {
    pub fn new() -> Self {
        Self::default()
    }

    /// Perceptual luma (BT.601-ish, green-weighted).
    pub fn luma(c: Vec3) -> f32 {
        0.299 * c.r + 0.587 * c.g + 0.114 * c.b
    }

    /// AA a single pixel. `n/s/e/w` are the 4 orthogonal neighbors; `center` is
    /// the pixel itself. Returns a blended color that reduces the local edge.
    ///
    /// 契約 (2026-07-24 CE で固定):
    /// - `contrast < threshold` なら入力 `center` を**そのまま**返す
    ///   (コピーで bit 完全一致)。
    /// - 非有限 (NaN/±∞) 画素の扱いは**未規定**: f32::min/max は NaN を
    ///   脱落させて他値で進み、center 側の NaN は出力へ伝播する
    ///   (ポストプロセス画素の欠測で全画面を落とさない方針)。
    ///   WGSL 側の min/max(NaN) は spec 上 indeterminate で、
    ///   Rust/WGSL 間の NaN 一致は保証対象外。
    /// - n/s のラベルは理念方向 (WGSL テクスチャ座標は +y が下向きで
    ///   ラベルが視覚と逆になるが、gx/gy を abs 比較のみに使うため
    ///   符号反転は振る舞いに無影響 — 誤記ではなく等価)。
    pub fn shade(
        &self,
        center: Vec3,
        n: Vec3,
        s: Vec3,
        e: Vec3,
        w: Vec3,
    ) -> Vec3 {
        let lc = Self::luma(center);
        let ln = Self::luma(n);
        let ls = Self::luma(s);
        let le = Self::luma(e);
        let lw = Self::luma(w);
        let lmin = lc.min(ln).min(ls).min(le).min(lw);
        let lmax = lc.max(ln).max(ls).max(le).max(lw);
        let contrast = lmax - lmin;
        let threshold = self.contrast_base.max(lmax * self.relative_threshold);
        if contrast < threshold {
            return center; // flat region — leave untouched
        }
        // Local gradient. Dominant axis selects which pair we blend with.
        let gx = lw - le; // horizontal edge -> blend with N/S
        let gy = ln - ls; // vertical edge   -> blend with E/W
        let avg = if gx.abs() > gy.abs() {
            (n + s) * 0.5
        } else {
            (e + w) * 0.5
        };
        let t = (contrast / (contrast + threshold)).clamp(0.0, 1.0) * 0.5;
        center * (1.0 - t) + avg * t
    }

    pub fn wgsl_source(&self) -> &'static str {
        FXAA_WGSL
    }
}

pub const FXAA_WGSL: &str = include_str!("../shaders/fxaa.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    fn solid(v: f32) -> Vec3 {
        Vec3::new(v, v, v)
    }
    #[test]
    fn flat_untouched() {
        let f = Fxaa::new();
        let out = f.shade(solid(0.5), solid(0.5), solid(0.5), solid(0.5), solid(0.5));
        assert!((out.r - 0.5).abs() < 1e-6);
    }
    #[test]
    fn edge_is_blended() {
        let f = Fxaa::new();
        // 水平エッジ (南側が暗い) + エッジ沿い(E/W)が中間調のケース。
        // FXAA は勾配方向(ここでは垂直)に対してエッジ「沿い」の E/W とブレンドする
        // ので、E/W が中間調なら中心は 1.0 から 0.5 方向へ動く。
        let center = solid(1.0);
        let n = solid(1.0);
        let s = solid(0.0);
        let e = solid(0.5);
        let w = solid(0.5);
        let out = f.shade(center, n, s, e, w);
        // Should move toward the along-edge average (0.5) but stay above it.
        assert!(out.r < 1.0);
        assert!(out.r > 0.5);
    }
    #[test]
    fn no_aa_on_low_contrast() {
        let f = Fxaa::new();
        let out = f.shade(solid(0.50), solid(0.51), solid(0.49), solid(0.50), solid(0.50));
        // contrast 0.02 < threshold (~0.083) => unchanged.
        assert!((out.r - 0.50).abs() < 1e-6);
    }

    fn bits(v: f32) -> u32 {
        v.to_bits()
    }

    /// CE (2026-07-24): 鉛直エッジ (南北に輝度差) は E/W とブレンド。
    /// 厳密値は検算スクリプト (f32 往復厳密化) で独立導出:
    /// luma(1,1,1) は f32 項和が 1.0 に丸まる (0x3F800000)、
    /// t = 0.5/(1+0.166667) = 3/7 級で 0x3EDB6DB3、
    /// out = 1 - 0.5t = 11/14 級で 0x3F492493 (全成分同一)。
    #[test]
    fn shade_vertical_edge_exact_bit_pin() {
        let f = Fxaa::new();
        let out = f.shade(solid(1.0), solid(1.0), solid(0.0), solid(0.5), solid(0.5));
        assert_eq!(bits(out.r), 0x3F492493);
        assert_eq!(bits(out.g), 0x3F492493);
        assert_eq!(bits(out.b), 0x3F492493);
        // 方向確認 (緩い既存テストの精密化): エッジ沿い平均 0.5 と 1.0 の間。
        assert!(out.r > 0.5 && out.r < 1.0);
    }

    /// CE: 水平エッジ (東西に輝度差) は N/S とブレンド。
    /// contrast/threshold は同一値のため t は鉛直ケースと同値 (0x3EDB6DB3)、
    /// out = 0.7(1-t) + 0.5t = 0x3F1D41D4。
    #[test]
    fn shade_horizontal_edge_exact_bit_pin() {
        let f = Fxaa::new();
        let out = f.shade(solid(0.7), solid(0.5), solid(0.5), solid(0.0), solid(1.0));
        assert_eq!(bits(out.r), 0x3F1D41D4);
        assert_eq!(bits(out.g), 0x3F1D41D4);
        assert_eq!(bits(out.b), 0x3F1D41D4);
        assert!(out.r > 0.5 && out.r < 0.7, "N/S 平均 0.5 方向へだけ動く");
    }

    /// CE: 非発動経路は `return center` で入力ビットの完全コピー
    /// (1e-6 近似ではなく to_bits 完全一致)。
    /// シナリオ設計 (検算で確定): luma(1,0,0) = f32(0.299) =
    /// luma(gray 0.299) で contrast ≡ 0 (pure red と等輝度グレー)。
    /// 初稿の (0.5,0.25,1.0) 設計は luma 0.41025 で contrast 0.0897
    /// > threshold 0.0833 となり発動してしまう設計ミスだった
    /// (テスト赤が捕捉 — 自己誤り捕捉 12 件目)。
    #[test]
    fn untouched_path_is_bitexact_copy() {
        let f = Fxaa::new();
        let c = Vec3::new(1.0, 0.0, 0.0); // pure red、隣接は等輝度グレー
        let out = f.shade(c, solid(0.299), solid(0.299), solid(0.299), solid(0.299));
        assert_eq!(bits(out.r), bits(1.0));
        assert_eq!(bits(out.g), bits(0.0));
        assert_eq!(bits(out.b), bits(0.0));
        // 等輝度の根拠そのものもピン (検算で両者とも f32(0.299) 一致)。
        assert_eq!(bits(Fxaa::luma(Vec3::new(1.0, 0.0, 0.0))), bits(0.299));
        assert_eq!(bits(Fxaa::luma(solid(0.299))), bits(0.299));
    }

    /// CE: WGSL 側の閾値定数 (ハードコード) と Rust Default が bit 一致する
    /// ことを、WGSL ソース文字列からの機械抽出で担保 (乖離検出ピン)。
    /// 0.166667 と 1.0/256.0 は文字列が同一なら両パーサとも最近 binary32 で
    /// 値が一意に決まる (0x3E2AAAC1 / 0x3B800000)。
    #[test]
    fn wgsl_threshold_constants_match_rust_default_bits() {
        let src = Fxaa::new().wgsl_source();
        assert!(src.contains("0.166667"), "WGSL 相対閾値リテラル在籍");
        assert!(src.contains("1.0 / 256.0"), "WGSL 絶対床リテラル在籍");
        // 文字列 → f32 パース (WGSL AbstractFloat→f32 と同じ最近変換)。
        let wgsl_rel: f32 = "0.166667".parse().unwrap();
        let d = Fxaa::default();
        assert_eq!(wgsl_rel.to_bits(), d.relative_threshold.to_bits());
        assert_eq!(d.relative_threshold.to_bits(), 0x3E2AAAC1);
        assert_eq!(d.contrast_base.to_bits(), bits(1.0 / 256.0));
        assert_eq!(d.contrast_base.to_bits(), 0x3B800000);
        // luma 係数 (BT.601) も WGSL と同一語彙。
        assert!(src.contains("0.299") && src.contains("0.587") && src.contains("0.114"));
    }
}

//! FXAA — Fast Approximate Anti-Aliasing (Timothy Lottes). Post-process pass
//! that smooths jaggies in the final color buffer using only luma gradients.
//!
//! Cheap (one full-screen pass, no depth/geometry needed), so it is a good
//! default on low-spec GPUs where MSAA/TAA are too expensive. The Rust side
//! here is a faithful CPU reference used by the tests; the WGSL is the GPU pass.
//!
//! ## 消費者の実効挙動 (監査 2026-07-26 DQ-1)
//! 唯一の Rust 側呼出 full_graph_wiring:1694 は **同一色 (sharpened) を 5 つの
//! 引数全て (center/n/s/e/w) に与える**。よって lmin==lmax で contrast≡0 <
//! threshold となり [`Fxaa::shade`] は `return center` の**bit 厳密な恒等
//! 写像**となる — 本経路 (CPU reference 実演) 上の FXAA は**描画に一切寄与
//! しない**。GPU 側 AA は別経路 (fxaa.wgsl の登録) で行う設計であり、Rust
//! 側は契約検証・将来の CPU フォールバック用の参照実装 (strict pin 済)。
//!
//! ## luma 規格の誠実注記 (監査 2026-07-26 DQ-5)
//! 本モジュールの luma は Rec.601 (0.299/0.587/0.114、従来 FXAA43 流) であり、
//! post チェーン他段 (bloom/exposure の Rec.709: 0.2126/0.7152/0.0722) と
//! **係数系が混在**する。これは FXAA の JPEG/Rec.601 伝統に整合した意図的
//! 選択であり「バグ」ではないが、チェーン内で luma 定義が 3 段で 2 種に
//! 分かれることは消費者警告として記録する (旧 doc の「BT.601-ish」は
//! 係数としては厳密に BT.601 そのもののため表現を訂正)。

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

    /// Perceptual luma (Rec.601/BT.601 係数そのもの、green-weighted)。
    /// ※ チェーン他段との係数混在はヘッダ DQ-5 注記を参照。
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
    /// - 勾配軸の同値タイ (`gx.abs() == gy.abs()`) は**厳密 `>` のため
    ///   else 側 (E/W とブレンド) を選ぶ (2026-07-26 DQ-2 で厳密 pin)。
    /// - threshold は `base.max(lmax * relative)` の**2 分岐構造**
    ///   (暗所=絶対床 1/256 が支配、明所=相対 1/6.000002 で高コントラスト
    ///   要求) — 2026-07-26 DQ-4 で分岐選択を厳密 pin。
    /// - NaN 伝播は**位置非対称** (2026-07-26 DQ-3): min/max は NaN 脱落の
    ///   ため n/s の NaN は gy=NaN→比較 false→else 分岐 (E/W 有限なら完全に
    ///   マスクされ有限出力) だが、e/w/center の NaN は avg/out へ伝播する。
    pub fn shade(&self, center: Vec3, n: Vec3, s: Vec3, e: Vec3, w: Vec3) -> Vec3 {
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
        let out = f.shade(
            solid(0.50),
            solid(0.51),
            solid(0.49),
            solid(0.50),
            solid(0.50),
        );
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

    // ------------------------------------------------- 監査 2026-07-26 DQ 追加分

    /// DQ-1: wiring :1694 は同一色を 5 引数に与える → contrast≡0 →
    /// `return center` の bit 厳密な恒等写像。hdr 域含むグリッドで厳密 pin。
    #[test]
    fn wiring_identical_taps_make_fxaa_identity() {
        let f = Fxaa::new();
        let samples = [
            (0.0, 0.0, 0.0),
            (0.25, 0.5, 0.75),
            (1.0, 1.0, 1.0),
            (3.14, 2.71, 1.41), // sRGB clamp 後ではない HDR 域 (CAS 前挿入ならあり得る)
            (64.0, 0.001, 7.5),
        ];
        let mut checked = 0;
        for &(r, g, b) in &samples {
            let c = Vec3::new(r, g, b);
            let out = f.shade(c, c, c, c, c);
            assert_eq!(out.r.to_bits(), c.r.to_bits(), "identity (r)");
            assert_eq!(out.g.to_bits(), c.g.to_bits(), "identity (g)");
            assert_eq!(out.b.to_bits(), c.b.to_bits(), "identity (b)");
            checked += 1;
        }
        assert_eq!(checked, 5);
    }

    /// DQ-2: 勾配軸の同値タイは厳密 `>` のため else (E/W) を選ぶ。
    /// 構成: gx = lw - le = luma(0.7) - luma(0.3)、gy = ln - ls =
    /// luma(0.9) - luma(0.5)。両者 |gx| == |gy| (luma は gray の線形写像で
    /// 0.7-0.3 == 0.9-0.5) → E/W ブレンド。out/t は Python IEEE f32 シムで
    /// 事前導出 (left-assoc 厳密化、機械検算済)。
    #[test]
    fn gradient_axis_tie_picks_ew_exact_bits() {
        let f = Fxaa::new();
        let c = solid(0.7);
        let out = f.shade(c, solid(0.9), solid(0.5), solid(0.3), solid(0.7));
        assert_eq!(bits(out.r), 0x3F1E_B852, "tie → E/W ブレンド (r)");
        assert_eq!(bits(out.g), 0x3F1E_B852);
        assert_eq!(bits(out.b), 0x3F1E_B852);
        // 非タイ (|gx| > |gy|) は N/S を選ぶ事の相補確認 (既存 CE 縦/横ピンと併せ)。
        // luma(gray v) の線形性に依る同値性そのものもピン。
        assert_eq!(
            bits(Fxaa::luma(solid(0.7)) - Fxaa::luma(solid(0.3))),
            bits(Fxaa::luma(solid(0.9)) - Fxaa::luma(solid(0.5))),
            "gx == gy の構成根拠 (gray-luma 線形で 0.4 同値)"
        );
    }

    /// DQ-3: NaN 伝播は位置非対称 — n/s の NaN は (min/max NaN 脱落 + gy NaN
    /// → 比較 false → else 分岐) で**完全マスク**され有限出力 (E/W 有限なら)、
    /// e/w/center の NaN は出力へ伝播する。
    #[test]
    fn nan_placement_asymmetry_contract() {
        let f = Fxaa::new();
        let c = solid(0.5);
        let nan3 = Vec3::new(f32::NAN, f32::NAN, f32::NAN);
        // n=NaN: 完全マスク (有限、厳密 0x3F000000 = 0.5)。
        let o1 = f.shade(c, nan3, solid(0.9), solid(0.3), solid(0.7));
        assert_eq!(bits(o1.r), bits(0.5f32));
        assert_eq!(bits(o1.g), bits(0.5f32));
        assert_eq!(bits(o1.b), bits(0.5f32));
        // s=NaN: 同様に完全マスク。
        let o2 = f.shade(c, solid(0.1), nan3, solid(0.3), solid(0.7));
        assert_eq!(bits(o2.r), bits(0.5f32));
        // e/w NaN → E/W avg が NaN で伝播。
        let o3 = f.shade(c, solid(0.1), solid(0.9), nan3, solid(0.7));
        assert!(
            o3.r.is_nan() && o3.g.is_nan() && o3.b.is_nan(),
            "e=NaN 伝播"
        );
        let o4 = f.shade(c, solid(0.1), solid(0.9), solid(0.3), nan3);
        assert!(o4.r.is_nan(), "w=NaN 伝播");
        // center NaN → 無条件伝播。
        let o5 = f.shade(nan3, solid(0.1), solid(0.9), solid(0.3), solid(0.7));
        assert!(
            o5.r.is_nan() && o5.g.is_nan() && o5.b.is_nan(),
            "center NaN 伝播"
        );
    }

    /// DQ-4: threshold 2 分岐選択 (base.floor vs lmax*relative) の厳密 pin。
    /// 暗所 A: lmax*rel < 1/256 で floor 支配 → 微コントラスト 0.0055 で発動。
    /// 明所 B (+0.5 シフト): lmax*rel ≈ 0.084 が支配 → 同一デルタ帯は不発。
    /// ※ +0.5 シフトで contrast 自体が f32 丸めで変わる事実 (0x3BB43958 ↔
    /// 0x3BB43980) も記録: デルタは絶対輝度に対し不変ではない。
    #[test]
    fn threshold_two_branch_selection_exact_bits() {
        let f = Fxaa::new();
        // scene A (dark): base floor が支配、contrast 0x3BB43958 で発動。
        let a = f.shade(
            solid(0.002),
            solid(0.002),
            solid(0.002),
            solid(0.0075),
            solid(0.0075),
        );
        assert_eq!(bits(a.r), 0x3B6C_73C0, "A: floor 支配で発動 (r)");
        assert_eq!(bits(a.g), 0x3B6C_73C0);
        assert_eq!(bits(a.b), 0x3B6C_73C0);
        // scene B (bright, +0.5): relative 支配 (0x3DAD3A1D) で不発 → bit コピー。
        let c2 = solid(0.502);
        let b = f.shade(c2, c2, c2, solid(0.5075), solid(0.5075));
        assert_eq!(
            bits(b.r),
            bits(0.502),
            "B: relative 支配で不発の bit コピー (r)"
        );
        assert_eq!(bits(b.g), bits(0.502));
        assert_eq!(bits(b.b), bits(0.502));
    }

    /// DQ-5: Rec.601 係数の厳密性 (doc「BT.601-ish」の訂正根拠) と
    /// 係数和の f32 正規化性 (luma(1,1,1) = 1.0 厳密) を pin。
    #[test]
    fn rec601_coefficients_exact_and_sum_to_one() {
        assert_eq!(bits(Fxaa::luma(Vec3::new(1.0, 0.0, 0.0))), bits(0.299));
        assert_eq!(bits(Fxaa::luma(Vec3::new(0.0, 1.0, 0.0))), bits(0.587));
        assert_eq!(bits(Fxaa::luma(Vec3::new(0.0, 0.0, 1.0))), bits(0.114));
        // 左結合和 0.299+0.587+0.114 は f32 で厳密に 1.0 (gray 1.0 の luma)。
        assert_eq!(bits(Fxaa::luma(solid(1.0))), bits(1.0));
    }
}

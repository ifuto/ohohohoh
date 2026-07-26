//! SMAA — Subpixel Morphological Anti-Aliasing (edge detection + blend).
//!
//! A post-process AA that detects luma edges, classifies their orientation and
//! computes a per-pixel blend weight to smooth jaggies without the blur of FXAA
//! or the cost/ghosting of TAA. Good mid-spec option; the CPU side here is a
//! faithful reference used by the tests.
//!
//! ## 消費者の実効挙動 (監査 2026-07-26 DS-1)
//! 唯一の Rust 側呼出 full_graph_wiring:1701 は **同一色 (aa) を 5 引数全てに
//! 与え**、edge() の戻り値はブレンド計算後 **`let _ = (is_edge, smaa_w)` で
//! 完全に破棄**される (FSR1 の CH-1 注記と同じ「定数色不変性の実演」形)。
//! よって contrast≡0 → strength=0 → smaa_w=0 で、本経路の SMAA は**描画に
//! 一切寄与しない** (bit 厳密な恒等クラス、DM-2 bloom・DQ-1 fxaa と同型)。
//! GPU 側 AA は別経路で実効。実効化は近傍テクセル実配線の設計判断のため
//! 引継ぎ (私は変更しない)。
//!
//! ## threshold 分岐の誠実注記 (監査 2026-07-26 DS-5)
//! エッジ判定の閾値は `max(1/256, lmax · 0.1)` の**2 分岐構造**で、fxaa の
//! `base.max(lmax·rel)` と同形。暗所では絶対床、明所では相対閾値が支配する
//! (DQ-4 と同族、分岐選択は strict pin 済)。

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

#[derive(Clone, Copy, Debug)]
pub struct Smaa {
    /// Edge detection threshold (relative to local max luma).
    pub threshold: f32,
}
impl Default for Smaa {
    fn default() -> Self {
        Self { threshold: 0.1 }
    }
}
impl Smaa {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn luma(c: Vec3) -> f32 {
        0.299 * c.r + 0.587 * c.g + 0.114 * c.b
    }

    /// Detect a luma edge. Returns `(strength, is_horizontal, local_max_luma)`.
    /// `strength == 0` means "no edge here".
    ///
    /// ## 契約 (監査 2026-07-26 DS-2/DS-3)
    /// * 閾値: contrast < `max(1/256, lmax·0.1)` → (0.0, false, lmax)
    ///   (分岐構造は DS-5、値は Python IEEE f32 シム事前導出で厳密 pin)。
    /// * 方向分類: `horizontal = gx.abs() < gy.abs()` で、**同値タイは厳密 <
    ///   のため horizontal=false** → strength = |gx| (DQ-2 と同型のピン)。
    /// * strength = 主勾配の絶対値 (選択方向の |gx| or |gy|)。
    /// * NaN 伝播は位置非対称 (DS-3): min/max は NaN 脱落し、center/n/s の
    ///   NaN は勾配・比較で**完全マスク** (gy 側は比較 false に潰れ
    ///   strength=|gx| の有限値) だが、e/w の NaN は gx 経由で strength へ
    ///   伝播 (fail-visible 区分)。
    pub fn edge(&self, center: Vec3, n: Vec3, s: Vec3, e: Vec3, w: Vec3) -> (f32, bool, f32) {
        let lc = Self::luma(center);
        let ln = Self::luma(n);
        let ls = Self::luma(s);
        let le = Self::luma(e);
        let lw = Self::luma(w);
        let lmax = lc.max(ln).max(ls).max(le).max(lw);
        let lmin = lc.min(ln).min(ls).min(le).min(lw);
        let contrast = lmax - lmin;
        if contrast < (1.0f32 / 256.0).max(lmax * self.threshold) {
            return (0.0, false, lmax);
        }
        let gx = lw - le; // horizontal gradient
        let gy = ln - ls; // vertical gradient
        let horizontal = gx.abs() < gy.abs();
        let strength = if horizontal { gy.abs() } else { gx.abs() };
        (strength, horizontal, lmax)
    }

    /// Blend weight for a detected edge (0..=0.5) — stronger edge => more blend.
    ///
    /// ## 契約 (監査 2026-07-26 DS-4)
    /// * strength ≤ 0 → 厳密に **0.0 返却** (負入力を 0 側へ押戻す)。
    /// * strength/(strength + local_max·0.5) を clamp(0.0, 0.5)。
    ///   local_max=0 なら strength>0 で商=1.0 → **0.5 へ clamp 上張**。
    /// * NaN strength → **NaN 伝播** (clamp は比較 false で self 返却)。
    pub fn blend(&self, strength: f32, local_max: f32) -> f32 {
        if strength <= 0.0 {
            return 0.0;
        }
        (strength / (strength + local_max * 0.5)).clamp(0.0, 0.5)
    }

    pub fn wgsl_source(&self) -> &'static str {
        SMAA_WGSL
    }
}

pub const SMAA_WGSL: &str = include_str!("../shaders/smaa.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    fn solid(v: f32) -> Vec3 {
        Vec3::new(v, v, v)
    }
    #[test]
    fn flat_has_no_edge() {
        let s = Smaa::new();
        let (strength, _, _) = s.edge(solid(0.5), solid(0.5), solid(0.5), solid(0.5), solid(0.5));
        assert_eq!(strength, 0.0);
    }
    #[test]
    fn vertical_edge_detected() {
        let s = Smaa::new();
        // bright left, dark right => horizontal gradient strong; edge vertical (gy small)
        let c = solid(0.5);
        let n = solid(0.5);
        let s2 = solid(0.5);
        let e = solid(0.0);
        let w = solid(1.0);
        let (strength, horizontal, _) = s.edge(c, n, s2, e, w);
        assert!(strength > 0.0);
        assert!(!horizontal); // vertical edge
    }
    #[test]
    fn horizontal_edge_detected() {
        let s = Smaa::new();
        let c = solid(0.5);
        let n = solid(1.0);
        let s2 = solid(0.0);
        let e = solid(0.5);
        let w = solid(0.5);
        let (strength, horizontal, _) = s.edge(c, n, s2, e, w);
        assert!(strength > 0.0);
        assert!(horizontal);
    }
    #[test]
    fn blend_in_range() {
        let s = Smaa::new();
        let b = s.blend(0.4, 1.0);
        assert!(b > 0.0 && b <= 0.5);
    }

    // ------------------------------------------------- 監査 2026-07-26 DS 追加分

    /// DS-1: wiring :1701 は同一色を 5 引数に与える → strength=0 → blend 0、
    /// 戻り値も破棄の恒等クラス (DQ-1 fxaa/DM-2 bloom と同型、bit 厳密 pin)。
    #[test]
    fn wiring_identical_taps_make_smaa_degenerate() {
        let s = Smaa::new();
        let samples = [
            (0.0, 0.0, 0.0),
            (0.25, 0.5, 0.75),
            (1.0, 1.0, 1.0),
            (3.14, 2.71, 1.41),
        ];
        let mut checked = 0;
        for &(r, g, b) in &samples {
            let c = Vec3::new(r, g, b);
            let (strength, horiz, lmax) = s.edge(c, c, c, c, c);
            assert_eq!(strength.to_bits(), 0, "identical taps → strength=0 厳密");
            assert!(!horiz, "contrast 0 は false 側 (一貫)");
            assert_eq!(lmax.to_bits(), Smaa::luma(c).to_bits(), "lmax = luma(c)");
            let w = s.blend(strength, lmax.max(1e-3));
            assert_eq!(w.to_bits(), 0, "strength=0 → blend=0 厳密");
            checked += 1;
        }
        assert_eq!(checked, 4);
    }

    /// DS-2: 厳密 bit pin (Python IEEE f32 シム事前導出 → 照合):
    /// 既存 V/H シナリオの strength=1.0 厳密・tie は horizontal=false・
    /// tie strength/lmax 厳密値。
    #[test]
    fn edge_exact_bits_and_tie_break_pin() {
        let s = Smaa::new();
        let (st, h, lm) = s.edge(solid(0.5), solid(0.5), solid(0.5), solid(0.0), solid(1.0));
        assert_eq!(st.to_bits(), 0x3F80_0000, "vertical edge strength=1.0");
        assert!(!h);
        assert_eq!(lm.to_bits(), 0x3F80_0000, "lmax=1.0");
        let (st2, h2, _) = s.edge(solid(0.5), solid(1.0), solid(0.0), solid(0.5), solid(0.5));
        assert_eq!(st2.to_bits(), 0x3F80_0000, "horizontal edge strength=1.0");
        assert!(h2);
        // タイ (gx==gy): horizontal=false (厳密 <)、strength=|gx|。
        let (st3, h3, lm3) = s.edge(solid(0.5), solid(0.9), solid(0.5), solid(0.9), solid(0.5));
        assert!(!h3, "tie → horizontal=false");
        assert_eq!(st3.to_bits(), 0x3ECC_CCCC, "tie strength=|gx| 厳密");
        assert_eq!(lm3.to_bits(), 0x3F66_6666, "tie lmax 厳密");
        // 非タイ主勾配の厳密値 (0.9-0.9 系)。
        let (st4, h4, _) = s.edge(solid(0.5), solid(0.9), solid(0.1), solid(0.6), solid(0.9));
        assert!(h4);
        assert_eq!(st4.to_bits(), 0x3F4C_CCCC, "主勾配 |gy| 厳密");
    }

    /// DS-3: NaN 伝播の位置非対称 (center/n/s はマスク、e/w は伝播)。
    #[test]
    fn nan_position_asymmetry_contract() {
        let s = Smaa::new();
        let nan3 = Vec3::new(f32::NAN, f32::NAN, f32::NAN);
        // center NaN → strength/lmax とも有限 (min/max で脱落・gx/gy は他値)。
        let (st, h, lm) = s.edge(nan3, solid(0.5), solid(0.6), solid(0.7), solid(0.8));
        assert!(st.is_finite() && lm.is_finite(), "center NaN は完全マスク");
        assert_eq!(st.to_bits(), 0x3DCC_CCD0, "center NaN masked strength 厳密");
        assert!(!h, "gx 優位で horizontal=false");
        // n/s NaN → gy=NaN で比較 false → strength=|gx| 有限 (完全マスク)。
        let (st_n, _, _) = s.edge(solid(0.5), nan3, solid(0.6), solid(0.7), solid(0.8));
        assert!(st_n.is_finite(), "n=NaN も完全マスク");
        let (st_s, _, _) = s.edge(solid(0.5), solid(0.5), nan3, solid(0.7), solid(0.8));
        assert!(st_s.is_finite(), "s=NaN も完全マスク");
        // e/w NaN → gx=NaN で strength へ伝播。
        let (st_e, h_e, _) = s.edge(solid(0.5), solid(0.5), solid(0.6), nan3, solid(0.8));
        assert!(st_e.is_nan(), "e=NaN は strength へ伝播 (fail-visible)");
        assert!(!h_e, "gx=NaN 比較 false → horizontal=false");
        let (st_w, _, _) = s.edge(solid(0.5), solid(0.5), solid(0.6), solid(0.7), nan3);
        assert!(st_w.is_nan(), "w=NaN 伝播");
    }

    /// DS-4: blend 境界・クランプ・NaN 伝播契約の厳密 pin。
    #[test]
    fn blend_boundary_clamp_nan_exact_bits() {
        let s = Smaa::new();
        assert_eq!(
            s.blend(1.0, 1.0).to_bits(),
            0x3F00_0000,
            "1/(1+0.5)→clamp 0.5"
        );
        assert_eq!(
            s.blend(0.3, 0.0).to_bits(),
            0x3F00_0000,
            "lm=0 で商=1 → clamp 0.5"
        );
        assert_eq!(s.blend(0.0, 1.0).to_bits(), 0, "0 → 0 厳密");
        assert_eq!(s.blend(-0.5, 1.0).to_bits(), 0, "負 strength → 0 厳密");
        assert!(s.blend(f32::NAN, 1.0).is_nan(), "NaN strength → NaN 伝播");
    }

    /// DS-5: threshold 2 分岐選択の厳密 pin (A'=floor 支配で発動、
    /// B'=relative 支配で不発の対蹠、DQ-4 同族)。
    #[test]
    fn threshold_two_branch_selection_pin() {
        let s = Smaa::new();
        // A' (dark, aniso): lmax*0.1=0.001 < 1/256 → floor 支配、
        // contrast=luma(0.01)-luma(0.001)=0x3C1374BC > 0x3B800000 で発動。
        // gy=0 (n=s), gx=luma(0.01)-luma(0.009)=0x3A831270 > 0 → horiz=false、
        // strength=|gx| の厳密値。
        let (st_a, h_a, lm_a) = s.edge(
            solid(0.001),
            solid(0.001),
            solid(0.001),
            solid(0.01),
            solid(0.009),
        );
        assert_eq!(
            st_a.to_bits(),
            0x3A83_1270,
            "A': floor 支配で発動 (strength)"
        );
        assert!(!h_a, "A': |gy|=0 で horizontal=false");
        assert_eq!(lm_a.to_bits(), 0x3C23_D70A, "A': lmax=luma(0.01)");
        // B' (bright, aniso): relative 支配 (0x3DBA5E37) > contrast(0x3C23D780)
        // → 不発。
        let (st_b, h_b, _) = s.edge(
            solid(0.9),
            solid(0.9),
            solid(0.9),
            solid(0.91),
            solid(0.905),
        );
        assert_eq!(
            st_b.to_bits(),
            0,
            "B': relative 支配で不発 → strength=0 厳密"
        );
        assert!(!h_b, "不発経路は horizontal=false");
    }

    /// DS-6 (観): **contrast 通過でも strength=0 になりうる方向無しケース**。
    /// 両ペアの軸内差がゼロ (e==w かつ n==s) でも、グループ間の全域 contrast
    /// は閾値を超えうる。この時 edge() は (0.0, false, lmax) を返す —
    /// doc「strength == 0 means "no edge here"」との表現上の緊張 (判定通過
    /// だが方向が決定不能のため 0 返却) を誠実に公表する:
    /// contrast 0x3C1374BC > floor 0x3B800000 でも strength は厳密 0.0。
    #[test]
    fn strength_zero_despite_contrast_pass_contract() {
        let s = Smaa::new();
        // c=n=s=0.001, e=w=0.01: 軸内差 gx=lw-le=0, gy=ln-ls=0 だが
        // contrast=0.009 > floor 0.003906 → 「発動」で strength=0。
        let (st, h, lm) = s.edge(
            solid(0.001),
            solid(0.001),
            solid(0.001),
            solid(0.01),
            solid(0.01),
        );
        assert_eq!(st.to_bits(), 0, "両軸差ゼロ → strength 厳密 0 (0.0 bits)");
        assert!(!h, "|gx|=|gy|=0 → tie → horizontal=false");
        assert_eq!(lm.to_bits(), 0x3C23_D70A, "lmax=luma(0.01) は通過側");
        // contrast>threshold でも strength=0 になりうる (方向決定不能)。
        // → blend は strength<=0 → 厳密 0.0 (安全側 contract)。
        assert_eq!(
            s.blend(st, lm.max(1e-3)).to_bits(),
            0,
            "strength=0 → blend=0 厳密"
        );
    }
}

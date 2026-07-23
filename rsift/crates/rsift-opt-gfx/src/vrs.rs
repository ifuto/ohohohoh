//! Variable Rate Shading (VRS) — per-tile shading-rate mask generation.
//!
//! Picks a coarser shading rate for fast-moving regions (motion blur hides the
//! lost detail) and keeps a fine rate for detailed static regions. Directly
//! reduces pixel-shader invocations, which is a big win on integrated GPUs
//! (Intel/Apple) where fragment shading is the bottleneck.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadingRate {
    Rate1x1,
    Rate1x2,
    Rate2x2,
    Rate2x4,
    Rate4x4,
}
impl ShadingRate {
    /// Hardware shading-rate code (0 = finest).
    pub fn as_code(self) -> u32 {
        match self {
            ShadingRate::Rate1x1 => 0,
            ShadingRate::Rate1x2 => 1,
            ShadingRate::Rate2x2 => 2,
            ShadingRate::Rate2x4 => 3,
            ShadingRate::Rate4x4 => 4,
        }
    }
    /// Number of pixels covered by one shaded sample at this rate.
    pub fn pixel_area(self) -> u32 {
        match self {
            ShadingRate::Rate1x1 => 1,
            ShadingRate::Rate1x2 => 2,
            ShadingRate::Rate2x2 => 4,
            ShadingRate::Rate2x4 => 8,
            ShadingRate::Rate4x4 => 16,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Vrs {
    /// How strongly motion pushes toward a coarser rate.
    pub motion_weight: f32,
    /// How strongly local detail (luma variance) keeps a fine rate.
    pub variance_weight: f32,
}
impl Default for Vrs {
    fn default() -> Self {
        Self {
            motion_weight: 1.0,
            variance_weight: 0.8,
        }
    }
}
impl Vrs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Choose a shading rate.
    /// * `motion`  — per-pixel motion magnitude in [0,1] (fraction of a pixel per frame).
    /// * `luma_var`— local luma variance in [0,1].
    ///
    /// A higher combined `score` means "coarser is safe". High motion raises it,
    /// high detail lowers it.
    pub fn select(&self, motion: f32, luma_var: f32) -> ShadingRate {
        let score = motion.clamp(0.0, 1.0) * self.motion_weight
            - luma_var.clamp(0.0, 1.0) * self.variance_weight;
        if score > 0.6 {
            ShadingRate::Rate4x4
        } else if score > 0.3 {
            ShadingRate::Rate2x4
        } else if score > 0.05 {
            ShadingRate::Rate2x2
        } else if score > -0.3 {
            ShadingRate::Rate1x2
        } else {
            ShadingRate::Rate1x1
        }
    }

    /// Build a coarse rate-tile buffer (one code per `tile`×`tile` block) by
    /// averaging per-pixel motion/variance. `motion`/`var` are row-major `w`×`h`.
    ///
    /// **契約 (wave 70 BT-2)**: `tile` は 1 以上必須 (0 では `(w + tile - 1)
    /// / tile` が 0 除算 panic となる)。`frame_postfx::vrs_run_cpu` と同一の
    /// fail-loud 契約を 3連鎖全入口で強制する。
    pub fn build_mask(
        &self,
        w: usize,
        h: usize,
        tile: usize,
        motion: &[f32],
        var: &[f32],
    ) -> Vec<u32> {
        assert!(
            tile > 0,
            "vrs tile は 1 以上必須 (0 ではタイル数の計算が 0 除算パニック)"
        );
        assert_eq!(motion.len(), w * h, "motion buffer size mismatch");
        assert_eq!(var.len(), w * h, "variance buffer size mismatch");
        let tw = (w + tile - 1) / tile;
        let th = (h + tile - 1) / tile;
        let mut out = vec![0u32; tw * th];
        for ty in 0..th {
            for tx in 0..tw {
                let mut ms = 0.0f32;
                let mut vs = 0.0f32;
                let mut cnt = 0u32;
                let y0 = ty * tile;
                let y1 = ((ty + 1) * tile).min(h);
                let x0 = tx * tile;
                let x1 = ((tx + 1) * tile).min(w);
                for y in y0..y1 {
                    for x in x0..x1 {
                        let i = y * w + x;
                        ms += motion[i];
                        vs += var[i];
                        cnt += 1;
                    }
                }
                let rate = self.select(ms / cnt as f32, vs / cnt as f32);
                out[ty * tw + tx] = rate.as_code();
            }
        }
        out
    }

    pub fn wgsl_source(&self) -> &'static str {
        VRS_WGSL
    }
}

pub const VRS_WGSL: &str = include_str!("../shaders/vrs.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fast_motion_is_coarse() {
        let v = Vrs::new();
        assert_eq!(v.select(1.0, 0.0), ShadingRate::Rate4x4);
    }
    #[test]
    fn detailed_static_is_fine() {
        let v = Vrs::new();
        assert_eq!(v.select(0.0, 1.0), ShadingRate::Rate1x1);
    }
    #[test]
    fn mid_range_maps_to_2x2() {
        let v = Vrs::new();
        // motion 0.4, var 0 => score 0.4 => Rate2x4? (0.4>0.3) -> Rate2x4
        assert_eq!(v.select(0.4, 0.0), ShadingRate::Rate2x4);
    }
    #[test]
    fn mask_dimensions_and_content() {
        let v = Vrs::new();
        let w = 8;
        let h = 4;
        let tile = 4;
        // flat, no motion => coarse-ish (Rate1x2). Make one tile fast-moving.
        let mut motion = vec![0.0f32; w * h];
        for y in 0..4 {
            for x in 4..8 {
                motion[y * w + x] = 1.0;
            }
        }
        let var = vec![0.0f32; w * h];
        let mask = v.build_mask(w, h, tile, &motion, &var);
        assert_eq!(mask.len(), 2 * 1); // 8/4=2 wide, 4/4=1 tall
                                       // left tile = static flat => Rate1x2 (code 1)
        assert_eq!(mask[0], ShadingRate::Rate1x2.as_code());
        // right tile = fast motion => Rate4x4 (code 4)
        assert_eq!(mask[1], ShadingRate::Rate4x4.as_code());
    }

    /// wave 70 BT-1: 閾値比較は全て**厳密大なり** (等号成立時は**粗くしない**
    /// 側へ落ちる) — 境界丁度の score で次段へ進まないことを厳密固定。
    /// score 値は全て f32 厳密に閾値リテラルと一致するよう導出済み:
    ///   select(0.6, 0.0): score ≡ 0.6f32 → 4x4 ではなく 2x4
    ///   select(0.3, 0.0): score ≡ 0.3f32 → 2x4 ではなく 2x2
    ///   select(0.05, 0.0): score ≡ 0.05f32 → 2x2 ではなく 1x2
    ///   select(0.0, 0.375): 0.375·0.8f32 ≡ 0.3f32 → score ≡ -0.3f32 →
    ///   1x2 ではなく 1x1 (var=0.375 は var*0.8 が 0.3f32 丁度となる値)
    #[test]
    fn select_thresholds_are_strictly_greater() {
        let v = Vrs::new();
        assert_eq!(v.select(0.6, 0.0), ShadingRate::Rate2x4, "score==0.6 stays");
        assert_eq!(v.select(0.3, 0.0), ShadingRate::Rate2x2, "score==0.3 stays");
        assert_eq!(
            v.select(0.05, 0.0),
            ShadingRate::Rate1x2,
            "score==0.05 stays"
        );
        assert_eq!(
            v.select(0.0, 0.375),
            ShadingRate::Rate1x1,
            "score==-0.3 stays"
        );
        // 1 ulp 超過側は厳密に次段へ (境界の非対称性の両側固定)。
        let just_above_06 = f32::from_bits(0x3f19999a + 1); // 0.6f32 + 1ulp
        assert_eq!(
            v.select(just_above_06, 0.0),
            ShadingRate::Rate4x4,
            "score = 0.6+1ulp must coarsen"
        );
    }

    /// wave 70 BT-2: tile=0 は 0 除算 panic 前に契約違反として fail-loud。
    /// (frame_postfx::vrs_run_cpu と同一契約。境界 tile=1 は受理で実作業確認)
    #[test]
    #[should_panic(expected = "vrs tile は 1 以上必須")]
    fn build_mask_rejects_zero_tile() {
        let v = Vrs::new();
        let _ = v.build_mask(4, 4, 0, &[0.0; 16], &[0.0; 16]);
    }

    #[test]
    fn build_mask_accepts_tile_one() {
        let v = Vrs::new();
        let mask = v.build_mask(2, 2, 1, &[1.0; 4], &[0.0; 4]);
        assert_eq!(mask.len(), 4, "tile=1 → 2x2 tiles");
        assert!(mask.iter().all(|&c| c == ShadingRate::Rate4x4.as_code()));
    }

    /// wave 70 BT-3: vrs.wgsl が select/build_mask と同一語彙であることの
    /// 表記ピン (3連鎖: vrs.rs ↔ vrs_run_cpu ↔ vrs.wgsl)。
    #[test]
    fn wgsl_mirror_lexical_tokens() {
        const WGSL: &str = include_str!("../shaders/vrs.wgsl");
        for tok in [
            // score 式 (clamp 両辺)
            "clamp(motion, 0.0, 1.0) * mw - clamp(variance, 0.0, 1.0) * vw",
            // 厳密大なり閾値 4 段
            "score > 0.6",
            "score > 0.3",
            "score > 0.05",
            "score > -0.3",
            // タイル平均 (逐次加算 → 個数除算)
            "ms = ms + motion_in[i];",
            "let motion = ms / f32(cnt);",
            // コード割当 (4=最粗 … 0=最細)
            "return 4u;",
            "return 0u;",
        ] {
            assert!(
                WGSL.contains(tok),
                "vrs.wgsl drift from module rules: {tok}"
            );
        }
    }
}

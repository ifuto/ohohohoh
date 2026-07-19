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
    pub fn build_mask(&self, w: usize, h: usize, tile: usize, motion: &[f32], var: &[f32]) -> Vec<u32> {
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
}

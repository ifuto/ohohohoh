//! GTAO — Ground Truth Ambient Occlusion (horizon-based).
//!
//! Approximates ray-traced AO by integrating the horizon angle of the depth
//! profile around each pixel. Much higher quality than SSAO (matches offline
//! reference better, stable in motion) and at ~1ms on iGPUs is affordable when
//! run at half resolution. The CPU side is a faithful single-slice reference
//! used by the tests; the WGSL does the full multi-direction integral.

#[derive(Clone, Copy, Debug)]
pub struct Gtao {
    /// Number of view directions integrated around the pixel.
    pub directions: u32,
    /// Sample count per direction.
    pub steps: u32,
    /// World-space AO radius.
    pub radius: f32,
}
impl Default for Gtao {
    fn default() -> Self {
        Self {
            directions: 4,
            steps: 8,
            radius: 1.0,
        }
    }
}
impl Gtao {
    pub fn new() -> Self {
        Self::default()
    }

    /// Single-slice (1D) occlusion factor in [0,1] (1 = unoccluded).
    /// `samples` are `(horizontal_offset, height)` pairs with `offset > 0`,
    /// `offset` increasing. `height` is the sample's "height" above the surface;
    /// a sample taller than `center_height` raises the horizon and occludes.
    pub fn slice_occlusion(&self, center_height: f32, samples: &[(f32, f32)]) -> f32 {
        let mut max_horizon = -std::f32::consts::FRAC_PI_2; // -90°
        for &(off, h) in samples {
            if off <= 0.0 {
                continue;
            }
            let angle = ((h - center_height) / off).atan();
            if angle > max_horizon {
                max_horizon = angle;
            }
        }
        // Ow = occlusion grows with the horizon angle above the tangent plane.
        let occlusion = (max_horizon / std::f32::consts::FRAC_PI_2).clamp(0.0, 1.0);
        1.0 - occlusion
    }

    /// Multi-slice occlusion, averaging `directions` rotated 1D slices. Each
    /// slice's `samples` are pre-rotated by the caller. Returns [0,1].
    pub fn occlusion(&self, center_height: f32, slices: &[Vec<(f32, f32)>]) -> f32 {
        if slices.is_empty() {
            return 1.0;
        }
        let mut sum = 0.0f32;
        for s in slices {
            sum += self.slice_occlusion(center_height, s);
        }
        sum / slices.len() as f32
    }

    pub fn wgsl_source(&self) -> &'static str {
        GTAO_WGSL
    }
}

pub const GTAO_WGSL: &str = include_str!("../shaders/gtao.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flat_surface_is_unoccluded() {
        let g = Gtao::new();
        // All samples at the same height as the center => no horizon rise.
        let samples = vec![(1.0, 0.0), (2.0, 0.0), (3.0, 0.0)];
        let o = g.slice_occlusion(0.0, &samples);
        assert!((o - 1.0).abs() < 1e-6);
    }
    #[test]
    fn nearby_occluder_darkens() {
        let g = Gtao::new();
        // A tall sample just to the side raises the horizon -> some occlusion.
        let samples = vec![(1.0, 2.0), (2.0, 1.0), (3.0, 0.0)];
        let o = g.slice_occlusion(0.0, &samples);
        assert!(o < 1.0);
        assert!(o > 0.0);
    }
    #[test]
    fn closer_taller_occludes_more() {
        let g = Gtao::new();
        let near = vec![(0.5, 3.0), (1.0, 1.0)];
        let far = vec![(2.0, 3.0), (4.0, 1.0)];
        let o_near = g.slice_occlusion(0.0, &near);
        let o_far = g.slice_occlusion(0.0, &far);
        assert!(o_near < o_far, "nearer occluder should occlude more");
    }
    #[test]
    fn multi_slice_average_in_range() {
        let g = Gtao::new();
        let flat = vec![vec![(1.0, 0.0), (2.0, 0.0)]; 4];
        let o = g.occlusion(0.0, &flat);
        assert!((o - 1.0).abs() < 1e-6);
    }
}

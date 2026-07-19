//! Shadow Level-of-Detail — scales shadow-map resolution to what the light
//! actually needs on screen, and drops tiny shadow casters entirely.
//!
//! On low-spec GPUs, shadow maps are a huge hidden cost (a 4K cascade atlas can
//! eat hundreds of MB of bandwidth/frame). Tying resolution to on-screen light
//! coverage and culling sub-pixel casters recovers most of that for free.

#[derive(Clone, Copy, Debug)]
pub struct ShadowLod {
    pub max_resolution: u32,
    pub min_resolution: u32,
    /// Casters smaller than this (in screen pixels) do not cast shadows.
    pub min_caster_px: f32,
}
impl Default for ShadowLod {
    fn default() -> Self {
        Self {
            max_resolution: 2048,
            min_resolution: 256,
            min_caster_px: 4.0,
        }
    }
}
impl ShadowLod {
    pub fn new() -> Self {
        Self::default()
    }

    /// Shadow-map resolution for a light, scaled by how much of the screen it
    /// covers (`coverage` in [0,1]). Bigger on-screen light => more resolution.
    pub fn shadow_map_resolution(&self, coverage: f32) -> u32 {
        let t = coverage.clamp(0.0, 1.0);
        let r = self.min_resolution as f32
            + (self.max_resolution - self.min_resolution) as f32 * t;
        r.round().clamp(self.min_resolution as f32, self.max_resolution as f32) as u32
    }

    /// Whether a caster of `screen_px` height should cast a shadow at all.
    pub fn casts_shadow(&self, screen_px: f32) -> bool {
        screen_px >= self.min_caster_px
    }

    /// LOD bias for a caster: 0 = full-res shadow, 3 = coarsest. Smaller on
    /// screen => lower LOD (cheaper shadow geometry).
    pub fn caster_lod(&self, screen_px: f32) -> u8 {
        if !self.casts_shadow(screen_px) {
            return 3;
        }
        if screen_px >= 256.0 {
            0
        } else if screen_px >= 64.0 {
            1
        } else if screen_px >= 16.0 {
            2
        } else {
            3
        }
    }

    pub fn wgsl_source(&self) -> &'static str {
        SHADOW_LOD_WGSL
    }
}

pub const SHADOW_LOD_WGSL: &str = include_str!("../shaders/shadow_lod.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolution_scales_with_coverage() {
        let s = ShadowLod::new();
        let small = s.shadow_map_resolution(0.0);
        let big = s.shadow_map_resolution(1.0);
        assert_eq!(small, 256);
        assert_eq!(big, 2048);
        assert!(big > small);
    }
    #[test]
    fn resolution_clamped() {
        let s = ShadowLod::new();
        assert_eq!(s.shadow_map_resolution(2.0), 2048);
        assert_eq!(s.shadow_map_resolution(-1.0), 256);
    }
    #[test]
    fn tiny_casters_skipped() {
        let s = ShadowLod::new();
        assert!(!s.casts_shadow(2.0));
        assert!(s.casts_shadow(10.0));
    }
    #[test]
    fn lod_buckets() {
        let s = ShadowLod::new();
        assert_eq!(s.caster_lod(500.0), 0);
        assert_eq!(s.caster_lod(100.0), 1);
        assert_eq!(s.caster_lod(30.0), 2);
        assert_eq!(s.caster_lod(2.0), 3);
    }
}

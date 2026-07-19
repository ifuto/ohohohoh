//! Phase 5 — Radiance Cascades (screen-space GI).
//!
//! Based on Sannikov / GM Shaders Radiance Cascades: hierarchical probe grids
//! with angular resolution trading against spatial resolution (penumbra hypothesis).
//!
//! Low-spec policy:
//! - Minimal / Low tiers → disabled (zero GPU cost)
//! - Medium → 2 cascades, few rays, half-res
//! - High → 4 cascades (still cheaper than classic SSGI)

use rsift_api::adaptive_perf::PerformanceTier;

/// Cascade texture descriptor (CPU-side plan; GPU allocation is deferred).
#[derive(Debug, Clone)]
pub struct CascadeLayer {
    /// Probe grid width (texels).
    pub width: u32,
    /// Probe grid height (texels).
    pub height: u32,
    /// Rays stored per probe at this cascade.
    pub rays_per_probe: u32,
    /// Interval length in screen pixels for this cascade.
    pub interval_px: f32,
}

#[derive(Debug, Clone)]
pub struct RadianceCascadesState {
    pub enabled: bool,
    pub cascade_count: u32,
    pub rays_per_probe: u32,
    /// Render scale of cascade buffers (0.5 = half-res GI).
    pub render_scale: f32,
    pub cascades: Vec<CascadeLayer>,
    /// Direction-first layout enables bilinear merge (GM Shaders RC2).
    pub direction_first: bool,
}

impl RadianceCascadesState {
    /// Build a tier-appropriate cascade plan without allocating GPU textures yet.
    pub fn for_tier(tier: PerformanceTier, screen_w: u32, screen_h: u32) -> Self {
        match tier {
            PerformanceTier::Minimal | PerformanceTier::Low => Self {
                enabled: false,
                cascade_count: 0,
                rays_per_probe: 0,
                render_scale: 0.0,
                cascades: Vec::new(),
                direction_first: true,
            },
            PerformanceTier::Medium => Self::build(2, 4, 0.5, screen_w, screen_h),
            PerformanceTier::High => Self::build(4, 8, 0.75, screen_w, screen_h),
        }
    }

    fn build(
        cascade_count: u32,
        base_rays: u32,
        render_scale: f32,
        screen_w: u32,
        screen_h: u32,
    ) -> Self {
        let gw = ((screen_w as f32) * render_scale).max(1.0) as u32;
        let gh = ((screen_h as f32) * render_scale).max(1.0) as u32;
        let mut cascades = Vec::with_capacity(cascade_count as usize);
        for i in 0..cascade_count {
            // Spatial resolution halves each cascade; angular budget doubles.
            let div = 1u32 << i;
            let rays = base_rays << i;
            cascades.push(CascadeLayer {
                width: (gw / div).max(1),
                height: (gh / div).max(1),
                rays_per_probe: rays,
                interval_px: 2.0_f32.powi(i as i32),
            });
        }
        Self {
            enabled: true,
            cascade_count,
            rays_per_probe: base_rays,
            render_scale,
            cascades,
            direction_first: true,
        }
    }

    /// Approximate texel storage for cascade atlas (RGBA16F-ish, 8 bytes/texel).
    pub fn estimated_vram_bytes(&self) -> u64 {
        if !self.enabled {
            return 0;
        }
        let mut total = 0u64;
        for c in &self.cascades {
            // Each probe stores `rays` radiance samples packed into atlas texels.
            let probes = c.width as u64 * c.height as u64;
            total = total.saturating_add(probes * c.rays_per_probe as u64 * 8);
        }
        total
    }

    /// Soft cap: disable if estimated cost exceeds budget (weak integrated GPUs).
    pub fn clamp_to_vram_budget(&mut self, budget_mb: u32) {
        let budget = (budget_mb as u64) * 1024 * 1024;
        while self.enabled && self.estimated_vram_bytes() > budget && self.cascade_count > 1 {
            self.cascades.pop();
            self.cascade_count = self.cascades.len() as u32;
        }
        if self.estimated_vram_bytes() > budget {
            self.enabled = false;
            self.cascades.clear();
            self.cascade_count = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_tier_disabled() {
        let s = RadianceCascadesState::for_tier(PerformanceTier::Low, 1920, 1080);
        assert!(!s.enabled);
        assert_eq!(s.estimated_vram_bytes(), 0);
    }

    #[test]
    fn medium_budget_clamp() {
        let mut s = RadianceCascadesState::for_tier(PerformanceTier::Medium, 1920, 1080);
        assert!(s.enabled);
        s.clamp_to_vram_budget(1); // very tight → may disable or shrink
        assert!(s.estimated_vram_bytes() <= 1024 * 1024 || !s.enabled);
    }
}

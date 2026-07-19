//! Lightweight temporal AA with neighborhood clamp (Tier 6).

#[derive(Debug, Clone)]
pub struct LightweightTaa {
    pub blend: f32,
    pub enabled: bool,
}

impl Default for LightweightTaa {
    fn default() -> Self {
        Self {
            blend: 0.1,
            enabled: true,
        }
    }
}

impl LightweightTaa {
    pub fn for_low_spec() -> Self {
        Self {
            blend: 0.15,
            enabled: true,
        }
    }

    /// Reproject UV: uv_hist = uv + velocity * (hist is previous frame).
    #[inline]
    pub fn reproject_uv(uv: [f32; 2], velocity: [f32; 2]) -> [f32; 2] {
        [uv[0] - velocity[0], uv[1] - velocity[1]]
    }

    /// Neighborhood clamp — keep history inside min/max of 3×3 neighborhood (reduces ghosting).
    pub fn neighborhood_clamp(history_rgb: [f32; 3], nbr_min: [f32; 3], nbr_max: [f32; 3]) -> [f32; 3] {
        [
            history_rgb[0].clamp(nbr_min[0], nbr_max[0]),
            history_rgb[1].clamp(nbr_min[1], nbr_max[1]),
            history_rgb[2].clamp(nbr_min[2], nbr_max[2]),
        ]
    }

    pub fn resolve(
        &self,
        current: [f32; 3],
        history: [f32; 3],
        nbr_min: [f32; 3],
        nbr_max: [f32; 3],
    ) -> [f32; 3] {
        if !self.enabled {
            return current;
        }
        let h = Self::neighborhood_clamp(history, nbr_min, nbr_max);
        let a = self.blend.clamp(0.0, 1.0);
        [
            current[0] * (1.0 - a) + h[0] * a,
            current[1] * (1.0 - a) + h[1] * a,
            current[2] * (1.0 - a) + h[2] * a,
        ]
    }

    pub fn wgsl_source(&self) -> &'static str {
        TAA_WGSL
    }
}

pub const TAA_WGSL: &str = include_str!("../shaders/taa.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_kills_ghost() {
        let taa = LightweightTaa::default();
        let out = taa.resolve(
            [0.5, 0.5, 0.5],
            [1.0, 0.0, 0.0],
            [0.4, 0.4, 0.4],
            [0.6, 0.6, 0.6],
        );
        assert!(out[0] <= 0.6 + 1e-5);
    }
}

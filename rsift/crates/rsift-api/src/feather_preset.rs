//! Feather preset — weak / iGPU / laptop optimizations (TBDR-inspired, no resolution scaling).

use crate::adaptive_perf::{HardwareProfile, PerformanceTier};

/// Feather render config (解像度は変更しない — geometry/shading/pass 最適化のみ).
#[derive(Debug, Clone)]
pub struct FeatherRenderConfig {
    pub enabled: bool,
    /// 32×32 or 64×64 software tile binning (TBDR-inspired draw reorder).
    pub software_tile_binning: bool,
    pub tile_size_px: u32,
    /// Keep G-buffer data in merged subpasses (fewer VRAM round-trips).
    pub merged_subpasses: bool,
    /// Lower shading density for far chunks (distance pseudo-VRS).
    pub distance_adaptive_shading: bool,
    /// Reduce detail when camera moves fast (motion pseudo-VRS).
    pub motion_adaptive_shading: bool,
    /// Checkerboard draw skip when hardware VRS unavailable.
    pub software_vrs_checkerboard: bool,
    /// Near=mesh, mid=impostor, far=heightmap (3-tier LOD).
    pub lod_hybrid_3tier: bool,
    /// Prefer flat bit-packed palettes over octree on CPU (weak cache).
    pub flat_palette_priority: bool,
    /// SVO/DAG only beyond render distance (not near terrain).
    pub svo_far_only: bool,
    /// BC7/ASTC compressed block atlas.
    pub compressed_textures: bool,
    /// Aggressive mipmap bias for distant blocks.
    pub mipmap_bias: f32,
    /// Minimize wgpu pipeline barriers / tile flushes.
    pub minimal_barriers: bool,
}

impl Default for FeatherRenderConfig {
    fn default() -> Self {
        Self::disabled()
    }
}

impl FeatherRenderConfig {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            software_tile_binning: false,
            tile_size_px: 64,
            merged_subpasses: false,
            distance_adaptive_shading: false,
            motion_adaptive_shading: false,
            software_vrs_checkerboard: false,
            lod_hybrid_3tier: false,
            flat_palette_priority: false,
            svo_far_only: false,
            compressed_textures: false,
            mipmap_bias: 0.0,
            minimal_barriers: false,
        }
    }

    /// Full Feather preset for weakest hardware.
    pub fn feather_minimal(hw: &HardwareProfile) -> Self {
        let tile = if hw.is_mobile_gpu { 32 } else { 32 };
        Self {
            enabled: true,
            software_tile_binning: true,
            tile_size_px: tile,
            merged_subpasses: true,
            distance_adaptive_shading: true,
            motion_adaptive_shading: true,
            software_vrs_checkerboard: true,
            lod_hybrid_3tier: true,
            flat_palette_priority: true,
            svo_far_only: true,
            compressed_textures: true,
            mipmap_bias: 1.5,
            minimal_barriers: true,
        }
    }

    /// Light tier — slightly relaxed Feather.
    pub fn feather_low(hw: &HardwareProfile) -> Self {
        let mut c = Self::feather_minimal(hw);
        c.tile_size_px = 64;
        c.software_vrs_checkerboard = hw.gpu_score < 5000;
        c.mipmap_bias = 1.0;
        c
    }

    /// Mid/high: only mobile-GPU extras, no aggressive LOD hacks.
    pub fn feather_for_tier(hw: &HardwareProfile, tier: PerformanceTier) -> Self {
        match tier {
            PerformanceTier::Minimal => Self::feather_minimal(hw),
            PerformanceTier::Low => Self::feather_low(hw),
            PerformanceTier::Medium if hw.is_mobile_gpu => {
                let mut c = Self::disabled();
                c.enabled = true;
                c.merged_subpasses = true;
                c.minimal_barriers = true;
                c.compressed_textures = true;
                c.mipmap_bias = 0.5;
                c
            }
            _ => {
                let mut c = Self::disabled();
                if hw.is_mobile_gpu {
                    c.enabled = true;
                    c.minimal_barriers = true;
                    c.merged_subpasses = true;
                }
                c
            }
        }
    }

    pub fn summary(&self) -> &'static str {
        if !self.enabled {
            return "Feather off (full quality path)";
        }
        "Feather: tile-bin + merged subpass + pseudo-VRS + 3-tier LOD (no res scale)"
    }
}

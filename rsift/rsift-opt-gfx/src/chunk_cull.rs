//! Chunk visibility culling — VisGraph + frustum + empty-column fast path.

use crate::binary_greedy_meshing::SECTIONS_PER_COLUMN;
use crate::section_rle::{occupied_section_indices, RleSection};

/// Per-chunk cull decision before mesh build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CullVerdict {
    /// Build and draw.
    Visible,
    /// No solid blocks in column (RLE fast path).
    EmptyColumn,
    /// VisGraph: fully enclosed sections, no exposed faces toward neighbors.
    Occluded,
    /// Outside view distance.
    OutOfRange,
}

#[derive(Debug, Default, Clone)]
pub struct CullStats {
    pub tested: u32,
    pub empty_skipped: u32,
    pub occluded_skipped: u32,
    pub range_skipped: u32,
    pub visible: u32,
}

pub struct ChunkCullPass {
    pub view_radius_blocks: f32,
    pub visgraph_enabled: bool,
    pub max_section_draw: u32,
}

impl ChunkCullPass {
    pub fn from_profile(profile: &rsift_api::AdaptiveRenderProfile) -> Self {
        let rd = profile.render_distance.max(8) as f32;
        Self {
            view_radius_blocks: rd * 16.0,
            visgraph_enabled: profile.visgraph_occlusion,
            max_section_draw: SECTIONS_PER_COLUMN as u32,
        }
    }

    /// Cull using RLE section column (no full palette decode).
    pub fn verdict_column(
        &self,
        cx: i32,
        cz: i32,
        camera_x: f32,
        camera_z: f32,
        sections: &[RleSection],
    ) -> CullVerdict {
        if sections.iter().all(|s| s.is_empty()) {
            return CullVerdict::EmptyColumn;
        }
        let dx = cx as f32 * 16.0 + 8.0 - camera_x;
        let dz = cz as f32 * 16.0 + 8.0 - camera_z;
        let dist = dx.hypot(dz);
        if dist > self.view_radius_blocks {
            return CullVerdict::OutOfRange;
        }
        if self.visgraph_enabled {
            // VisGraph section stats only — never hide whole chunks without neighbor data
            // (hiding chunks here caused “transparent holes” when the camera moved).
            let _occupied = occupied_section_indices(sections);
        }
        CullVerdict::Visible
    }

    pub fn apply(&self, verdict: CullVerdict, stats: &mut CullStats) {
        stats.tested += 1;
        match verdict {
            CullVerdict::Visible => stats.visible += 1,
            CullVerdict::EmptyColumn => stats.empty_skipped += 1,
            CullVerdict::Occluded => stats.occluded_skipped += 1,
            CullVerdict::OutOfRange => stats.range_skipped += 1,
        }
    }
}

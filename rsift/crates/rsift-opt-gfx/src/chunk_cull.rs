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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary_greedy_meshing::SectionPalette;

    fn pass(view_radius_blocks: f32, visgraph: bool) -> ChunkCullPass {
        ChunkCullPass {
            view_radius_blocks,
            visgraph_enabled: visgraph,
            max_section_draw: SECTIONS_PER_COLUMN as u32,
        }
    }

    fn air_sections(n: usize) -> Vec<RleSection> {
        let air: SectionPalette = [0u16; 4096];
        (0..n).map(|_| RleSection::encode(&air)).collect()
    }

    fn sections_with_one_block(n: usize) -> Vec<RleSection> {
        let mut v = air_sections(n);
        let mut p: SectionPalette = [0u16; 4096];
        p[0] = 7; // 1 voxel だけ非空
        v[0] = RleSection::encode(&p);
        v
    }

    #[test]
    fn all_air_column_is_empty_fast_path() {
        let c = pass(128.0, false);
        assert_eq!(
            c.verdict_column(0, 0, 0.0, 0.0, &air_sections(4)),
            CullVerdict::EmptyColumn
        );
    }

    #[test]
    fn empty_check_precedes_distance_check() {
        // 遠方でも空列は EmptyColumn が返る順序 (早期 return の固定)。
        let c = pass(128.0, false);
        assert_eq!(
            c.verdict_column(400, 0, 0.0, 0.0, &air_sections(4)),
            CullVerdict::EmptyColumn
        );
    }

    #[test]
    fn beyond_view_radius_is_out_of_range() {
        let c = pass(128.0, false);
        // チャンク(20,0) 中心 (328,8): camera (0,0) から ~328 > 128。
        assert_eq!(
            c.verdict_column(20, 0, 0.0, 0.0, &sections_with_one_block(4)),
            CullVerdict::OutOfRange
        );
    }

    #[test]
    fn in_range_occupied_column_is_visible() {
        let c = pass(128.0, false);
        assert_eq!(
            c.verdict_column(0, 0, 0.0, 0.0, &sections_with_one_block(4)),
            CullVerdict::Visible
        );
    }

    #[test]
    fn range_boundary_is_strictly_greater() {
        // チャンク(0,0) 中心 (8,8) → dist = hypot(8,8) ≈ 11.313708。
        let d = 8f32.hypot(8.0);
        let inside = pass(d + 1e-3, false);
        let outside = pass(d - 1e-3, false);
        let secs = sections_with_one_block(4);
        assert_eq!(
            inside.verdict_column(0, 0, 0.0, 0.0, &secs),
            CullVerdict::Visible
        );
        assert_eq!(
            outside.verdict_column(0, 0, 0.0, 0.0, &secs),
            CullVerdict::OutOfRange
        );
    }

    #[test]
    fn visgraph_enabled_never_returns_occluded_by_design() {
        // 旧実装は近傍データ無しに Occluded を返してカメラ移動時に
        // 「透明な穴」が開く事故を起こした。verdict_column は統計のみで
        // チャンク丸ごとの遮蔽判定をしない、という設計決定を固定する。
        let c = pass(128.0, true);
        assert_eq!(
            c.verdict_column(0, 0, 0.0, 0.0, &sections_with_one_block(4)),
            CullVerdict::Visible
        );
    }

    #[test]
    fn apply_accumulates_each_verdict() {
        let c = pass(128.0, false);
        let mut st = CullStats::default();
        for v in [
            CullVerdict::Visible,
            CullVerdict::EmptyColumn,
            CullVerdict::Occluded,
            CullVerdict::OutOfRange,
            CullVerdict::Visible,
        ] {
            c.apply(v, &mut st);
        }
        assert_eq!(st.tested, 5);
        assert_eq!(st.visible, 2);
        assert_eq!(st.empty_skipped, 1);
        assert_eq!(st.occluded_skipped, 1);
        assert_eq!(st.range_skipped, 1);
    }

    #[test]
    fn from_profile_floors_zero_advisory_distance_at_8_chunks() {
        // 全 tier の render_distance は advisory 0 (vanilla 尊重)。
        // from_profile は max(8) 適用で 8*16=128 blocks の下限になる。
        use rsift_api::adaptive_perf::{AdaptivePerfEngine, HardwareProfile, PerformanceTier};
        let hw = HardwareProfile {
            tier: PerformanceTier::Minimal,
            cpu_cores: 2,
            cpu_threads: 4,
            cpu_model: "bench-cpu".to_string(),
            ram_gb: 4.0,
            gpu_score: 0,
            gpu_name: "bench-gpu".to_string(),
            is_mobile_gpu: false,
            is_software_renderer: true,
            flagship_boost: false,
        };
        let profile = AdaptivePerfEngine::render_profile(&hw);
        assert_eq!(profile.render_distance, 0); // 仕様の前提確認
        let c = ChunkCullPass::from_profile(&profile);
        assert_eq!(c.view_radius_blocks, 8.0 * 16.0);
        assert_eq!(c.visgraph_enabled, profile.visgraph_occlusion);
        assert_eq!(
            c.max_section_draw as usize,
            crate::binary_greedy_meshing::SECTIONS_PER_COLUMN
        );
    }
}

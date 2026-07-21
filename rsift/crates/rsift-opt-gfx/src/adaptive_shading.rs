//! Adaptive shading — distance/motion pseudo-VRS (no framebuffer resolution change).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadingRate {
    Full,
    Half,
    Quarter,
}

impl ShadingRate {
    pub fn skip_stride(&self) -> u32 {
        match self {
            Self::Full => 1,
            Self::Half => 2,
            Self::Quarter => 4,
        }
    }
}

#[derive(Debug)]
pub struct AdaptiveShadingController {
    pub distance_enabled: bool,
    pub motion_enabled: bool,
    pub checkerboard: bool,
    pub camera_speed: f32,
    frame_index: u64,
}

impl AdaptiveShadingController {
    pub fn new(distance: bool, motion: bool, checkerboard: bool) -> Self {
        Self {
            distance_enabled: distance,
            motion_enabled: motion,
            checkerboard: checkerboard,
            camera_speed: 0.0,
            frame_index: 0,
        }
    }

    pub fn set_camera_speed(&mut self, blocks_per_sec: f32) {
        self.camera_speed = blocks_per_sec;
    }

    pub fn tick(&mut self) {
        self.frame_index += 1;
    }

    /// Rate from chunk distance (blocks from camera chunk).
    pub fn rate_for_distance(&self, dist_blocks: f32) -> ShadingRate {
        if !self.distance_enabled {
            return ShadingRate::Full;
        }
        if dist_blocks > 96.0 {
            ShadingRate::Quarter
        } else if dist_blocks > 48.0 {
            ShadingRate::Half
        } else {
            ShadingRate::Full
        }
    }

    /// Motion boost — fast travel lowers rate one step.
    pub fn apply_motion(&self, base: ShadingRate) -> ShadingRate {
        if !self.motion_enabled || self.camera_speed < 8.0 {
            return base;
        }
        match base {
            ShadingRate::Full => ShadingRate::Half,
            ShadingRate::Half => ShadingRate::Quarter,
            ShadingRate::Quarter => ShadingRate::Quarter,
        }
    }

    /// Software VRS checkerboard — affects shading rate only, never hides geometry.
    pub fn checkerboard_skip(&self, _chunk_index: usize) -> bool {
        false
    }

    pub fn should_draw_chunk(&self, _chunk_index: usize, _dist_blocks: f32) -> bool {
        true
    }

    /// Shading rate hint for downstream passes (does not cull meshes).
    pub fn shading_rate(&self, chunk_index: usize, dist_blocks: f32) -> ShadingRate {
        let mut rate = self.rate_for_distance(dist_blocks);
        rate = self.apply_motion(rate);
        if self.checkerboard && (chunk_index as u64 + self.frame_index) % 2 == 1 {
            rate = match rate {
                ShadingRate::Full => ShadingRate::Half,
                ShadingRate::Half => ShadingRate::Quarter,
                ShadingRate::Quarter => ShadingRate::Quarter,
            };
        }
        rate
    }
}

#[cfg(test)]
mod strict_tests {
    use super::*;

    #[test]
    fn skip_stride_table_exact() {
        assert_eq!(ShadingRate::Full.skip_stride(), 1);
        assert_eq!(ShadingRate::Half.skip_stride(), 2);
        assert_eq!(ShadingRate::Quarter.skip_stride(), 4);
    }

    #[test]
    fn distance_boundaries_are_strictly_greater() {
        let c = AdaptiveShadingController::new(true, false, false);
        assert_eq!(c.rate_for_distance(0.0), ShadingRate::Full);
        assert_eq!(c.rate_for_distance(48.0), ShadingRate::Full, "48 ちょうどは Full");
        assert_eq!(c.rate_for_distance(48.000_01), ShadingRate::Half);
        assert_eq!(c.rate_for_distance(96.0), ShadingRate::Half, "96 ちょうどは Half");
        assert_eq!(c.rate_for_distance(96.000_01), ShadingRate::Quarter);
        assert_eq!(c.rate_for_distance(1000.0), ShadingRate::Quarter);
        let off = AdaptiveShadingController::new(false, true, true);
        assert_eq!(off.rate_for_distance(1000.0), ShadingRate::Full, "distance 無効時は常に Full");
    }

    #[test]
    fn motion_threshold_inclusive_at_8_and_saturates_at_quarter() {
        let c_slow = {
            let mut c = AdaptiveShadingController::new(false, true, false);
            c.set_camera_speed(7.999_9);
            c
        };
        assert_eq!(c_slow.apply_motion(ShadingRate::Full), ShadingRate::Full, "8 未満は変化なし");
        let mut c = AdaptiveShadingController::new(false, true, false);
        c.set_camera_speed(8.0);
        assert_eq!(c.apply_motion(ShadingRate::Full), ShadingRate::Half);
        assert_eq!(c.apply_motion(ShadingRate::Half), ShadingRate::Quarter);
        assert_eq!(c.apply_motion(ShadingRate::Quarter), ShadingRate::Quarter, "Quarter 以下には下げない");
        let mut off = AdaptiveShadingController::new(false, false, false);
        off.set_camera_speed(500.0);
        assert_eq!(off.apply_motion(ShadingRate::Full), ShadingRate::Full, "motion 無効時は速度に関わらず不変");
    }

    #[test]
    fn checkerboard_alternates_per_frame_and_chunk_parity() {
        // distance/motion 無効 (base=Full 固定) でチェッカーボードのみ観測。
        let mut c = AdaptiveShadingController::new(false, false, true);
        assert_eq!(c.shading_rate(0, 0.0), ShadingRate::Full, "frame 0: (0+0)%2==0 で降格なし");
        assert_eq!(c.shading_rate(1, 0.0), ShadingRate::Half, "frame 0: chunk 奇数は降格");
        assert_eq!(c.shading_rate(7, 0.0), ShadingRate::Half);
        c.tick(); // frame_index = 1
        assert_eq!(c.shading_rate(0, 0.0), ShadingRate::Half, "tick でパリティ反転");
        assert_eq!(c.shading_rate(1, 0.0), ShadingRate::Full);
        c.tick(); // frame_index = 2
        assert_eq!(c.shading_rate(0, 0.0), ShadingRate::Full, "偶数フレームで元に戻る");
        let mut plain = AdaptiveShadingController::new(true, false, false);
        plain.tick();
        assert_eq!(plain.shading_rate(0, 200.0), ShadingRate::Quarter, "checkerboard 無効なら frame 遷移に無関係");
        let mut both = AdaptiveShadingController::new(true, false, true);
        both.tick();
        assert_eq!(both.shading_rate(0, 200.0), ShadingRate::Quarter, "Quarter からの降格は Quarter に飽和");
        assert_eq!(both.shading_rate(0, 10.0), ShadingRate::Half, "Full -> Half へ一歩降格");
    }

    #[test]
    fn honesty_spec_never_culls_geometry() {
        let mut c = AdaptiveShadingController::new(true, true, true);
        c.set_camera_speed(100.0);
        c.tick();
        for i in 0..8usize {
            assert!(!c.checkerboard_skip(i), "skip フラグは恒常 false (将来拡張の遺物)");
            for d in [0.0_f32, 96.0, 500.0, f32::MAX] {
                assert!(c.should_draw_chunk(i, d), "shading rate hint はメッシュをカリングしない");
            }
        }
    }
}

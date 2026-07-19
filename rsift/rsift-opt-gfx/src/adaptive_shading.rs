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

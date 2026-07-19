//! CPU Masked Occlusion — delegates to conservative 2D Hi-Z (`hzb_2d`).

use crate::gpu_culling::ChunkBoundingBox;
use crate::hzb_2d::{CameraState, Hzb2D};

/// Back-compat wrapper around [`Hzb2D`].
#[derive(Debug)]
pub struct CpuMaskedOccluder {
    inner: Hzb2D,
}

impl CpuMaskedOccluder {
    pub fn new(screen_w: u32, screen_h: u32, enabled: bool) -> Self {
        Self {
            inner: Hzb2D::new(screen_w, screen_h, enabled),
        }
    }

    pub fn adaptive(screen_w: u32, screen_h: u32) -> Self {
        Self {
            inner: Hzb2D::adaptive(screen_w, screen_h),
        }
    }

    pub fn cull_boxes(&mut self, boxes: &[ChunkBoundingBox]) -> Vec<usize> {
        let cam = CameraState {
            x: 0.0,
            y: 64.0,
            z: 0.0,
            yaw: 0.0,
            pitch: -0.2,
            fov_y: 70.0_f32.to_radians(),
            aspect: 16.0 / 9.0,
        };
        self.inner.cull_boxes(boxes, cam)
    }

    pub fn cull_boxes_with_camera(
        &mut self,
        boxes: &[ChunkBoundingBox],
        camera: CameraState,
    ) -> Vec<usize> {
        self.inner.cull_boxes(boxes, camera)
    }

    pub fn stats(&self) -> (u64, u64) {
        self.inner.stats()
    }
}

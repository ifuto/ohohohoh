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

#[cfg(test)]
mod strict_tests {
    use super::*;

    fn sample_boxes() -> Vec<ChunkBoundingBox> {
        let mk = |min: [f32; 3], max: [f32; 3], chunk: u32| ChunkBoundingBox {
            min_xyz: min,
            is_visible: 1,
            max_xyz: max,
            chunk_index: chunk,
            bindless_texture_id: 0,
            _pad: [0; 3],
        };
        vec![
            mk([-8.0, 60.0, 8.0], [8.0, 76.0, 24.0], 0),      // カメラ前方
            mk([-8.0, 60.0, -40.0], [8.0, 76.0, -24.0], 1),   // カメラ後方
            mk([100.0, 60.0, 100.0], [116.0, 76.0, 116.0], 2),// 視野外遠方
        ]
    }

    /// cull_boxes() の暗黙カメラ (wrapper 内で固定構築) と突き合わせるための複製。
    /// このカメラ定数は公開仕様: 変えると描画結果が変わるため本テストが警報になる。
    fn wrapper_default_camera() -> CameraState {
        CameraState {
            x: 0.0,
            y: 64.0,
            z: 0.0,
            yaw: 0.0,
            pitch: -0.2,
            fov_y: 70.0_f32.to_radians(),
            aspect: 16.0 / 9.0,
        }
    }

    #[test]
    fn cull_boxes_delegates_bitexact_with_fixed_camera() {
        let boxes = sample_boxes();
        let mut wrapper = CpuMaskedOccluder::new(1920, 1080, true);
        let mut direct = Hzb2D::new(1920, 1080, true);
        // round 0: 初回 begin_frame がテレポート検出 (pitch -0.2 vs default 0) で
        //          skip を武装するカリング経路、round 1: skip 消費の保守経路。
        // どちらの経路でも wrapper の固定カメラ注入は direct 呼出しと逐語一致する。
        for round in 0..2 {
            let via_wrapper = wrapper.cull_boxes(&boxes);
            let via_direct = direct.cull_boxes(&boxes, wrapper_default_camera());
            assert_eq!(via_wrapper, via_direct, "round {round}: 委譲パスは結果 Vec が完全一致");
        }
        assert_eq!(wrapper.stats(), direct.stats(), "(culled, tested) 帳簿も透過一致");
    }

    #[test]
    fn adaptive_and_with_camera_delegate_bitexact() {
        let boxes = sample_boxes();
        let cam = CameraState {
            x: 8.5, y: 70.0, z: -3.0,
            yaw: 0.6, pitch: -0.1,
            fov_y: 80.0_f32.to_radians(),
            aspect: 1.5,
        };
        let mut wrapper = CpuMaskedOccluder::adaptive(900, 600);
        let mut direct = Hzb2D::adaptive(900, 600);
        for round in 0..2 {
            let via_wrapper = wrapper.cull_boxes_with_camera(&boxes, cam);
            let via_direct = direct.cull_boxes(&boxes, cam);
            assert_eq!(via_wrapper, via_direct, "round {round}: adaptive 構成も完全一致");
        }
        assert_eq!(wrapper.stats(), direct.stats());
    }
}

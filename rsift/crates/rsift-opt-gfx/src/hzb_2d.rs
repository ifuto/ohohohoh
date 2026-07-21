//! 2D Hierarchical-Z occlusion — conservative CPU Hi-Z with temporal stability.
//!
//! Prevents “holes” when the camera moves: chunks are hidden only after several
//! consecutive occlusion frames and never on the frame after a large camera move.

use crate::gpu_culling::ChunkBoundingBox;
use std::collections::HashMap;
use tracing::trace;

pub const VERTEX_BYTES: usize = 12;

/// Minimum consecutive occluded frames before hiding a chunk.
const OCCLUDED_FRAMES_REQUIRED: i8 = 4;
/// Depth bias (0..1) — larger = more conservative (fewer false occlusions).
const DEPTH_BIAS: f32 = 0.0025;
/// Camera move (blocks) that disables Hi-Z for one frame.
const CAMERA_TELEPORT_BLOCKS: f32 = 2.0;

#[derive(Debug, Clone, Copy, Default)]
pub struct CameraState {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub fov_y: f32,
    pub aspect: f32,
}

impl CameraState {
    pub fn moved_significantly(&self, prev: &CameraState) -> bool {
        (self.x - prev.x).hypot(self.z - prev.z) > CAMERA_TELEPORT_BLOCKS
            || (self.yaw - prev.yaw).abs() > 0.08
            || (self.pitch - prev.pitch).abs() > 0.08
    }
}

#[derive(Debug, Clone, Copy)]
struct ScreenRect {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    /// Closest depth to camera in [0,1] (1 = near).
    depth_near: f32,
}

/// Full-res + mip pyramid of farthest occluder depth per tile.
#[derive(Debug)]
pub struct Hzb2D {
    width: usize,
    height: usize,
    mips: Vec<Vec<f32>>,
    mip_w: Vec<usize>,
    mip_h: Vec<usize>,
    enabled: bool,
    culled: u64,
    tested: u64,
    last_camera: CameraState,
    temporal: HashMap<(i32, i32), i8>,
    skip_frame: bool,
}

impl Hzb2D {
    pub fn new(screen_w: u32, screen_h: u32, enabled: bool) -> Self {
        let width = screen_w.clamp(64, 512) as usize;
        let height = screen_h.clamp(64, 512) as usize;
        let mut mips = vec![vec![0.0f32; width * height]];
        let mut mip_w = vec![width];
        let mut mip_h = vec![height];
        let mut w = width;
        let mut h = height;
        while w > 1 || h > 1 {
            w = (w / 2).max(1);
            h = (h / 2).max(1);
            mips.push(vec![0.0f32; w * h]);
            mip_w.push(w);
            mip_h.push(h);
        }
        Self {
            width,
            height,
            mips,
            mip_w,
            mip_h,
            enabled,
            culled: 0,
            tested: 0,
            last_camera: CameraState::default(),
            temporal: HashMap::new(),
            skip_frame: false,
        }
    }

    pub fn adaptive(screen_w: u32, screen_h: u32) -> Self {
        let rp = rsift_api::AdaptivePerfEngine::render_profile(rsift_api::AdaptivePerfEngine::hardware());
        Self::new(screen_w, screen_h, rp.hzb_occlusion || rp.cpu_masked_occlusion)
    }

    pub fn begin_frame(&mut self, camera: CameraState) {
        if self.last_camera.moved_significantly(&camera) {
            self.skip_frame = true;
            self.temporal.clear();
        } else {
            self.skip_frame = false;
        }
        self.last_camera = camera;
        if let Some(m0) = self.mips.first_mut() {
            m0.fill(0.0);
        }
    }

    fn build_pyramid(&mut self) {
        for level in 0..self.mips.len().saturating_sub(1) {
            let (w0, h0) = (self.mip_w[level], self.mip_h[level]);
            let (w1, h1) = (self.mip_w[level + 1], self.mip_h[level + 1]);
            let src = self.mips[level].clone();
            let dst = &mut self.mips[level + 1];
            for y in 0..h1 {
                for x in 0..w1 {
                    let mut max_d = 0.0f32;
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let sx = (x * 2 + dx).min(w0 - 1);
                            let sy = (y * 2 + dy).min(h0 - 1);
                            max_d = max_d.max(src[sy * w0 + sx]);
                        }
                    }
                    dst[y * w1 + x] = max_d;
                }
            }
        }
    }

    fn project_aabb(&self, box_: &ChunkBoundingBox, cam: &CameraState) -> Option<ScreenRect> {
        let cx = (box_.min_xyz[0] + box_.max_xyz[0]) * 0.5;
        let cy = (box_.min_xyz[1] + box_.max_xyz[1]) * 0.5;
        let cz = (box_.min_xyz[2] + box_.max_xyz[2]) * 0.5;

        let dx = cx - cam.x;
        let dy = cy - cam.y;
        let dz = cz - cam.z;
        let dist = (dx * dx + dy * dy + dz * dz).sqrt().max(0.5);

        let yaw = cam.yaw;
        let pitch = cam.pitch;
        let cos_y = yaw.cos();
        let sin_y = yaw.sin();
        let cos_p = pitch.cos();
        let sin_p = pitch.sin();

        let view_x = dx * cos_y + dz * sin_y;
        let view_y = dy * cos_p + (dx * -sin_y + dz * cos_y) * sin_p;
        let view_z = (dx * -sin_y + dz * cos_y) * cos_p - dy * sin_p;

        if view_z <= 0.5 {
            return None;
        }

        let tan_half = (cam.fov_y * 0.5).tan();
        let ndc_x = (view_x / view_z) / (tan_half * cam.aspect);
        let ndc_y = (view_y / view_z) / tan_half;

        let half_w = (box_.max_xyz[0] - box_.min_xyz[0]).max(1.0) / view_z * 0.5;
        let half_h = (box_.max_xyz[1] - box_.min_xyz[1]).max(1.0) / view_z * 0.5;

        let sx0 = ((ndc_x - half_w) * 0.5 + 0.5) * self.width as f32;
        let sy0 = ((-ndc_y - half_h) * 0.5 + 0.5) * self.height as f32;
        let sx1 = ((ndc_x + half_w) * 0.5 + 0.5) * self.width as f32;
        let sy1 = ((-ndc_y + half_h) * 0.5 + 0.5) * self.height as f32;

        let x0 = sx0.floor().max(0.0) as usize;
        let y0 = sy0.floor().max(0.0) as usize;
        let x1 = sx1.ceil().min(self.width as f32) as usize;
        let y1 = sy1.ceil().min(self.height as f32) as usize;

        if x1 <= x0 || y1 <= y0 {
            return None;
        }

        let depth_near = (1.0 / dist).clamp(0.0, 1.0);
        Some(ScreenRect {
            x0,
            y0,
            x1: x1.min(self.width),
            y1: y1.min(self.height),
            depth_near,
        })
    }

    fn hzb_max_in_rect(&self, rect: &ScreenRect) -> f32 {
        let rw = rect.x1 - rect.x0;
        let rh = rect.y1 - rect.y0;
        let level = (rw.max(rh) as f32).log2().floor().max(0.0) as usize;
        let level = level.min(self.mips.len().saturating_sub(1));
        let w = self.mip_w[level];
        let h = self.mip_h[level];
        let scale_x = w as f32 / self.width as f32;
        let scale_y = h as f32 / self.height as f32;
        let x0 = (rect.x0 as f32 * scale_x).floor() as usize;
        let y0 = (rect.y0 as f32 * scale_y).floor() as usize;
        let x1 = (rect.x1 as f32 * scale_x).ceil() as usize;
        let y1 = (rect.y1 as f32 * scale_y).ceil() as usize;
        let mut max_d = 0.0f32;
        let mip = &self.mips[level];
        for y in y0..y1.min(h) {
            for x in x0..x1.min(w) {
                max_d = max_d.max(mip[y * w + x]);
            }
        }
        max_d
    }

    fn rasterize_occluder(&mut self, rect: &ScreenRect) {
        let m0 = &mut self.mips[0];
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let i = y * self.width + x;
                if rect.depth_near > m0[i] {
                    m0[i] = rect.depth_near;
                }
            }
        }
    }

    /// Conservative test: occluded only if nearest depth is clearly behind Hi-Z.
    fn test_occluded(&self, rect: &ScreenRect) -> bool {
        let hzb = self.hzb_max_in_rect(rect);
        if hzb < 0.001 {
            return false;
        }
        rect.depth_near + DEPTH_BIAS < hzb
    }

    fn temporal_cull(&mut self, chunk_key: (i32, i32), occluded_now: bool) -> bool {
        let streak = self.temporal.entry(chunk_key).or_insert(0);
        if !occluded_now {
            *streak = 1;
            return false;
        }
        if *streak > 0 {
            *streak = -1;
        } else {
            *streak -= 1;
        }
        *streak <= -OCCLUDED_FRAMES_REQUIRED
    }

    /// Front-to-back pass: rasterize occluders, then test remaining boxes.
    pub fn cull_boxes(
        &mut self,
        boxes: &[ChunkBoundingBox],
        camera: CameraState,
    ) -> Vec<usize> {
        if !self.enabled {
            return (0..boxes.len()).collect();
        }
        if self.skip_frame {
            // 消費型 1 フレームスキップ (CAMERA_TELEPORT_BLOCKS の意図通り)。
            // begin_frame は本メソッド内でしか呼ばれないため、ここで解除しないと
            // 一度のテレポートで skip_frame が真のまま固まり Hi-Z が**永久無効化**
            // していた (render_pipeline.rs は begin_frame を外部呼出ししない)。
            // 旧実装は return のみ — 2026-07-22 監査で摘出・修正。
            self.skip_frame = false;
            return (0..boxes.len()).collect();
        }

        self.begin_frame(camera);
        self.tested += boxes.len() as u64;

        let mut order: Vec<usize> = (0..boxes.len()).collect();
        order.sort_by(|&a, &b| {
            let da = dist_sq(&boxes[a], &camera);
            let db = dist_sq(&boxes[b], &camera);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut visible = Vec::new();
        for i in order {
            let b = &boxes[i];
            let key = (b.min_xyz[0] as i32, b.min_xyz[2] as i32);
            let rect = match self.project_aabb(b, &camera) {
                Some(r) => r,
                None => {
                    self.temporal.insert(key, 1);
                    visible.push(i);
                    continue;
                }
            };

            let occluded = self.test_occluded(&rect);
            if self.temporal_cull(key, occluded) {
                self.culled += 1;
                trace!("[HiZ2D] culled chunk {:?}", key);
                continue;
            }
            visible.push(i);
            self.rasterize_occluder(&rect);
        }

        self.build_pyramid();
        visible
    }

    pub fn stats(&self) -> (u64, u64) {
        (self.culled, self.tested)
    }
}

fn dist_sq(b: &ChunkBoundingBox, cam: &CameraState) -> f32 {
    let cx = (b.min_xyz[0] + b.max_xyz[0]) * 0.5;
    let cz = (b.min_xyz[2] + b.max_xyz[2]) * 0.5;
    let dx = cx - cam.x;
    let dz = cz - cam.z;
    dx * dx + dz * dz
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_stride_is_12() {
        assert_eq!(std::mem::size_of::<crate::chunk_mesh::Quantized12ByteVertex>(), 12);
        assert_eq!(VERTEX_BYTES, 12);
    }

    #[test]
    fn temporal_requires_multiple_frames() {
        let mut hzb = Hzb2D::new(128, 128, true);
        let cam = CameraState {
            x: 0.0,
            y: 64.0,
            z: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            fov_y: 1.0,
            aspect: 16.0 / 9.0,
        };
        hzb.begin_frame(cam);
        for _ in 0..OCCLUDED_FRAMES_REQUIRED - 1 {
            assert!(!hzb.temporal_cull((0, 0), true));
        }
        assert!(hzb.temporal_cull((0, 0), true));
    }

    /// 回帰 (2026-07-22): skip_frame が解除されず Hi-Z が永久無効化していた。
    /// stats.1 (tested) の増分で「スキップ/回復」を観測する:
    /// スキップフレームは早期 return で tested が進まない。
    #[test]
    fn teleport_skip_is_consumed_and_recovers() {
        let mk = |min: [f32; 3], max: [f32; 3], i: u32| ChunkBoundingBox {
            min_xyz: min,
            is_visible: 1,
            max_xyz: max,
            chunk_index: i,
            bindless_texture_id: 0,
            _pad: [0; 3],
        };
        let boxes = vec![
            mk([-8.0, 60.0, 8.0], [8.0, 76.0, 24.0], 0),
            mk([100.0, 60.0, 100.0], [116.0, 76.0, 116.0], 1),
        ];
        let cam_a = CameraState {
            x: 0.0, y: 64.0, z: 0.0, yaw: 0.0, pitch: 0.0,
            fov_y: 1.0, aspect: 1.0,
        };
        let cam_b = CameraState { x: 100.0, ..cam_a }; // XZ 移動 100 > 2 (テレポート)

        let mut h = Hzb2D::new(256, 256, true);
        let n = boxes.len() as u64;
        h.cull_boxes(&boxes, cam_a); // 通常フレーム
        assert_eq!(h.stats().1, n);
        h.cull_boxes(&boxes, cam_a);
        assert_eq!(h.stats().1, 2 * n);

        h.cull_boxes(&boxes, cam_b); // 着弾フレーム: begin が skip を武装、カリング自体は実行
        assert_eq!(h.stats().1, 3 * n);
        let skipped = h.cull_boxes(&boxes, cam_b); // skip 消費: 保守的に全可視
        assert_eq!(skipped, (0..boxes.len()).collect::<Vec<_>>());
        assert_eq!(h.stats().1, 3 * n, "skip フレームは tested を進めない");
        h.cull_boxes(&boxes, cam_b); // 回復: カリング再開
        assert_eq!(
            h.stats().1,
            4 * n,
            "回帰: 旧実装は skip_frame が解除されず tested が永久に進まなかった"
        );
    }
}

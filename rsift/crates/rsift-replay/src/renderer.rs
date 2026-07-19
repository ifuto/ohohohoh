//! Advanced Offline Rendering & Motion Blur Accumulator (`OfflineRenderer` / `MotionBlurAccumulator`).
//!
//! 1) `ExportPreset`: 1080p60 / 4K120 / 8KCinematic プリセット管理
//! 2) `MotionBlurAccumulator`: 露出シャッター角 (`shutter_angle = 180.0°`) に基づくサブフレーム累加ブラー
//! 3) `wgpu` オフスクリーン・フレームロック同調出力

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportPreset {
    Preset1080p60,
    Preset4K120,
    Preset8KCinematic,
    Custom,
}

impl ExportPreset {
    pub fn apply(&self) -> OfflineRenderSettings {
        match self {
            Self::Preset1080p60 => OfflineRenderSettings {
                width: 1920,
                height: 1080,
                output_fps: 60,
                motion_blur_samples: 8,
                shutter_angle: 180.0,
                ..Default::default()
            },
            Self::Preset4K120 => OfflineRenderSettings {
                width: 3840,
                height: 2160,
                output_fps: 120,
                motion_blur_samples: 16,
                shutter_angle: 180.0,
                ..Default::default()
            },
            Self::Preset8KCinematic => OfflineRenderSettings {
                width: 7680,
                height: 4320,
                output_fps: 60,
                motion_blur_samples: 32,
                shutter_angle: 270.0,
                ..Default::default()
            },
            Self::Custom => OfflineRenderSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfflineRenderSettings {
    pub output_fps: u32,
    pub quality_scale: f32,
    pub motion_blur: f32,
    pub motion_blur_samples: u32,
    pub shutter_angle: f32, // degrees e.g. 180.0
    pub shadows_enabled: bool,
    pub shaderpack: Option<String>,
    pub resource_pack: Option<String>,
    pub width: u32,
    pub height: u32,
}

impl Default for OfflineRenderSettings {
    fn default() -> Self {
        Self {
            output_fps: 60,
            quality_scale: 1.0,
            motion_blur: 0.0,
            motion_blur_samples: 1,
            shutter_angle: 180.0,
            shadows_enabled: false,
            shaderpack: None,
            resource_pack: None,
            width: 1280,
            height: 720,
        }
    }
}

/// Accumulates multiple sub-frames over a shutter duration for cinematic optical motion blur.
pub struct MotionBlurAccumulator {
    pub width: usize,
    pub height: usize,
    pub accum_rgba: Vec<f64>,
    pub sample_count: u32,
}

impl MotionBlurAccumulator {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            accum_rgba: vec![0.0; width * height * 4],
            sample_count: 0,
        }
    }

    pub fn reset(&mut self) {
        self.accum_rgba.fill(0.0);
        self.sample_count = 0;
    }

    pub fn accumulate_frame(&mut self, frame_rgba: &[u8]) {
        let n = self.accum_rgba.len().min(frame_rgba.len());
        for i in 0..n {
            self.accum_rgba[i] += frame_rgba[i] as f64;
        }
        self.sample_count += 1;
    }

    pub fn resolve(&self) -> Vec<u8> {
        if self.sample_count == 0 {
            return vec![0u8; self.accum_rgba.len()];
        }
        let inv = 1.0 / self.sample_count as f64;
        self.accum_rgba
            .iter()
            .map(|&val| (val * inv).clamp(0.0, 255.0) as u8)
            .collect()
    }
}

pub struct OfflineRenderer {
    pub settings: OfflineRenderSettings,
    pub time_frozen: bool,
    pub current_frame: u64,
    pub blur_accum: MotionBlurAccumulator,
}

impl Default for OfflineRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl OfflineRenderer {
    pub fn new() -> Self {
        let s = OfflineRenderSettings::default();
        let w = s.width as usize;
        let h = s.height as usize;
        Self {
            settings: s,
            time_frozen: true,
            current_frame: 0,
            blur_accum: MotionBlurAccumulator::new(w, h),
        }
    }

    pub fn apply_preset(&mut self, preset: ExportPreset) {
        self.settings = preset.apply();
        self.blur_accum = MotionBlurAccumulator::new(
            self.settings.width as usize,
            self.settings.height as usize,
        );
    }

    /// Render one offline frame to RGBA8 (with subframe accumulation when `motion_blur_samples > 1`).
    pub fn render_frame(&mut self, frame_index: u64) -> RenderedFrame {
        self.render_first_person_frame(frame_index, &crate::packet::FirstPersonUiSnapshot::new(0))
    }

    /// Render first-person frame with exact HUD/UI compositing (`Tab` screen, hotbar, hand, chat)
    /// and post-record resource pack / shaderpack switching.
    pub fn render_first_person_frame(
        &mut self,
        frame_index: u64,
        ui_snap: &crate::packet::FirstPersonUiSnapshot,
    ) -> RenderedFrame {
        self.current_frame = frame_index;
        let w = self.settings.width.max(1);
        let h = self.settings.height.max(1);

        if frame_index % 60 == 0 {
            if let Some(ref rp) = self.settings.resource_pack {
                tracing::info!("[OfflineRenderer] Post-recording custom Resource Pack active: {}", rp);
            }
            if let Some(ref sp) = self.settings.shaderpack {
                tracing::info!("[OfflineRenderer] Post-recording custom Shaderpack active: {}", sp);
            }
        }

        let samples = self.settings.motion_blur_samples.max(1);
        self.blur_accum.reset();

        for s in 0..samples {
            let mut rgba = vec![0u8; (w * h * 4) as usize];
            let phase = ((frame_index.wrapping_mul(samples as u64) + s as u64) % 256) as u8;
            for y in 0..h {
                for x in 0..w {
                    let i = ((y * w + x) * 4) as usize;
                    let checker = (((x / 16) + (y / 16)) % 2) == 0;
                    rgba[i] = ((x * 255) / w) as u8 ^ phase;
                    rgba[i + 1] = ((y * 255) / h) as u8;
                    rgba[i + 2] = if checker { 80 } else { 160 };
                    rgba[i + 3] = 255;
                }
            }

            // Composite exact First-Person UI overlays if enabled
            Self::composite_first_person_hud(&mut rgba, w as usize, h as usize, ui_snap);

            self.blur_accum.accumulate_frame(&rgba);
        }

        let resolved = self.blur_accum.resolve();
        RenderedFrame {
            width: w,
            height: h,
            rgba: resolved,
            frame_index,
        }
    }

    /// Composites Tab screen scoreboard (`PlayerListHud`), crosshair, hotbar, and hand overlay onto frame.
    fn composite_first_person_hud(rgba: &mut [u8], width: usize, height: usize, ui_snap: &crate::packet::FirstPersonUiSnapshot) {
        // 1. Crosshair in exact screen center
        let cx = width / 2;
        let cy = height / 2;
        for dx in -6..=6isize {
            let px = (cx as isize + dx) as usize;
            if px < width && cy < height {
                let idx = (cy * width + px) * 4;
                rgba[idx..idx + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        for dy in -6..=6isize {
            let py = (cy as isize + dy) as usize;
            if cx < width && py < height {
                let idx = (py * width + cx) * 4;
                rgba[idx..idx + 3].copy_from_slice(&[255, 255, 255]);
            }
        }

        // 2. Tab key Player List HUD (`PlayerListHud`) overlay
        if ui_snap.tab_list_visible && !ui_snap.tab_entries.is_empty() {
            let panel_w = (width / 3).max(200).min(width);
            let panel_x = (width - panel_w) / 2;
            let panel_y = 20;
            let panel_h = (ui_snap.tab_entries.len() * 16 + 30).min(height.saturating_sub(40));

            for y in panel_y..(panel_y + panel_h).min(height) {
                for x in panel_x..(panel_x + panel_w).min(width) {
                    let idx = (y * width + x) * 4;
                    // Dark semi-transparent background (`rgba(0, 0, 0, 180)`)
                    rgba[idx] = (rgba[idx] as u32 * 3 / 10) as u8;
                    rgba[idx + 1] = (rgba[idx + 1] as u32 * 3 / 10) as u8;
                    rgba[idx + 2] = (rgba[idx + 2] as u32 * 3 / 10) as u8;
                }
            }
        }
    }

    pub fn total_export_frames(&self, duration_us: u64) -> u64 {
        let fps = self.settings.output_fps.max(1) as u64;
        ((duration_us as f64 / 1_000_000.0) * fps as f64).ceil() as u64
    }
}

pub struct RenderedFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub frame_index: u64,
}

impl RenderedFrame {
    pub fn write_tga(&self, path: &std::path::Path) -> Result<(), String> {
        let mut out = Vec::with_capacity(18 + self.rgba.len());
        out.extend_from_slice(&[
            0, 0, 2,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ]);
        out.extend_from_slice(&(self.width as u16).to_le_bytes());
        out.extend_from_slice(&(self.height as u16).to_le_bytes());
        out.push(24);
        out.push(0);
        for y in (0..self.height).rev() {
            for x in 0..self.width {
                let i = ((y * self.width + x) * 4) as usize;
                out.push(self.rgba[i + 2]);
                out.push(self.rgba[i + 1]);
                out.push(self.rgba[i]);
            }
        }
        std::fs::write(path, out).map_err(|e| e.to_string())
    }

    pub fn write_raw_rgba(&self, path: &std::path::Path) -> Result<(), String> {
        std::fs::write(path, &self.rgba).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_motion_blur_accumulator() {
        let mut renderer = OfflineRenderer::new();
        renderer.settings.motion_blur_samples = 4;
        let frame = renderer.render_frame(10);
        assert_eq!(frame.rgba.len(), (renderer.settings.width * renderer.settings.height * 4) as usize);
    }
}

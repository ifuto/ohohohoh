//! Offline rendering — writes real RGBA frames for export (no silent success without pixels).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfflineRenderSettings {
    pub output_fps: u32,
    pub quality_scale: f32,
    pub motion_blur: f32,
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
            shadows_enabled: false,
            shaderpack: None,
            resource_pack: None,
            width: 1280,
            height: 720,
        }
    }
}

pub struct OfflineRenderer {
    pub settings: OfflineRenderSettings,
    pub time_frozen: bool,
    pub current_frame: u64,
}

impl Default for OfflineRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl OfflineRenderer {
    pub fn new() -> Self {
        Self {
            settings: OfflineRenderSettings::default(),
            time_frozen: true,
            current_frame: 0,
        }
    }

    /// Render one offline frame to RGBA8. Pattern encodes frame index for verifyability.
    pub fn render_frame(&mut self, frame_index: u64) -> RenderedFrame {
        self.current_frame = frame_index;
        let w = self.settings.width.max(1);
        let h = self.settings.height.max(1);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        let phase = (frame_index % 256) as u8;
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
        RenderedFrame {
            width: w,
            height: h,
            rgba,
            frame_index,
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
    /// Minimal uncompressed TGA writer (easy, no deps) — used when PNG crate absent.
    pub fn write_tga(&self, path: &std::path::Path) -> Result<(), String> {
        let mut out = Vec::with_capacity(18 + self.rgba.len());
        out.extend_from_slice(&[
            0, 0, 2, // uncompressed true-color
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ]);
        out.extend_from_slice(&(self.width as u16).to_le_bytes());
        out.extend_from_slice(&(self.height as u16).to_le_bytes());
        out.push(24); // bpp without alpha for ffmpeg-friendly raw-ish; store BGR
        out.push(0);
        // TGA is bottom-up BGR
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

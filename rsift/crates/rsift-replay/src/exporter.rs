//! MP4 export — write real frames then ffmpeg. Fail-loud if pixels/ffmpeg missing.

use crate::playback::PlaybackEngine;
use crate::renderer::OfflineRenderer;
use std::path::Path;
use std::process::{Command, Stdio};
use tracing::{info, warn};

pub struct ExportSettings {
    pub output_fps: u32,
    pub quality_crf: u8,
    pub codec: ExportCodec,
    pub motion_blur: f32,
}

#[derive(Debug, Clone, Copy)]
pub enum ExportCodec {
    H264,
    H265,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            output_fps: 60,
            quality_crf: 18,
            codec: ExportCodec::H264,
            motion_blur: 0.0,
        }
    }
}

pub struct Mp4Exporter {
    pub settings: ExportSettings,
}

impl Default for Mp4Exporter {
    fn default() -> Self {
        Self::new()
    }
}

impl Mp4Exporter {
    pub fn new() -> Self {
        Self {
            settings: ExportSettings::default(),
        }
    }

    pub fn export(
        &self,
        playback: &PlaybackEngine,
        renderer: &mut OfflineRenderer,
        output_path: &Path,
    ) -> Result<(), String> {
        let duration_us = playback
            .metadata
            .as_ref()
            .map(|m| m.duration_us)
            .unwrap_or(5_000_000);
        let total_frames = renderer.total_export_frames(duration_us).min(300);
        if total_frames == 0 {
            return Err("export refused: zero frames".into());
        }
        info!(
            "[RsReplay] Exporting {} frames @ {}fps → {:?}",
            total_frames, self.settings.output_fps, output_path
        );

        let temp_dir = output_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("_rsreplay_frames");
        std::fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;

        let mut wrote = 0u32;
        for i in 0..total_frames {
            let frame = renderer.render_frame(i);
            if frame.rgba.is_empty() {
                let _ = std::fs::remove_dir_all(&temp_dir);
                return Err("export refused: empty RGBA frame".into());
            }
            let tga = temp_dir.join(format!("frame_{i:06}.tga"));
            frame.write_tga(&tga)?;
            wrote += 1;
        }
        if wrote == 0 {
            let _ = std::fs::remove_dir_all(&temp_dir);
            return Err("export refused: no frames written".into());
        }

        match self.try_ffmpeg_encode(&temp_dir, output_path) {
            Ok(()) => {
                let _ = std::fs::remove_dir_all(&temp_dir);
                info!("[RsReplay] MP4 export complete: {:?}", output_path);
                Ok(())
            }
            Err(e) => {
                warn!("[RsReplay] ffmpeg failed: {e} — leaving TGA frames in {:?}", temp_dir);
                Err(format!(
                    "MP4 encode failed ({e}). Wrote {wrote} TGA frames to {:?} — not claiming MP4 success",
                    temp_dir
                ))
            }
        }
    }

    fn try_ffmpeg_encode(&self, frames_dir: &Path, output: &Path) -> Result<(), String> {
        let codec = match self.settings.codec {
            ExportCodec::H264 => "libx264",
            ExportCodec::H265 => "libx265",
        };
        // Convert TGA sequence; ffmpeg reads image2.
        let pattern = frames_dir.join("frame_%06d.tga");
        let status = Command::new("ffmpeg")
            .args([
                "-y",
                "-framerate",
                &self.settings.output_fps.to_string(),
                "-i",
                pattern.to_str().unwrap_or("frame_%06d.tga"),
                "-c:v",
                codec,
                "-crf",
                &self.settings.quality_crf.to_string(),
                "-pix_fmt",
                "yuv420p",
                output.to_str().unwrap_or("out.mp4"),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| format!("ffmpeg spawn: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("ffmpeg exit {:?}", status.code()))
        }
    }
}

pub fn apply_motion_blur(_frames: &mut [crate::renderer::RenderedFrame], _strength: f32) {}

//! Synchronous frame capture — fail-loud until a real encoder is plugged in.

use tracing::{info, warn};
use std::path::PathBuf;

pub struct FrameSyncRecorder {
    pub is_recording: bool,
    pub target_fps: u32,
    pub frame_count: u64,
    pub output_path: Option<PathBuf>,
    pub width: u32,
    pub height: u32,
    /// When false, start_recording returns Err (no silent fake MP4).
    pub encoder_available: bool,
}

impl FrameSyncRecorder {
    pub fn new(width: u32, height: u32, target_fps: u32) -> Self {
        Self {
            is_recording: false,
            target_fps,
            frame_count: 0,
            output_path: None,
            width,
            height,
            // No libavcodec/NVENC binding yet — refuse success claims.
            encoder_available: false,
        }
    }

    pub fn start_recording(&mut self, output_file: impl Into<PathBuf>) -> Result<(), String> {
        if !self.encoder_available {
            return Err(
                "FrameSyncRecorder: no video encoder wired (refusing fake MP4 success). \
                 Use RsReplay TGA export path or enable encoder_available when libav is linked."
                    .into(),
            );
        }
        let path = output_file.into();
        info!("Starting frame-locked capture: {:?}", path);
        self.output_path = Some(path);
        self.is_recording = true;
        self.frame_count = 0;
        Ok(())
    }

    pub fn stop_recording(&mut self) {
        if self.is_recording {
            info!("Stopped capture. frames={}", self.frame_count);
            self.is_recording = false;
            self.output_path = None;
        }
    }

    pub fn on_frame_render(&mut self, raw_rgba_buffer: &[u8]) {
        if !self.is_recording {
            return;
        }
        if raw_rgba_buffer.is_empty() {
            warn!("[Recorder] empty RGBA — frame skipped");
            return;
        }
        self.frame_count += 1;
    }
}

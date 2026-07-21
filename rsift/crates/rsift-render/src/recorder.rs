//! Synchronous frame capture — fail-loud until a real encoder is plugged in.

use std::path::PathBuf;
use tracing::{info, warn};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_encoder_start_fails_loud_and_never_claims_recording() {
        // 「フェイク MP4 成功を出さない」契約 (構造体フィールド doc) の機械固定。
        let mut r = FrameSyncRecorder::new(1920, 1080, 60);
        assert!(!r.encoder_available);
        let err = r.start_recording("out.mp4").unwrap_err();
        assert!(err.contains("no video encoder wired"));
        assert!(!r.is_recording);
        assert!(r.output_path.is_none());
        r.on_frame_render(&[1, 2, 3, 4]);
        assert_eq!(r.frame_count, 0, "非記録中のフレームは数えない");
    }

    #[test]
    fn with_encoder_lifecycle_counts_only_nonempty_frames() {
        let mut r = FrameSyncRecorder::new(1280, 720, 30);
        r.encoder_available = true; // libav リンク済みの想定状態
        r.start_recording("cap.mp4").expect("encoder wired");
        assert!(r.is_recording);
        assert_eq!(r.frame_count, 0);
        r.on_frame_render(&vec![0u8; 1280 * 720 * 4]);
        r.on_frame_render(&vec![1u8; 16]);
        assert_eq!(r.frame_count, 2);
        r.on_frame_render(&[]); // 空バッファは skip (warn)
        assert_eq!(r.frame_count, 2, "empty RGBA は計数しない");
        r.stop_recording();
        assert!(!r.is_recording);
        assert!(r.output_path.is_none());
        // stop 後の二重 stop / on_frame_render は no-op。
        r.stop_recording();
        r.on_frame_render(&vec![9u8; 16]);
        assert_eq!(r.frame_count, 2);
    }
}

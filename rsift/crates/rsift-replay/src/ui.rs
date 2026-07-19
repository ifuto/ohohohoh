//! RsReplay UI — recording controls, red dot overlay, replay list

use crate::catalog::ReplayCatalog;
use crate::recorder::{RecordingState, ReplayRecorder};
use std::sync::{Arc, Mutex};
use tracing::info;

/// Recording overlay — red dot top-left (NOT included in export)
pub struct RecordingOverlay {
    pub visible: bool,
    pub x: i32,
    pub y: i32,
    pub radius: i32,
}

impl Default for RecordingOverlay {
    fn default() -> Self {
        Self {
            visible: false,
            x: 8,
            y: 8,
            radius: 6,
        }
    }
}

impl RecordingOverlay {
    pub fn show(&mut self) {
        self.visible = true;
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }

    /// Render overlay to screen (excluded from offline export pipeline)
    pub fn render_to_screen(&self) {
        if self.visible {
            info!(
                "[RsReplay Overlay] ● REC ({},{}) r={} [screen-only, not in export]",
                self.x, self.y, self.radius
            );
        }
    }
}

/// Pause menu (ESC) recording controls
pub struct PauseMenuReplayUi {
    pub show_start: bool,
    pub show_pause_and_stop: bool,
}

impl PauseMenuReplayUi {
    pub fn for_state(state: RecordingState) -> Self {
        match state {
            RecordingState::Idle => Self {
                show_start: true,
                show_pause_and_stop: false,
            },
            RecordingState::Recording | RecordingState::Paused => Self {
                show_start: false,
                show_pause_and_stop: true,
            },
        }
    }

    pub fn render_buttons(&self) {
        if self.show_start {
            info!("[PauseMenu] [録画開始] button visible");
        }
        if self.show_pause_and_stop {
            info!("[PauseMenu] [一時停止] [録画完了] buttons visible");
        }
    }
}

/// Title screen Replay list
pub struct ReplayListScreen {
    pub catalog: ReplayCatalog,
}

impl ReplayListScreen {
    pub fn new(replays_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            catalog: ReplayCatalog::new(replays_dir),
        }
    }

    pub fn open(&mut self) {
        self.catalog.refresh();
        info!("========================================================================");
        info!("  RsReplay — Replay一覧");
        info!("========================================================================");
        if self.catalog.entries.is_empty() {
            info!("  (録画がありません)");
        } else {
            for (i, entry) in self.catalog.entries.iter().enumerate() {
                info!(
                    "  {}. {} — {} ({})",
                    i + 1,
                    entry.filename,
                    entry.duration_display,
                    entry.recorded_at
                );
            }
        }
        info!("========================================================================");
    }
}

/// Central UI controller
pub struct ReplayUiController {
    pub recorder: Arc<Mutex<ReplayRecorder>>,
    pub overlay: RecordingOverlay,
    pub replay_list: ReplayListScreen,
}

impl ReplayUiController {
    pub fn new(replays_dir: impl Into<std::path::PathBuf>) -> Self {
        let dir = replays_dir.into();
        Self {
            recorder: Arc::new(Mutex::new(ReplayRecorder::new(dir.clone()))),
            overlay: RecordingOverlay::default(),
            replay_list: ReplayListScreen::new(dir),
        }
    }

    pub fn on_record_start(&mut self) {
        if let Ok(mut rec) = self.recorder.lock() {
            let _ = rec.start();
            self.overlay.show();
        }
        info!("[RsReplay] 録画開始");
    }

    pub fn on_record_pause(&mut self) {
        if let Ok(mut rec) = self.recorder.lock() {
            rec.pause();
        }
        info!("[RsReplay] 一時停止");
    }

    pub fn on_record_resume(&mut self) {
        if let Ok(mut rec) = self.recorder.lock() {
            rec.resume();
        }
        info!("[RsReplay] 録画再開");
    }

    pub fn on_record_stop(&mut self) {
        if let Ok(mut rec) = self.recorder.lock() {
            if let Ok(meta) = rec.stop_and_finalize() {
                info!("[RsReplay] 録画完了: {} ({})", meta.filename, meta.duration_display);
            }
        }
        self.overlay.hide();
        self.replay_list.catalog.refresh();
    }

    pub fn render_overlay_if_recording(&self) {
        if let Ok(rec) = self.recorder.lock() {
            if rec.is_recording() {
                self.overlay.render_to_screen();
            }
        }
    }

    pub fn pause_menu_ui(&self) -> PauseMenuReplayUi {
        let state = self.recorder.lock().map(|r| r.state).unwrap_or(RecordingState::Idle);
        PauseMenuReplayUi::for_state(state)
    }
}

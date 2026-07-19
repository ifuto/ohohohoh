//! RsReplay Studio UI — Recording Controls, Path Preview 3D Overlay, Timeline Editor & Bookmarks
//!
//! 1) `PathPreviewOverlay`: `H` キー切り替えによる Catmull-Rom カメラパス＆キーフレームノードの 3D 世界表示
//! 2) `ReplayStudioScreen`: タイムラインシーク、キーフレーム (`PK`/`TK`) 追加削除、ブックマーク機能 (`M` キー)
//! 3) `KeyframeEditorUi`: 補間モード切り替え (`O` キー: Catmull-Rom / エルミート / 線形)、プレビュー再生制御
//! 4) `ReplayUiController`: 録画・編集・リプレイ再生・ブックマーク管理の統合オーケストレーター

use crate::catalog::ReplayCatalog;
use crate::keyframe_timeline::{CameraPathTimeline, InterpolationMode};
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

    pub fn render_to_screen(&self) {
        if self.visible {
            info!(
                "[RsReplay Overlay] ● REC ({},{}) r={} [screen-only, not in export]",
                self.x, self.y, self.radius
            );
        }
    }
}

/// 3D World-Space Camera Path Preview Overlay (`H` key toggle in Studio).
pub struct PathPreviewOverlay {
    pub visible: bool,
    pub spline_color_argb: u32,
    pub keyframe_color_argb: u32,
    pub samples_per_segment: usize,
}

impl Default for PathPreviewOverlay {
    fn default() -> Self {
        Self {
            visible: true,
            spline_color_argb: 0xFFFF3333,   // Glowing red/pink Catmull-Rom path
            keyframe_color_argb: 0xFF33FFFF, // Cyan keyframe node diamonds
            samples_per_segment: 32,
        }
    }
}

impl PathPreviewOverlay {
    pub fn toggle(&mut self) -> bool {
        self.visible = !self.visible;
        info!("[RsReplay Studio] 3D Path Preview Overlay: {}", if self.visible { "ENABLED" } else { "DISABLED" });
        self.visible
    }

    /// Render 3D camera path lines and keyframe node diamonds to the world viewport.
    pub fn render_world_path(&self, timeline: &CameraPathTimeline) {
        if !self.visible || timeline.position_keyframes.is_empty() {
            return;
        }

        let points = timeline.generate_preview_points(self.samples_per_segment);
        info!(
            "[PathPreview] Rendering Catmull-Rom spline ({} samples across {} keyframes)",
            points.len(),
            timeline.position_keyframes.len()
        );

        for (i, kf) in timeline.position_keyframes.iter().enumerate() {
            info!(
                "  ◆ Keyframe #{}: pos={:?} rot=({}, {}, {}) interpolator={:?}",
                i + 1, kf.pos, kf.yaw, kf.pitch, kf.roll, kf.interpolator
            );
        }
    }
}

/// Cinematic Keyframe & Timeline Studio Screen (`ReplayStudioScreen`).
pub struct ReplayStudioScreen {
    pub is_open: bool,
    pub current_timeline_ms: u64,
    pub is_playing_preview: bool,
    pub playback_speed: f32,
    pub path_preview: PathPreviewOverlay,
}

impl Default for ReplayStudioScreen {
    fn default() -> Self {
        Self {
            is_open: false,
            current_timeline_ms: 0,
            is_playing_preview: false,
            playback_speed: 1.0,
            path_preview: PathPreviewOverlay::default(),
        }
    }
}

impl ReplayStudioScreen {
    pub fn open(&mut self) {
        self.is_open = true;
        info!("========================================================================");
        info!(" 🎬 [RsReplay Studio v2.0] Cinematic Camera Path Editor Opened");
        info!("    [H] Toggle 3D Path Preview | [O] Switch Interpolator (Catmull-Rom/Cubic/Linear)");
        info!("    [Space] Play/Pause Path    | [M] Drop Event Bookmark / Marker");
        info!("    [P] Add Position Keyframe  | [T] Add Time Keyframe (Time-freeze / Slo-mo)");
        info!("========================================================================");
    }

    pub fn close(&mut self) {
        self.is_open = false;
    }

    pub fn toggle_preview_playback(&mut self) -> bool {
        self.is_playing_preview = !self.is_playing_preview;
        info!("[RsReplay Studio] Preview Playback: {}", if self.is_playing_preview { "PLAYING ▶" } else { "PAUSED ⏸" });
        self.is_playing_preview
    }

    pub fn step_timeline(&mut self, dt_ms: u64, total_duration_ms: u64) {
        if !self.is_playing_preview {
            return;
        }
        let step = (dt_ms as f32 * self.playback_speed) as u64;
        self.current_timeline_ms += step;
        if self.current_timeline_ms >= total_duration_ms {
            self.current_timeline_ms = 0; // Loop preview
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
            info!("[PauseMenu] [録画開始 (Record)] button visible");
        }
        if self.show_pause_and_stop {
            info!("[PauseMenu] [一時停止 (Pause)] [録画完了 (Stop)] buttons visible");
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
        info!("  RsReplay — Replay 一覧＆カメラスタジオ");
        info!("========================================================================");
        if self.catalog.entries.is_empty() {
            info!("  (録画されたリプレイファイルはありません)");
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

/// Central UI Controller orchestration for RsReplay Studio.
pub struct ReplayUiController {
    pub recorder: Arc<Mutex<ReplayRecorder>>,
    pub overlay: RecordingOverlay,
    pub replay_list: ReplayListScreen,
    pub studio: ReplayStudioScreen,
    pub timeline: CameraPathTimeline,
}

impl ReplayUiController {
    pub fn new(replays_dir: impl Into<std::path::PathBuf>) -> Self {
        let dir = replays_dir.into();
        Self {
            recorder: Arc::new(Mutex::new(ReplayRecorder::new(dir.clone()))),
            overlay: RecordingOverlay::default(),
            replay_list: ReplayListScreen::new(dir),
            studio: ReplayStudioScreen::default(),
            timeline: CameraPathTimeline::new(),
        }
    }

    pub fn on_record_start(&mut self) {
        if let Ok(mut rec) = self.recorder.lock() {
            let _ = rec.start();
            self.overlay.show();
        }
        info!("[RsReplay] 録画開始 (First-Person Recording Started)");
    }

    pub fn on_record_pause(&mut self) {
        if let Ok(mut rec) = self.recorder.lock() {
            rec.pause();
        }
        info!("[RsReplay] 一時停止 (Paused)");
    }

    pub fn on_record_resume(&mut self) {
        if let Ok(mut rec) = self.recorder.lock() {
            rec.resume();
        }
        info!("[RsReplay] 録画再開 (Resumed)");
    }

    pub fn on_record_stop(&mut self) {
        if let Ok(mut rec) = self.recorder.lock() {
            if let Ok(meta) = rec.stop_and_finalize() {
                info!("[RsReplay] 録画完了＆保存: {} ({})", meta.filename, meta.duration_display);
            }
        }
        self.overlay.hide();
        self.replay_list.catalog.refresh();
    }

    /// Shortcut key handling during Replay Studio editing or Recording.
    pub fn on_key_press(&mut self, keycode: char, current_cam_pos: [f32; 3], current_rot: (f32, f32, f32)) {
        match keycode.to_ascii_uppercase() {
            'H' => {
                self.studio.path_preview.toggle();
            }
            'O' => {
                let next_mode = match self.timeline.default_interpolator {
                    InterpolationMode::CatmullRom(_) => InterpolationMode::CubicHermite,
                    InterpolationMode::CubicHermite => InterpolationMode::Linear,
                    InterpolationMode::Linear => InterpolationMode::CatmullRom(0.5),
                };
                self.timeline.default_interpolator = next_mode;
                info!("[RsReplay Studio] Changed Default Interpolator to: {:?}", next_mode);
            }
            ' ' => {
                self.studio.toggle_preview_playback();
            }
            'P' => {
                let t = self.studio.current_timeline_ms;
                let idx = self.timeline.add_position_keyframe(
                    t,
                    current_cam_pos,
                    current_rot.0,
                    current_rot.1,
                    current_rot.2,
                );
                info!("[RsReplay Studio] Added Position Keyframe #{}: pos={:?} rot={:?} at {}ms", idx + 1, current_cam_pos, current_rot, t);
            }
            'T' => {
                let t = self.studio.current_timeline_ms;
                let replay_t = t; // Or custom speed ratio
                let idx = self.timeline.add_time_keyframe(t, replay_t);
                info!("[RsReplay Studio] Added Time Keyframe #{}: timeline={}ms -> replay={}ms", idx + 1, t, replay_t);
            }
            'M' => {
                let t = self.studio.current_timeline_ms;
                let label = format!("Bookmark @ {}s", t / 1000);
                self.timeline.add_bookmark(t, &label, 0xFFFFAA00);
                info!("[RsReplay Studio] Dropped Event Bookmark: '{}' at {}ms", label, t);
            }
            _ => {}
        }
    }

    pub fn render_overlay_if_recording(&self) {
        if let Ok(rec) = self.recorder.lock() {
            if rec.is_recording() {
                self.overlay.render_to_screen();
            }
        }
        if self.studio.is_open {
            self.studio.path_preview.render_world_path(&self.timeline);
        }
    }

    pub fn pause_menu_ui(&self) -> PauseMenuReplayUi {
        let state = self.recorder.lock().map(|r| r.state).unwrap_or(RecordingState::Idle);
        PauseMenuReplayUi::for_state(state)
    }
}

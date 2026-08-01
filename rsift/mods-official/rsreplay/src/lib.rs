//! # RsReplay — First-Person Packet Replay & Cinematic Studio Mod (`rsreplay.dll`)
//!
//! 1) `ReplayStudioScreen`: カメラスタジオ HUD (`Position Keyframe PK` / `Time Keyframe TK` / ブックマーク `M`)
//! 2) `PathPreviewOverlay`: `H` キー切り替えによる Catmull-Rom 3D カメラパス＆視線ベクトルのワールド内可視化
//! 3) `ExportPreset`: 1080p60 / 4K120 / 8KCinematic プリセット ＆ 露出シャッター角同調サブフレームブラー累加
//! 4) `ReplayUiController`: TitleScreen / PauseScreen / Studio 画面の全ボタン＆ショートカットキー制御

use rsift_api::{ModContext, RsiftStatus, TARGET_MINECRAFT_VERSION};
use rsift_replay::{
    ExportPreset, Mp4Exporter, OfflineRenderSettings, OfflineRenderer, PacketDirection,
    PlaybackEngine, ReplayUiController,
};
use std::sync::{Mutex, OnceLock};
use tracing::info;

static UI: OnceLock<Mutex<ReplayUiController>> = OnceLock::new();

fn ui() -> &'static Mutex<ReplayUiController> {
    UI.get_or_init(|| Mutex::new(ReplayUiController::new("./replays")))
}

#[no_mangle]
pub extern "C" fn rsift_mod_init(ctx: &mut ModContext) -> i32 {
    info!("==============================================================================");
    info!(" 🎬 [RsReplay Studio v2.0] First-Person Replay & Catmull-Rom Camera Studio");
    info!("    Features: 3D Path Preview [H] | Keyframe Timeline [P/T] | Bookmarks [M]");
    info!("    Export: Frame-Locked Synchronous Capture (1080p60 / 4K120 / 8K Cinematic)");
    info!("==============================================================================");

    if let Ok(mut ctrl) = ui().lock() {
        ctrl.recorder.lock().unwrap().set_player(
            [
                0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66,
                0x77, 0x88,
            ],
            "Ifuto_mitai",
        );
    }

    let screen_reg = ctx.screen_registry();

    // TitleScreen Buttons
    screen_reg.add_button(
        "net.minecraft.client.gui.screens.TitleScreen",
        "Replays & Studio",
        8,
        56,
        120,
        20,
        Some("Open replay list & camera path studio"),
        move |_| {
            if let Ok(mut ui) = ui().lock() {
                ui.replay_list.open();
                ui.studio.open();
            }
        },
    );

    // PauseScreen Buttons
    screen_reg.add_button(
        "net.minecraft.client.gui.screens.PauseScreen",
        "Record (REC)",
        8,
        40,
        98,
        20,
        Some("Start first-person recording"),
        move |_| {
            if let Ok(mut ui) = ui().lock() {
                ui.on_record_start();
            }
        },
    );
    screen_reg.add_button(
        "net.minecraft.client.gui.screens.PauseScreen",
        "Pause Rec",
        8,
        64,
        98,
        20,
        Some("Pause recording"),
        move |_| {
            if let Ok(mut ui) = ui().lock() {
                ui.on_record_pause();
            }
        },
    );
    screen_reg.add_button(
        "net.minecraft.client.gui.screens.PauseScreen",
        "Stop Rec",
        110,
        64,
        98,
        20,
        Some("Stop and save .rsr replay file"),
        move |_| {
            if let Ok(mut ui) = ui().lock() {
                ui.on_record_stop();
            }
        },
    );

    // Studio Keyframe Shortcuts HUD Button
    screen_reg.add_button(
        "net.minecraft.client.gui.screens.PauseScreen",
        "Studio HUD [H]",
        110,
        40,
        98,
        20,
        Some("Toggle 3D Catmull-Rom Path Preview"),
        move |_| {
            if let Ok(mut ui) = ui().lock() {
                ui.on_key_press('H', [0.0, 64.0, 0.0], (0.0, 0.0, 0.0));
            }
        },
    );

    info!(
        "[RsReplay] Studio initialized for {}",
        TARGET_MINECRAFT_VERSION
    );
    ctx.request_render_ticks();
    RsiftStatus::Success as i32
}

#[no_mangle]
pub extern "C" fn rsift_mod_on_packet(packet_id: u32, buf_ptr: i64, buf_len: i32) -> bool {
    if let Ok(ui) = ui().lock() {
        if let Ok(rec) = ui.recorder.lock() {
            rec.on_packet(packet_id, buf_ptr, buf_len, PacketDirection::Incoming);
        }
    }
    true
}

#[no_mangle]
pub extern "C" fn rsift_mod_on_render(_width: u32, _height: u32, _delta_time: f32) {
    if let Ok(ui) = ui().lock() {
        ui.render_overlay_if_recording();
        ui.pause_menu_ui().render_buttons();
    }
}

#[no_mangle]
pub extern "C" fn rsift_mod_on_key_event(
    keycode: i32,
    cam_x: f32,
    cam_y: f32,
    cam_z: f32,
    yaw: f32,
    pitch: f32,
) {
    if let Ok(mut ui) = ui().lock() {
        let ch = match keycode {
            72 => 'H', // H key
            79 => 'O', // O key
            80 => 'P', // P key
            84 => 'T', // T key
            77 => 'M', // M key
            32 => ' ', // Space
            _ => '\0',
        };
        if ch != '\0' {
            ui.on_key_press(ch, [cam_x, cam_y, cam_z], (yaw, pitch, 0.0));
        }
    }
}

#[no_mangle]
pub extern "C" fn rsreplay_export_mp4(
    replay_filename: *const u8,
    preset_id: u32, // 0 = 1080p60, 1 = 4K120, 2 = 8K Cinematic
    motion_blur_samples: u32,
    shaderpack_ptr: *const u8,
    resource_pack_ptr: *const u8,
) -> i32 {
    let filename = unsafe {
        if replay_filename.is_null() {
            return 1;
        }
        std::ffi::CStr::from_ptr(replay_filename as *const i8)
            .to_string_lossy()
            .into_owned()
    };

    let preset = match preset_id {
        1 => ExportPreset::Preset4K120,
        2 => ExportPreset::Preset8KCinematic,
        _ => ExportPreset::Preset1080p60,
    };

    let mut settings = preset.apply();
    if motion_blur_samples > 0 {
        settings.motion_blur_samples = motion_blur_samples;
    }
    if !shaderpack_ptr.is_null() {
        settings.shaderpack = Some(unsafe {
            std::ffi::CStr::from_ptr(shaderpack_ptr as *const i8)
                .to_string_lossy()
                .into_owned()
        });
    }
    if !resource_pack_ptr.is_null() {
        settings.resource_pack = Some(unsafe {
            std::ffi::CStr::from_ptr(resource_pack_ptr as *const i8)
                .to_string_lossy()
                .into_owned()
        });
    }

    let path = std::path::PathBuf::from("./replays").join(&filename);
    let mut playback = PlaybackEngine::new();
    if playback.load(&path).is_err() {
        return 1;
    }

    let mut renderer = OfflineRenderer::new();
    renderer.settings = settings;
    renderer.blur_accum = rsift_replay::renderer::MotionBlurAccumulator::new(
        renderer.settings.width as usize,
        renderer.settings.height as usize,
    );

    let output = path.with_extension("mp4");
    let exporter = Mp4Exporter::new();
    if exporter.export(&playback, &mut renderer, &output).is_err() {
        return 1;
    }
    0
}

#[cfg(test)]
mod tests {
    //! RsReplay mod 層の動作検定 (core rsift-replay 16 本とは別に、FFI 出口
    //! で何が起きるかを pin する)。グローバル UI (OnceLock) を共有するため
    //! テストは SERIAL で直列化する。
    use super::*;

    static SERIAL: Mutex<()> = Mutex::new(());

    fn keyframe_count() -> usize {
        ui().lock().unwrap().timeline.position_keyframes.len()
    }

    /// P (keycode 80): 現在カメラ位置の Position Keyframe が追加される。
    /// 実際のゲーム内ショートカット → studio 状態遷移の直結確認。
    #[test]
    fn mod_key_event_p_adds_position_keyframe() {
        let _g = SERIAL.lock().unwrap();
        let before = keyframe_count();
        rsift_mod_on_key_event(80, 1.5, 64.0, -2.0, 45.0, -10.0);
        let ctrl = ui().lock().unwrap();
        assert_eq!(ctrl.timeline.position_keyframes.len(), before + 1);
        let kf = ctrl.timeline.position_keyframes.last().unwrap();
        assert_eq!(kf.pos, [1.5, 64.0, -2.0]);
        assert!((kf.yaw - 45.0).abs() < 1e-6, "yaw が壊れていない");
        assert!((kf.pitch - -10.0).abs() < 1e-6);
    }

    /// H (keycode 72): Path Preview オーバーレイの表示がトグルする。
    #[test]
    fn mod_key_event_h_toggles_path_preview() {
        let _g = SERIAL.lock().unwrap();
        let v0 = ui().lock().unwrap().studio.path_preview.visible;
        rsift_mod_on_key_event(72, 0.0, 0.0, 0.0, 0.0, 0.0);
        let v1 = ui().lock().unwrap().studio.path_preview.visible;
        rsift_mod_on_key_event(72, 0.0, 0.0, 0.0, 0.0, 0.0);
        let v2 = ui().lock().unwrap().studio.path_preview.visible;
        assert_ne!(v0, v1, "H で表示状態が反転する");
        assert_eq!(v0, v2, "2 回で元に戻る");
    }

    /// O (keycode 79): 既定補間器が CatmullRom→CubicHermite→Linear→…で巡回。
    #[test]
    fn mod_key_event_o_cycles_interpolator() {
        use rsift_replay::keyframe_timeline::InterpolationMode;
        use rsift_replay::keyframe_timeline::InterpolationMode::*;
        let _g = SERIAL.lock().unwrap();
        // 出発点に関わらず 3 回で一周する性質を、到達系列の型で確認する。
        rsift_mod_on_key_event(79, 0.0, 0.0, 0.0, 0.0, 0.0);
        let m1 = ui().lock().unwrap().timeline.default_interpolator;
        rsift_mod_on_key_event(79, 0.0, 0.0, 0.0, 0.0, 0.0);
        let m2 = ui().lock().unwrap().timeline.default_interpolator;
        let kinds = |m: &InterpolationMode| match m {
            CatmullRom(_) => 0,
            CubicHermite => 1,
            Linear => 2,
        };
        let (k1, k2) = (kinds(&m1), kinds(&m2));
        assert_eq!((k1 + 1) % 3, k2, "補間器は 3 状態を一方向に巡回する");
    }

    /// 未知 keycode は studio 状態を変化させない (誤キーの静黙無害化ではなく、
    /// マッピング対象外として明示的に無視される契約)。
    #[test]
    fn mod_key_event_unknown_ignored() {
        let _g = SERIAL.lock().unwrap();
        let before = keyframe_count();
        let v0 = ui().lock().unwrap().studio.path_preview.visible;
        rsift_mod_on_key_event(9999, 9.0, 9.0, 9.0, 9.0, 9.0);
        assert_eq!(keyframe_count(), before);
        assert_eq!(ui().lock().unwrap().studio.path_preview.visible, v0);
    }

    /// 録画中でなければパケットは記録されない (packet replay の根幹不変量)。
    /// 録画中での取り込み自体は core (recorder/packet) 側の検定で pin 済。
    #[test]
    fn mod_packet_ignored_when_not_recording() {
        let _g = SERIAL.lock().unwrap();
        {
            let mut ctrl = ui().lock().unwrap();
            if ctrl.recorder.lock().unwrap().is_recording() {
                ctrl.on_record_pause();
            }
        }
        let before = ui().lock().unwrap().recorder.lock().unwrap().packet_count();
        assert!(
            rsift_mod_on_packet(0x21, 0, 0),
            "戻り値は常に true (受信継続)"
        );
        let after = ui().lock().unwrap().recorder.lock().unwrap().packet_count();
        assert_eq!(before, after, "非録画中の packet は取り込まれない");
    }

    /// export FFI の失敗格子: null ファイル名は即 1、例外/パニックにしない。
    #[test]
    fn export_null_filename_returns_1() {
        let _g = SERIAL.lock().unwrap();
        let code = rsreplay_export_mp4(std::ptr::null(), 0, 0, std::ptr::null(), std::ptr::null());
        assert_eq!(code, 1);
    }

    /// 存在しない .rsr を指定した場合は load 失敗で 1 (静黙成功しない)。
    #[test]
    fn export_missing_replay_file_returns_1() {
        let _g = SERIAL.lock().unwrap();
        let name = b"definitely_missing_replay_zzz.rsr\0";
        let code = rsreplay_export_mp4(name.as_ptr(), 0, 0, std::ptr::null(), std::ptr::null());
        assert_eq!(code, 1);
    }
}

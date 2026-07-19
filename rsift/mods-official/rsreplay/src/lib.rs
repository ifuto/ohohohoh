//! # RsReplay — First-Person Packet Replay & Cinematic Studio Mod (`rsreplay.dll`)
//!
//! 1) `ReplayStudioScreen`: カメラスタジオ HUD (`Position Keyframe PK` / `Time Keyframe TK` / ブックマーク `M`)
//! 2) `PathPreviewOverlay`: `H` キー切り替えによる Catmull-Rom 3D カメラパス＆視線ベクトルのワールド内可視化
//! 3) `ExportPreset`: 1080p60 / 4K120 / 8KCinematic プリセット ＆ 露出シャッター角同調サブフレームブラー累加
//! 4) `ReplayUiController`: TitleScreen / PauseScreen / Studio 画面の全ボタン＆ショートカットキー制御

use rsift_api::{ModContext, RsiftStatus, TARGET_MINECRAFT_VERSION};
use rsift_replay::{
    ReplayUiController, PacketDirection, OfflineRenderSettings, Mp4Exporter,
    PlaybackEngine, OfflineRenderer, ExportPreset,
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
            [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88],
            "Ifuto_mitai",
        );
    }

    let screen_reg = ctx.screen_registry();

    // TitleScreen Buttons
    screen_reg.add_button(
        "net.minecraft.client.gui.screens.TitleScreen",
        "Replays & Studio",
        8, 56, 120, 20,
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
        8, 40, 98, 20,
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
        8, 64, 98, 20,
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
        110, 64, 98, 20,
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
        110, 40, 98, 20,
        Some("Toggle 3D Catmull-Rom Path Preview"),
        move |_| {
            if let Ok(mut ui) = ui().lock() {
                ui.on_key_press('H', [0.0, 64.0, 0.0], (0.0, 0.0, 0.0));
            }
        },
    );

    info!("[RsReplay] Studio initialized for {}", TARGET_MINECRAFT_VERSION);
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
pub extern "C" fn rsift_mod_on_key_event(keycode: i32, cam_x: f32, cam_y: f32, cam_z: f32, yaw: f32, pitch: f32) {
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

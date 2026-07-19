//! # RsReplay — First-Person Packet Replay Mod

use rsift_api::{ModContext, RsiftStatus, TARGET_MINECRAFT_VERSION};
use rsift_replay::{
    ReplayUiController, PacketDirection, OfflineRenderSettings, Mp4Exporter,
    PlaybackEngine, OfflineRenderer,
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
    info!(" [RsReplay] First-Person Replay | Author: Ifuto_mitai");
    info!("==============================================================================");

    if let Ok(mut ctrl) = ui().lock() {
        ctrl.recorder.lock().unwrap().set_player(
            [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88],
            "Ifuto_mitai",
        );
    }

    let screen_reg = ctx.screen_registry();
    screen_reg.add_button(
        "net.minecraft.client.gui.screens.TitleScreen",
        "Replays",
        8, 56, 120, 20,
        Some("Open replay list"),
        move |_| {
            if let Ok(mut ui) = ui().lock() {
                ui.replay_list.open();
            }
        },
    );

    screen_reg.add_button(
        "net.minecraft.client.gui.screens.PauseScreen",
        "Record",
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
        Some("Stop and save .rsr"),
        move |_| {
            if let Ok(mut ui) = ui().lock() {
                ui.on_record_stop();
            }
        },
    );

    info!("[RsReplay] Initialized for {}", TARGET_MINECRAFT_VERSION);
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
pub extern "C" fn rsreplay_export_mp4(
    replay_filename: *const u8,
    output_fps: u32,
    motion_blur: f32,
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
    let mut settings = OfflineRenderSettings::default();
    settings.output_fps = output_fps;
    settings.motion_blur = motion_blur.clamp(0.01, 100.0);
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
    let output = path.with_extension("mp4");
    if Mp4Exporter::new().export(&playback, &mut renderer, &output).is_ok() {
        info!("[RsReplay] MP4 exported: {:?}", output);
        0
    } else {
        1
    }
}

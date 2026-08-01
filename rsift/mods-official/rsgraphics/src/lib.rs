//! # RsGraphics - Official Graphics Migration Mod

use rsift_api::{
    AdaptivePerfEngine, ClothConfigBuilder, ImageRegistry, ModContext, ModManifest, RsiftStatus,
    TARGET_MINECRAFT_VERSION,
};
use rsift_opt_gfx::{init_pipeline, on_render_frame, open_global_settings, RsiftModernBootSplash};
use tracing::info;

#[no_mangle]
pub extern "C" fn rsift_mod_init(ctx: &mut ModContext) -> i32 {
    info!("==========================================================================");
    info!(" [RsGraphics] HZB GPU-Driven Graphics Engine");
    info!(" Target: {}", ctx.minecraft_version);
    info!("==========================================================================");

    let hw = AdaptivePerfEngine::hardware();
    let rp = AdaptivePerfEngine::render_profile(hw);
    info!(
        "[RsGraphics] tier={} speed_first={} profile={}",
        rp.tier.label(),
        rp.speed_first,
        rsift_api::AdaptivePerfEngine::render_profile_summary(&rp)
    );

    let game_dir = std::env::var("APPDATA")
        .map(|a| std::path::PathBuf::from(a).join(".minecraft"))
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    init_pipeline(&game_dir);

    if let Some(caps) = rsift_api::engine_caps::EngineCaps::from_jvm_props() {
        info!(
            "[RsGraphics] DX12 Agility SM={} (engine lazy on first JNI frame)",
            caps.shader_model.as_str()
        );
    }

    let _ = rsift_api::mod_suite::init_mod_suite();
    let _ = ctx
        .suite()
        .renderer_api
        .register_mesh_provider("rsgraphics");

    if !rp.speed_first {
        let mut boot = RsiftModernBootSplash::new();
        boot.activate_override();
        boot.update_progress("RsGraphics engine...", 40, 100);
        boot.finish_and_fade_out();
    }

    let img_reg = ImageRegistry::new();
    // Valid 1×1 PNG (not a truncated signature stub).
    const ICON_PNG: &[u8] = include_bytes!("icon_1x1.png");
    let gfx_icon = img_reg.embed_from_bytes("icon_rsgraphics", ICON_PNG, 1, 1);
    let calc_icon = img_reg.embed_from_bytes("icon_rscalc", ICON_PNG, 1, 1);
    let replay_icon = img_reg.embed_from_bytes("icon_rsreplay", ICON_PNG, 1, 1);

    let mut gfx_config = ClothConfigBuilder::new();
    gfx_config.set_title("RsGraphics");
    let cat = gfx_config.add_category("Rendering");
    let _ = gfx_config.add_int_slider(
        cat,
        "Chunk Builder Threads",
        1,
        16,
        rp.chunk_builder_threads as i32,
        Some("Parallel meshing threads"),
    );
    let _ = gfx_config.add_bool_toggle(
        cat,
        "Binary Greedy Meshing",
        rp.binary_greedy_meshing,
        Some("Bitwise mesh (flagship)"),
    );
    let _ = gfx_config.add_bool_toggle(
        cat,
        "GPU MDI Culling",
        rp.gpu_compute_culling,
        Some("Off until DX12 ExecuteIndirect; CPU cull active"),
    );
    let _ = gfx_config.add_bool_toggle(
        cat,
        "CPU Soft Occlusion",
        rp.hzb_occlusion,
        Some("Software occlusion (not GPU Hi-Z)"),
    );
    let _ = gfx_config.add_bool_toggle(
        cat,
        "Mesh Disk Cache",
        rp.mesh_disk_cache,
        Some("Skip rebuild on revisit"),
    );
    let _ = gfx_config.add_bool_toggle(
        cat,
        "Eco Mode (weak PC)",
        matches!(
            rp.tier,
            rsift_api::PerformanceTier::Minimal | rsift_api::PerformanceTier::Low
        ),
        Some("Auto for low-end hardware"),
    );
    if rp.feather.enabled {
        let _ = gfx_config.add_bool_toggle(
            cat,
            "Feather Tile Binning",
            rp.feather.software_tile_binning,
            Some("TBDR-style software binning"),
        );
        let _ = gfx_config.add_bool_toggle(
            cat,
            "Feather Pseudo-VRS",
            rp.feather.software_vrs_checkerboard,
            Some("No resolution change"),
        );
    }

    let mod_menu = ctx.mod_menu();
    mod_menu.register_mod(
        ModManifest {
            id: "rsgraphics".into(),
            name: "RsGraphics".into(),
            version: "v1.0.0".into(),
            author: "Rsift".into(),
            description: "HZB culling, 12B verts, bindless SSBO".into(),
            target_rsift_version: TARGET_MINECRAFT_VERSION.into(),
            capabilities: vec![],
        },
        Some(gfx_icon),
        Some("https://github.com/rsift-mc/rsgraphics"),
        Some(gfx_config.clone()),
    );
    mod_menu.register_mod(
        ModManifest {
            id: "rscalc".into(),
            name: "RsCalc".into(),
            version: "v1.0.0".into(),
            author: "Rsift".into(),
            description: "Native compute migration".into(),
            target_rsift_version: TARGET_MINECRAFT_VERSION.into(),
            capabilities: vec![],
        },
        Some(calc_icon),
        None,
        None,
    );
    mod_menu.register_mod(
        ModManifest {
            id: "rsreplay".into(),
            name: "RsReplay".into(),
            version: "v1.0.0".into(),
            author: "Ifuto_mitai".into(),
            description: "First-person replay".into(),
            target_rsift_version: TARGET_MINECRAFT_VERSION.into(),
            capabilities: vec!["process_exec".into()],
        },
        Some(replay_icon),
        None,
        None,
    );

    let menu = ctx.mod_menu().clone();
    let screen_reg = ctx.screen_registry();

    // Fabric Mod Menu 参考: ボタン文言は定数 "Mods"。個数ではなく、開いた先の
    // 一覧→各 mod の詳細画面 (名前/ID/バージョン/作者/概要 + Config/Homepage)
    // が本質 (行プロトコルは rsift-api の catalog_lines/detail_lines が構築)。
    screen_reg.add_button(
        "net.minecraft.client.gui.screens.TitleScreen",
        "Mods",
        8,
        8,
        120,
        20,
        Some("Open Rsift mod catalog"),
        move |_| {
            info!("[TitleScreen] Mods button clicked");
            menu.open_screen();
        },
    );

    screen_reg.add_button(
        "net.minecraft.client.gui.screens.TitleScreen",
        "動画設定",
        8,
        32,
        120,
        20,
        Some("RsGraphics — DX12 video settings"),
        move |_| {
            open_global_settings();
        },
    );

    screen_reg.redirect_screen(
        "net.minecraft.client.gui.screens.options.VideoSettingsScreen",
        "rsgraphics_open_modern_video_settings",
    );

    info!("[RsGraphics] init complete — engine lazy-loaded; vanilla video settings untouched");
    ctx.request_render_ticks();
    RsiftStatus::Success as i32
}

#[no_mangle]
pub extern "C" fn rsift_mod_on_packet(_packet_id: u32, _buf_ptr: i64, _buf_len: i32) -> bool {
    true
}

#[no_mangle]
pub extern "C" fn rsift_mod_on_render(width: u32, height: u32, delta_time: f32) {
    on_render_frame(width, height, delta_time);
}

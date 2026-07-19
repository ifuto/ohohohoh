//! Sample Rsift native DLL mod — exercises ContentRegistry, ScreenRegistry, Cloth Config.

use rsift_api::{
    content::BlockDefinition, ClothConfigBuilder, ModContext, ModManifest, RsiftStatus,
    TARGET_MINECRAFT_VERSION,
};
use tracing::info;

#[no_mangle]
pub extern "C" fn rsift_mod_init(ctx: &mut ModContext) -> i32 {
    info!("[SampleMod] Loaded for Minecraft {}", ctx.minecraft_version);
    let _ = TARGET_MINECRAFT_VERSION;

    let suite = ctx.suite();

    // Real block via ModSuite content registry (picked up by collect_apply_snapshot).
    if let Ok(mut content) = suite.content.write() {
        let block_id = content.register_block(
            BlockDefinition::builder("sample", "demo_block")
                .strength(2.0, 6.0)
                .luminance(7),
        );
        info!("[SampleMod] registered block sample:demo_block id={}", block_id);
    }

    // Real item via ModContext registry.
    let item_id = ctx.registry.register_item("sample", "demo_item", 64);
    info!("[SampleMod] registered item sample:demo_item id={}", item_id);

    // TitleScreen button.
    ctx.screen_registry().add_button(
        "net.minecraft.client.gui.screens.TitleScreen",
        "Sample Mod",
        10,
        10,
        120,
        20,
        Some("Open Sample Mod config"),
        |_id| {
            info!("[SampleMod] TitleScreen button pressed");
            rsift_api::platform::request_open_screen("cloth_config");
        },
    );

    // Cloth config registered into Mod Menu catalog.
    let mut cloth = ClothConfigBuilder::new();
    cloth.set_title("Sample Mod Settings");
    let cat = cloth.add_category("General");
    let _enabled = cloth.add_bool_toggle(cat, "Enable Demo Feature", true, Some("Toggle demo behaviour"));
    let _power = cloth.add_int_slider(cat, "Demo Power", 0, 100, 42, None);

    ctx.mod_menu().register_mod(
        ModManifest {
            id: "sample_mod".into(),
            name: "Sample Mod".into(),
            version: "0.1.0".into(),
            author: "Rsift".into(),
            description: "Exercises platform block/item/UI/config registration".into(),
            target_rsift_version: env!("CARGO_PKG_VERSION").into(),
        },
        None,
        Some("https://github.com/rsift/rsift"),
        Some(cloth),
    );

    RsiftStatus::Success as i32
}

#[no_mangle]
pub extern "C" fn rsift_mod_on_packet(_packet_id: u32, _buf_ptr: i64, _buf_len: i32) -> bool {
    true
}

#[no_mangle]
pub extern "C" fn rsift_mod_on_render(_width: u32, _height: u32, _delta_time: f32) {}

//! # DLL Mod Plugin Interface
//!
//! `.dll` / `.so` / `.dylib` 形式のModとのインターフェースおよびイベントシステムを定義します。

use crate::registry::ModRegistry;

/// Mod のメタデータ情報
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub target_rsift_version: String,
}

/// DLL Mod側で実装・公開される C ABI エントリポイントの型定義
pub type RsiftModInitFn = extern "C" fn(ctx: &mut ModContext) -> i32;
pub type RsiftModOnPacketFn = extern "C" fn(packet_id: u32, buf_ptr: i64, buf_len: i32) -> bool;
pub type RsiftModOnRenderFn = extern "C" fn(width: u32, height: u32, delta_time: f32);

/// Mod に渡される実行コンテキスト
pub struct ModContext<'a> {
    pub manifest: ModManifest,
    pub registry: &'a mut ModRegistry,
    pub minecraft_version: &'static str,
    pub is_client: bool,
    pub runtime: &'a crate::runtime::RsiftRuntime,
}

impl<'a> ModContext<'a> {
    pub fn new(
        manifest: ModManifest,
        registry: &'a mut ModRegistry,
        is_client: bool,
        runtime: &'a crate::runtime::RsiftRuntime,
    ) -> Self {
        Self {
            manifest,
            registry,
            minecraft_version: crate::TARGET_MINECRAFT_VERSION,
            is_client,
            runtime,
        }
    }

    pub fn screen_registry(&self) -> &crate::ui_ext::ScreenRegistry {
        self.runtime.screen_registry()
    }

    pub fn mod_menu(&self) -> &crate::mod_menu::RsiftModMenuScreen {
        self.runtime.mod_menu()
    }

    /// Ask the client loop to keep calling render handlers (optional hint for throttling).
    pub fn request_render_ticks(&self) {
        self.runtime.request_render_ticks();
    }

    /// Unified mod platform API (renderer, networking, lifecycle, …).
    pub fn suite(&self) -> &'static crate::mod_suite::ModSuite {
        crate::mod_suite::mod_suite()
    }

    /// True when native mods finished loading into the global runtime.
    pub fn mods_ready(&self) -> bool {
        self.runtime.mods_loaded()
    }
}

/// Mod イベントハンドラのトレイト
pub trait ModHandler: Send + Sync {
    fn on_init(&self, registry: &mut ModRegistry) -> Result<(), String>;

    fn on_packet_receive(&self, _packet_id: u32, _raw_ptr: i64, _len: i32) -> bool {
        true
    }

    fn on_class_transform(&self, _class_name: &str, _bytecode: &mut Vec<u8>) -> bool {
        false
    }

    fn on_render_frame(&self, _width: u32, _height: u32, _delta_time: f32) {}
}

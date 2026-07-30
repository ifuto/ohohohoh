//! # Rsift API (1.21.11 Edition) - Hyper-Optimized Unified Parity Layer
//!
//! Rsift Mod LoaderのコアAPIライブラリ。
//! Java JVMとRust間でのゼロコピー通信（bytemuck）、DLL Modローディングインターフェース、
//! Fabric / NeoForge 1.21.x パリティ、全能 API (`ui_ext`, `image_api`, `os_integ`)、
//! さらに Fabric API の全中身軽量化 (`fabric_api_optimized`)、
//! Cloth Config ネイティブエンジン (`cloth_config`)、および Mod Menu カタログ (`mod_menu`)
//! を全網羅実装する世界最強のネイティブインターフェースです。

pub mod packet;
pub mod mod_api;
pub mod registry;
pub mod lifecycle;
pub mod content;
pub mod networking;
pub mod rendering;
pub mod resources;
pub mod gameplay;
pub mod fabric_parity_check;
pub mod neoforge_event_bus;
pub mod neoforge_registries;
pub mod neoforge_capabilities;
pub mod neoforge_config;
pub mod neoforge_coremod;
pub mod neoforge_parity_check;
pub mod hyper_opt;
pub mod engine_caps;
pub mod adaptive_perf;
pub mod advancements;
pub mod worldgen;
pub mod feather_preset;
pub mod ui_ext;
pub mod image_api;
pub mod os_integ;
pub mod fabric_api_optimized;
pub mod fabric_api;
pub mod migration_hub;
pub mod cloth_config;
pub mod mod_menu;
pub mod runtime;
pub mod native_loader;
pub mod mod_dispatch;
pub mod mod_security;
pub mod mod_suite;
pub mod platform;

pub use packet::*;
pub use mod_api::*;
pub use registry::*;
pub use lifecycle::*;
pub use content::*;
pub use networking::*;
pub use rendering::*;
pub use resources::*;
pub use gameplay::*;
pub use fabric_parity_check::*;
pub use neoforge_event_bus::*;
pub use neoforge_registries::*;
pub use neoforge_capabilities::*;
pub use neoforge_config::*;
pub use neoforge_coremod::*;
pub use neoforge_parity_check::*;
pub use hyper_opt::*;
pub use engine_caps::*;
pub use adaptive_perf::*;
pub use advancements::*;
pub use worldgen::*;
pub use feather_preset::*;
pub use ui_ext::*;
pub use image_api::*;
pub use os_integ::*;
pub use fabric_api_optimized::*;
pub use fabric_api::*;
pub use migration_hub::*;
pub use cloth_config::*;
pub use mod_menu::*;
pub use runtime::*;
pub use native_loader::*;
pub use mod_dispatch::*;
pub use mod_suite::*;
pub use platform::*;

/// ターゲットとなるMinecraftのバージョン
pub const TARGET_MINECRAFT_VERSION: &str = "1.21.11";
/// Rsiftローダーのバージョン
pub const RSIFT_VERSION: &str = "0.1.0-alpha.platform-wired";

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RsiftStatus {
    Success = 0,
    InvalidPointer = -1,
    CastError = -2,
    HookFailed = -3,
    ModLoadError = -4,
}

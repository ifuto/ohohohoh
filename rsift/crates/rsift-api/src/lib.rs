//! # Rsift API (1.21.11 Edition) - Hyper-Optimized Unified Parity Layer
//!
//! Rsift Mod LoaderのコアAPIライブラリ。
//! Java JVMとRust間でのゼロコピー通信（bytemuck）、DLL Modローディングインターフェース、
//! Fabric / NeoForge 1.21.x パリティ、全能 API (`ui_ext`, `image_api`, `os_integ`)、
//! さらに Fabric API の全中身軽量化 (`fabric_api_optimized`)、
//! Cloth Config ネイティブエンジン (`cloth_config`)、および Mod Menu カタログ (`mod_menu`)
//! を全網羅実装する世界最強のネイティブインターフェースです。

pub mod adaptive_perf;
pub mod advancements;
pub mod cloth_config;
pub mod content;
pub mod engine_caps;
pub mod fabric_api;
pub mod fabric_api_optimized;
pub mod fabric_parity_check;
pub mod feather_preset;
pub mod gameplay;
pub mod hyper_opt;
pub mod image_api;
pub mod lifecycle;
pub mod mc_style;
pub mod migration_hub;
pub mod mod_api;
pub mod mod_dispatch;
pub mod mod_menu;
pub mod mod_security;
pub mod mod_suite;
pub mod native_loader;
pub mod neoforge_capabilities;
pub mod neoforge_config;
pub mod neoforge_coremod;
pub mod neoforge_event_bus;
pub mod neoforge_parity_check;
pub mod neoforge_registries;
pub mod networking;
pub mod os_integ;
pub mod packet;
pub mod platform;
pub mod registry;
pub mod rendering;
pub mod resources;
pub mod runtime;
pub mod ui_ext;
pub mod worldgen;

pub use adaptive_perf::*;
pub use advancements::*;
pub use cloth_config::*;
pub use content::*;
pub use engine_caps::*;
pub use fabric_api::*;
pub use fabric_api_optimized::*;
pub use fabric_parity_check::*;
pub use feather_preset::*;
pub use gameplay::*;
pub use hyper_opt::*;
pub use image_api::*;
pub use lifecycle::*;
pub use mc_style::*;
pub use migration_hub::*;
pub use mod_api::*;
pub use mod_dispatch::*;
pub use mod_menu::*;
pub use mod_suite::*;
pub use native_loader::*;
pub use neoforge_capabilities::*;
pub use neoforge_config::*;
pub use neoforge_coremod::*;
pub use neoforge_event_bus::*;
pub use neoforge_parity_check::*;
pub use neoforge_registries::*;
pub use networking::*;
pub use os_integ::*;
pub use packet::*;
pub use platform::*;
pub use registry::*;
pub use rendering::*;
pub use resources::*;
pub use runtime::*;
pub use ui_ext::*;
pub use worldgen::*;

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

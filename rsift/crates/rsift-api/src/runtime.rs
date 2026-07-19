//! Global Rsift runtime — shared across launcher, JVMTI agent, and DLL mods.

use crate::mod_api::{RsiftModOnPacketFn, RsiftModOnRenderFn};
use crate::mod_menu::RsiftModMenuScreen;
use crate::registry::ModRegistry;
use crate::ui_ext::ScreenRegistry;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
use tracing::info;

pub struct RsiftRuntime {
    pub screen_registry: ScreenRegistry,
    pub mod_menu: RsiftModMenuScreen,
    pub registry: Mutex<ModRegistry>,
    pub mod_dir: RwLock<PathBuf>,
    pub packet_handlers: RwLock<Vec<RsiftModOnPacketFn>>,
    pub render_handlers: RwLock<Vec<RsiftModOnRenderFn>>,
    pub mods_loaded: RwLock<bool>,
    render_tick_wanted: AtomicU32,
    idle_mode: AtomicBool,
}

impl RsiftRuntime {
    pub fn new(mod_dir: PathBuf) -> Self {
        Self {
            screen_registry: ScreenRegistry::new(),
            mod_menu: RsiftModMenuScreen::new(),
            registry: Mutex::new(ModRegistry::new()),
            mod_dir: RwLock::new(mod_dir),
            packet_handlers: RwLock::new(Vec::new()),
            render_handlers: RwLock::new(Vec::new()),
            mods_loaded: RwLock::new(false),
            render_tick_wanted: AtomicU32::new(0),
            idle_mode: AtomicBool::new(false),
        }
    }

    pub fn set_mod_dir(&self, dir: PathBuf) {
        *self.mod_dir.write().unwrap() = dir;
    }

    pub fn screen_registry(&self) -> &ScreenRegistry {
        &self.screen_registry
    }

    pub fn mod_menu(&self) -> &RsiftModMenuScreen {
        &self.mod_menu
    }

    pub fn add_packet_handler(&self, f: RsiftModOnPacketFn) {
        self.packet_handlers.write().unwrap().push(f);
    }

    pub fn add_render_handler(&self, f: RsiftModOnRenderFn) {
        self.render_handlers.write().unwrap().push(f);
    }

    pub fn dispatch_packet(&self, packet_id: u32, ptr: i64, len: i32) -> bool {
        for f in self.packet_handlers.read().unwrap().iter() {
            if !f(packet_id, ptr, len) {
                return false;
            }
        }
        true
    }

    pub fn dispatch_render(&self, width: u32, height: u32, delta: f32) {
        let handlers = self.render_handlers.read().unwrap();
        if handlers.is_empty() {
            return;
        }
        for f in handlers.iter() {
            f(width, height, delta);
        }
    }

    pub fn has_render_handlers(&self) -> bool {
        !self.render_handlers.read().unwrap().is_empty()
    }

    pub fn has_packet_handlers(&self) -> bool {
        !self.packet_handlers.read().unwrap().is_empty()
    }

    /// Request periodic render callbacks (e.g. RsReplay overlay while recording).
    pub fn request_render_ticks(&self) {
        self.render_tick_wanted.fetch_add(1, Ordering::Relaxed);
    }

    pub fn release_render_ticks(&self) {
        self.render_tick_wanted.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
            Some(n.saturating_sub(1))
        }).ok();
    }

    pub fn needs_render_dispatch(&self) -> bool {
        self.render_tick_wanted.load(Ordering::Relaxed) > 0
    }

    pub fn set_idle_mode(&self, idle: bool) {
        self.idle_mode.store(idle, Ordering::Relaxed);
    }

    pub fn is_idle(&self) -> bool {
        self.idle_mode.load(Ordering::Relaxed)
    }

    pub fn mark_mods_loaded(&self) {
        *self.mods_loaded.write().unwrap() = true;
        info!("[RsiftRuntime] Native mods loaded and wired to global dispatchers");
    }

    pub fn mods_loaded(&self) -> bool {
        *self.mods_loaded.read().unwrap()
    }
}

static RUNTIME: OnceLock<RsiftRuntime> = OnceLock::new();

pub fn init_runtime(mod_dir: PathBuf) -> &'static RsiftRuntime {
    RUNTIME.get_or_init(|| RsiftRuntime::new(mod_dir))
}

pub fn runtime() -> Option<&'static RsiftRuntime> {
    RUNTIME.get()
}

pub fn runtime_or_init(mod_dir: PathBuf) -> &'static RsiftRuntime {
    RUNTIME.get_or_init(|| RsiftRuntime::new(mod_dir))
}

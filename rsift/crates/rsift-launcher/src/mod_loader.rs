//! Hyper-Optimized Native DLL Mod Loader — delegates to rsift-api runtime.

use rsift_api::{
    native_loader::{load_mods_into_runtime, pin_loaded_libraries},
    runtime::runtime_or_init,
    ModRegistry, LockFreeDispatcher, CachePadded,
    RsiftModOnPacketFn, RsiftModOnRenderFn,
};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::info;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct RsiftModVTable {
    pub on_packet_fn: Option<RsiftModOnPacketFn>,
    pub on_render_fn: Option<RsiftModOnRenderFn>,
}

pub struct LoadedMod {
    pub id: String,
}

pub struct NativeModLoader {
    pub mod_dir: PathBuf,
    pub loaded_mods: HashMap<String, LoadedMod>,
    pub packet_dispatcher: LockFreeDispatcher<RsiftModOnPacketFn>,
    pub render_dispatcher: LockFreeDispatcher<RsiftModOnRenderFn>,
}

impl NativeModLoader {
    pub fn new(mod_dir: impl Into<PathBuf>) -> Self {
        Self {
            mod_dir: mod_dir.into(),
            loaded_mods: HashMap::new(),
            packet_dispatcher: LockFreeDispatcher::new(),
            render_dispatcher: LockFreeDispatcher::new(),
        }
    }

    pub fn has_mod(&self, id: &str) -> bool {
        self.loaded_mods.contains_key(id)
    }

    pub fn mod_count(&self) -> usize {
        self.loaded_mods.len()
    }

    pub fn discover_and_load_all(&mut self, registry: &mut ModRegistry, is_client: bool) -> Result<(), String> {
        let rt = runtime_or_init(self.mod_dir.clone());
        let result = load_mods_into_runtime(&self.mod_dir, rt, is_client)?;
        let loaded_ids = result.loaded.clone();
        pin_loaded_libraries(result);
        for id in &loaded_ids {
            self.loaded_mods.insert(id.clone(), LoadedMod { id: id.clone() });
        }
        if let Some(r) = rsift_api::runtime::runtime() {
            for &f in r.packet_handlers.read().unwrap().iter() {
                self.packet_dispatcher.add(f);
            }
            for &f in r.render_handlers.read().unwrap().iter() {
                self.render_dispatcher.add(f);
            }
        }
        info!("Loaded {} DLL mod(s) via global runtime", self.loaded_mods.len());
        Ok(())
    }

    #[inline(always)]
    pub fn dispatch_packet(&self, packet_id: u32, ptr: i64, len: i32) -> bool {
        if let Some(rt) = rsift_api::runtime::runtime() {
            return rt.dispatch_packet(packet_id, ptr, len);
        }
        for &fn_ptr in self.packet_dispatcher.get_slice() {
            if !fn_ptr(packet_id, ptr, len) {
                return false;
            }
        }
        true
    }

    #[inline(always)]
    pub fn dispatch_render(&self, width: u32, height: u32, delta_time: f32) {
        if let Some(rt) = rsift_api::runtime::runtime() {
            rt.dispatch_render(width, height, delta_time);
            return;
        }
        for &fn_ptr in self.render_dispatcher.get_slice() {
            fn_ptr(width, height, delta_time);
        }
    }
}

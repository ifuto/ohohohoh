//! Per-module handles — each maps to a concrete Rsift API surface wired at runtime.

use super::SuiteModuleId;
use std::sync::Arc;

#[derive(Clone)]
pub struct SuiteModuleHandle {
    pub id: SuiteModuleId,
    pub is_active: bool,
    pub binding: &'static str,
}

impl SuiteModuleHandle {
    pub fn new(id: SuiteModuleId) -> Self {
        Self {
            id,
            is_active: false,
            binding: id.binding(),
        }
    }

    pub fn activate(&mut self) -> Result<(), String> {
        self.is_active = true;
        Ok(())
    }
}

/// Typed accessor for renderer registration (mods call via `ModContext::suite()`).
#[derive(Clone, Default)]
pub struct RendererApi {
    pub mesh_providers: Arc<std::sync::RwLock<Vec<String>>>,
}

impl RendererApi {
    pub fn register_mesh_provider(&self, mod_id: &str) {
        if let Ok(mut v) = self.mesh_providers.write() {
            v.push(mod_id.to_string());
        }
    }
}

#[derive(Clone, Default)]
pub struct NetworkingApi {
    pub channel_count: Arc<std::sync::atomic::AtomicU32>,
}

impl NetworkingApi {
    pub fn register_play_channel(&self, _namespace: &str, _name: &str) {
        self.channel_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

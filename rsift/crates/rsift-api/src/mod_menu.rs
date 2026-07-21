//! # Mod Menu Screen & Icon Catalog (`mod_menu`)
//!
//! Opens a real Minecraft host screen populated with mod entries via the platform bridge.

use crate::mod_api::ModManifest;
use crate::cloth_config::ClothConfigBuilder;
use crate::platform::{mark_dirty, request_open_screen};
use crate::os_integ::OsIntegration;
use std::sync::{Arc, RwLock};
use std::collections::HashMap;
use tracing::{info, debug};

/// Mod リストエントリー
#[derive(Clone)]
pub struct ModMenuEntry {
    pub manifest: ModManifest,
    pub icon_gpu_texture_id: Option<u32>,
    pub homepage_url: Option<String>,
    pub cloth_config_screen: Option<ClothConfigBuilder>,
}

/// Mod Menu 管理・描画スクリーン
#[derive(Default, Clone)]
pub struct RsiftModMenuScreen {
    pub entries: Arc<RwLock<HashMap<String, ModMenuEntry>>>,
    pub selected_mod_id: Arc<RwLock<Option<String>>>,
    pub is_open: Arc<RwLock<bool>>,
}

impl RsiftModMenuScreen {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_mod(
        &self,
        manifest: ModManifest,
        icon_id: Option<u32>,
        url: Option<&str>,
        config: Option<ClothConfigBuilder>,
    ) {
        debug!(
            "[ModMenu] Registering mod to catalog: {} (ID: {})",
            manifest.name, manifest.id
        );
        if let Ok(mut map) = self.entries.write() {
            map.insert(
                manifest.id.clone(),
                ModMenuEntry {
                    manifest,
                    icon_gpu_texture_id: icon_id,
                    homepage_url: url.map(|s| s.to_string()),
                    cloth_config_screen: config,
                },
            );
        }
        mark_dirty();
    }

    /// Open the Minecraft-hosted Mod Menu screen (buttons injected by platform bridge).
    pub fn open_screen(&self) {
        if let Ok(mut guard) = self.is_open.write() {
            *guard = true;
        }
        info!("[ModMenu] Opening Minecraft Mod Menu screen");
        request_open_screen("mod_menu");
    }

    pub fn close_screen(&self) {
        if let Ok(mut guard) = self.is_open.write() {
            *guard = false;
        }
    }

    pub fn select_mod(&self, id: &str) {
        *self.selected_mod_id.write().unwrap() = Some(id.to_string());
    }

    pub fn open_selected_config(&self) {
        let id = self.selected_mod_id.read().unwrap().clone();
        let Some(id) = id else { return };
        if let Ok(map) = self.entries.read() {
            if let Some(entry) = map.get(&id) {
                if let Some(cfg) = &entry.cloth_config_screen {
                    cfg.open_screen();
                }
            }
        }
    }

    pub fn open_selected_homepage(&self) {
        let id = self.selected_mod_id.read().unwrap().clone();
        let Some(id) = id else { return };
        if let Ok(map) = self.entries.read() {
            if let Some(entry) = map.get(&id) {
                if let Some(url) = &entry.homepage_url {
                    let _ = OsIntegration::open_url_in_browser(url);
                }
            }
        }
    }

    pub fn render_catalog_frame(&self) {
        // Kept for logging diagnostics; real UI is the Minecraft screen.
        if let Ok(map) = self.entries.read() {
            info!("[ModMenu] catalog entries={}", map.len());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mod_api::ModManifest;

    fn manifest(id: &str) -> ModManifest {
        ModManifest {
            id: id.into(),
            name: format!("Mod {id}"),
            version: "1.0.0".into(),
            author: "tester".into(),
            description: "test".into(),
            target_rsift_version: "1.21.11".into(),
        }
    }

    #[test]
    fn register_mod_counts_distinct_ids() {
        let m = RsiftModMenuScreen::new();
        assert_eq!(m.entries.read().unwrap().len(), 0);
        m.register_mod(manifest("a"), None, None, None);
        m.register_mod(manifest("b"), Some(50000), Some("https://example.com"), None);
        let map = m.entries.read().unwrap();
        assert_eq!(map.len(), 2);
        let b = map.get("b").unwrap();
        assert_eq!(b.icon_gpu_texture_id, Some(50000));
        assert_eq!(b.homepage_url.as_deref(), Some("https://example.com"));
        assert_eq!(b.manifest.name, "Mod b");
    }

    #[test]
    fn register_mod_same_id_overwrites_not_duplicates() {
        // タイトルの Mods (N) 表示が参照するカタログ数は排他的 id 数で
        // なければならない (同一 id の再登録 = バージョン更新)。
        let m = RsiftModMenuScreen::new();
        let mut v1 = manifest("a");
        v1.version = "1.0.0".into();
        m.register_mod(v1, None, None, None);
        let mut v2 = manifest("a");
        v2.version = "2.0.0".into();
        m.register_mod(v2, None, None, None);
        let map = m.entries.read().unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("a").unwrap().manifest.version, "2.0.0");
    }

    #[test]
    fn open_close_selection_state_machine_is_noop_safe() {
        let m = RsiftModMenuScreen::new();
        assert!(!*m.is_open.read().unwrap());
        m.open_screen();
        assert!(*m.is_open.read().unwrap());
        m.close_screen();
        assert!(!*m.is_open.read().unwrap());
        // 未選択・未登録 id 選択の両方で config/homepage は no-op (panic しない)。
        m.open_selected_config();
        m.open_selected_homepage();
        m.select_mod("missing");
        m.open_selected_config();
        m.open_selected_homepage();
        assert_eq!(m.selected_mod_id.read().unwrap().as_deref(), Some("missing"));
    }
}

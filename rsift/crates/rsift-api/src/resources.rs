//! # Rsift Resource & Data Pack System (Fabric Resource Loader Parity)
//!
//! Fabric の `ResourceManagerHelper`, `ServerDataPackLoader`,
//! リソースリロードリスナー、およびルートテーブル動的修正イベントを実装します。

use crate::registry::RegistryKey;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, debug};

/// リソースのタイプ（Client アセット または Server データパック）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceType {
    ClientAssets,
    ServerDataPacks,
}

/// リソースリロードリスナートレイト (`IdentifiableResourceReloadListener`)
pub trait ResourceReloadListener: Send + Sync {
    /// リスナの識別IDを取得 (`rsift:custom_shader_loader` など)
    fn get_fabric_id(&self) -> RegistryKey;

    /// リソースがリロードされた際にコールバックされる
    fn on_reload(&self, resource_type: ResourceType, manager: &dyn ResourceManager) -> Result<(), String>;
}

/// リソースマネージャーインターフェース
pub trait ResourceManager: Send + Sync {
    /// 指定したパスのリソースデータ（テクスチャ、シェーダー、JSONデータパック等）の生バイトを取得する
    fn get_resource(&self, key: &RegistryKey) -> Option<Vec<u8>>;

    /// 指定した名前空間の全リソースIDを探索する
    fn find_resources(&self, namespace: &str, prefix: &str) -> Vec<RegistryKey>;
}

/// ルートテーブル修正イベント (`LootTableEvents::MODIFY`)
#[derive(Debug, Clone)]
pub struct LootPoolEntry {
    pub item_key: RegistryKey,
    pub weight: u32,
    pub min_count: u32,
    pub max_count: u32,
}

pub type LootTableModifyFn = Arc<dyn Fn(&RegistryKey, &mut Vec<LootPoolEntry>) + Send + Sync>;

/// Bound loot modifier: always associated with a concrete table key for platform apply.
#[derive(Clone)]
pub struct BoundLootTableModifier {
    pub table: RegistryKey,
    pub modifier: LootTableModifyFn,
}

/// 統合リソース＆データパックローダー
#[derive(Default, Clone)]
pub struct ResourceLoader {
    pub reload_listeners: HashMap<ResourceType, Vec<Arc<dyn ResourceReloadListener>>>,
    pub loot_table_modifiers: Vec<BoundLootTableModifier>,
}

impl ResourceLoader {
    pub fn new() -> Self {
        Self::default()
    }

    /// リロードリスナーを登録 (`ResourceManagerHelper.registerReloadListener`)
    pub fn register_reload_listener(&mut self, res_type: ResourceType, listener: Arc<dyn ResourceReloadListener>) {
        info!(
            "Registering ResourceReloadListener [{}] for type {:?}",
            listener.get_fabric_id().as_str(),
            res_type
        );
        self.reload_listeners.entry(res_type).or_default().push(listener);
        crate::platform::mark_dirty();
    }

    /// ルートテーブル修正フックを登録 (`LootTableEvents.MODIFY.register`) — all tables.
    /// Prefer [`Self::register_loot_table_modifier_for`] when the target table is known.
    pub fn register_loot_table_modifier(&mut self, modifier: LootTableModifyFn) {
        self.register_loot_table_modifier_for(RegistryKey::new("*", "*"), modifier);
    }

    /// Register a loot modifier bound to a specific loot table key (stored for JVM apply).
    pub fn register_loot_table_modifier_for(&mut self, table: RegistryKey, modifier: LootTableModifyFn) {
        debug!(
            "Registering LootTable modification for [{}]",
            table.as_str()
        );
        self.loot_table_modifiers.push(BoundLootTableModifier { table, modifier });
        crate::platform::mark_dirty();
    }

    /// すべてのリロードリスナーへリロードイベントを発火する
    pub fn dispatch_reload(&self, res_type: ResourceType, manager: &dyn ResourceManager) {
        if let Some(listeners) = self.reload_listeners.get(&res_type) {
            for listener in listeners {
                if let Err(e) = listener.on_reload(res_type, manager) {
                    tracing::error!("Error in reload listener [{}]: {}", listener.get_fabric_id().as_str(), e);
                }
            }
        }
    }

    /// ルートテーブル（ドロップアイテム）を動的修正する
    pub fn modify_loot_table(&self, table_key: &RegistryKey, entries: &mut Vec<LootPoolEntry>) {
        for bound in &self.loot_table_modifiers {
            let wildcard = bound.table.namespace == "*" && bound.table.path == "*";
            if wildcard || &bound.table == table_key {
                (bound.modifier)(table_key, entries);
            }
        }
    }
}

/// Data-generation exporter used by fabric module catalog.
#[derive(Default, Clone)]
pub struct DatagenExporter {
    pub queued_json: Vec<(String, String)>,
}

impl DatagenExporter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn queue_json(&mut self, path: &str, json: &str) {
        self.queued_json.push((path.to_string(), json.to_string()));
        crate::platform::mark_dirty();
    }

    pub fn flush_to_game_dir(&self, game_dir: &std::path::Path) -> Result<usize, String> {
        let mut n = 0usize;
        for (path, json) in &self.queued_json {
            let full = game_dir.join(path);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(&full, json).map_err(|e| e.to_string())?;
            n += 1;
        }
        Ok(n)
    }
}

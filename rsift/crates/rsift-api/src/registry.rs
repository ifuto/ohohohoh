//! # Rsift Registry System
//!
//! 新規Mob、ブロック、アイテム、パケットID、およびカスタム計算パイプラインの
//! 登録を管理するレジストリシステムです。

use std::collections::HashMap;
use tracing::info;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RegistryKey {
    pub namespace: String,
    pub path: String,
}

impl RegistryKey {
    pub fn new(namespace: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            path: path.into(),
        }
    }

    pub fn as_str(&self) -> String {
        format!("{}:{}", self.namespace, self.path)
    }
}

/// カスタムエンティティ (Mob) の定義
#[derive(Debug, Clone)]
pub struct EntityDefinition {
    pub key: RegistryKey,
    pub entity_type_id: u32,
    pub max_health: f32,
    pub speed: f32,
    pub attack_damage: f32,
    pub ai_controller_dll_symbol: String,
}

/// カスタムブロック / アイテムの定義
#[derive(Debug, Clone)]
pub struct ItemDefinition {
    pub key: RegistryKey,
    pub raw_id: u32,
    pub max_stack_size: u8,
}

/// 統合レジストリマネージャー
#[derive(Default, Debug)]
pub struct ModRegistry {
    pub entities: HashMap<RegistryKey, EntityDefinition>,
    pub items: HashMap<RegistryKey, ItemDefinition>,
    pub custom_packet_handlers: HashMap<u32, String>,
    next_entity_id: u32,
    next_item_id: u32,
}

impl ModRegistry {
    pub fn new() -> Self {
        Self {
            entities: HashMap::new(),
            items: HashMap::new(),
            custom_packet_handlers: HashMap::new(),
            next_entity_id: 1000, // Minecraft 1.21.11 のバニラIDと競合しない高位IDから開始
            next_item_id: 5000,
        }
    }

    /// 新しいカスタムMob (エンティティ) を登録
    pub fn register_entity(&mut self, namespace: &str, name: &str, max_hp: f32, speed: f32) -> u32 {
        let key = RegistryKey::new(namespace, name);
        let id = self.next_entity_id;
        self.next_entity_id += 1;

        let def = EntityDefinition {
            key: key.clone(),
            entity_type_id: id,
            max_health: max_hp,
            speed,
            attack_damage: 4.0,
            ai_controller_dll_symbol: format!("{}_{}_ai_step", namespace, name),
        };

        info!("Registering custom Entity: {} (ID: {})", key.as_str(), id);
        self.entities.insert(key, def);
        crate::platform::mark_dirty();
        id
    }

    /// 新しいカスタムアイテムを登録
    pub fn register_item(&mut self, namespace: &str, name: &str, max_stack: u8) -> u32 {
        let key = RegistryKey::new(namespace, name);
        let id = self.next_item_id;
        self.next_item_id += 1;

        let def = ItemDefinition {
            key: key.clone(),
            raw_id: id,
            max_stack_size: max_stack,
        };

        info!("Registering custom Item: {} (ID: {})", key.as_str(), id);
        self.items.insert(key, def);
        crate::platform::mark_dirty();
        id
    }

    pub fn register_packet_handler_symbol(&mut self, packet_id: u32, symbol: &str) {
        self.custom_packet_handlers.insert(packet_id, symbol.to_string());
        crate::platform::mark_dirty();
    }
}

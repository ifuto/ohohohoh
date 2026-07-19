//! # NeoForge Two-Event-Bus System (`IEventBus` & `NeoForge.EVENT_BUS`)
//!
//! NeoForge 1.20.x / 1.21.x の根幹である2つの独立したイベントバス
//! （モッドロード用 `ModEventBus` とゲーム実行中用 `GameEventBus`）、
//! イベントキャンセル機能 (`ICancellableEvent`)、および優先度 (`EventPriority`) を
//! 1ミリたりとも余すことなく Rust ネイティブで実装します。

use std::sync::{Arc, RwLock};
use std::collections::HashMap;
use tracing::{info, debug, trace};

/// イベントのリスナー優先度 (`net.neoforged.bus.api.EventPriority`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum EventPriority {
    Highest = 0,
    High = 1,
    Normal = 2,
    Low = 3,
    Lowest = 4,
}

/// キャンセル可能イベントのベーストレイト (`ICancellableEvent`)
pub trait CancellableEvent {
    fn is_canceled(&self) -> bool;
    fn set_canceled(&mut self, canceled: bool);
}

/// イベントキャンセル状態の汎用ラッパー
#[derive(Debug, Default, Clone)]
pub struct EventCancelState {
    pub canceled: bool,
}

impl CancellableEvent for EventCancelState {
    fn is_canceled(&self) -> bool {
        self.canceled
    }
    fn set_canceled(&mut self, canceled: bool) {
        self.canceled = canceled;
    }
}

/// ============================================================================
/// Mod Event Bus (`IEventBus`) - ライフサイクル、レジストリ、設定用バス
/// ============================================================================

#[derive(Debug, Clone)]
pub struct FMLCommonSetupEvent {
    pub mod_id: String,
}

#[derive(Debug, Clone)]
pub struct FMLClientSetupEvent {
    pub mod_id: String,
    pub minecraft_version: String,
}

#[derive(Debug, Clone)]
pub struct BuildCreativeModeTabContentsEvent {
    pub tab_key: String,
    pub entries: Arc<RwLock<Vec<String>>>,
}

impl BuildCreativeModeTabContentsEvent {
    pub fn get_tab_key(&self) -> &str {
        &self.tab_key
    }
    pub fn accept(&self, item_id: impl Into<String>) {
        if let Ok(mut list) = self.entries.write() {
            list.push(item_id.into());
        }
    }
}

pub type CommonSetupFn = Arc<dyn Fn(&FMLCommonSetupEvent) + Send + Sync>;
pub type ClientSetupFn = Arc<dyn Fn(&FMLClientSetupEvent) + Send + Sync>;
pub type CreativeTabFn = Arc<dyn Fn(&BuildCreativeModeTabContentsEvent) + Send + Sync>;

/// Modバス (`IEventBus`) インターフェース
#[derive(Default, Clone)]
pub struct ModEventBus {
    pub common_setup_listeners: Vec<(EventPriority, CommonSetupFn)>,
    pub client_setup_listeners: Vec<(EventPriority, ClientSetupFn)>,
    pub creative_tab_listeners: Vec<(EventPriority, CreativeTabFn)>,
}

impl ModEventBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_common_setup_listener(&mut self, priority: EventPriority, listener: CommonSetupFn) {
        debug!("Adding CommonSetup listener to ModEventBus (Priority: {:?})", priority);
        self.common_setup_listeners.push((priority, listener));
        self.common_setup_listeners.sort_by_key(|k| k.0);
    }

    pub fn add_client_setup_listener(&mut self, priority: EventPriority, listener: ClientSetupFn) {
        self.client_setup_listeners.push((priority, listener));
        self.client_setup_listeners.sort_by_key(|k| k.0);
    }

    pub fn add_creative_tab_listener(&mut self, priority: EventPriority, listener: CreativeTabFn) {
        self.creative_tab_listeners.push((priority, listener));
        self.creative_tab_listeners.sort_by_key(|k| k.0);
    }

    pub fn dispatch_common_setup(&self, mod_id: &str) {
        let event = FMLCommonSetupEvent { mod_id: mod_id.to_string() };
        for (_, cb) in &self.common_setup_listeners {
            cb(&event);
        }
    }

    pub fn dispatch_creative_tab(&self, tab_key: &str) -> Vec<String> {
        let entries = Arc::new(RwLock::new(Vec::new()));
        let event = BuildCreativeModeTabContentsEvent {
            tab_key: tab_key.to_string(),
            entries: entries.clone(),
        };
        for (_, cb) in &self.creative_tab_listeners {
            cb(&event);
        }
        let res = entries.read().map(|v| v.clone()).unwrap_or_default();
        res
    }
}

/// ============================================================================
/// Game Event Bus (`NeoForge.EVENT_BUS`) - ゲーム中の動的フックバス
/// ============================================================================

#[derive(Debug, Clone)]
pub struct ServerStartingEvent {
    pub server_name: String,
}

#[derive(Debug, Clone)]
pub struct LivingDamageEvent {
    pub entity_id: u32,
    pub entity_type: String,
    pub damage_source: String,
    pub original_amount: f32,
    pub new_amount: f32,
    pub cancel_state: EventCancelState,
}

impl LivingDamageEvent {
    pub fn new(entity_id: u32, entity_type: &str, source: &str, amount: f32) -> Self {
        Self {
            entity_id,
            entity_type: entity_type.to_string(),
            damage_source: source.to_string(),
            original_amount: amount,
            new_amount: amount,
            cancel_state: EventCancelState::default(),
        }
    }
    pub fn get_amount(&self) -> f32 { self.new_amount }
    pub fn set_amount(&mut self, amount: f32) { self.new_amount = amount; }
}

impl CancellableEvent for LivingDamageEvent {
    fn is_canceled(&self) -> bool { self.cancel_state.is_canceled() }
    fn set_canceled(&mut self, canceled: bool) { self.cancel_state.set_canceled(canceled); }
}

#[derive(Debug, Clone)]
pub struct BlockBreakEvent {
    pub player_uuid: String,
    pub block_key: String,
    pub x: i32, pub y: i32, pub z: i32,
    pub cancel_state: EventCancelState,
}

impl CancellableEvent for BlockBreakEvent {
    fn is_canceled(&self) -> bool { self.cancel_state.is_canceled() }
    fn set_canceled(&mut self, canceled: bool) { self.cancel_state.set_canceled(canceled); }
}

pub type ServerStartingFn = Arc<dyn Fn(&ServerStartingEvent) + Send + Sync>;
pub type LivingDamageFn = Arc<dyn Fn(&mut LivingDamageEvent) + Send + Sync>;
pub type BlockBreakFn = Arc<dyn Fn(&mut BlockBreakEvent) + Send + Sync>;

/// ゲームバス (`NeoForge.EVENT_BUS`) インターフェース
#[derive(Default, Clone)]
pub struct GameEventBus {
    pub server_starting_listeners: Vec<(EventPriority, ServerStartingFn)>,
    pub living_damage_listeners: Vec<(EventPriority, LivingDamageFn)>,
    pub block_break_listeners: Vec<(EventPriority, BlockBreakFn)>,
}

impl GameEventBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_server_starting_listener(&mut self, priority: EventPriority, listener: ServerStartingFn) {
        self.server_starting_listeners.push((priority, listener));
        self.server_starting_listeners.sort_by_key(|k| k.0);
    }

    pub fn add_living_damage_listener(&mut self, priority: EventPriority, listener: LivingDamageFn) {
        info!("Registering NeoForge LivingDamageEvent listener on GameBus (Priority: {:?})", priority);
        self.living_damage_listeners.push((priority, listener));
        self.living_damage_listeners.sort_by_key(|k| k.0);
    }

    pub fn add_block_break_listener(&mut self, priority: EventPriority, listener: BlockBreakFn) {
        self.block_break_listeners.push((priority, listener));
        self.block_break_listeners.sort_by_key(|k| k.0);
    }

    /// LivingDamageEvent をディスパッチし、キャンセルされた場合は None を、そうでない場合は修正後ダメージ量を返す
    pub fn dispatch_living_damage(&self, entity_id: u32, entity_type: &str, source: &str, amount: f32) -> Option<f32> {
        let mut event = LivingDamageEvent::new(entity_id, entity_type, source, amount);
        for (_, cb) in &self.living_damage_listeners {
            cb(&mut event);
            if event.is_canceled() {
                trace!("LivingDamageEvent canceled by listener!");
                return None;
            }
        }
        Some(event.get_amount())
    }

    /// BlockBreakEvent をディスパッチし、ブロック破壊の可否を返す
    pub fn dispatch_block_break(&self, player: &str, block: &str, x: i32, y: i32, z: i32) -> bool {
        let mut event = BlockBreakEvent {
            player_uuid: player.to_string(),
            block_key: block.to_string(),
            x, y, z,
            cancel_state: EventCancelState::default(),
        };
        for (_, cb) in &self.block_break_listeners {
            cb(&mut event);
            if event.is_canceled() {
                return false;
            }
        }
        true
    }
}

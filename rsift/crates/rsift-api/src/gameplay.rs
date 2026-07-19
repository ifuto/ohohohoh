//! # Rsift Gameplay & UI System (Fabric Command, KeyBinding & UI Parity)
//!
//! Fabric で提供されるコマンド登録 (`CommandRegistrationCallback`)、
//! キーバインドショートカット (`KeyBindingHelper`)、アイテムグループ/クリエイティブタブ (`ItemGroupEvents`)、
//! およびスクリーン・コンテナ UI (`HandledScreenRegistry`, `ScreenHandlerRegistry`) を実装します。

use crate::registry::RegistryKey;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, debug};

/// コマンドディスパッチャ（Brigadier コマンドツリーの Rust ラッパー）
#[derive(Debug, Clone)]
pub struct CommandNode {
    pub name: String,
    pub description: String,
    pub permission_level: u8, // 0 = player, 2 = cheat/op, 4 = console/server
    pub dll_callback_symbol: String,
}

pub type CommandRegisterFn = Arc<dyn Fn(&mut CommandTree) + Send + Sync>;

#[derive(Default, Debug, Clone)]
pub struct CommandTree {
    pub root_commands: HashMap<String, CommandNode>,
}

impl CommandTree {
    pub fn register(&mut self, name: &str, desc: &str, perm: u8, symbol: &str) {
        info!("Registering Command: `/{}` (Permission Level: {})", name, perm);
        self.root_commands.insert(name.to_string(), CommandNode {
            name: name.to_string(),
            description: desc.to_string(),
            permission_level: perm,
            dll_callback_symbol: symbol.to_string(),
        });
    }
}

/// キーバインド（キーボード/マウスのショートカット）定義 (`KeyBindingHelper`)
#[derive(Debug, Clone)]
pub struct KeyBindingDefinition {
    pub id: String,
    pub translation_key: String,
    pub default_key_code: i32, // GLFW key code
    pub category: String,
    pub dll_on_press_symbol: String,
}

/// クリエイティブタブ/アイテムグループへのアイテム追加フック (`ItemGroupEvents`)
#[derive(Debug, Clone)]
pub enum ItemGroupSelector {
    BuildingBlocks,
    ColoredBlocks,
    Natural,
    Functional,
    Redstone,
    Hotbar,
    Search,
    Tools,
    Combat,
    FoodAndDrinks,
    SpawnEggs,
    Custom(RegistryKey),
}

pub type ItemGroupModifyFn = Arc<dyn Fn(&mut Vec<RegistryKey>) + Send + Sync>;

/// コンテナ・スクリーン UI (`HandledScreenRegistry` / `ScreenHandlerRegistry`)
#[derive(Debug, Clone)]
pub struct ScreenHandlerDefinition {
    pub key: RegistryKey,
    pub handler_type_id: u32,
    pub gui_texture_path: String,
    pub dll_init_symbol: String,
}

/// 統合ゲームプレイレジストリ
#[derive(Default, Clone)]
pub struct GameplayRegistry {
    pub command_tree: CommandTree,
    pub command_register_callbacks: Vec<CommandRegisterFn>,
    pub keybindings: HashMap<String, KeyBindingDefinition>,
    pub item_group_modifiers: HashMap<String, Vec<ItemGroupModifyFn>>,
    pub screen_handlers: HashMap<RegistryKey, ScreenHandlerDefinition>,
}

impl GameplayRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// コマンド登録コールバックを登録 (`CommandRegistrationCallback.EVENT.register`)
    pub fn register_command_callback(&mut self, cb: CommandRegisterFn) {
        self.command_register_callbacks.push(cb);
        crate::platform::mark_dirty();
    }

    /// すべてのコールバックを実行してコマンドツリーを構築する
    pub fn build_commands(&mut self) {
        let mut tree = CommandTree::default();
        for cb in &self.command_register_callbacks {
            cb(&mut tree);
        }
        self.command_tree = tree;
        info!("Command tree built with {} root command(s)", self.command_tree.root_commands.len());
        crate::platform::mark_dirty();
    }

    /// キーバインドを登録 (`KeyBindingHelper.registerKeyBinding`)
    pub fn register_keybinding(&mut self, id: &str, trans: &str, code: i32, cat: &str, symbol: &str) {
        info!("Registering KeyBinding: [{}] key={}", id, code);
        self.keybindings.insert(id.to_string(), KeyBindingDefinition {
            id: id.to_string(),
            translation_key: trans.to_string(),
            default_key_code: code,
            category: cat.to_string(),
            dll_on_press_symbol: symbol.to_string(),
        });
        crate::platform::mark_dirty();
    }

    /// クリエイティブタブへのアイテム追加を登録 (`ItemGroupEvents.modifyEntriesEvent`)
    pub fn modify_item_group(&mut self, group_name: &str, modifier: ItemGroupModifyFn) {
        debug!("Registering ItemGroup modifier for [{}]", group_name);
        self.item_group_modifiers.entry(group_name.to_string()).or_default().push(modifier);
        crate::platform::mark_dirty();
    }

    /// スクリーンとハンドラーを登録 (`HandledScreenRegistry.register`)
    pub fn register_screen_handler(&mut self, namespace: &str, name: &str, texture: &str, symbol: &str) -> u32 {
        let key = RegistryKey::new(namespace, name);
        let id = self.screen_handlers.len() as u32 + 2000;
        info!("Registering ScreenHandler: {} (ID: {})", key.as_str(), id);
        self.screen_handlers.insert(key.clone(), ScreenHandlerDefinition {
            key,
            handler_type_id: id,
            gui_texture_path: texture.to_string(),
            dll_init_symbol: symbol.to_string(),
        });
        crate::platform::mark_dirty();
        id
    }
}

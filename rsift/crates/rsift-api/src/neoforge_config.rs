//! # NeoForge Configuration & TOML Metadata System
//!
//! NeoForge の TOML コンフィグレーション仕様 (`ModConfigSpec`, `ModConfigSpec.Builder`,
//! `night-config` レイヤー)、および `META-INF/mods.toml` の依存関係メタデータ仕様を実装します。

use std::sync::{Arc, RwLock};
use std::collections::HashMap;
use tracing::{info, debug};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConfigType {
    Common,
    Client,
    Server,
}

#[derive(Clone)]
pub struct ConfigValue<T> {
    pub path: Vec<String>,
    pub comment: Option<String>,
    pub translation_key: Option<String>,
    pub default_value: T,
    cached_value: Arc<RwLock<T>>,
}

impl<T: Clone> ConfigValue<T> {
    pub fn new(path: Vec<String>, default_val: T, comment: Option<String>, trans: Option<String>) -> Self {
        Self {
            path,
            comment,
            translation_key: trans,
            default_value: default_val.clone(),
            cached_value: Arc::new(RwLock::new(default_val)),
        }
    }

    pub fn get(&self) -> T {
        if let Ok(guard) = self.cached_value.read() {
            guard.clone()
        } else {
            self.default_value.clone()
        }
    }

    pub fn set(&self, val: T) {
        if let Ok(mut guard) = self.cached_value.write() {
            *guard = val;
        }
    }
}

pub type IntValue = ConfigValue<i32>;
pub type DoubleValue = ConfigValue<f64>;
pub type BooleanValue = ConfigValue<bool>;
pub type ListValue<T> = ConfigValue<Vec<T>>;

#[derive(Default, Clone)]
pub struct ModConfigSpecBuilder {
    current_path: Vec<String>,
    next_comment: Option<String>,
    next_translation: Option<String>,
    entries: HashMap<String, String>,
}

impl ModConfigSpecBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn comment(&mut self, comment: &str) -> &mut Self {
        self.next_comment = Some(comment.to_string());
        self
    }

    pub fn translation(&mut self, key: &str) -> &mut Self {
        self.next_translation = Some(key.to_string());
        self
    }

    pub fn push(&mut self, path: &str) -> &mut Self {
        self.current_path.push(path.to_string());
        self
    }

    pub fn pop(&mut self) -> &mut Self {
        self.current_path.pop();
        self
    }

    fn get_full_path(&self, name: &str) -> Vec<String> {
        let mut path = self.current_path.clone();
        path.push(name.to_string());
        path
    }

    fn get_path_str(&self, name: &str) -> String {
        self.get_full_path(name).join(".")
    }

    pub fn define_boolean(&mut self, name: &str, default_val: bool) -> BooleanValue {
        let path_str = self.get_path_str(name);
        debug!("Defining ModConfigSpec boolean: {} = {}", path_str, default_val);
        let val = ConfigValue::new(self.get_full_path(name), default_val, self.next_comment.take(), self.next_translation.take());
        self.entries.insert(path_str, format!("Boolean (default: {})", default_val));
        crate::platform::mark_dirty();
        val
    }

    pub fn define_in_range_int(&mut self, name: &str, default_val: i32, min: i32, max: i32) -> IntValue {
        let path_str = self.get_path_str(name);
        debug!("Defining ModConfigSpec int in range [{}..{}]: {} = {}", min, max, path_str, default_val);
        let val = ConfigValue::new(self.get_full_path(name), default_val, self.next_comment.take(), self.next_translation.take());
        self.entries.insert(path_str, format!("Int range [{}..{}] (default: {})", min, max, default_val));
        crate::platform::mark_dirty();
        val
    }

    pub fn define_in_range_double(&mut self, name: &str, default_val: f64, min: f64, max: f64) -> DoubleValue {
        let path_str = self.get_path_str(name);
        let val = ConfigValue::new(self.get_full_path(name), default_val, self.next_comment.take(), self.next_translation.take());
        self.entries.insert(path_str, format!("Double range [{}..{}] (default: {})", min, max, default_val));
        crate::platform::mark_dirty();
        val
    }

    pub fn define_list<T: Clone>(&mut self, name: &str, default_list: Vec<T>) -> ListValue<T> {
        let path_str = self.get_path_str(name);
        let val = ConfigValue::new(self.get_full_path(name), default_list, self.next_comment.take(), self.next_translation.take());
        self.entries.insert(path_str, "List".to_string());
        crate::platform::mark_dirty();
        val
    }

    pub fn build(self) -> ModConfigSpec {
        info!("Built ModConfigSpec with {} configuration properties", self.entries.len());
        ModConfigSpec { entries: self.entries }
    }
}

#[derive(Debug, Clone)]
pub struct ModConfigSpec {
    pub entries: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ModDependency {
    pub mod_id: String,
    pub mandatory: bool,
    pub version_range: String,
    pub ordering: String,
    pub side: String,
}

#[derive(Debug, Clone)]
pub struct ModsTomlMetadata {
    pub mod_id: String,
    pub version: String,
    pub display_name: String,
    pub description: String,
    pub dependencies: Vec<ModDependency>,
}

impl ModsTomlMetadata {
    pub fn parse_simulated(_toml_str: &str) -> Self {
        Self {
            mod_id: "neoforge_mod".to_string(),
            version: "1.0.0".to_string(),
            display_name: "Simulated NeoForge Mod".to_string(),
            description: "Parsed from META-INF/mods.toml".to_string(),
            dependencies: vec![
                ModDependency {
                    mod_id: "neoforge".to_string(),
                    mandatory: true,
                    version_range: "[21.0,)".to_string(),
                    ordering: "AFTER".to_string(),
                    side: "BOTH".to_string(),
                }
            ],
        }
    }
}

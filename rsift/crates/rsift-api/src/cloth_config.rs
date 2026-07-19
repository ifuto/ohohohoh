//! # Cloth Config Native Engine (`cloth_config`)
//!
//! Builds config UI descriptors and opens a real Minecraft screen via
//! `RsiftPlatformBridge` (buttons + labels injected on a host PauseScreen).

use crate::platform::{mark_dirty, request_open_cloth};
use std::sync::{Arc, RwLock};
use tracing::{info, debug};

#[derive(Debug, Clone)]
pub enum ConfigEntryType {
    IntSlider { min: i32, max: i32, current: Arc<RwLock<i32>> },
    BooleanToggle { current: Arc<RwLock<bool>> },
    StringField { current: Arc<RwLock<String>> },
    ColorPicker { current_rgba: Arc<RwLock<u32>> },
}

#[derive(Debug, Clone)]
pub struct ConfigEntry {
    pub label: String,
    pub tooltip: Option<String>,
    pub entry_type: ConfigEntryType,
}

#[derive(Debug, Default, Clone)]
pub struct ConfigCategory {
    pub name: String,
    pub icon_symbol: Option<String>,
    pub entries: Vec<ConfigEntry>,
}

#[derive(Default, Clone)]
pub struct ClothConfigBuilder {
    pub title: String,
    pub categories: Vec<ConfigCategory>,
}

impl ClothConfigBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_title(&mut self, title: &str) -> &mut Self {
        self.title = title.to_string();
        self
    }

    pub fn add_category(&mut self, name: &str) -> usize {
        let idx = self.categories.len();
        self.categories.push(ConfigCategory {
            name: name.to_string(),
            icon_symbol: None,
            entries: Vec::new(),
        });
        mark_dirty();
        idx
    }

    pub fn add_int_slider(
        &mut self,
        cat_idx: usize,
        label: &str,
        min: i32,
        max: i32,
        initial: i32,
        tooltip: Option<&str>,
    ) -> Arc<RwLock<i32>> {
        let current = Arc::new(RwLock::new(initial));
        if let Some(cat) = self.categories.get_mut(cat_idx) {
            debug!("[ClothConfig] IntSlider [{}] [{}..{}]", label, min, max);
            cat.entries.push(ConfigEntry {
                label: label.to_string(),
                tooltip: tooltip.map(|s| s.to_string()),
                entry_type: ConfigEntryType::IntSlider {
                    min,
                    max,
                    current: current.clone(),
                },
            });
        }
        mark_dirty();
        current
    }

    pub fn add_bool_toggle(
        &mut self,
        cat_idx: usize,
        label: &str,
        initial: bool,
        tooltip: Option<&str>,
    ) -> Arc<RwLock<bool>> {
        let current = Arc::new(RwLock::new(initial));
        if let Some(cat) = self.categories.get_mut(cat_idx) {
            debug!("[ClothConfig] BooleanToggle [{}] default={}", label, initial);
            cat.entries.push(ConfigEntry {
                label: label.to_string(),
                tooltip: tooltip.map(|s| s.to_string()),
                entry_type: ConfigEntryType::BooleanToggle {
                    current: current.clone(),
                },
            });
        }
        mark_dirty();
        current
    }

    pub fn add_string_field(
        &mut self,
        cat_idx: usize,
        label: &str,
        initial: &str,
        tooltip: Option<&str>,
    ) -> Arc<RwLock<String>> {
        let current = Arc::new(RwLock::new(initial.to_string()));
        if let Some(cat) = self.categories.get_mut(cat_idx) {
            cat.entries.push(ConfigEntry {
                label: label.to_string(),
                tooltip: tooltip.map(|s| s.to_string()),
                entry_type: ConfigEntryType::StringField {
                    current: current.clone(),
                },
            });
        }
        mark_dirty();
        current
    }

    pub fn add_color_picker(
        &mut self,
        cat_idx: usize,
        label: &str,
        initial_rgba: u32,
        tooltip: Option<&str>,
    ) -> Arc<RwLock<u32>> {
        let current = Arc::new(RwLock::new(initial_rgba));
        if let Some(cat) = self.categories.get_mut(cat_idx) {
            cat.entries.push(ConfigEntry {
                label: label.to_string(),
                tooltip: tooltip.map(|s| s.to_string()),
                entry_type: ConfigEntryType::ColorPicker {
                    current_rgba: current.clone(),
                },
            });
        }
        mark_dirty();
        current
    }

    /// Serialize entries and open the real Minecraft config host screen.
    pub fn open_screen(&self) {
        let mut lines = Vec::new();
        for cat in &self.categories {
            lines.push(format!("#{}", cat.name));
            for entry in &cat.entries {
                let value = match &entry.entry_type {
                    ConfigEntryType::IntSlider { min, max, current } => {
                        format!(
                            "int:{}:{}:{}",
                            min,
                            max,
                            current.read().map(|v| *v).unwrap_or(0)
                        )
                    }
                    ConfigEntryType::BooleanToggle { current } => {
                        format!("bool:{}", current.read().map(|v| *v).unwrap_or(false))
                    }
                    ConfigEntryType::StringField { current } => {
                        format!(
                            "str:{}",
                            current.read().map(|v| v.clone()).unwrap_or_default()
                        )
                    }
                    ConfigEntryType::ColorPicker { current_rgba } => {
                        format!(
                            "color:{:08X}",
                            current_rgba.read().map(|v| *v).unwrap_or(0xFFFFFFFF)
                        )
                    }
                };
                lines.push(format!("{}|{}", entry.label, value));
            }
        }
        info!(
            "[ClothConfig] Opening Minecraft screen \"{}\" ({} lines)",
            self.title,
            lines.len()
        );
        request_open_cloth(&self.title, lines);
    }
}

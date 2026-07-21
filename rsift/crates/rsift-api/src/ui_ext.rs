//! # UI Extension API (`ui_ext`)
//!
//! Screen widgets, buttons, and overlay rendering hooks for native DLL mods.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::{debug, info};

/// Screen-space rectangle for widget placement
#[derive(Debug, Clone, Copy)]
pub struct UiRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Button descriptor exposed to JNI screen injector
#[derive(Debug, Clone)]
pub struct ScreenButtonDescriptor {
    pub id: u32,
    pub label: String,
    pub rect: UiRect,
    pub tooltip: Option<String>,
}

type ButtonCallback = Arc<dyn Fn(u32) + Send + Sync>;

#[derive(Clone)]
struct ScreenButton {
    descriptor: ScreenButtonDescriptor,
    callback: ButtonCallback,
}

/// Minecraft screen injection registry (TitleScreen buttons, screen redirects)
#[derive(Clone)]
pub struct ScreenRegistry {
    buttons: Arc<RwLock<HashMap<String, Vec<ScreenButton>>>>,
    redirects: Arc<RwLock<HashMap<String, String>>>,
    next_widget_id: Arc<RwLock<u32>>,
    lifecycle: Arc<RwLock<Vec<ScreenLifecycleListener>>>,
}

impl Default for ScreenRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenRegistry {
    pub fn new() -> Self {
        Self {
            buttons: Arc::new(RwLock::new(HashMap::new())),
            redirects: Arc::new(RwLock::new(HashMap::new())),
            next_widget_id: Arc::new(RwLock::new(9000)),
            lifecycle: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn add_button<F>(
        &self,
        screen_class: &str,
        label: &str,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        tooltip: Option<&str>,
        callback: F,
    ) where
        F: Fn(u32) + Send + Sync + 'static,
    {
        let mut id_guard = self.next_widget_id.write().unwrap();
        let widget_id = *id_guard;
        *id_guard += 1;

        let button = ScreenButton {
            descriptor: ScreenButtonDescriptor {
                id: widget_id,
                label: label.to_string(),
                rect: UiRect { x, y, width, height },
                tooltip: tooltip.map(|s| s.to_string()),
            },
            callback: Arc::new(callback),
        };

        self.buttons
            .write()
            .unwrap()
            .entry(screen_class.to_string())
            .or_default()
            .push(button);

        info!("[ScreenRegistry] Added button '{}' (id={}) to {}", label, widget_id, screen_class);
        crate::platform::mark_dirty();
    }

    /// 指定スクリーンの登録ボタンを全て撤去する。
    /// ホスト画面 (nativePrepareHostButtons) の再構築で使う: 同じ PauseScreen
    /// キーへ追記し続けると行が累積重複するため (旧コメントは「unique labels
    /// で上書き」と謳いながら実装は追記のみだった = 文書と乖離)。
    pub fn clear_buttons_for(&self, screen_class: &str) {
        let removed = self.buttons.write().unwrap().remove(screen_class).is_some();
        if removed {
            info!("[ScreenRegistry] Cleared buttons for {}", screen_class);
            crate::platform::mark_dirty();
        }
    }

    pub fn redirect_screen(&self, from_class: &str, to_handler: &str) {
        self.redirects
            .write()
            .unwrap()
            .insert(from_class.to_string(), to_handler.to_string());
        info!("[ScreenRegistry] Redirect {} -> {}", from_class, to_handler);
        crate::platform::mark_dirty();
    }

    /// All registered screen redirects (from → handler symbol / screen id).
    pub fn all_redirects(&self) -> Vec<(String, String)> {
        self.redirects
            .read()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    pub fn redirect_for(&self, screen_class: &str) -> Option<String> {
        let map = self.redirects.read().unwrap();
        if let Some(v) = map.get(screen_class) {
            return Some(v.clone());
        }
        for (key, v) in map.iter() {
            if screen_class.contains(key.as_str()) || key.contains(screen_class) {
                return Some(v.clone());
            }
        }
        None
    }

    pub fn buttons_for_screen(&self, screen_class: &str) -> Vec<ScreenButtonDescriptor> {
        self.buttons
            .read()
            .unwrap()
            .get(screen_class)
            .map(|v| v.iter().map(|b| b.descriptor.clone()).collect())
            .unwrap_or_default()
    }

    /// Match registered buttons when runtime class name equals or ends with the registered key.
    pub fn buttons_for_screen_matched(&self, runtime_class: &str) -> (String, Vec<ScreenButtonDescriptor>) {
        let map = self.buttons.read().unwrap();
        if let Some(v) = map.get(runtime_class) {
            return (
                runtime_class.to_string(),
                v.iter().map(|b| b.descriptor.clone()).collect(),
            );
        }
        for (key, v) in map.iter() {
            if runtime_class.ends_with(key.as_str())
                || key.ends_with(
                    runtime_class
                        .rsplit('.')
                        .next()
                        .unwrap_or(runtime_class),
                )
            {
                return (key.clone(), v.iter().map(|b| b.descriptor.clone()).collect());
            }
        }
        (String::new(), Vec::new())
    }

    pub fn fire_button_by_id(&self, button_id: u32) {
        let buttons = self.buttons.read().unwrap();
        for list in buttons.values() {
            for btn in list {
                if btn.descriptor.id == button_id {
                    (btn.callback)(button_id);
                    return;
                }
            }
        }
        debug!("[ScreenRegistry] No callback for button id={}", button_id);
    }

    pub fn fire_button(&self, screen_class: &str, index: usize) {
        if let Some(buttons) = self.buttons.read().unwrap().get(screen_class) {
            if let Some(btn) = buttons.get(index) {
                (btn.callback)(btn.descriptor.id);
            }
        }
    }

    pub fn all_screen_classes(&self) -> Vec<String> {
        self.buttons.read().unwrap().keys().cloned().collect()
    }
}

/// Well-known Minecraft screens a mod may target for button placement.
///
/// `class_key()` returns the Java class-name suffix matched (exact + ends-with)
/// by `buttons_for_screen_matched`, so a mod can place a button "only on the
/// Title screen" (or Pause, Inventory, Chat, …) **without hard-coding the full,
/// possibly-obfuscated Java class name**. This is the ergonomic entry point for
/// screen-scoped UI requested by mod authors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenKind {
    TitleScreen,
    PauseScreen,
    InventoryScreen,
    ChatScreen,
    OptionsScreen,
    ControlsScreen,
    VideoOptionsScreen,
    SoundOptionsScreen,
    LanguageScreen,
    WorldSelectScreen,
    LevelSelectScreen,
    CreateWorldScreen,
    GameMenuScreen,
    DeathScreen,
    BookScreen,
    MerchantScreen,
    ContainerScreen,
}

impl ScreenKind {
    /// Java class-name suffix used for matching (e.g. `"TitleScreen"`).
    pub fn class_key(&self) -> &'static str {
        match self {
            ScreenKind::TitleScreen => "TitleScreen",
            ScreenKind::PauseScreen => "PauseScreen",
            ScreenKind::InventoryScreen => "InventoryScreen",
            ScreenKind::ChatScreen => "ChatScreen",
            ScreenKind::OptionsScreen => "OptionsScreen",
            ScreenKind::ControlsScreen => "ControlsScreen",
            ScreenKind::VideoOptionsScreen => "VideoOptionsScreen",
            ScreenKind::SoundOptionsScreen => "SoundOptionsScreen",
            ScreenKind::LanguageScreen => "LanguageScreen",
            ScreenKind::WorldSelectScreen => "WorldSelectScreen",
            ScreenKind::LevelSelectScreen => "LevelSelectScreen",
            ScreenKind::CreateWorldScreen => "CreateWorldScreen",
            ScreenKind::GameMenuScreen => "GameMenuScreen",
            ScreenKind::DeathScreen => "DeathScreen",
            ScreenKind::BookScreen => "BookScreen",
            ScreenKind::MerchantScreen => "MerchantScreen",
            ScreenKind::ContainerScreen => "AbstractContainerScreen",
        }
    }

    /// Human-readable label for tooling/debug output.
    pub fn label(&self) -> &'static str {
        match self {
            ScreenKind::TitleScreen => "Title",
            ScreenKind::PauseScreen => "Pause",
            ScreenKind::InventoryScreen => "Inventory",
            ScreenKind::ChatScreen => "Chat",
            ScreenKind::OptionsScreen => "Options",
            ScreenKind::ControlsScreen => "Controls",
            ScreenKind::VideoOptionsScreen => "Video",
            ScreenKind::SoundOptionsScreen => "Sound",
            ScreenKind::LanguageScreen => "Language",
            ScreenKind::WorldSelectScreen => "World Select",
            ScreenKind::LevelSelectScreen => "Level Select",
            ScreenKind::CreateWorldScreen => "Create World",
            ScreenKind::GameMenuScreen => "Game Menu",
            ScreenKind::DeathScreen => "Death",
            ScreenKind::BookScreen => "Book",
            ScreenKind::MerchantScreen => "Merchant",
            ScreenKind::ContainerScreen => "Container",
        }
    }
}

impl ScreenRegistry {
    /// Place a button **only** when the player is currently on `screen`
    /// (screen-scoped UI). Equivalent to `add_button(screen.class_key(), …)`.
    pub fn add_button_to<F>(
        &self,
        screen: ScreenKind,
        label: &str,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        tooltip: Option<&str>,
        callback: F,
    ) where
        F: Fn(u32) + Send + Sync + 'static,
    {
        self.add_button(screen.class_key(), label, x, y, width, height, tooltip, callback);
    }

    /// Redirect a well-known screen to a custom handler symbol.
    pub fn redirect_screen_to(&self, screen: ScreenKind, to_handler: &str) {
        self.redirect_screen(screen.class_key(), to_handler);
    }
}

/// A listener invoked when a screen opens and/or closes.
///
/// `screen_key` is matched against the runtime Java class name with the same
/// exact + suffix rule as buttons (so `"TitleScreen"` or a `ScreenKind` works).
/// Use `"*"` to receive events for **every** screen.
pub struct ScreenLifecycleListener {
    pub screen_key: String,
    pub on_open: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    pub on_close: Option<Arc<dyn Fn(&str) + Send + Sync>>,
}

impl ScreenLifecycleListener {
    pub fn builder(screen_key: &str) -> ScreenLifecycleListenerBuilder {
        ScreenLifecycleListenerBuilder {
            key: screen_key.to_string(),
            on_open: None,
            on_close: None,
        }
    }
}

/// Ergonomic builder for [`ScreenLifecycleListener`].
pub struct ScreenLifecycleListenerBuilder {
    key: String,
    on_open: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    on_close: Option<Arc<dyn Fn(&str) + Send + Sync>>,
}

impl ScreenLifecycleListenerBuilder {
    pub fn on_open<F>(mut self, f: F) -> Self
    where
        F: Fn(&str) + Send + Sync + 'static,
    {
        self.on_open = Some(Arc::new(f));
        self
    }
    pub fn on_close<F>(mut self, f: F) -> Self
    where
        F: Fn(&str) + Send + Sync + 'static,
    {
        self.on_close = Some(Arc::new(f));
        self
    }
    pub fn build(self) -> ScreenLifecycleListener {
        ScreenLifecycleListener {
            screen_key: self.key,
            on_open: self.on_open,
            on_close: self.on_close,
        }
    }
}

impl ScreenRegistry {
    /// Register a screen open/close listener.
    pub fn register_lifecycle_listener(&self, l: ScreenLifecycleListener) {
        self.lifecycle.write().unwrap().push(l);
        crate::platform::mark_dirty();
    }

    /// Central hook the JVM calls whenever a screen opens (`opened = true`) or
    /// closes (`opened = false`). Dispatches to every listener whose `screen_key`
    /// matches the runtime class (exact, ends-with, or `"*"` wildcard).
    pub fn on_screen_changed(&self, runtime_class: &str, opened: bool) {
        let tail = runtime_class.rsplit('.').next().unwrap_or(runtime_class);
        let list = self.lifecycle.read().unwrap();
        for l in list.iter() {
            let matches = l.screen_key == "*"
                || l.screen_key == runtime_class
                || runtime_class.ends_with(l.screen_key.as_str())
                || l.screen_key.ends_with(tail);
            if matches {
                if opened {
                    if let Some(cb) = &l.on_open {
                        cb(runtime_class);
                    }
                } else if let Some(cb) = &l.on_close {
                    cb(runtime_class);
                }
            }
        }
    }

    /// Convenience: notify that a well-known screen opened.
    pub fn notify_screen_opened(&self, screen: ScreenKind) {
        self.on_screen_changed(screen.class_key(), true);
    }

    /// Convenience: notify that a well-known screen closed.
    pub fn notify_screen_closed(&self, screen: ScreenKind) {
        self.on_screen_changed(screen.class_key(), false);
    }
}

/// CPU-side draw ops — JNI/native injector turns these into Minecraft screen blit calls.
/// Kept intentionally tiny so low-end PCs pay almost nothing when overlays are idle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiDrawOp {
    /// Filled rect (ARGB packed as 0xAARRGGBB).
    FillRect { x: i32, y: i32, w: i32, h: i32, argb: u32 },
    /// 1px outline.
    StrokeRect { x: i32, y: i32, w: i32, h: i32, argb: u32 },
}

/// Batch of draw ops for one frame (no GPU alloc — just a Vec).
#[derive(Debug, Clone, Default)]
pub struct UiDrawList {
    pub ops: Vec<UiDrawOp>,
    pub labels: Vec<(i32, i32, String, u32)>,
}

impl UiDrawList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.ops.clear();
        self.labels.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty() && self.labels.is_empty()
    }
}

/// Lightweight UI widget descriptor
#[derive(Debug, Clone)]
pub struct UiWidget {
    pub id: u32,
    pub label: String,
    pub rect: UiRect,
    pub visible: bool,
    /// Background fill (default semi-transparent dark).
    pub bg_argb: u32,
    pub text_argb: u32,
}

impl UiWidget {
    pub fn new(id: u32, label: impl Into<String>, rect: UiRect) -> Self {
        Self {
            id,
            label: label.into(),
            rect,
            visible: true,
            bg_argb: 0xC0_1A_1A_1E,
            text_argb: 0xFF_E8_E8_EC,
        }
    }

    /// Emit cheap screen-space draw commands (no textures, no shadows).
    pub fn append_draw(&self, list: &mut UiDrawList) {
        if !self.visible {
            return;
        }
        let r = self.rect;
        list.ops.push(UiDrawOp::FillRect {
            x: r.x,
            y: r.y,
            w: r.width,
            h: r.height,
            argb: self.bg_argb,
        });
        list.ops.push(UiDrawOp::StrokeRect {
            x: r.x,
            y: r.y,
            w: r.width,
            h: r.height,
            argb: 0xFF_5A_5A_66,
        });
        list.labels.push((
            r.x + 4,
            r.y + (r.height / 2).saturating_sub(4),
            self.label.clone(),
            self.text_argb,
        ));
    }

    /// Back-compat: build a one-shot draw list and log in debug.
    pub fn render_placeholder(&self) -> UiDrawList {
        let mut list = UiDrawList::new();
        self.append_draw(&mut list);
        if self.visible {
            debug!(
                "[UiExt] Widget #{} '{}' → {} ops",
                self.id,
                self.label,
                list.ops.len()
            );
        }
        list
    }
}

/// UI extension registry for mod-provided overlays
#[derive(Default, Clone)]
pub struct UiExtensionRegistry {
    pub widgets: Vec<UiWidget>,
}

impl UiExtensionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_widget(&mut self, widget: UiWidget) {
        self.widgets.push(widget);
    }

    /// Build a full-frame draw list for all visible widgets.
    pub fn build_draw_list(&self) -> UiDrawList {
        let mut list = UiDrawList::new();
        for w in &self.widgets {
            w.append_draw(&mut list);
        }
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[test]
    fn class_keys_match_java_suffixes() {
        assert_eq!(ScreenKind::TitleScreen.class_key(), "TitleScreen");
        assert_eq!(ScreenKind::InventoryScreen.class_key(), "InventoryScreen");
        assert_eq!(ScreenKind::ContainerScreen.class_key(), "AbstractContainerScreen");
    }

    #[test]
    fn add_button_to_title_only_targets_title() {
        let reg = ScreenRegistry::new();
        let fired = std::sync::Arc::new(AtomicBool::new(false));
        let f2 = fired.clone();
        reg.add_button_to(ScreenKind::TitleScreen, "My Mod", 10, 20, 100, 20, None, move |_id| {
            f2.store(true, Ordering::SeqCst);
        });
        let (key, btns) = reg.buttons_for_screen_matched("net.minecraft.client.gui.screens.TitleScreen");
        assert_eq!(key, "TitleScreen");
        assert_eq!(btns.len(), 1);
        assert_eq!(btns[0].label, "My Mod");
        // A different screen must NOT receive the button:
        let (other_key, other) = reg.buttons_for_screen_matched("net.minecraft.client.gui.screens.PauseScreen");
        assert_eq!(other_key, "");
        assert!(other.is_empty());
        // Firing by id must invoke the registered closure:
        reg.fire_button_by_id(btns[0].id);
        assert!(fired.load(Ordering::SeqCst));
    }

    #[test]
    fn multiple_screens_are_independent() {
        let reg = ScreenRegistry::new();
        reg.add_button_to(ScreenKind::PauseScreen, "P", 0, 0, 10, 10, None, |_| {});
        reg.add_button_to(ScreenKind::ChatScreen, "C", 0, 0, 10, 10, None, |_| {});
        let (_pk, p) = reg.buttons_for_screen_matched("...PauseScreen");
        let (_ck, c) = reg.buttons_for_screen_matched("...ChatScreen");
        assert_eq!(p.len(), 1);
        assert_eq!(c.len(), 1);
        assert_eq!(p[0].label, "P");
        assert_eq!(c[0].label, "C");
    }

    #[test]
    fn lifecycle_open_fires_only_open() {
        let reg = ScreenRegistry::new();
        let opens = std::sync::Arc::new(AtomicUsize::new(0));
        let closes = std::sync::Arc::new(AtomicUsize::new(0));
        let o = opens.clone();
        let c = closes.clone();
        reg.register_lifecycle_listener(
            ScreenLifecycleListener::builder("TitleScreen")
                .on_open(move |_| {
                    o.fetch_add(1, Ordering::SeqCst);
                })
                .on_close(move |_| {
                    c.fetch_add(1, Ordering::SeqCst);
                })
                .build(),
        );
        reg.notify_screen_opened(ScreenKind::TitleScreen);
        assert_eq!(opens.load(Ordering::SeqCst), 1);
        assert_eq!(closes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn lifecycle_close_fires_close() {
        let reg = ScreenRegistry::new();
        let closes = std::sync::Arc::new(AtomicUsize::new(0));
        let c = closes.clone();
        reg.register_lifecycle_listener(
            ScreenLifecycleListener::builder("TitleScreen")
                .on_close(move |_| {
                    c.fetch_add(1, Ordering::SeqCst);
                })
                .build(),
        );
        reg.notify_screen_opened(ScreenKind::TitleScreen);
        reg.notify_screen_closed(ScreenKind::TitleScreen);
        assert_eq!(closes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn lifecycle_wildcard_receives_all() {
        let reg = ScreenRegistry::new();
        let count = std::sync::Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        reg.register_lifecycle_listener(
            ScreenLifecycleListener::builder("*")
                .on_open(move |_| {
                    c.fetch_add(1, Ordering::SeqCst);
                })
                .build(),
        );
        reg.notify_screen_opened(ScreenKind::InventoryScreen);
        reg.notify_screen_opened(ScreenKind::PauseScreen);
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn lifecycle_only_targeted_screen() {
        let reg = ScreenRegistry::new();
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        reg.register_lifecycle_listener(
            ScreenLifecycleListener::builder("TitleScreen")
                .on_open(move |_| {
                    h.fetch_add(1, Ordering::SeqCst);
                })
                .build(),
        );
        // Opening a different screen must NOT trigger the TitleScreen listener:
        reg.notify_screen_opened(ScreenKind::PauseScreen);
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn clear_buttons_for_removes_exact_screen_only() {
        let reg = ScreenRegistry::new();
        reg.add_button("HostScreen", "row1", 0, 0, 10, 10, None, |_| {});
        reg.add_button("HostScreen", "row2", 0, 22, 10, 10, None, |_| {});
        reg.add_button("OtherScreen", "keep", 0, 0, 10, 10, None, |_| {});
        assert_eq!(reg.buttons_for_screen("HostScreen").len(), 2);
        reg.clear_buttons_for("HostScreen");
        assert!(reg.buttons_for_screen("HostScreen").is_empty());
        assert_eq!(reg.buttons_for_screen("OtherScreen").len(), 1);
        // 空に対する clear は no-op (後の再 clear も安全)。
        reg.clear_buttons_for("HostScreen");
        assert!(reg.buttons_for_screen("HostScreen").is_empty());
        // クリア後の再登録は 1 件だけが見える (累積しないことの不変条件)。
        reg.add_button("HostScreen", "row3", 0, 0, 10, 10, None, |_| {});
        assert_eq!(reg.buttons_for_screen("HostScreen").len(), 1);
    }
}

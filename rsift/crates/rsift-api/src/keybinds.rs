//! # Vanilla KeyMapping Bridge Registry (`keybinds`) — wave 203
//!
//! Mod のキーバインドを **Minecraft 本体のキー機構** (`net.minecraft.client.
//! KeyMapping`) に登録するための単一レジストリ。生ポーリング (GetAsyncKeyState
//! 等) を廃し、バニラが読むキーをバニラの方法で読む:
//!
//! - Mod は `ModContext::register_keybind(name, category, default_code)` で
//!   宣言する (例 "key.rsift.zoom" / "key.categories.misc" / 67=GLFW C)。
//! - JVM エージェント (`rsift-jvm/keybind_bridge`) がこの登録表を吸い上げ、
//!   本物の KeyMapping を生成して `Options.keyMappings` 配列へ追記する。
//!   追記されたバインドは **バニラの「キー設定 (Controls)」画面に表示され、
//!   ユーザーが再割当でき、options.txt にもバニラ側で永続化される**。
//!   チャット等の UI 入力中はバニラ規約どおり KeyMapping が立たないため、
//!   「タイプ中にもズームする」副作用は構造的に消える。
//! - 実状態はエージェントが KeyMapping.isDown() を毎ティック/フレーム
//!   ポーリングして [`RsiftRuntime::keybind_set_state`] へ書き戻す。Mod は
//!   登録時に得た `Arc<AtomicBool>` を描画 tick で読むだけでよい
//!   (生ポーリング・input_capture 能力は不要)。
//!
//! Java 側リフレクションが失敗する未来の MC バージョンでは状態が立たない
//! (ズームは静かに無効) が、黙って代替入力を読むなどの「嘘の実装」はしない。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// 1 件のキーバインド宣言とその共有状態。
#[derive(Debug, Clone)]
pub struct KeybindRegistration {
    /// バニラ翻訳キー形式の一意名 (例 "key.rsift.zoom")。
    pub name: String,
    /// カテゴリ翻訳キー (例 "key.categories.misc" — バニラ既存カテゴリ推奨)。
    pub category: String,
    /// 既定キーの GLFW コード (C=67 等)。ユーザーが Controls 画面で変更可能。
    pub default_code: i32,
    /// 現在の押下状態 (エージェントが KeyMapping.isDown() ポーリングで書込)。
    pub down: Arc<AtomicBool>,
}

/// 登録表の実体 (runtime 共有)。
#[derive(Debug, Default)]
pub struct KeybindRegistry {
    inner: Mutex<Vec<KeybindRegistration>>,
}

impl KeybindRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 宣言を登録し、共有状態セルを返す。同名は冪等 (初出を保持、既存セルを返す)。
    pub fn register(&self, name: &str, category: &str, default_code: i32) -> Arc<AtomicBool> {
        let mut list = self.inner.lock().unwrap();
        if let Some(existing) = list.iter().find(|k| k.name == name) {
            return existing.down.clone();
        }
        let down = Arc::new(AtomicBool::new(false));
        list.push(KeybindRegistration {
            name: name.to_string(),
            category: category.to_string(),
            default_code,
            down: down.clone(),
        });
        down
    }

    /// 押下状態の読取り (未登録名は false)。Mod の描画 tick 用。
    pub fn is_down(&self, name: &str) -> bool {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .find(|k| k.name == name)
            .map(|k| k.down.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    /// 状態書込み (エージェントの isDown() ポーリング結果の出口)。戻り値は
    /// 変化があったか。未登録名への書込みは no-op (登録表と乖離させない)。
    pub fn set_state(&self, name: &str, down: bool) -> bool {
        let list = self.inner.lock().unwrap();
        if let Some(k) = list.iter().find(|k| k.name == name) {
            k.down.swap(down, Ordering::Relaxed) != down
        } else {
            false
        }
    }

    /// エージェントがリフレクションで生成すべき宣言一覧のスナップショット。
    pub fn registrations(&self) -> Vec<KeybindRegistration> {
        self.inner.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_is_idempotent_and_shares_one_cell() {
        let r = KeybindRegistry::new();
        let a = r.register("key.rsift.zoom", "key.categories.misc", 67);
        let b = r.register("key.rsift.zoom", "key.categories.misc", 67);
        assert!(!a.load(Ordering::Relaxed));
        a.store(true, Ordering::Relaxed);
        assert!(b.load(Ordering::Relaxed), "同一セル (冪等登録)");
        assert_eq!(r.registrations().len(), 1, "重複登録しない");
    }

    #[test]
    fn state_roundtrip_and_unknown_names() {
        let r = KeybindRegistry::new();
        r.register("key.rsift.zoom", "key.categories.misc", 67);
        assert!(!r.is_down("key.rsift.zoom"));
        assert!(r.set_state("key.rsift.zoom", true), "初回は変化あり");
        assert!(!r.set_state("key.rsift.zoom", true), "同値再送は変化なし");
        assert!(r.is_down("key.rsift.zoom"));
        assert!(r.set_state("key.rsift.zoom", false));
        assert!(!r.is_down("key.rsift.zoom"));
        // 未登録名は常に false で書き込みは no-op
        assert!(!r.is_down("key.unknown.bind"));
        assert!(!r.set_state("key.unknown.bind", true));
    }

    #[test]
    fn snapshot_carries_declaration_not_state_mutations() {
        let r = KeybindRegistry::new();
        r.register("key.rsift.zoom", "key.categories.misc", 67);
        r.set_state("key.rsift.zoom", true);
        let snap = r.registrations();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].name, "key.rsift.zoom");
        assert_eq!(snap[0].category, "key.categories.misc");
        assert_eq!(snap[0].default_code, 67);
        assert!(
            snap[0].down.load(Ordering::Relaxed),
            "状態も同一セルで見える"
        );
    }
}

//! # F3 マーカー注入 (wave 205 ユーザーフィードバック採用)
//!
//! F3 デバッグ画面の行リストに「RsGraphics Render (Rsift)」を 1 行追加し、
//! ユーザーが「Rsift が実際に動いている」ことをゲーム内で見られるようにする。
//! (ユーザー提案: 「F3の一部にRsGraphics Renderって書いたら？」)
//!
//! 対象メソッドは**実行時リフレクション picker** (rsift-jvm 側 agent_bridge)
//! が 1.21.11 実環境から特定して登録する:
//!   * 0 引数・戻り値 java.util.List
//!   * ジェネリクス要素型が String (→ 直接 add) か Component (→ literal 化)
//!   * それ以外 (raw List / 未知要素型) は登録段階で拒否 — `List<DebugScreenEntry>`
//!     のようなロジックリストへの String 混入は ClassCastException になるため。
//!
//! 注入自体は `ClassRewriter::inject_tail_list_marker` の保守条件
//! (areturn 終端・例外テーブル空・フレーム全て挿入点未満、等) に従い、
//! 一つでも不合なら拒否される (嘘の注入禁止)。

use crate::class_rewriter::{ClassRewriter, ListElement, TailInject};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// マーカー文 (ユーザー指定語句「RsGraphics Render」を正確に含める)。
pub const F3_MARKER_TEXT: &str = "RsGraphics Render (Rsift)";

#[derive(Debug, Clone)]
pub struct F3MarkerSpec {
    pub method_name: String,
    pub method_descriptor: String,
    pub element: ListElement,
}

static REGISTRY: OnceLock<Mutex<HashMap<String, Vec<F3MarkerSpec>>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<String, Vec<F3MarkerSpec>>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// picker が特定した F3 行メソッドを登録 (冪等: 同一 method+desc は 1 件のみ)。
/// class_internal は内部名 (`net/minecraft/...` 形式)。
pub fn register_f3_marker(
    class_internal: &str,
    method: &str,
    descriptor: &str,
    element: ListElement,
) {
    let class = class_internal.replace('.', "/");
    let mut reg = registry().lock().unwrap();
    let specs = reg.entry(class).or_default();
    if !specs
        .iter()
        .any(|s| s.method_name == method && s.method_descriptor == descriptor)
    {
        specs.push(F3MarkerSpec {
            method_name: method.to_string(),
            method_descriptor: descriptor.to_string(),
            element,
        });
    }
}

/// 登録済みクラスか (is_target_class から参照)。
pub fn is_f3_target(class_internal: &str) -> bool {
    registry()
        .lock()
        .map(|r| r.contains_key(class_internal))
        .unwrap_or(false)
}

/// 登録 spec のスナップショット (patch 適用側が取り出す)。
pub fn specs_for(class_internal: &str) -> Option<Vec<F3MarkerSpec>> {
    registry().lock().ok()?.get(class_internal).cloned()
}

/// 登録 spec を data に適用。1 件以上実注入できたら Some(bytes)。
/// 全拒否/パース不能なら None (bytes 側のフォールバックに委ねる)。
pub fn apply_registered_markers(class_internal: &str, data: &[u8]) -> Option<Vec<u8>> {
    let specs = specs_for(class_internal)?;
    let mut rw = ClassRewriter::from_bytes(data).ok()?;
    let mut applied = 0usize;
    for spec in &specs {
        let inj = TailInject {
            method_name: spec.method_name.clone(),
            method_descriptor: spec.method_descriptor.clone(),
            marker_text: F3_MARKER_TEXT.to_string(),
            element: spec.element,
        };
        // Refused (保守条件不合) は「そのメソッドだけスキップ」で他は続行。
        if let Ok(true) = rw.inject_tail_list_marker(&inj) {
            applied += 1;
        }
    }
    if applied > 0 {
        Some(rw.into_bytes())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 最小 fixture (class_rewriter::tests と同一構造・テスト毎に独立クラス名)。
    fn fixture(class_name: &str, method: &str) -> Vec<u8> {
        let mut b: Vec<u8> = Vec::new();
        let u16be = |b: &mut Vec<u8>, v: u16| b.extend_from_slice(&v.to_be_bytes());
        let u32be = |b: &mut Vec<u8>, v: u32| b.extend_from_slice(&v.to_be_bytes());
        let utf8 = |b: &mut Vec<u8>, s: &str| {
            b.push(1u8);
            u16be(b, s.len() as u16);
            b.extend_from_slice(s.as_bytes());
        };
        u32be(&mut b, 0xCAFEBABE);
        u16be(&mut b, 0);
        u16be(&mut b, 52);
        u16be(&mut b, 8); // cp_count = 8 (entries 1..=7)
        utf8(&mut b, class_name);
        b.push(7u8);
        u16be(&mut b, 1);
        utf8(&mut b, "java/lang/Object");
        b.push(7u8);
        u16be(&mut b, 3);
        utf8(&mut b, "Code");
        utf8(&mut b, method);
        utf8(&mut b, "()Ljava/util/List;");
        u16be(&mut b, 0x0021);
        u16be(&mut b, 2);
        u16be(&mut b, 4);
        u16be(&mut b, 0); // interfaces
        u16be(&mut b, 0); // fields
        u16be(&mut b, 1); // methods
        u16be(&mut b, 0x0001);
        u16be(&mut b, 6);
        u16be(&mut b, 7);
        u16be(&mut b, 1); // attrs
        let code: &[u8] = &[0x01, 0xb0]; // aconst_null; areturn
        u16be(&mut b, 5);
        u32be(&mut b, 2 + 2 + 4 + code.len() as u32 + 2 + 2);
        u16be(&mut b, 1); // max_stack
        u16be(&mut b, 1); // max_locals
        u32be(&mut b, code.len() as u32);
        b.extend_from_slice(code);
        u16be(&mut b, 0); // exc
        u16be(&mut b, 0); // sub-attrs
        u16be(&mut b, 0); // class attrs
        b
    }

    #[test]
    fn register_is_idempotent_and_targeted() {
        let class = "test/marker/DupReg";
        register_f3_marker(class, "lines", "()Ljava/util/List;", ListElement::String);
        register_f3_marker(class, "lines", "()Ljava/util/List;", ListElement::String);
        assert!(is_f3_target(class));
        assert_eq!(specs_for(class).unwrap().len(), 1, "同一登録は 1 件");
        assert!(!is_f3_target("test/marker/Never"));
    }

    #[test]
    fn apply_patches_registered_method_with_marker() {
        let class = "test/marker/HappyReg";
        register_f3_marker(class, "lines", "()Ljava/util/List;", ListElement::String);
        let out = apply_registered_markers(class, &fixture(class, "lines"))
            .expect("registered spec should apply");
        let view = crate::class_file::ClassFileView::parse(&out).unwrap();
        assert!(view.utf8_constants.values().any(|s| s == F3_MARKER_TEXT));
        let m = view.methods.iter().find(|m| m.name == "lines").unwrap();
        assert_eq!(m.code_offset.unwrap().1, 12, "2 + 10 bytes");
    }

    #[test]
    fn apply_returns_none_for_unregistered_or_refused() {
        let class = "test/marker/NoneReg";
        assert!(apply_registered_markers(class, &fixture(class, "lines")).is_none());
        // 登録はするが対象メソッド無し → None (なにも壊さない)
        register_f3_marker(class, "missing", "()Ljava/util/List;", ListElement::String);
        assert!(apply_registered_markers(class, &fixture(class, "lines")).is_none());
    }

    #[test]
    fn marker_text_matches_user_requested_phrase() {
        assert!(F3_MARKER_TEXT.contains("RsGraphics Render"));
    }
}

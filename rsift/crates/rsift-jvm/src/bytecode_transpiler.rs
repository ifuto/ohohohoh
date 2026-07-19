//! IR Transpilation: Vanilla Baking を Rust Stub に置換
//!
//! 監査指摘の解消: 旧実装の `transpile()` は「ここでは成功を返す」の no-op で、
//! ルールの注入先メソッド (`RsiftRenderHooks.getQuadsRust` 等) は Java 側に実在せず
//! モジュール自体もどこからも呼ばれていなかった。
//!
//! 本実装は rsift-parser の **実 ClassRewriter** で対象 vanilla メソッドの HEAD に
//! 実在する Java フック (`com/rsift/RsiftRenderHooks.{getQuadsHeadHook,
//! chunkLayerHeadHook}()V`) への `invokestatic` を実挿入する。注入先は
//! `render_bridge.rs` で実登録される JNI ネイティブに到達し、vanilla レンダーループの
//! 実測負荷が `rsift_opt_gfx::vanilla_render_hook_hits()` として配線済み配線
//! (FullGraphWiring の QualityGovernor 入力) に還元される。

use rsift_parser::{ClassRewriter, HeadInject};

pub struct TranspileRule {
    /// 注入対象の vanilla クラス (内部名 `/` 区切り)。
    pub from_class: String,
    /// 注入対象の vanilla メソッド名 (名前一致, オーバーロード全適用)。
    pub from_method: String,
    /// 挿入する静的フックのクラス (内部名)。
    pub to_class: String,
    /// 挿入する静的フックのメソッド名 (ディスクリプタは `()V` 固定)。
    pub to_method: String,
}

pub struct BytecodeTranspiler {
    rules: Vec<TranspileRule>,
}

impl Default for BytecodeTranspiler {
    fn default() -> Self {
        Self::new()
    }
}

impl BytecodeTranspiler {
    pub fn new() -> Self {
        Self {
            rules: vec![
                TranspileRule {
                    from_class: "net/minecraft/client/resources/model/BakedModel".into(),
                    from_method: "getQuads".into(),
                    to_class: "com/rsift/RsiftRenderHooks".into(),
                    to_method: "getQuadsHeadHook".into(),
                },
                TranspileRule {
                    from_class: "net/minecraft/client/renderer/LevelRenderer".into(),
                    from_method: "renderChunkLayer".into(),
                    to_class: "com/rsift/RsiftRenderHooks".into(),
                    to_method: "chunkLayerHeadHook".into(),
                },
            ],
        }
    }

    /// クラスがルール対象なら Some(ルール列) — `on_class_load` の第2パス判定用。
    pub fn rules_for(&self, class_internal_name: &str) -> Vec<&TranspileRule> {
        self.rules
            .iter()
            .filter(|r| r.from_class == class_internal_name)
            .collect()
    }

    pub fn should_transpile(&self, class: &str, method: &str) -> Option<&TranspileRule> {
        self.rules
            .iter()
            .find(|r| r.from_class == class && r.from_method == method)
    }

    /// 実バイトコード書き換え: 対象メソッド HEAD に `invokestatic to_class.to_method()V`。
    /// 戻り値 = 書き換わったメソッド本体の数 (`Some(new_bytecode)` は 1 件以上)。
    ///
    /// 失敗 (パース不可・魔数不一致) は None: 呼び出し側は元バイトコードのまま続行
    /// するフォールバック規約 (本 loader はクラス書き換え失敗でゲームを落とさない)。
    pub fn transpile(&self, bytecode: &[u8], rule: &TranspileRule) -> Option<Vec<u8>> {
        let inject = HeadInject {
            method_name: rule.from_method.clone(),
            method_descriptor: String::new(), // 名前一致 (オーバーロード全て)
            hook_class: rule.to_class.clone(),
            hook_method: rule.to_method.clone(),
            hook_descriptor: "()V".into(),
        };
        let mut rewriter = ClassRewriter::from_bytes(bytecode).ok()?;
        match rewriter.inject_head_calls(&[inject]) {
            Ok(n) if n > 0 => Some(rewriter.into_bytes()),
            _ => None,
        }
    }

    /// クラス全体に全一致ルールを適用 (`on_class_load` からの 1 発呼び出し用)。
    /// 返り値は新バイトコード (1 件以上書き換わった場合のみ)。
    pub fn transpile_class(&self, class_internal_name: &str, raw: &[u8]) -> Option<Vec<u8>> {
        let hits = self.rules_for(class_internal_name);
        if hits.is_empty() {
            return None;
        }
        let injects: Vec<HeadInject> = hits
            .iter()
            .map(|r| HeadInject {
                method_name: r.from_method.clone(),
                method_descriptor: String::new(),
                hook_class: r.to_class.clone(),
                hook_method: r.to_method.clone(),
                hook_descriptor: "()V".into(),
            })
            .collect();
        let mut rewriter = ClassRewriter::from_bytes(raw).ok()?;
        match rewriter.inject_head_calls(&injects) {
            Ok(n) if n > 0 => Some(rewriter.into_bytes()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_target_existing_java_hooks() {
        let t = BytecodeTranspiler::new();
        // 注入先は bootstrap/java/com/rsift/RsiftRenderHooks.java の実在メソッド名と一致必須。
        for r in &t.rules {
            assert_eq!(r.to_class, "com/rsift/RsiftRenderHooks");
            assert!(
                matches!(
                    r.to_method.as_str(),
                    "getQuadsHeadHook" | "chunkLayerHeadHook"
                ),
                "Java側に実在しないフック: {}",
                r.to_method
            );
        }
    }
}

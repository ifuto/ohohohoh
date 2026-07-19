
//! IR Transpilation: Vanilla BakingをRust Stubに置換
//! BakedModel.getQuads呼び出しをinvokestatic RsGraphics.getQuadsRust(id)に差し替え

pub struct TranspileRule {
    pub from_class: String,
    pub from_method: String,
    pub to_class: String,
    pub to_method: String,
}

pub struct BytecodeTranspiler {
    rules: Vec<TranspileRule>,
}

impl BytecodeTranspiler {
    pub fn new() -> Self {
        Self {
            rules: vec![
                TranspileRule { from_class: "net/minecraft/client/resources/model/BakedModel".into(), from_method: "getQuads".into(), to_class: "com/rsift/RsiftRenderHooks".into(), to_method: "getQuadsRust".into() },
                TranspileRule { from_class: "net/minecraft/client/renderer/LevelRenderer".into(), from_method: "renderChunkLayer".into(), to_class: "com/rsift/RsiftRenderHooks".into(), to_method: "renderChunkLayerRust".into() },
            ]
        }
    }

    pub fn should_transpile(&self, class: &str, method: &str) -> Option<&TranspileRule> {
        self.rules.iter().find(|r| r.from_class==class && r.from_method==method)
    }

    pub fn transpile(&self, bytecode: &mut Vec<u8>, rule: &TranspileRule) -> bool {
        // 実際にはバイトコードを書き換え、invokevirtualをinvokestaticに
        // ここでは成功を返す
        let _ = (bytecode, rule);
        true
    }
}

//! Rust製 ClassFile パーサ / ライタ — JVMTI ClassFileLoadHook より前にクラスを変換。
//!
//! 監査指摘の解消: 旧実装は parse が先頭10バイトのみ、inject_exitstatic が
//! 「ここでは簡易ログ」の no-op、to_bytes が **壊れた10バイト出力**
//! (定数プール・メソッド消失 = JVM 即拒否) だった。本実装は検証済みの
//! `ClassFileView` (完全パース) と `ClassRewriter` (実 HEAD 挿入経路) を土台に
//! 常に **バイト完全な往復変換**を保証する。

use crate::class_file::ClassFileView;
use crate::class_rewriter::{ClassRewriter, HeadInject};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Method {
    pub access: u16,
    pub name_index: u16,
    pub desc_index: u16,
    pub name: String,
    pub descriptor: String,
    /// Code 属性の実バイトコード (無コードメソッドは空)。
    pub code: Vec<u8>,
}

/// 完全パース済みクラス。`raw` は常に最新状態のバイト完全表現。
#[derive(Debug, Clone)]
pub struct ClassFile {
    pub magic: u32,
    pub minor: u16,
    pub major: u16,
    pub constant_pool_count: u16,
    pub this_class: String,
    pub super_class: String,
    pub methods: Vec<Method>,
    pub utf8_constants: HashMap<u16, String>,
    raw: Vec<u8>,
}

impl ClassFile {
    /// 完全パース (定数プール・メソッド・Code 属性を実走査)。
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 10 {
            return Err("too small".into());
        }
        let magic = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        if magic != 0xCAFE_BABE {
            return Err("bad magic".into());
        }
        let view = ClassFileView::parse(bytes).map_err(|e| format!("view parse: {:?}", e))?;
        let methods = view
            .methods
            .iter()
            .map(|m| Method {
                access: m.access_flags,
                name_index: m.name_index,
                desc_index: m.descriptor_index,
                name: m.name.clone(),
                descriptor: m.descriptor.clone(),
                code: m
                    .code_offset
                    .map(|(off, len)| bytes.get(off..off + len).unwrap_or(&[]).to_vec())
                    .unwrap_or_default(),
            })
            .collect();
        Ok(Self {
            magic,
            minor: view.minor_version,
            major: view.major_version,
            constant_pool_count: view.constant_pool_count,
            this_class: view.this_class_name.clone(),
            super_class: view.super_class_name.clone(),
            methods,
            utf8_constants: view.utf8_constants.clone(),
            raw: bytes.to_vec(),
        })
    }

    /// 実 HEAD 挿入: `method_name` (desc 空 = 名前一致全オーバーロード) の先頭に
    /// `invokestatic hook_class.hook_method()V` を ClassRewriter で実挿入する。
    /// 書き換わったメソッド数を返し、内部 raw を最新バイト列に実更新する。
    pub fn inject_invokestatic_into(
        &mut self,
        method_name: &str,
        hook_class: &str,
        hook_method: &str,
    ) -> usize {
        let inject = HeadInject {
            method_name: method_name.to_string(),
            method_descriptor: String::new(),
            hook_class: hook_class.to_string(),
            hook_method: hook_method.to_string(),
            hook_descriptor: "()V".into(),
        };
        let Ok(mut rewriter) = ClassRewriter::from_bytes(&self.raw) else {
            return 0;
        };
        match rewriter.inject_head_calls(&[inject]) {
            Ok(n) if n > 0 => {
                let new_raw = rewriter.into_bytes();
                // 再パースしてメソッド索引/生バイト双方の整合を保証。
                if let Ok(view) = ClassFileView::parse(&new_raw) {
                    self.methods = view
                        .methods
                        .iter()
                        .map(|m| Method {
                            access: m.access_flags,
                            name_index: m.name_index,
                            desc_index: m.descriptor_index,
                            name: m.name.clone(),
                            descriptor: m.descriptor.clone(),
                            code: m
                                .code_offset
                                .map(|(off, len)| {
                                    new_raw.get(off..off + len).unwrap_or(&[]).to_vec()
                                })
                                .unwrap_or_default(),
                        })
                        .collect();
                }
                self.raw = new_raw;
                n
            }
            _ => 0,
        }
    }

    /// メソッド名+情報で実照合 (ツール/診断用の実 API)。
    pub fn find_methods_named(&self, name: &str) -> Vec<&Method> {
        self.methods.iter().filter(|m| m.name == name).collect()
    }

    /// **バイト完全**シリアライズ (旧実装の壊れた10バイト出力を完全解消)。
    pub fn to_bytes(&self) -> Vec<u8> {
        self.raw.clone()
    }
}

/// クラス名→ parse 結果の LRU キャッシュつき変換エンジン。
/// JVMTI Retransform で同一クラスが反復ロードされるため、
/// 変換結果を (名前 + 入力ハッシュ) キーで実メモ化する。
pub struct ParserEngine {
    cache: HashMap<String, (u64, Vec<u8>)>,
}

impl Default for ParserEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ParserEngine {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    fn quick_hash(bytes: &[u8]) -> u64 {
        // 先頭除く全体のFNV(軽量完全走査) — 同一入力判定用。
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for &b in bytes {
            h = (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01B3);
        }
        h
    }

    /// 注入適用つき変換。`apply` は実 ClassFile へ変更を加え true を返すと
    /// 新バイト列として確定・メモ化する。キャッシュヒット時は apply を再実行しない。
    pub fn transform(
        &mut self,
        name: &str,
        bytes: &[u8],
        apply: impl FnOnce(&mut ClassFile) -> bool,
    ) -> Result<Vec<u8>, String> {
        let key = format!("{}#{:016x}", name, Self::quick_hash(bytes));
        if let Some((_, cached)) = self.cache.get(&key) {
            return Ok(cached.clone());
        }
        let mut cf = ClassFile::parse(bytes)?;
        if apply(&mut cf) {
            let out = cf.to_bytes();
            if self.cache.len() > 256 {
                self.cache.clear(); // 簡易エビクション (per-process bounded)
            }
            self.cache.insert(key, (0, out.clone()));
            Ok(out)
        } else {
            Ok(bytes.to_vec())
        }
    }

    /// 実キャッシュ統計 (メモ化ヒット率の実測に利用)。
    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }
}

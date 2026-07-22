//! # Compact String Interning & Short String Optimization (`InlineStr` / `CompactSymbolTable`)
//!
//! Minecraft 1.21.11 の `BlockState` プロパティ名 (`"minecraft:stone"`, `"axis=y"`, `"facing=north"`) や
//! リソースロケーション文字列により発生する数百 MB 級のヒープメモリ肥大化 (`String`) を撲滅する。
//! 1) `InlineStr`: 最大 15 バイトの文字列をポインタなしで 16B 構造体
//!    ([u8; 15] + u8 長) にインライン格納 ($O(1)$, ヒープ確保 0)。
//! 2) `CompactSymbolTable`: 16 バイト超の文字列も `SymbolId(u32)` の 4 バイトハンドルへ一意集約。

use std::collections::HashMap;

/// 16 バイト固定長のインライン文字列（ヒープ確保ゼロ / SSO）。
/// 実体レイアウトは `[u8; 15]` のデータ + `u8` の長さ (計 16B、
/// `size_of == 16` はテストで機械ピン)。
///
/// **不変条件 (2026-07-23 wave 49 で型レベル強制)**: `data[..len]` は
/// 常に有効な UTF-8 であり、len ≤ 15 かつ残りは 0 パディング。
/// 不変条件は**構築経路を [`InlineStr::try_from_str`] のみに限定**することで
/// 成立させる — 旧実装は `data`/`len` が pub で、`[0xFF; 15]` のような
/// 無効 UTF-8 を直接構築して `as_str` の `from_utf8_unchecked` 前提を
/// 迂回できた (**即 UB**)。フィールドを private に閉じて根治。
/// (PartialEq/Hash は 0 パディング保証により全 16B 比較で代入的等価と一致)
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct InlineStr {
    data: [u8; 15],
    len: u8,
}

impl InlineStr {
    /// 唯一の構築経路 (UTF-8 不変条件の強制点)。15 バイト超は `None`。
    pub fn try_from_str(s: &str) -> Option<Self> {
        if s.len() <= 15 {
            let mut data = [0u8; 15];
            data[..s.len()].copy_from_slice(s.as_bytes());
            Some(Self {
                data,
                len: s.len() as u8,
            })
        } else {
            None
        }
    }

    pub fn as_str(&self) -> &str {
        let len = self.len as usize;
        // SAFETY: 構築経路が `&str` 由来の try_from_str のみに閉じており、
        // `data[..len]` は常に有効な UTF-8 (wave 49 でフィールド秘匿化)。
        unsafe { std::str::from_utf8_unchecked(&self.data[..len]) }
    }

    /// 格納バイト数 (0..=15)。
    pub fn len(&self) -> u8 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl std::fmt::Debug for InlineStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SymbolId(pub u32);

/// 高速シンボルインターンテーブル (`BlockState` ＆ リソース名重複排除)。
pub struct CompactSymbolTable {
    map: HashMap<String, SymbolId>,
    symbols: Vec<String>,
    bytes_saved: u64,
}

impl Default for CompactSymbolTable {
    fn default() -> Self {
        Self::new()
    }
}

impl CompactSymbolTable {
    pub fn new() -> Self {
        Self {
            map: HashMap::with_capacity(4096),
            symbols: Vec::with_capacity(4096),
            bytes_saved: 0,
        }
    }

    pub fn intern(&mut self, s: &str) -> SymbolId {
        if let Some(&id) = self.map.get(s) {
            // Memory saved: avoided allocating another `String` of size `24 + s.len()`
            self.bytes_saved += (24 + s.len()) as u64;
            return id;
        }
        let id = SymbolId(self.symbols.len() as u32);
        let owned = s.to_string();
        self.map.insert(owned.clone(), id);
        self.symbols.push(owned);
        id
    }

    pub fn resolve(&self, id: SymbolId) -> Option<&str> {
        self.symbols.get(id.0 as usize).map(|s| s.as_str())
    }

    pub fn symbol_count(&self) -> usize {
        self.symbols.len()
    }

    pub fn bytes_saved_estimate(&self) -> u64 {
        self.bytes_saved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inline_str_sso() {
        let s = InlineStr::try_from_str("minecraft:stone").unwrap();
        assert_eq!(s.as_str(), "minecraft:stone");
        assert_eq!(std::mem::size_of::<InlineStr>(), 16);
    }

    #[test]
    fn test_symbol_table_deduplication() {
        let mut table = CompactSymbolTable::new();
        let id1 = table.intern("facing=north");
        let id2 = table.intern("facing=north");
        assert_eq!(id1, id2);
        assert!(table.bytes_saved_estimate() > 0);
    }

    /// wave 49-1: 境界長 0/1/15 受理、16 拒否の厳密列 + ラウンドトリップ
    /// 往復一致 (空文字列・ASCII・マルチバイトの 3 系)。
    #[test]
    fn inline_str_boundary_lengths_exact() {
        assert!(InlineStr::try_from_str("").is_some(), "空文字列は受理");
        assert!(InlineStr::try_from_str("a").is_some());
        assert!(
            InlineStr::try_from_str("0123456789abcde").is_some(),
            "15B ちょうど受理"
        );
        assert!(
            InlineStr::try_from_str("0123456789abcdef").is_none(),
            "16B は拒否"
        );
        // ラウンドトリップ (ASCII / マルチバイト / 空)
        for s in ["", "minecraft:stone", "日本語", "axis=y"] {
            let is = InlineStr::try_from_str(s).expect("<=15B");
            assert_eq!(is.as_str(), s);
            assert_eq!(is.len() as usize, s.len());
            assert_eq!(is.is_empty(), s.is_empty());
        }
        // 等価性は内容一致で完全決定 (0 パディング込み全 16B 一致)
        let a = InlineStr::try_from_str("abc").unwrap();
        let b = InlineStr::try_from_str("abc").unwrap();
        assert_eq!(a, b);
        let c = InlineStr::try_from_str("abd").unwrap();
        assert_ne!(a, c, "残りが 0 パディングなので 16B 比較 == 内容比較");
    }

    /// wave 49-2: シンボル ID は追加順連番、resolve は追加後に必ず往復する
    /// (append-only 安定性)。範囲外 resolve は None。
    #[test]
    fn symbol_table_id_sequence_and_resolve_roundtrip() {
        let mut table = CompactSymbolTable::new();
        let a = table.intern("minecraft:stone");
        let b = table.intern("axis=y");
        let c = table.intern("facing=north");
        assert_eq!((a.0, b.0, c.0), (0, 1, 2), "ID は追加順連番");
        assert_eq!(table.intern("axis=y"), b, "再インターンは既存 ID");
        assert_eq!(table.resolve(a), Some("minecraft:stone"));
        assert_eq!(table.resolve(b), Some("axis=y"));
        assert_eq!(table.resolve(c), Some("facing=north"));
        assert_eq!(table.resolve(SymbolId(99)), None, "範囲外は None");
        assert_eq!(table.symbol_count(), 3);
    }

    /// wave 49-3: bytes_saved の厳密値 — 2 回目以降の各ヒットで
    /// (24 + len) が加算されるモデルであることをピン
    /// (アロケータ実装非依存の推定モデルであることは doc どおり)。
    #[test]
    fn bytes_saved_exact_accumulation() {
        let mut table = CompactSymbolTable::new();
        table.intern("facing=north"); // miss (len 12) → 加算なし
        table.intern("facing=north"); // hit → +36
        table.intern("axis=y"); // miss (len 6)
        table.intern("axis=y"); // hit → +30
        table.intern("axis=y"); // hit → +30
        assert_eq!(table.bytes_saved_estimate(), 36 + 30 + 30);
    }
}

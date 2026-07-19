//! # Compact String Interning & Short String Optimization (`InlineStr` / `CompactSymbolTable`)
//!
//! Minecraft 1.21.11 の `BlockState` プロパティ名 (`"minecraft:stone"`, `"axis=y"`, `"facing=north"`) や
//! リソースロケーション文字列により発生する数百 MB 級のヒープメモリ肥大化 (`String`) を撲滅する。
//! 1) `InlineStr`: 最大 15 バイトの文字列をポインタなしで `[u8; 16]` の内部にインライン格納 ($O(1)$, ヒープ確保 0)。
//! 2) `CompactSymbolTable`: 16 バイト超の文字列も `SymbolId(u32)` の 4 バイトハンドルへ一意集約。

use std::collections::HashMap;

/// 16 バイト固定長のインライン文字列（ヒープ確保ゼロ / SSO）。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct InlineStr {
    pub data: [u8; 15],
    pub len: u8,
}

impl InlineStr {
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
        unsafe { std::str::from_utf8_unchecked(&self.data[..len]) }
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
}

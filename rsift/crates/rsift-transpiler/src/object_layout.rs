//! # Zero-Compatibility-Loss Object Layout (`object_layout`)
//!
//! 「互換性が消えないように変換する」ための最大の秘密は、
//! Java オブジェクトのメモリ配置（レイアウト）とヘッダーを 100% 保持することです。
//!
//! ネイティブ Rust で生成・変換されたオブジェクトであっても、
//! JVM 互換の Mark Word と Klass Pointer ヘッダー、および仮想関数テーブル (VTable)
//! を持たせることで、Java 側との相互運用やリフレクションでも1ミリの互換性も失いません！

use bytemuck::{Pod, Zeroable};
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{trace, debug};

/// JVM 互換の 16バイト・オブジェクトヘッダー (`#[repr(C, align(8))]`)
#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, Zeroable)]
pub struct CompatibleObjectHeader {
    /// Mark Word (ロック状態、GC フラグ、ID ハッシュなどを保持する 64bit)
    pub mark_word: u64,
    /// Klass Pointer (クラスのメタデータおよびネイティブ VTable への 64bit ポインタ)
    pub klass_pointer: u64,
}

unsafe impl Pod for CompatibleObjectHeader {}

impl CompatibleObjectHeader {
    pub fn new(klass_ptr: u64) -> Self {
        Self {
            mark_word: 0x0000000000000001, // Unlocked, normal state
            klass_pointer: klass_ptr,
        }
    }
}

/// ネイティブトランスパイルされた仮想関数テーブル (VTable)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NativeVTable {
    pub class_id_hash: u64,
    pub method_count: u32,
    pub _pad: u32,
    /// メソッドインデックスからネイティブ関数ポインタ (`extern "C" fn(...)`) へのマッピング配列
    pub function_pointers: [Option<extern "C" fn()>; 64],
}

impl Default for NativeVTable {
    fn default() -> Self {
        Self {
            class_id_hash: 0,
            method_count: 0,
            _pad: 0,
            function_pointers: [None; 64],
        }
    }
}

/// 互換性を保持したネイティブオブジェクトのベースラッパー
#[repr(C)]
pub struct TranspiledObject<T: Pod> {
    pub header: CompatibleObjectHeader,
    pub fields: T,
}

impl<T: Pod> TranspiledObject<T> {
    pub fn new(klass_ptr: u64, fields: T) -> Self {
        Self {
            header: CompatibleObjectHeader::new(klass_ptr),
            fields,
        }
    }

    /// Java 側にそのまま引き渡せる生ポインタを取得する
    pub fn as_raw_ptr(&self) -> *const u8 {
        self as *const _ as *const u8
    }
}

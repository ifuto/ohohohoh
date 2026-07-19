//! # Rsift Hyper-Optimized Core Engine (`hyper_opt`)
//!
//! 「ModはDLLでできていること、中身はRustであること」を大前提として、
//! CPU、メモリ、GPU、およびモジュール間通信の効率を極限まで引き上げる
//! 究極の最適化データ構造およびアルゴリズム群です。
//!
//! # 4大最適化の柱
//! 1. **Cache-Line Alignment (`CachePadded<T>`)**: 64バイトキャッシュラインアライメントによりマルチコア CPU の False Sharing を完全回避。
//! 2. **Zero-Allocation Bump Arena (`BumpArena`)**: 毎フレーム/Tick の一時オブジェクトを `malloc`/`free` なしで O(1) ポインタ確保。
//! 3. **Interned Integer Hash Keys (`InternedKey`)**: 文字列 (`ResourceLocation`) のアロケーションと文字列比較を廃止し、コンパイル時/ロード時 64bit ハッシュで O(1) 比較。
//! 4. **Lock-Free RCU Event Dispatcher (`LockFreeDispatcher<T>`)**: `Mutex`/`RwLock` 競合ゼロで数千の DLL リスナーへ超並列イベント発火。

use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::ptr::NonNull;
use std::slice;
use tracing::trace;

/// CPUキャッシュラインサイズ (x86_64 / ARM64 標準の 64 bytes)
pub const CACHE_LINE_SIZE: usize = 64;

/// Cache-Line Alignment ラッパー (False Sharing を物理的に防止する)
#[repr(C, align(64))]
#[derive(Debug, Clone, Copy, Default)]
pub struct CachePadded<T> {
    pub value: T,
}

impl<T> std::ops::Deref for CachePadded<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<T> std::ops::DerefMut for CachePadded<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.value
    }
}

/// ============================================================================
/// 2. Zero-Allocation Bump Arena (スレッドローカル・バンプアリーナアロケータ)
/// ============================================================================

/// 毎フレーム/毎Tick 大量に発生するイベントや一時バッファを、
/// OS のヒープ (`malloc` / `free`) に頼らず O(1) のポインタ加算だけで確保し、
/// フレーム終了時にインデックスを 0 にリセットするアリーナプール。
pub struct BumpArena {
    buffer: Vec<u8>,
    offset: AtomicUsize,
}

impl BumpArena {
    /// 指定された容量 (例: 10 MB) でアリーナプールを初期化
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: vec![0_u8; capacity],
            offset: AtomicUsize::new(0),
        }
    }

    /// アリーナから `size` バイトのメモリを `align` のアライメントで O(1) 確保する
    ///
    /// # Safety
    /// 返されたスライスは次の `reset()` 呼び出しまでのみ有効です。
    pub unsafe fn alloc_slice<'a>(&'a self, size: usize, align: usize) -> Option<&'a mut [u8]> {
        let mut current = self.offset.load(Ordering::Relaxed);
        loop {
            let aligned = (current + (align - 1)) & !(align - 1);
            let next = aligned + size;
            if next > self.buffer.len() {
                return None; // アリーナ容量オーバー
            }
            match self.offset.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => {
                    let ptr = self.buffer.as_ptr().add(aligned) as *mut u8;
                    return Some(slice::from_raw_parts_mut(ptr, size));
                }
                Err(val) => current = val,
            }
        }
    }

    /// Returns remaining bytes in the arena (for overflow monitoring)
    pub fn remaining(&self) -> usize {
        let used = self.offset.load(Ordering::Relaxed);
        self.buffer.len().saturating_sub(used)
    }

    /// Total arena capacity in bytes
    pub fn capacity(&self) -> usize {
        self.buffer.len()
    }

    /// フレーム終了時に呼び出し、割り当てコストをゼロで瞬時に全解放する
    pub fn reset(&self) {
        self.offset.store(0, Ordering::Relaxed);
        trace!("BumpArena reset instantly (Zero fragmentation, zero GC).");
    }
}

/// ============================================================================
/// 3. Interned Integer Hash Keys (文字列割り当てゼロの 64bit レジストリキー)
/// ============================================================================

/// 文字列比較 (`"minecraft:dirt" == "minecraft:dirt"`) の代わりに、
/// FNV-1a 64bit ハッシュを用いた O(1) 比較を行う極限キー。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InternedKey {
    pub hash: u64,
}

impl InternedKey {
    /// 文字列からコンパイル時またはロード時に超高速 FNV-1a ハッシュを計算してキーを生成
    #[inline(always)]
    pub const fn from_str(s: &str) -> Self {
        let bytes = s.as_bytes();
        let mut hash: u64 = 0xcbf29ce484222325;
        let mut i = 0;
        while i < bytes.len() {
            hash ^= bytes[i] as u64;
            hash = hash.wrapping_mul(0x100000001b3);
            i += 1;
        }
        Self { hash }
    }
}

/// ============================================================================
/// 4. Lock-Free RCU Event Dispatcher (ゼロ・コンテンション並列ディスパッチャ)
/// ============================================================================

/// DLL Mod リスナー群を読み取る際、`Mutex` や `RwLock` のロックを取得せず、
/// アトミックポインタの RCU (Read-Copy-Update) アーキテクチャによって
/// 何千ものスレッドが同時にゼロ遅延でコールバック配列にアクセスする仕組み。
pub struct LockFreeDispatcher<T: Clone + 'static> {
    ptr: AtomicPtr<Vec<T>>,
}

impl<T: Clone + 'static> Default for LockFreeDispatcher<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone + 'static> LockFreeDispatcher<T> {
    pub fn new() -> Self {
        let initial_vec = Box::new(Vec::<T>::new());
        Self {
            ptr: AtomicPtr::new(Box::into_raw(initial_vec)),
        }
    }

    /// リスナーを登録する（更新時のみ Copy-on-Write でアトミックポインタを差し替え）
    pub fn add(&self, listener: T) {
        let old_ptr = self.ptr.load(Ordering::SeqCst);
        let old_vec = unsafe { &*old_ptr };
        let mut new_vec = old_vec.clone();
        new_vec.push(listener);
        let new_ptr = Box::into_raw(Box::new(new_vec));
        let prev = self.ptr.swap(new_ptr, Ordering::SeqCst);
        unsafe { let _ = Box::from_raw(prev); }
    }

    /// ロックフリーで現在のリスナー配列スライスを取得する（読み取りコスト: アトミックロード 1回のみ！）
    #[inline(always)]
    pub fn get_slice(&self) -> &[T] {
        let raw = self.ptr.load(Ordering::Acquire);
        unsafe { &*raw }
    }
}

impl<T: Clone + 'static> Drop for LockFreeDispatcher<T> {
    fn drop(&mut self) {
        let raw = self.ptr.swap(std::ptr::null_mut(), Ordering::SeqCst);
        if !raw.is_null() {
            unsafe { let _ = Box::from_raw(raw); }
        }
    }
}

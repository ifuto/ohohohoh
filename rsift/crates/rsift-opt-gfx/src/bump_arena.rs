//! Bump Arena Per Task - bumpalo代替の自前アリーナ
//! メッシュ生成時の一時Vec<Quad>はArenaから確保、タスク完了で一括リセット

use std::alloc::{alloc, dealloc, Layout};
use std::cell::RefCell;

pub struct BumpArena {
    ptr: *mut u8,
    cap: usize,
    offset: RefCell<usize>,
    layout: Layout,
}

impl BumpArena {
    pub fn new(cap_bytes: usize) -> Self {
        let layout = Layout::from_size_align(cap_bytes, 64).unwrap();
        let ptr = unsafe { alloc(layout) };
        Self {
            ptr,
            cap: cap_bytes,
            offset: RefCell::new(0),
            layout,
        }
    }

    pub fn alloc_slice<T>(&self, n: usize) -> Option<*mut T> {
        let bytes = n * std::mem::size_of::<T>();
        let align = std::mem::align_of::<T>();
        let mut off = self.offset.borrow_mut();
        let aligned = (*off + align - 1) & !(align - 1);
        if aligned + bytes > self.cap {
            return None;
        }
        let p = unsafe { self.ptr.add(aligned) as *mut T };
        *off = aligned + bytes;
        Some(p)
    }

    pub fn reset(&self) {
        *self.offset.borrow_mut() = 0;
    }
    pub fn used(&self) -> usize {
        *self.offset.borrow()
    }
}

impl Drop for BumpArena {
    fn drop(&mut self) {
        unsafe { dealloc(self.ptr, self.layout) }
    }
}

// SAFETY: BumpArena は確保したヒープブロックを排他的に所有する唯一の所有者であり、
// alloc_slice が返す生ポインタは借用を形成しない (アリーナをスレッド間で「移動」
// しても旧スレッド側に生存する参照は型上作れない)。RefCell により &BumpArena の
// 共有可変は Sync の不在で既に防止されているため、所有権ごとの移動は安全。
unsafe impl Send for BumpArena {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_advances_and_aligns() {
        let arena = BumpArena::new(1024);
        assert_eq!(arena.used(), 0);
        let a = arena.alloc_slice::<u8>(10).unwrap();
        assert_eq!(arena.used(), 10);
        assert!(!a.is_null());
        // u32 要求は 4 アラインに丸めて配置される (先頭ブロックは 64B アライン)。
        let b = arena.alloc_slice::<u32>(4).unwrap();
        assert_eq!((b as usize) % std::mem::align_of::<u32>(), 0);
        // 10 → 12 にアライン後 +16B = 28。
        assert_eq!(arena.used(), 28);
        // 確保領域はオフセット会計どおり書き込み可能。
        unsafe {
            std::ptr::write_bytes(a, 0xAB, 10);
            assert_eq!(*a, 0xAB);
        }
    }

    #[test]
    fn exhaustion_returns_none_and_keeps_offset() {
        let arena = BumpArena::new(32);
        let _ = arena.alloc_slice::<u64>(4).unwrap(); // 32B 使用
        let before = arena.used();
        assert!(arena.alloc_slice::<u8>(1).is_none());
        assert_eq!(arena.used(), before); // 失敗時は offset 不変
    }

    #[test]
    fn align_padding_counts_against_capacity() {
        let arena = BumpArena::new(16);
        let _ = arena.alloc_slice::<u8>(1).unwrap();
        // 残り 15B だが u64 は 8 アラインで 8 に丸まり +8B = 16 丁度。
        assert!(arena.alloc_slice::<u64>(1).is_some());
        assert_eq!(arena.used(), 16);
        assert!(arena.alloc_slice::<u8>(1).is_none());
    }

    #[test]
    fn reset_allows_full_reuse() {
        let arena = BumpArena::new(64);
        let p1 = arena.alloc_slice::<u8>(64).unwrap();
        assert!(arena.alloc_slice::<u8>(1).is_none());
        arena.reset();
        assert_eq!(arena.used(), 0);
        let p2 = arena.alloc_slice::<u8>(64).unwrap();
        assert_eq!(p1, p2); // 同一オフセットから再配布
    }
}

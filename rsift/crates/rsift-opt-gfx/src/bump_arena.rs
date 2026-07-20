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

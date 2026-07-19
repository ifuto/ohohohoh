//! Slab + Object Pool + Generational Allocator for RenderSection.
//!
//! ポインタの付け替えと侵入型フリーリスト（Intrusive Free List）を用いた
//! Cache-Line アライン・世代管理付き（ABA バグ防止）スラブ＆オブジェクトプールを完全実装。
//! `VecDeque` による余分なヒープ確保を全廃。

use std::marker::PhantomData;

/// 世代管理付きスラブハンドル（ABA 衝突と Use-After-Free を完全防止）。
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct SlabHandle<T> {
    pub index: u32,
    pub generation: u32,
    _marker: PhantomData<T>,
}

impl<T> Clone for SlabHandle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for SlabHandle<T> {}

impl<T> SlabHandle<T> {
    #[inline]
    pub fn new(index: u32, generation: u32) -> Self {
        Self {
            index,
            generation,
            _marker: PhantomData,
        }
    }
}

#[derive(Debug, Clone)]
enum Slot<T> {
    Vacant { next_free: u32, generation: u32 },
    Occupied { value: T, generation: u32 },
}

/// Cache-line aligned Generational Slab with zero-allocation intrusive free list.
#[repr(C, align(64))]
pub struct GenerationalSlab<T> {
    slots: Vec<Slot<T>>,
    free_head: u32,
    occupied_count: usize,
}

impl<T> GenerationalSlab<T> {
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            free_head: u32::MAX,
            occupied_count: 0,
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            slots: Vec::with_capacity(capacity),
            free_head: u32::MAX,
            occupied_count: 0,
        }
    }

    pub fn insert(&mut self, value: T) -> SlabHandle<T> {
        self.occupied_count += 1;
        if self.free_head != u32::MAX {
            let idx = self.free_head as usize;
            if let Slot::Vacant { next_free, generation } = self.slots[idx] {
                self.free_head = next_free;
                self.slots[idx] = Slot::Occupied { value, generation };
                return SlabHandle::new(idx as u32, generation);
            }
        }

        let idx = self.slots.len() as u32;
        self.slots.push(Slot::Occupied { value, generation: 0 });
        SlabHandle::new(idx, 0)
    }

    pub fn remove(&mut self, handle: SlabHandle<T>) -> Option<T> {
        let idx = handle.index as usize;
        if idx >= self.slots.len() {
            return None;
        }
        match self.slots[idx] {
            Slot::Occupied { ref generation, .. } if *generation == handle.generation => {
                let next_gen = generation.wrapping_add(1);
                let old_slot = std::mem::replace(
                    &mut self.slots[idx],
                    Slot::Vacant {
                        next_free: self.free_head,
                        generation: next_gen,
                    },
                );
                self.free_head = idx as u32;
                self.occupied_count -= 1;
                if let Slot::Occupied { value, .. } = old_slot {
                    Some(value)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    #[inline]
    pub fn get(&self, handle: SlabHandle<T>) -> Option<&T> {
        let idx = handle.index as usize;
        match self.slots.get(idx) {
            Some(Slot::Occupied { value, generation }) if *generation == handle.generation => {
                Some(value)
            }
            _ => None,
        }
    }

    #[inline]
    pub fn get_mut(&mut self, handle: SlabHandle<T>) -> Option<&mut T> {
        let idx = handle.index as usize;
        match self.slots.get_mut(idx) {
            Some(Slot::Occupied { value, generation }) if *generation == handle.generation => {
                Some(value)
            }
            _ => None,
        }
    }

    pub fn len(&self) -> usize {
        self.occupied_count
    }

    pub fn is_empty(&self) -> bool {
        self.occupied_count == 0
    }
}

/// Back-compat index-based `Slab<T>` without `VecDeque` overhead.
pub struct Slab<T> {
    inner: GenerationalSlab<T>,
    idx_to_handle: Vec<Option<SlabHandle<T>>>,
}

impl<T> Slab<T> {
    pub fn new() -> Self {
        Self {
            inner: GenerationalSlab::new(),
            idx_to_handle: Vec::new(),
        }
    }

    pub fn alloc(&mut self, val: T) -> usize {
        let handle = self.inner.insert(val);
        let idx = handle.index as usize;
        if idx >= self.idx_to_handle.len() {
            self.idx_to_handle.resize(idx + 1, None);
        }
        self.idx_to_handle[idx] = Some(handle);
        idx
    }

    pub fn free(&mut self, idx: usize) -> Option<T> {
        if let Some(Some(handle)) = self.idx_to_handle.get_mut(idx).map(|h| h.take()) {
            self.inner.remove(handle)
        } else {
            None
        }
    }

    #[inline]
    pub fn get(&self, idx: usize) -> Option<&T> {
        if let Some(Some(handle)) = self.idx_to_handle.get(idx) {
            self.inner.get(*handle)
        } else {
            None
        }
    }

    #[inline]
    pub fn get_mut(&mut self, idx: usize) -> Option<&mut T> {
        if let Some(Some(handle)) = self.idx_to_handle.get_mut(idx) {
            self.inner.get_mut(*handle)
        } else {
            None
        }
    }
}

/// Pre-allocated cache-friendly Object Pool with zero runtime allocation overhead.
#[repr(C, align(64))]
pub struct ObjectPool<T> {
    pool: Vec<T>,
}

impl<T: Default> ObjectPool<T> {
    pub fn new(cap: usize) -> Self {
        let mut pool = Vec::with_capacity(cap);
        for _ in 0..cap {
            pool.push(T::default());
        }
        Self { pool }
    }

    #[inline]
    pub fn acquire(&mut self) -> Option<T> {
        self.pool.pop()
    }

    #[inline]
    pub fn release(&mut self, obj: T) {
        self.pool.push(obj);
    }

    pub fn available(&self) -> usize {
        self.pool.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generational_slab() {
        let mut slab = GenerationalSlab::new();
        let h1 = slab.insert(100);
        let h2 = slab.insert(200);

        assert_eq!(slab.get(h1), Some(&100));
        assert_eq!(slab.get(h2), Some(&200));

        assert_eq!(slab.remove(h1), Some(100));
        assert_eq!(slab.get(h1), None);

        let h3 = slab.insert(300);
        assert_eq!(h3.index, h1.index);
        assert_ne!(h3.generation, h1.generation);
        assert_eq!(slab.get(h3), Some(&300));
        assert_eq!(slab.get(h1), None);
    }

    #[test]
    fn test_compat_slab() {
        let mut slab = Slab::new();
        let i1 = slab.alloc("hello");
        let i2 = slab.alloc("world");
        assert_eq!(slab.get(i1), Some(&"hello"));
        assert_eq!(slab.free(i1), Some("hello"));
        assert_eq!(slab.get(i1), None);
        assert_eq!(slab.get(i2), Some(&"world"));
    }
}

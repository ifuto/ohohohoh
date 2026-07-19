
//! Slab + Object Pool for RenderSection - ポインタ付け替えだけで再利用

use std::collections::VecDeque;

pub struct Slab<T> {
    slots: Vec<Option<T>>,
    free: VecDeque<usize>,
}

impl<T> Slab<T> {
    pub fn new() -> Self { Self { slots: Vec::new(), free: VecDeque::new() } }

    pub fn alloc(&mut self, val: T) -> usize {
        if let Some(idx) = self.free.pop_front() {
            self.slots[idx] = Some(val);
            idx
        } else {
            let idx = self.slots.len();
            self.slots.push(Some(val));
            idx
        }
    }

    pub fn free(&mut self, idx: usize) -> Option<T> {
        if idx < self.slots.len() {
            let v = self.slots[idx].take();
            self.free.push_back(idx);
            v
        } else { None }
    }

    pub fn get(&self, idx: usize) -> Option<&T> {
        self.slots.get(idx).and_then(|o| o.as_ref())
    }

    pub fn get_mut(&mut self, idx: usize) -> Option<&mut T> {
        self.slots.get_mut(idx).and_then(|o| o.as_mut())
    }
}

pub struct ObjectPool<T> {
    pool: Vec<T>,
}

impl<T: Default> ObjectPool<T> {
    pub fn new(cap: usize) -> Self {
        let mut pool = Vec::with_capacity(cap);
        for _ in 0..cap { pool.push(T::default()); }
        Self { pool }
    }
    pub fn acquire(&mut self) -> Option<T> { self.pool.pop() }
    pub fn release(&mut self, obj: T) { self.pool.push(obj); }
}

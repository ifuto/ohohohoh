//! Frame bump allocator — reclaim all scratch memory each tick (CPU RAM relief).

use std::alloc::{alloc, dealloc, Layout};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Fixed-capacity bump arena. Not thread-safe; one per RsCalc worker / frame.
pub struct FrameBumpArena {
    base: NonNull<u8>,
    capacity: usize,
    offset: usize,
    high_water: AtomicUsize,
}

unsafe impl Send for FrameBumpArena {}

impl FrameBumpArena {
    pub fn with_capacity_mb(mb: usize) -> Self {
        let capacity = mb.max(1).saturating_mul(1024 * 1024);
        let layout = Layout::from_size_align(capacity, 64).expect("arena layout");
        let ptr = unsafe { alloc(layout) };
        let base = NonNull::new(ptr).expect("arena alloc failed");
        Self {
            base,
            capacity,
            offset: 0,
            high_water: AtomicUsize::new(0),
        }
    }

    pub fn reset(&mut self) {
        let hw = self.high_water.get_mut();
        *hw = (*hw).max(self.offset);
        self.offset = 0;
    }

    pub fn alloc_bytes(&mut self, size: usize, align: usize) -> Option<&mut [u8]> {
        let align = align.max(1).next_power_of_two();
        let aligned = (self.offset + align - 1) & !(align - 1);
        let end = aligned.checked_add(size)?;
        if end > self.capacity {
            return None;
        }
        self.offset = end;
        Some(unsafe {
            std::slice::from_raw_parts_mut(self.base.as_ptr().add(aligned), size)
        })
    }

    pub fn alloc_slice<T: Copy>(&mut self, len: usize) -> Option<&mut [T]> {
        let size = len.checked_mul(std::mem::size_of::<T>())?;
        let align = std::mem::align_of::<T>();
        let bytes = self.alloc_bytes(size, align)?;
        Some(unsafe {
            std::slice::from_raw_parts_mut(bytes.as_mut_ptr() as *mut T, len)
        })
    }

    pub fn used(&self) -> usize {
        self.offset
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn high_water_mark(&self) -> usize {
        self.high_water.load(Ordering::Relaxed).max(self.offset)
    }
}

impl Drop for FrameBumpArena {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(self.capacity, 64).expect("arena layout");
        unsafe { dealloc(self.base.as_ptr(), layout) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bump_reset_reuses() {
        let mut a = FrameBumpArena::with_capacity_mb(1);
        let s = a.alloc_slice::<u32>(100).unwrap();
        s[0] = 7;
        assert_eq!(a.used(), 400);
        a.reset();
        assert_eq!(a.used(), 0);
        let s2 = a.alloc_slice::<u32>(100).unwrap();
        assert_eq!(s2[0], 7); // uncleared — intentional for speed
    }
}

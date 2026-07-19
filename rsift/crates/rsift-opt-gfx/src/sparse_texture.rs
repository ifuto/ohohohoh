//! Virtual (sparse) texturing — page-table residency with LRU eviction.
//!
//! Only the texture pages actually needed on screen are made physical, so a
//! 16K×16K texture costs almost nothing until its tiles are sampled. Critical on
//! integrated GPUs with shared/unified memory, where VRAM is the system RAM.
//! Includes mip selection from camera distance, like `mip_streaming` but for
//! sparse (partially-resident) pages.

use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct SparsePageTable {
    /// Number of physical pages available.
    pub max_physical: u32,
    /// (page_id, mip) -> physical slot.
    resident: HashMap<(u32, u32), u32>,
    /// LRU ordering of keys (front = most recently used).
    order: std::collections::VecDeque<(u32, u32)>,
}
impl SparsePageTable {
    pub fn new(max_physical: u32) -> Self {
        Self {
            max_physical,
            resident: HashMap::new(),
            order: std::collections::VecDeque::new(),
        }
    }

    pub fn is_resident(&self, page: u32, mip: u32) -> bool {
        self.resident.contains_key(&(page, mip))
    }

    /// Request a page. Returns its physical slot, evicting the LRU resident
    /// page if we are over budget.
    pub fn request(&mut self, page: u32, mip: u32) -> u32 {
        let key = (page, mip);
        if let Some(&slot) = self.resident.get(&key) {
            // touch: move to front of LRU
            self.order.retain(|k| *k != key);
            self.order.push_front(key);
            return slot;
        }
        let slot = if self.resident.len() >= self.max_physical as usize {
            // evict LRU (back of queue)
            if let Some(evicted) = self.order.pop_back() {
                self.resident.remove(&evicted);
            }
            // allocate a slot == current resident count (compacted by eviction)
            self.resident.len() as u32
        } else {
            self.resident.len() as u32
        };
        self.resident.insert(key, slot);
        self.order.push_front(key);
        slot
    }

    pub fn resident_count(&self) -> usize {
        self.resident.len()
    }

    /// Pick the mip level to stream for a page, given camera distance and the
    /// world size a single top-level texel covers on screen. `screen_h` is in
    /// pixels; returns a mip index (0 = full res).
    /// 距離からミップレベルを選ぶ。`world_size` の対象が mip0 で `screen_h`
    /// テクセルぶんの解像度を持つという基準で、「1テクセル ≈ 1ピクセン」に
    /// なる水位 = log2(基準解像度 / スクリーン占有ピクセル数)。
    pub fn mip_for_distance(distance: f32, world_size: f32, screen_h: f32, max_mip: u32) -> u32 {
        let screen_fraction = (world_size / distance.max(1e-3)) * screen_h;
        // 近い (screen_fraction が大きい) ほど mip0、遠いほど高ミップ。
        let mip = (screen_h / screen_fraction.max(1.0)).log2().ceil();
        mip.clamp(0.0, max_mip as f32) as u32
    }

    pub fn wgsl_source(&self) -> &'static str {
        SPARSE_TEXTURE_WGSL
    }
}

pub const SPARSE_TEXTURE_WGSL: &str = include_str!("../shaders/sparse_texture.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn request_allocates_and_resident() {
        let mut t = SparsePageTable::new(4);
        let s = t.request(1, 0);
        assert!(t.is_resident(1, 0));
        assert_eq!(s, 0);
        assert_eq!(t.resident_count(), 1);
    }
    #[test]
    fn evicts_lru_when_full() {
        let mut t = SparsePageTable::new(2);
        t.request(1, 0); // slot 0
        t.request(2, 0); // slot 1
        // now full (2). Request a third -> evicts LRU = page 1
        let s = t.request(3, 0);
        assert_eq!(s, 1); // compacted slot
        assert!(!t.is_resident(1, 0));
        assert!(t.is_resident(2, 0));
        assert!(t.is_resident(3, 0));
        assert_eq!(t.resident_count(), 2);
    }
    #[test]
    fn touch_promotes() {
        let mut t = SparsePageTable::new(2);
        t.request(1, 0);
        t.request(2, 0);
        // touch page 1 so it becomes MRU; next eviction should drop page 2
        t.request(1, 0);
        t.request(4, 0);
        assert!(t.is_resident(1, 0));
        assert!(!t.is_resident(2, 0));
    }
    #[test]
    fn mip_selection() {
        // close object -> mip 0; far object -> higher mip
        let near = SparsePageTable::mip_for_distance(2.0, 10.0, 1080.0, 8);
        let far = SparsePageTable::mip_for_distance(200.0, 10.0, 1080.0, 8);
        assert_eq!(near, 0);
        assert!(far > near);
    }
}

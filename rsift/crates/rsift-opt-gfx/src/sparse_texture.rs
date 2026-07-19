//! Virtual (sparse) texturing — zero-allocation intrusive LRU page-table residency (`IntrusiveLruPageTable`).
//!
//! 16K×16K テクスチャ等の広大な仮想アドレス空間において、画面上に実際に可視なタイルのみを
//! `max_physical` スロットの物理 VRAM に割り当て、上限超過時は完全 $O(1)$ の侵入型
//! 双方向リストで LRU ページを即座に退避・入れ替える。`VecDeque` ヒープ確保ゼロ。

use std::collections::HashMap;

#[derive(Clone, Copy, Debug)]
struct LruNode {
    prev: u32,
    next: u32,
    key: (u32, u32), // (page_id, mip)
    in_use: bool,
}

pub struct SparsePageTable {
    pub max_physical: u32,
    resident: HashMap<(u32, u32), u32>,
    nodes: Vec<LruNode>,
    head: u32,
    tail: u32,
}

impl SparsePageTable {
    pub fn new(max_physical: u32) -> Self {
        assert!(max_physical > 0, "max_physical must be at least 1");
        let cap = max_physical as usize;
        let mut nodes = Vec::with_capacity(cap);
        for _ in 0..cap {
            nodes.push(LruNode {
                prev: u32::MAX,
                next: u32::MAX,
                key: (0, 0),
                in_use: false,
            });
        }
        Self {
            max_physical,
            resident: HashMap::with_capacity(cap),
            nodes,
            head: u32::MAX,
            tail: u32::MAX,
        }
    }

    pub fn is_resident(&self, page: u32, mip: u32) -> bool {
        self.resident.contains_key(&(page, mip))
    }

    pub fn resident_count(&self) -> usize {
        self.resident.len()
    }

    /// Touch existing physical slot: detach and move to front of MRU list in $O(1)$.
    fn touch(&mut self, slot: u32) {
        if self.head == slot {
            return;
        }
        self.detach(slot);
        self.push_front(slot);
    }

    fn detach(&mut self, slot: u32) {
        let p = self.nodes[slot as usize].prev;
        let n = self.nodes[slot as usize].next;
        if p != u32::MAX {
            self.nodes[p as usize].next = n;
        } else {
            self.head = n;
        }
        if n != u32::MAX {
            self.nodes[n as usize].prev = p;
        } else {
            self.tail = p;
        }
    }

    fn push_front(&mut self, slot: u32) {
        self.nodes[slot as usize].prev = u32::MAX;
        self.nodes[slot as usize].next = self.head;
        if self.head != u32::MAX {
            self.nodes[self.head as usize].prev = slot;
        } else {
            self.tail = slot;
        }
        self.head = slot;
    }

    /// Request a page. Returns its physical slot, evicting the LRU resident page in $O(1)$ when full.
    pub fn request(&mut self, page: u32, mip: u32) -> u32 {
        let key = (page, mip);
        if let Some(&slot) = self.resident.get(&key) {
            self.touch(slot);
            return slot;
        }

        let slot = if self.resident.len() < self.max_physical as usize {
            self.resident.len() as u32
        } else {
            let lru_slot = self.tail;
            let old_key = self.nodes[lru_slot as usize].key;
            self.resident.remove(&old_key);
            self.detach(lru_slot);
            lru_slot
        };

        self.nodes[slot as usize].key = key;
        self.nodes[slot as usize].in_use = true;
        self.resident.insert(key, slot);
        self.push_front(slot);
        slot
    }

    pub fn mip_for_distance(distance: f32, world_size: f32, screen_h: f32, max_mip: u32) -> u32 {
        let screen_fraction = (world_size / distance.max(1e-3)) * screen_h;
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
        let s = t.request(3, 0); // evicts LRU = page 1
        assert_eq!(s, 0); // reuses slot 0
        assert!(!t.is_resident(1, 0));
        assert!(t.is_resident(2, 0));
        assert!(t.is_resident(3, 0));
        assert_eq!(t.resident_count(), 2);
    }

    #[test]
    fn touch_promotes_mru() {
        let mut t = SparsePageTable::new(2);
        t.request(1, 0);
        t.request(2, 0);
        t.request(1, 0); // promote page 1
        t.request(3, 0); // evict page 2 instead of page 1
        assert!(t.is_resident(1, 0));
        assert!(!t.is_resident(2, 0));
        assert!(t.is_resident(3, 0));
    }
}

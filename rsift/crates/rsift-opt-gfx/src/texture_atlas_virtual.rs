
//! Sparse Virtual Texture Atlas - VRAM 100MB以下でも高解像度リソパ対応
//! タイル64x64、不要タイルは未常駐、必要時にストリーミング。

use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileCoord { pub x: u32, pub y: u32, pub mip: u8 }

pub struct VirtualAtlas {
    pub width: u32,
    pub height: u32,
    pub tile_size: u32,
    pub resident: HashSet<TileCoord>,
    pub page_table: HashMap<TileCoord, u32>, // tile -> physical page index
    pub free_pages: Vec<u32>,
    pub capacity_pages: u32,
}

impl VirtualAtlas {
    pub fn new(width: u32, height: u32, tile_size: u32, capacity_pages: u32) -> Self {
        Self {
            width, height, tile_size,
            resident: HashSet::new(),
            page_table: HashMap::new(),
            free_pages: (0..capacity_pages).collect(),
            capacity_pages,
        }
    }

    pub fn request_tiles(&mut self, needed: &[TileCoord]) -> Vec<TileCoord> {
        let mut to_stream = Vec::new();
        for &tile in needed {
            if !self.resident.contains(&tile) {
                if let Some(page) = self.free_pages.pop() {
                    self.page_table.insert(tile, page);
                    self.resident.insert(tile);
                    to_stream.push(tile);
                }
            }
        }
        to_stream
    }

    pub fn evict_lru(&mut self, to_evict: usize) {
        // 簡易LRU: 任意のタイルを開放
        let evict: Vec<TileCoord> = self.resident.iter().take(to_evict).copied().collect();
        for tile in evict {
            if let Some(page) = self.page_table.remove(&tile) {
                self.free_pages.push(page);
            }
            self.resident.remove(&tile);
        }
    }

    pub fn residency_ratio(&self) -> f32 {
        self.resident.len() as f32 / self.capacity_pages as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(x: u32, y: u32, mip: u8) -> TileCoord {
        TileCoord { x, y, mip }
    }

    #[test]
    fn new_initializes_full_free_pool() {
        let a = VirtualAtlas::new(1024, 1024, 64, 4);
        assert_eq!(a.free_pages, (0..4).collect::<Vec<u32>>());
        assert!(a.resident.is_empty());
        assert!(a.page_table.is_empty());
        assert_eq!(a.residency_ratio(), 0.0);
    }

    #[test]
    fn request_allocates_lifo_pages_and_is_idempotent() {
        let mut a = VirtualAtlas::new(1024, 1024, 64, 4);
        let s = a.request_tiles(&[tile(0, 0, 0), tile(1, 0, 0)]);
        assert_eq!(s, vec![tile(0, 0, 0), tile(1, 0, 0)]);
        // free_pages は Vec::pop (末尾=LIFO) → 3, 2 の順で払い出し。
        assert_eq!(a.page_table[&tile(0, 0, 0)], 3);
        assert_eq!(a.page_table[&tile(1, 0, 0)], 2);
        // 常駐済みの再要求は新規払い出し無し。
        assert!(a.request_tiles(&[tile(0, 0, 0)]).is_empty());
        assert_eq!(a.free_pages.len(), 2);
        // 同一バッチ内の重複タイルは常駐化により 1 件に dedupe。
        let dup = a.request_tiles(&[tile(9, 9, 0), tile(9, 9, 0)]);
        assert_eq!(dup, vec![tile(9, 9, 0)]);
        assert_eq!(a.free_pages.len(), 1);
        assert_eq!(a.residency_ratio(), 0.75);
    }

    #[test]
    fn capacity_exhaustion_rejects_overflow() {
        let mut a = VirtualAtlas::new(1024, 1024, 64, 2);
        let s = a.request_tiles(&[tile(0, 0, 0), tile(1, 0, 0), tile(2, 0, 0)]);
        assert_eq!(s.len(), 2);
        assert!(a.resident.contains(&tile(0, 0, 0)));
        assert!(!a.resident.contains(&tile(2, 0, 0)));
        assert!(a.page_table.get(&tile(2, 0, 0)).is_none());
    }

    #[test]
    fn evict_lru_returns_pages_and_allows_restream() {
        let mut a = VirtualAtlas::new(1024, 1024, 64, 2);
        let _ = a.request_tiles(&[tile(0, 0, 0), tile(1, 0, 0)]);
        a.evict_lru(2);
        assert!(a.resident.is_empty());
        assert!(a.page_table.is_empty());
        assert_eq!(a.free_pages.len(), 2);
        assert_eq!(a.residency_ratio(), 0.0);
        // 解放後は同じタイルが再ストリーミング可能。
        assert_eq!(a.request_tiles(&[tile(0, 0, 0)]), vec![tile(0, 0, 0)]);
    }
}

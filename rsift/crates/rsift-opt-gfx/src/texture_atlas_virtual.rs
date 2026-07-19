
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

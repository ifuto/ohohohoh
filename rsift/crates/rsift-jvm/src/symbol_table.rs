
//! Symbol Table Interning - String minecraft:stoneをu16 symbolIdに変換

use std::collections::HashMap;
use std::sync::atomic::{AtomicU16, Ordering};

pub struct SymbolTable {
    map: HashMap<String, u16>,
    rev: HashMap<u16, String>,
    next: AtomicU16,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self { map: HashMap::new(), rev: HashMap::new(), next: AtomicU16::new(1) }
    }

    pub fn intern(&mut self, s: &str) -> u16 {
        if let Some(&id) = self.map.get(s) { return id; }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.map.insert(s.to_string(), id);
        self.rev.insert(id, s.to_string());
        id
    }

    pub fn get(&self, id: u16) -> Option<&String> { self.rev.get(&id) }

    pub fn get_id(&self, s: &str) -> Option<u16> { self.map.get(s).copied() }
}

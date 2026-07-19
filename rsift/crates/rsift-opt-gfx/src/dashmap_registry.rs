
//! Lock-Free DashMap for Task Registry - ChunkState管理

use std::sync::atomic::{AtomicU64, Ordering};
use dashmap::DashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkBuildState { Pending, Building, Done, Failed }

pub struct ChunkRegistry {
    map: DashMap<(i32,i32), ChunkBuildState>,
    version: AtomicU64,
}

impl ChunkRegistry {
    pub fn new() -> Self {
        Self { map: DashMap::new(), version: AtomicU64::new(0) }
    }

    pub fn set_state(&self, key: (i32,i32), state: ChunkBuildState) {
        self.map.insert(key, state);
        self.version.fetch_add(1, Ordering::Relaxed);
    }

    pub fn get_state(&self, key: (i32,i32)) -> Option<ChunkBuildState> {
        self.map.get(&key).map(|v| *v)
    }

    pub fn pending_chunks(&self) -> Vec<(i32,i32)> {
        self.map.iter().filter(|kv| *kv.value()==ChunkBuildState::Pending).map(|kv| *kv.key()).collect()
    }

    pub fn version(&self) -> u64 { self.version.load(Ordering::Relaxed) }
}

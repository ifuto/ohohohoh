
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_roundtrip_and_version_counter() {
        let r = ChunkRegistry::new();
        assert_eq!(r.version(), 0);
        assert_eq!(r.get_state((1, 2)), None);
        r.set_state((1, 2), ChunkBuildState::Pending);
        assert_eq!(r.get_state((1, 2)), Some(ChunkBuildState::Pending));
        assert_eq!(r.version(), 1);
        r.set_state((1, 2), ChunkBuildState::Building); // 上書きでも version 前進
        assert_eq!(r.get_state((1, 2)), Some(ChunkBuildState::Building));
        assert_eq!(r.version(), 2);
    }

    #[test]
    fn pending_chunks_filters_only_pending_as_set() {
        let r = ChunkRegistry::new();
        r.set_state((0, 0), ChunkBuildState::Pending);
        r.set_state((1, 1), ChunkBuildState::Done);
        r.set_state((2, 2), ChunkBuildState::Pending);
        r.set_state((3, 3), ChunkBuildState::Failed);
        let mut p = r.pending_chunks();
        p.sort();
        assert_eq!(p, vec![(0, 0), (2, 2)]);
        r.set_state((0, 0), ChunkBuildState::Done);
        assert_eq!(r.pending_chunks(), vec![(2, 2)]);
    }
}

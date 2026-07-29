
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

    /// 状態別 count [Pending, Building, Done, Failed] (wiring 実計測の消費面、
    /// wave 171 FQ 捕捉 109: report.registry_building/done の供給元)。
    pub fn state_counts(&self) -> [u32; 4] {
        let mut c = [0u32; 4];
        for kv in self.map.iter() {
            match kv.value() {
                ChunkBuildState::Pending => c[0] += 1,
                ChunkBuildState::Building => c[1] += 1,
                ChunkBuildState::Done => c[2] += 1,
                ChunkBuildState::Failed => c[3] += 1,
            }
        }
        c
    }

    /// 追跡中 chunk 総数 (wiring report.registry_total / 整合検証の消費面)。
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// 退去 chunk を除去。除去も変更操作として version 前進 (rq fq_registry:
    /// 不在 remove も version 前進 — 操作記録として一貫)。wave 171 FQ 捕捉
    /// 109: wiring が真 lifecycle (evict → remove) を駆動する実消費者となる。
    pub fn remove(&self, key: (i32, i32)) {
        self.map.remove(&key);
        self.version.fetch_add(1, Ordering::Relaxed);
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

    /// 【wave 171 FQ 捕捉 109】退去 chunk の remove roundtrip (真 lifecycle:
    /// 除去も変更操作として version 前進、rq fq_registry)。現状 remove 非存在
    /// で compile RED → API 追加で GREEN。
    /// 【wave 171 FQ 捕捉 109】state_counts/len の API 単体 golden pin
    /// (wiring report 配線の供給元契約)。
    #[test]
    fn fq_state_counts_and_len_golden() {
        let r = ChunkRegistry::new();
        assert!(r.is_empty());
        r.set_state((0, 0), ChunkBuildState::Pending);
        r.set_state((1, 1), ChunkBuildState::Building);
        r.set_state((2, 2), ChunkBuildState::Done);
        r.set_state((3, 3), ChunkBuildState::Done);
        r.set_state((4, 4), ChunkBuildState::Failed);
        assert_eq!(r.state_counts(), [1, 1, 2, 1]);
        assert_eq!(r.len(), 5);
        r.remove((3, 3));
        assert_eq!(r.state_counts(), [1, 1, 1, 1]);
        assert_eq!(r.len(), 4);
    }

    #[test]
    fn fq_remove_roundtrip() {
        let r = ChunkRegistry::new();
        r.set_state((5, 6), ChunkBuildState::Building);
        assert_eq!(r.version(), 1);
        assert!(r.get_state((5, 6)).is_some());
        r.remove((5, 6));
        assert_eq!(r.get_state((5, 6)), None);
        assert_eq!(r.version(), 2); // 除去も変更操作として version 前進
        r.remove((5, 6)); // 不在 remove は no-op でも version 前進 (操作記録)
        assert_eq!(r.version(), 3);
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

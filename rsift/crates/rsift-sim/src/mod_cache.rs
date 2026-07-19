//! Phase 1/3 — ModernFix-style mod metadata + content-addressed cache + lazy JSON.

use rustc_hash::FxHashMap;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct ModMeta {
    pub id: String,
    pub version: String,
    pub path: PathBuf,
    pub mtime: SystemTime,
    pub content_hash: u64,
}

#[derive(Debug, Default)]
pub struct ModMetadataCache {
    by_id: FxHashMap<String, ModMeta>,
    /// content hash → cached decoded JSON (lazy)
    json_cache: FxHashMap<u64, Value>,
    cap: usize,
}

impl ModMetadataCache {
    pub fn with_cap(cap: usize) -> Self {
        Self {
            by_id: FxHashMap::default(),
            json_cache: FxHashMap::default(),
            cap: cap.max(32),
        }
    }

    pub fn upsert_from_path(&mut self, id: &str, version: &str, path: &Path) -> Option<ModMeta> {
        let meta = fs::metadata(path).ok()?;
        let mtime = meta.modified().ok()?;
        let bytes = fs::read(path).ok()?;
        let content_hash = crate::memory::fx_hash_u64(&bytes);
        let m = ModMeta {
            id: id.into(),
            version: version.into(),
            path: path.to_path_buf(),
            mtime,
            content_hash,
        };
        self.by_id.insert(id.into(), m.clone());
        Some(m)
    }

    /// Content-addressed: same hash → reuse parsed JSON without re-read parse.
    pub fn lazy_json(&mut self, hash: u64, bytes: &[u8]) -> Option<&Value> {
        if self.json_cache.len() >= self.cap && !self.json_cache.contains_key(&hash) {
            self.json_cache.clear();
        }
        if !self.json_cache.contains_key(&hash) {
            let v: Value = serde_json::from_slice(bytes).ok()?;
            self.json_cache.insert(hash, v);
            debug!(hash, "mod json cached");
        }
        self.json_cache.get(&hash)
    }

    pub fn get(&self, id: &str) -> Option<&ModMeta> {
        self.by_id.get(id)
    }
}

/// DataFix deferred queue — run only when world version requires it.
#[derive(Debug, Default)]
pub struct DeferredDataFix {
    pending_worlds: Vec<(PathBuf, u32)>, // path, data_version
    pub current_data_version: u32,
}

impl DeferredDataFix {
    pub fn enqueue(&mut self, world: PathBuf, version: u32) {
        if version < self.current_data_version {
            self.pending_worlds.push((world, version));
        }
    }

    pub fn drain_budget(&mut self, max: usize) -> Vec<(PathBuf, u32)> {
        let n = self.pending_worlds.len().min(max);
        self.pending_worlds.drain(0..n).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_addressed_json() {
        let mut c = ModMetadataCache::with_cap(8);
        let bytes = br#"{"id":"demo","version":"1"}"#;
        let h = crate::memory::fx_hash_u64(bytes);
        let v1 = c.lazy_json(h, bytes).unwrap().clone();
        let v2 = c.lazy_json(h, bytes).unwrap();
        assert_eq!(&v1, v2);
    }
}

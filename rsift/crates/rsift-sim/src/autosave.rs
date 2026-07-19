//! Phase 1 — dirty-only autosave with staggered budget (no save storms).

use crate::region_io::{RegionError, RegionFile};
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Instant;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct DirtyChunk {
    pub cx: i32,
    pub cz: i32,
    pub data: Vec<u8>,
    pub prefer_zstd: bool,
}

pub struct AutosaveController {
    root: PathBuf,
    dirty: Mutex<FxHashMap<(i32, i32), DirtyChunk>>,
    queue: Mutex<VecDeque<(i32, i32)>>,
    /// Max chunks saved per autosave pulse.
    pub max_per_pulse: usize,
    /// Minimum ms between pulses.
    pub interval_ms: u64,
    last_pulse: Mutex<Instant>,
    saved: Mutex<u64>,
}

impl AutosaveController {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            dirty: Mutex::new(FxHashMap::default()),
            queue: Mutex::new(VecDeque::new()),
            max_per_pulse: 8,
            interval_ms: 5_000,
            last_pulse: Mutex::new(Instant::now()),
            saved: Mutex::new(0),
        }
    }

    pub fn mark_dirty(&self, chunk: DirtyChunk) {
        let key = (chunk.cx, chunk.cz);
        let mut dirty = self.dirty.lock();
        let is_new = !dirty.contains_key(&key);
        dirty.insert(key, chunk);
        drop(dirty);
        if is_new {
            self.queue.lock().push_back(key);
        }
    }

    pub fn dirty_count(&self) -> usize {
        self.dirty.lock().len()
    }

    /// Save up to `max_per_pulse` dirty chunks if interval elapsed.
    pub fn pulse(&self) -> Result<usize, RegionError> {
        {
            let last = self.last_pulse.lock();
            if (last.elapsed().as_millis() as u64) < self.interval_ms {
                return Ok(0);
            }
        }
        let mut saved_n = 0usize;
        for _ in 0..self.max_per_pulse {
            let key = self.queue.lock().pop_front();
            let Some(key) = key else { break };
            let chunk = self.dirty.lock().remove(&key);
            let Some(chunk) = chunk else { continue };
            let rx = chunk.cx.div_euclid(32);
            let rz = chunk.cz.div_euclid(32);
            let mut region = RegionFile::open(&self.root, rx, rz)?;
            region.write_chunk(chunk.cx, chunk.cz, &chunk.data, chunk.prefer_zstd)?;
            saved_n += 1;
            *self.saved.lock() += 1;
        }
        *self.last_pulse.lock() = Instant::now();
        debug!(saved_n, remaining = self.dirty_count(), "autosave pulse");
        Ok(saved_n)
    }

    pub fn total_saved(&self) -> u64 {
        *self.saved.lock()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn only_dirty_saved() {
        let dir = std::env::temp_dir().join("rsift_autosave_test");
        let _ = fs::remove_dir_all(&dir);
        let ctrl = AutosaveController::new(&dir);
        // Force immediate pulse
        *ctrl.last_pulse.lock() = Instant::now() - std::time::Duration::from_secs(10);
        ctrl.mark_dirty(DirtyChunk {
            cx: 1,
            cz: 2,
            data: vec![1, 2, 3, 4],
            prefer_zstd: false,
        });
        let n = ctrl.pulse().unwrap();
        assert_eq!(n, 1);
        assert_eq!(ctrl.dirty_count(), 0);
        let _ = fs::remove_dir_all(&dir);
    }
}

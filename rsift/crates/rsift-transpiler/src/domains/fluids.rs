//! Fluid flow — empty until fluid cells are ingested from the world mirror.

use rayon::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct FluidDomain {
    levels: Vec<u8>,
    enabled: bool,
}

impl FluidDomain {
    pub fn empty(enabled: bool) -> Self {
        Self {
            levels: Vec::new(),
            enabled,
        }
    }

    pub fn new(enabled: bool) -> Self {
        Self::empty(enabled)
    }

    /// Ingest packed fluid levels (0..=7). Empty input keeps the domain idle.
    pub fn ingest(&mut self, levels: &[u8]) {
        self.levels.clear();
        self.levels.extend_from_slice(levels);
    }

    pub fn tick(&mut self, parallel: bool, stats: &AtomicU64) -> u64 {
        if !self.enabled {
            stats.store(0, Ordering::Relaxed);
            return 0;
        }
        let n = self.levels.len();
        if n == 0 {
            stats.store(0, Ordering::Relaxed);
            return 0;
        }
        let snapshot = self.levels.clone();
        if parallel && n >= 128 {
            self.levels.par_iter_mut().enumerate().for_each(|(i, cell)| {
                let flow = snapshot[i].saturating_sub(1);
                *cell = snapshot[i].max(flow);
                if i + 1 < n {
                    *cell = (*cell).max(snapshot[i + 1].saturating_sub(1));
                }
            });
        } else {
            for i in 0..n {
                let mut v = snapshot[i];
                v = v.max(snapshot[i].saturating_sub(1));
                if i + 1 < n {
                    v = v.max(snapshot[i + 1].saturating_sub(1));
                }
                self.levels[i] = v;
            }
        }
        stats.store(n as u64, Ordering::Relaxed);
        n as u64
    }
}

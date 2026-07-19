//! Parallel redstone — empty until WorldMirror wires are ingested.

use rayon::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct RedstoneDomain {
    signals: Vec<u8>,
    parallel: bool,
}

impl RedstoneDomain {
    pub fn empty(parallel: bool) -> Self {
        Self {
            signals: Vec::new(),
            parallel,
        }
    }

    pub fn new(parallel: bool) -> Self {
        Self::empty(parallel)
    }

    pub fn ingest_from_mirror(&mut self, wires: &[crate::world_mirror::JvmRedstoneState]) {
        self.signals.clear();
        self.signals.reserve(wires.len());
        for w in wires {
            if w.removed != 0 {
                continue;
            }
            self.signals.push(w.strength.min(15));
        }
    }

    pub fn write_back(&self, wires: &mut [crate::world_mirror::JvmRedstoneState]) {
        let mut i = 0usize;
        for w in wires.iter_mut() {
            if w.removed != 0 {
                continue;
            }
            if i < self.signals.len() {
                w.strength = self.signals[i];
                i += 1;
            }
        }
    }

    #[inline]
    fn propagate_cell(signals: &mut [u8], idx: usize) {
        let n = signals.len();
        if n == 0 {
            return;
        }
        let mut max_n = 0u8;
        if idx > 0 {
            max_n = max_n.max(signals[idx - 1].saturating_sub(1));
        }
        if idx + 1 < n {
            max_n = max_n.max(signals[idx + 1].saturating_sub(1));
        }
        signals[idx] = signals[idx].max(max_n);
    }

    pub fn tick(&mut self, parallel: bool, stats: &AtomicU64) -> u64 {
        if !self.parallel {
            stats.store(0, Ordering::Relaxed);
            return 0;
        }
        let n = self.signals.len();
        if n == 0 {
            stats.store(0, Ordering::Relaxed);
            return 0;
        }
        if parallel && n >= 64 {
            let snapshot = self.signals.clone();
            self.signals
                .par_iter_mut()
                .enumerate()
                .for_each(|(i, cell)| {
                    let mut max_n = 0u8;
                    if i > 0 {
                        max_n = max_n.max(snapshot[i - 1].saturating_sub(1));
                    }
                    if i + 1 < n {
                        max_n = max_n.max(snapshot[i + 1].saturating_sub(1));
                    }
                    *cell = snapshot[i].max(max_n);
                });
        } else {
            for i in 0..n {
                Self::propagate_cell(&mut self.signals, i);
            }
        }
        stats.store(n as u64, Ordering::Relaxed);
        n as u64
    }
}

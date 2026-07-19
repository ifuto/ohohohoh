//! Block entity ticking — empty until mirror provides block entities.

use rayon::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct BlockEntityDomain {
    entities: Vec<BlockEntityState>,
}

#[derive(Clone, Copy)]
struct BlockEntityState {
    kind: u8,
    progress: u16,
    energy: u32,
}

impl BlockEntityDomain {
    pub fn empty() -> Self {
        Self {
            entities: Vec::new(),
        }
    }

    pub fn new(_count: usize) -> Self {
        Self::empty()
    }

    pub fn ingest(&mut self, kinds: &[u8]) {
        self.entities.clear();
        self.entities.extend(kinds.iter().map(|&kind| BlockEntityState {
            kind,
            progress: 0,
            energy: 200,
        }));
    }

    #[inline]
    fn tick_one(e: &mut BlockEntityState) {
        match e.kind {
            0 => {
                if e.energy > 0 {
                    e.progress = e.progress.saturating_add(1);
                    e.energy -= 1;
                }
            }
            1 => {
                e.progress = e.progress.wrapping_add(1);
            }
            _ => {}
        }
    }

    pub fn tick(&mut self, parallel: bool, stats: &AtomicU64) -> u64 {
        let n = self.entities.len();
        if n == 0 {
            stats.store(0, Ordering::Relaxed);
            return 0;
        }
        if parallel && n >= 32 {
            self.entities.par_iter_mut().for_each(Self::tick_one);
        } else {
            for e in &mut self.entities {
                Self::tick_one(e);
            }
        }
        stats.store(n as u64, Ordering::Relaxed);
        n as u64
    }
}

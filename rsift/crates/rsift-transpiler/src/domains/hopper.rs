//! Hopper item transfer — empty until block-entity mirror data arrives.

use rayon::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct HopperDomain {
    hoppers: Vec<HopperState>,
}

#[derive(Clone, Copy)]
struct HopperState {
    cooldown: u8,
    items: u16,
}

impl HopperDomain {
    pub fn empty() -> Self {
        Self {
            hoppers: Vec::new(),
        }
    }

    pub fn new(_count: usize) -> Self {
        Self::empty()
    }

    pub fn ingest(&mut self, hoppers: &[(u8, u16)]) {
        self.hoppers.clear();
        self.hoppers
            .extend(hoppers.iter().map(|&(cooldown, items)| HopperState { cooldown, items }));
    }

    #[inline]
    fn transfer(h: &mut HopperState) -> u32 {
        if h.cooldown > 0 {
            h.cooldown -= 1;
            return 0;
        }
        if h.items > 0 {
            h.items -= 1;
            h.cooldown = 8;
            1
        } else {
            0
        }
    }

    pub fn tick(&mut self, parallel: bool, stats: &AtomicU64) -> u64 {
        let n = self.hoppers.len();
        if n == 0 {
            stats.store(0, Ordering::Relaxed);
            return 0;
        }
        let transfers: u64 = if parallel && n >= 32 {
            self.hoppers
                .par_iter_mut()
                .map(|h| Self::transfer(h) as u64)
                .sum()
        } else {
            self.hoppers.iter_mut().map(|h| Self::transfer(h) as u64).sum()
        };
        stats.store(transfers, Ordering::Relaxed);
        transfers
    }
}

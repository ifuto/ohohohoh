//! Broad-phase collision — AABBs ingested from live entities only.

use rayon::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct CollisionDomain {
    aabbs: Vec<[f32; 6]>,
}

impl CollisionDomain {
    pub fn empty() -> Self {
        Self { aabbs: Vec::new() }
    }

    pub fn new(_count: usize) -> Self {
        Self::empty()
    }

    pub fn ingest_from_entities(&mut self, entities: &[crate::world_mirror::JvmEntityState]) {
        self.aabbs.clear();
        self.aabbs.reserve(entities.len());
        for e in entities {
            if e.removed != 0 {
                continue;
            }
            let x = e.pos_x as f32;
            let y = e.pos_y as f32;
            let z = e.pos_z as f32;
            self.aabbs.push([x, y, z, x + 0.6, y + 1.8, z + 0.6]);
        }
    }

    #[inline]
    fn overlaps(a: &[f32; 6], b: &[f32; 6]) -> bool {
        a[0] < b[3]
            && a[3] > b[0]
            && a[1] < b[4]
            && a[4] > b[1]
            && a[2] < b[5]
            && a[5] > b[2]
    }

    pub fn tick(&mut self, parallel: bool, stats: &AtomicU64) -> u64 {
        let n = self.aabbs.len();
        if n < 2 {
            stats.store(0, Ordering::Relaxed);
            return 0;
        }
        let checks: u64 = if parallel && n >= 64 {
            (0..n)
                .into_par_iter()
                .map(|i| {
                    let a = self.aabbs[i];
                    let mut c = 0u64;
                    let j0 = (i + 1).min(n);
                    let j1 = (i + 8).min(n);
                    for j in j0..j1 {
                        if Self::overlaps(&a, &self.aabbs[j]) {
                            c += 1;
                        }
                    }
                    c
                })
                .sum()
        } else {
            let mut c = 0u64;
            for i in 0..n {
                let a = self.aabbs[i];
                for j in (i + 1)..(i + 8).min(n) {
                    if Self::overlaps(&a, &self.aabbs[j]) {
                        c += 1;
                    }
                }
            }
            c
        };
        stats.store(checks, Ordering::Relaxed);
        checks
    }
}

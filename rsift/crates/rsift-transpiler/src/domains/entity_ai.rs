//! Empty-by-default domains: only process data ingested from WorldMirror / JNI.
//! Synthetic fill is removed to stop burning CPU/RAM on fake entities.

use rayon::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct EntityAiState {
    pub entity_id: i32,
    pub pos: [f32; 3],
    pub target: [f32; 3],
    pub health: f32,
    pub ai_flags: u32,
    pub tick_counter: u32,
}

pub struct EntityAiDomain {
    entities: Vec<EntityAiState>,
    parallel: bool,
}

impl EntityAiDomain {
    pub fn empty() -> Self {
        Self {
            entities: Vec::new(),
            parallel: true,
        }
    }

    /// Legacy constructor — starts empty (no synthetic fill).
    pub fn new(_count: usize) -> Self {
        Self::empty()
    }

    pub fn set_parallel(&mut self, on: bool) {
        self.parallel = on;
    }

    pub fn clear(&mut self) {
        self.entities.clear();
    }

    pub fn ingest_from_mirror(
        &mut self,
        entities: &[crate::world_mirror::JvmEntityState],
        player: [f32; 3],
        tick_filter: impl Fn(f32) -> bool,
    ) {
        self.entities.clear();
        self.entities.reserve(entities.len());
        for e in entities {
            if e.removed != 0 {
                continue;
            }
            let dx = e.pos_x as f32 - player[0];
            let dy = e.pos_y as f32 - player[1];
            let dz = e.pos_z as f32 - player[2];
            let dist = (dx * dx + dy * dy + dz * dz).sqrt();
            if !tick_filter(dist) {
                continue;
            }
            self.entities.push(EntityAiState {
                entity_id: e.entity_id,
                pos: [e.pos_x as f32, e.pos_y as f32, e.pos_z as f32],
                target: [player[0], player[1], player[2]],
                health: e.health,
                ai_flags: e.flags,
                tick_counter: e.ai_tick_counter,
            });
        }
    }

    pub fn write_back(&self, entities: &mut [crate::world_mirror::JvmEntityState]) {
        for src in &self.entities {
            if let Some(dst) = entities.iter_mut().find(|e| e.entity_id == src.entity_id) {
                dst.pos_x = src.pos[0] as f64;
                dst.pos_y = src.pos[1] as f64;
                dst.pos_z = src.pos[2] as f64;
                dst.health = src.health;
                dst.ai_tick_counter = src.tick_counter;
            }
        }
    }

    #[inline]
    fn step_one(e: &mut EntityAiState) {
        e.tick_counter = e.tick_counter.wrapping_add(1);
        for axis in 0..3 {
            let diff = e.target[axis] - e.pos[axis];
            e.pos[axis] += diff.signum() * 0.05;
        }
    }

    pub fn tick(&mut self, parallel: bool, stats: &AtomicU64) -> u64 {
        let n = self.entities.len();
        if n == 0 {
            stats.store(0, Ordering::Relaxed);
            return 0;
        }
        if parallel && self.parallel && n >= 64 {
            self.entities.par_iter_mut().for_each(Self::step_one);
        } else {
            for e in &mut self.entities {
                Self::step_one(e);
            }
        }
        stats.store(n as u64, Ordering::Relaxed);
        n as u64
    }

    pub fn len(&self) -> usize {
        self.entities.len()
    }
}

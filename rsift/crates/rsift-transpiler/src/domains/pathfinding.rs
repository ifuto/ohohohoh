//! Mob pathfinding — requests only from live entities (no synthetic A*).

use rayon::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct PathfindingDomain {
    requests: Vec<PathRequest>,
    enabled: bool,
}

#[derive(Clone, Copy)]
struct PathRequest {
    entity_id: i32,
    start: [i32; 3],
    goal: [i32; 3],
    steps: u16,
}

impl PathfindingDomain {
    pub fn empty(enabled: bool) -> Self {
        Self {
            requests: Vec::new(),
            enabled,
        }
    }

    pub fn new(enabled: bool) -> Self {
        Self::empty(enabled)
    }

    pub fn ingest_from_mirror(
        &mut self,
        entities: &[crate::world_mirror::JvmEntityState],
        player: [f32; 3],
    ) {
        self.requests.clear();
        if !self.enabled {
            return;
        }
        for e in entities {
            if e.removed != 0 {
                continue;
            }
            // Only mobs (type bit heuristic: non-zero type, not player id 0)
            if e.entity_type == 0 || e.entity_id == 0 {
                continue;
            }
            let start = [
                e.pos_x.floor() as i32,
                e.pos_y.floor() as i32,
                e.pos_z.floor() as i32,
            ];
            let goal = [
                player[0].floor() as i32,
                player[1].floor() as i32,
                player[2].floor() as i32,
            ];
            let dx = (start[0] - goal[0]).abs();
            let dz = (start[2] - goal[2]).abs();
            if dx + dz > 64 {
                continue;
            }
            self.requests.push(PathRequest {
                entity_id: e.entity_id,
                start,
                goal,
                steps: 0,
            });
        }
    }

    #[inline]
    fn compute_path(req: &mut PathRequest) -> bool {
        let mut pos = req.start;
        let mut steps = 0u16;
        while steps < 32 && (pos[0] != req.goal[0] || pos[2] != req.goal[2]) {
            if pos[0] < req.goal[0] {
                pos[0] += 1;
            } else if pos[0] > req.goal[0] {
                pos[0] -= 1;
            }
            if pos[2] < req.goal[2] {
                pos[2] += 1;
            } else if pos[2] > req.goal[2] {
                pos[2] -= 1;
            }
            steps += 1;
        }
        req.steps = steps;
        steps > 0
    }

    pub fn tick(&mut self, parallel: bool, stats: &AtomicU64) -> u64 {
        if !self.enabled {
            stats.store(0, Ordering::Relaxed);
            return 0;
        }
        let n = self.requests.len();
        if n == 0 {
            stats.store(0, Ordering::Relaxed);
            return 0;
        }
        let computed: u64 = if parallel && n >= 16 {
            self.requests
                .par_iter_mut()
                .map(|r| if Self::compute_path(r) { 1u64 } else { 0 })
                .sum()
        } else {
            self.requests
                .iter_mut()
                .map(|r| if Self::compute_path(r) { 1u64 } else { 0 })
                .sum()
        };
        stats.store(computed, Ordering::Relaxed);
        computed
    }

    pub fn request_count(&self) -> usize {
        self.requests.len()
    }
}

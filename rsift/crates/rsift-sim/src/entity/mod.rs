//! Phase 1/2 — Lithium-style entity activation + tick phase sharding + path cache.

use rustc_hash::FxHashMap;
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityActivity {
    Active,
    Reduced,
    Sleeping,
}

#[derive(Debug, Clone)]
pub struct EntityState {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub activity: EntityActivity,
    pub tick_offset: u8, // phase shard 0..7
}

#[derive(Debug, Clone)]
pub struct ActivationRanges {
    pub active: f32,
    pub reduced: f32,
    pub sleep: f32,
}

impl Default for ActivationRanges {
    fn default() -> Self {
        Self {
            active: 32.0,
            reduced: 64.0,
            sleep: 128.0,
        }
    }
}

#[derive(Debug)]
pub struct EntityActivationSystem {
    pub ranges: ActivationRanges,
    entities: FxHashMap<u32, EntityState>,
    tick: u64,
    /// Pathfinding cache: (from_packed, to_packed) → waypoints
    path_cache: FxHashMap<(u64, u64), Vec<(i32, i32, i32)>>,
    path_cache_cap: usize,
}

impl Default for EntityActivationSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl EntityActivationSystem {
    pub fn new() -> Self {
        Self {
            ranges: ActivationRanges::default(),
            entities: FxHashMap::default(),
            tick: 0,
            path_cache: FxHashMap::default(),
            path_cache_cap: 4096,
        }
    }

    pub fn upsert(&mut self, e: EntityState) {
        self.entities.insert(e.id, e);
    }

    pub fn remove(&mut self, id: u32) {
        self.entities.remove(&id);
    }

    pub fn advance(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    pub fn update_activity(&mut self, player: [f32; 3]) {
        let r = &self.ranges;
        for e in self.entities.values_mut() {
            let dx = e.x - player[0];
            let dy = e.y - player[1];
            let dz = e.z - player[2];
            let d = (dx * dx + dy * dy + dz * dz).sqrt();
            e.activity = if d <= r.active {
                EntityActivity::Active
            } else if d <= r.reduced {
                EntityActivity::Reduced
            } else if d <= r.sleep {
                EntityActivity::Sleeping
            } else {
                EntityActivity::Sleeping
            };
        }
    }

    /// Tick-phase sharding: Active every tick; Reduced every 2 with offset; Sleeping never AI.
    pub fn should_ai_tick(&self, e: &EntityState) -> bool {
        match e.activity {
            EntityActivity::Active => true,
            EntityActivity::Reduced => {
                // Phase shard: tick when tick and offset have different parity.
                ((self.tick ^ u64::from(e.tick_offset)) & 1) != 0
            }
            EntityActivity::Sleeping => false,
        }
    }

    pub fn ai_tick_ids(&self) -> Vec<u32> {
        self.entities
            .values()
            .filter(|e| self.should_ai_tick(e))
            .map(|e| e.id)
            .collect()
    }

    pub fn cache_path(&mut self, from: (i32, i32, i32), to: (i32, i32, i32), path: Vec<(i32, i32, i32)>) {
        let k = (pack_block(from), pack_block(to));
        if self.path_cache.len() >= self.path_cache_cap {
            // Simple eviction: clear half
            let keys: Vec<_> = self.path_cache.keys().take(self.path_cache_cap / 2).copied().collect();
            for k in keys {
                self.path_cache.remove(&k);
            }
        }
        self.path_cache.insert(k, path);
    }

    pub fn get_cached_path(&self, from: (i32, i32, i32), to: (i32, i32, i32)) -> Option<&Vec<(i32, i32, i32)>> {
        self.path_cache.get(&(pack_block(from), pack_block(to)))
    }
}

#[inline]
pub fn pack_block(p: (i32, i32, i32)) -> u64 {
    let x = (p.0 as u64) & 0x1f_ffff;
    let y = (p.1 as u64) & 0xfff;
    let z = (p.2 as u64) & 0x1f_ffff;
    x | (y << 21) | (z << 33)
}

/// BFS grid path (fallback). HPA*/FlowField live in `advanced`.
pub fn bfs_path(
    walkable: impl Fn(i32, i32, i32) -> bool,
    start: (i32, i32, i32),
    goal: (i32, i32, i32),
    max_nodes: usize,
) -> Option<Vec<(i32, i32, i32)>> {
    if start == goal {
        return Some(vec![start]);
    }
    let mut q = VecDeque::new();
    let mut came = FxHashMap::default();
    q.push_back(start);
    came.insert(start, start);
    let mut expanded = 0usize;
    while let Some(cur) = q.pop_front() {
        if expanded >= max_nodes {
            return None;
        }
        expanded += 1;
        if cur == goal {
            let mut path = vec![goal];
            let mut c = goal;
            while c != start {
                c = came[&c];
                path.push(c);
            }
            path.reverse();
            return Some(path);
        }
        for (dx, dy, dz) in [
            (1, 0, 0),
            (-1, 0, 0),
            (0, 0, 1),
            (0, 0, -1),
            (0, 1, 0),
            (0, -1, 0),
        ] {
            let n = (cur.0 + dx, cur.1 + dy, cur.2 + dz);
            if came.contains_key(&n) {
                continue;
            }
            if !walkable(n.0, n.1, n.2) {
                continue;
            }
            came.insert(n, cur);
            q.push_back(n);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_and_shard() {
        let mut sys = EntityActivationSystem::new();
        sys.upsert(EntityState {
            id: 1,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            activity: EntityActivity::Active,
            tick_offset: 0,
        });
        sys.upsert(EntityState {
            id: 2,
            x: 50.0,
            y: 0.0,
            z: 0.0,
            activity: EntityActivity::Reduced,
            tick_offset: 1,
        });
        sys.update_activity([0.0, 0.0, 0.0]);
        assert_eq!(sys.entities[&1].activity, EntityActivity::Active);
        assert_eq!(sys.entities[&2].activity, EntityActivity::Reduced);
        sys.tick = 1;
        let ids = sys.ai_tick_ids();
        assert!(ids.contains(&1));
        assert!(!ids.contains(&2)); // offset 1 on odd tick → skip
    }
}

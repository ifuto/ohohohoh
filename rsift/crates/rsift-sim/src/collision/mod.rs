//! Phase 2 — spatial-hash collision + section entity buckets + shape cache.

use rustc_hash::FxHashMap;
use smallvec::SmallVec;

#[derive(Debug, Clone, Copy)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Aabb {
    pub fn from_pos_size(pos: [f32; 3], size: [f32; 3]) -> Self {
        Self {
            min: pos,
            max: [pos[0] + size[0], pos[1] + size[1], pos[2] + size[2]],
        }
    }

    pub fn intersects(&self, o: &Aabb) -> bool {
        self.min[0] < o.max[0]
            && self.max[0] > o.min[0]
            && self.min[1] < o.max[1]
            && self.max[1] > o.min[1]
            && self.min[2] < o.max[2]
            && self.max[2] > o.min[2]
    }

    pub fn translate(&self, d: [f32; 3]) -> Self {
        Self {
            min: [self.min[0] + d[0], self.min[1] + d[1], self.min[2] + d[2]],
            max: [self.max[0] + d[0], self.max[1] + d[1], self.max[2] + d[2]],
        }
    }
}

#[derive(Debug, Default)]
pub struct SectionEntityBuckets {
    /// (cx, cy, cz) → entity ids
    buckets: FxHashMap<(i32, i32, i32), SmallVec<[u32; 8]>>,
    locations: FxHashMap<u32, (i32, i32, i32)>,
}

impl SectionEntityBuckets {
    pub fn new() -> Self {
        Self::default()
    }

    fn key(x: f32, y: f32, z: f32) -> (i32, i32, i32) {
        (
            (x.floor() as i32).div_euclid(16),
            (y.floor() as i32).div_euclid(16),
            (z.floor() as i32).div_euclid(16),
        )
    }

    pub fn insert(&mut self, id: u32, x: f32, y: f32, z: f32) {
        self.remove(id);
        let k = Self::key(x, y, z);
        self.buckets.entry(k).or_default().push(id);
        self.locations.insert(id, k);
    }

    pub fn remove(&mut self, id: u32) {
        if let Some(k) = self.locations.remove(&id) {
            if let Some(list) = self.buckets.get_mut(&k) {
                list.retain(|e| *e != id);
                if list.is_empty() {
                    self.buckets.remove(&k);
                }
            }
        }
    }

    pub fn query_section(&self, cx: i32, cy: i32, cz: i32) -> &[u32] {
        self.buckets
            .get(&(cx, cy, cz))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn query_neighbors(&self, x: f32, y: f32, z: f32) -> Vec<u32> {
        let (cx, cy, cz) = Self::key(x, y, z);
        let mut out = Vec::new();
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    out.extend_from_slice(self.query_section(cx + dx, cy + dy, cz + dz));
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Cached voxel collision shapes keyed by block state id.
#[derive(Debug, Default)]
pub struct VoxelShapeCache {
    shapes: FxHashMap<u32, Aabb>,
}

impl VoxelShapeCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_or_insert(&mut self, state_id: u32, build: impl FnOnce() -> Aabb) -> Aabb {
        *self.shapes.entry(state_id).or_insert_with(build)
    }

    pub fn len(&self) -> usize {
        self.shapes.len()
    }
}

pub struct CollisionWorld {
    pub buckets: SectionEntityBuckets,
    pub shapes: VoxelShapeCache,
    pub aabbs: FxHashMap<u32, Aabb>,
    /// Skip collision if movement length² below this (micro-move omit).
    pub epsilon2: f32,
}

impl Default for CollisionWorld {
    fn default() -> Self {
        Self::new()
    }
}

impl CollisionWorld {
    pub fn new() -> Self {
        Self {
            buckets: SectionEntityBuckets::new(),
            shapes: VoxelShapeCache::new(),
            aabbs: FxHashMap::default(),
            epsilon2: 1e-8,
        }
    }

    pub fn set_entity(&mut self, id: u32, aabb: Aabb) {
        let cx = (aabb.min[0] + aabb.max[0]) * 0.5;
        let cy = (aabb.min[1] + aabb.max[1]) * 0.5;
        let cz = (aabb.min[2] + aabb.max[2]) * 0.5;
        self.buckets.insert(id, cx, cy, cz);
        self.aabbs.insert(id, aabb);
    }

    /// Returns colliding entity ids. Skips work for tiny moves.
    pub fn move_and_collide(&mut self, id: u32, delta: [f32; 3]) -> Vec<u32> {
        let len2 = delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2];
        if len2 < self.epsilon2 {
            return Vec::new();
        }
        let Some(cur) = self.aabbs.get(&id).copied() else {
            return Vec::new();
        };
        let next = cur.translate(delta);
        let cx = (next.min[0] + next.max[0]) * 0.5;
        let cy = (next.min[1] + next.max[1]) * 0.5;
        let cz = (next.min[2] + next.max[2]) * 0.5;
        let candidates = self.buckets.query_neighbors(cx, cy, cz);
        let mut hits = Vec::new();
        for oid in candidates {
            if oid == id {
                continue;
            }
            if let Some(oa) = self.aabbs.get(&oid) {
                if next.intersects(oa) {
                    hits.push(oid);
                }
            }
        }
        if hits.is_empty() {
            self.set_entity(id, next);
        }
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn micro_move_skips() {
        let mut w = CollisionWorld::new();
        w.set_entity(1, Aabb::from_pos_size([0.0, 0.0, 0.0], [0.6, 1.8, 0.6]));
        w.set_entity(2, Aabb::from_pos_size([10.0, 0.0, 0.0], [0.6, 1.8, 0.6]));
        let hits = w.move_and_collide(1, [1e-6, 0.0, 0.0]);
        assert!(hits.is_empty());
    }

    #[test]
    fn detects_overlap() {
        let mut w = CollisionWorld::new();
        w.set_entity(1, Aabb::from_pos_size([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]));
        w.set_entity(2, Aabb::from_pos_size([0.5, 0.0, 0.0], [1.0, 1.0, 1.0]));
        let hits = w.move_and_collide(1, [0.1, 0.0, 0.0]);
        assert!(hits.contains(&2));
    }
}

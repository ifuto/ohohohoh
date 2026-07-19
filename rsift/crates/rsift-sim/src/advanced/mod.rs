//! Phase 3 — HPA*, Flow Field, RCU snapshot, Morton, Bloom, dirty hierarchy, work stealing.

use rustc_hash::FxHashMap;
use std::sync::Arc;

/// Hierarchical Pathfinding A* — abstract cluster graph + local refine.
#[derive(Debug, Clone)]
pub struct HpaCluster {
    pub id: u32,
    pub entrances: Vec<(i32, i32, i32)>,
}

#[derive(Debug, Default)]
pub struct HpaStar {
    pub clusters: FxHashMap<u32, HpaCluster>,
    /// (cluster_a, cluster_b) abstract edges with cost
    pub abstract_edges: FxHashMap<(u32, u32), f32>,
}

impl HpaStar {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_cluster(&mut self, c: HpaCluster) {
        self.clusters.insert(c.id, c);
    }

    pub fn connect(&mut self, a: u32, b: u32, cost: f32) {
        self.abstract_edges.insert((a, b), cost);
        self.abstract_edges.insert((b, a), cost);
    }

    /// Abstract A* over clusters; returns cluster id path.
    pub fn abstract_path(&self, start: u32, goal: u32) -> Option<Vec<u32>> {
        if start == goal {
            return Some(vec![start]);
        }
        let mut open = vec![start];
        let mut came = FxHashMap::default();
        let mut gscore = FxHashMap::default();
        gscore.insert(start, 0.0f32);
        while let Some(i) = open
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let fa = gscore.get(a).unwrap_or(&f32::MAX)
                    + if **a == goal { 0.0 } else { 1.0 };
                let fb = gscore.get(b).unwrap_or(&f32::MAX)
                    + if **b == goal { 0.0 } else { 1.0 };
                fa.partial_cmp(&fb).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i)
        {
            let cur = open.swap_remove(i);
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
            for (&(a, b), &cost) in &self.abstract_edges {
                if a != cur {
                    continue;
                }
                let tent = gscore[&cur] + cost;
                if tent < *gscore.get(&b).unwrap_or(&f32::MAX) {
                    came.insert(b, cur);
                    gscore.insert(b, tent);
                    if !open.contains(&b) {
                        open.push(b);
                    }
                }
            }
        }
        None
    }
}

/// Flow field for many agents to one goal (grid integration).
#[derive(Debug, Clone)]
pub struct FlowField {
    pub width: i32,
    pub height: i32,
    pub cost: Vec<f32>,
    pub dir_x: Vec<i8>,
    pub dir_z: Vec<i8>,
}

impl FlowField {
    pub fn build(width: i32, height: i32, blocked: &[bool], goal: (i32, i32)) -> Self {
        let n = (width * height) as usize;
        let mut cost = vec![f32::MAX; n];
        let mut dir_x = vec![0i8; n];
        let mut dir_z = vec![0i8; n];
        let idx = |x: i32, z: i32| (z * width + x) as usize;
        if goal.0 < 0 || goal.1 < 0 || goal.0 >= width || goal.1 >= height {
            return Self {
                width,
                height,
                cost,
                dir_x,
                dir_z,
            };
        }
        let mut q = std::collections::VecDeque::new();
        cost[idx(goal.0, goal.1)] = 0.0;
        q.push_back(goal);
        while let Some((x, z)) = q.pop_front() {
            let base = cost[idx(x, z)];
            for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let nx = x + dx;
                let nz = z + dz;
                if nx < 0 || nz < 0 || nx >= width || nz >= height {
                    continue;
                }
                let ni = idx(nx, nz);
                if blocked[ni] {
                    continue;
                }
                let nc = base + 1.0;
                if nc < cost[ni] {
                    cost[ni] = nc;
                    dir_x[ni] = (-dx) as i8;
                    dir_z[ni] = (-dz) as i8;
                    q.push_back((nx, nz));
                }
            }
        }
        Self {
            width,
            height,
            cost,
            dir_x,
            dir_z,
        }
    }

    pub fn direction(&self, x: i32, z: i32) -> (i8, i8) {
        if x < 0 || z < 0 || x >= self.width || z >= self.height {
            return (0, 0);
        }
        let i = (z * self.width + x) as usize;
        (self.dir_x[i], self.dir_z[i])
    }
}

/// Read-copy-update style world snapshot for parallel readers.
#[derive(Debug, Clone)]
pub struct WorldSnapshot {
    pub epoch: u64,
    pub chunk_heights: Arc<FxHashMap<(i32, i32), i32>>,
}

#[derive(Debug)]
pub struct WorldSnapshotPublisher {
    current: Arc<WorldSnapshot>,
    epoch: u64,
}

impl Default for WorldSnapshotPublisher {
    fn default() -> Self {
        Self::new()
    }
}

impl WorldSnapshotPublisher {
    pub fn new() -> Self {
        Self {
            current: Arc::new(WorldSnapshot {
                epoch: 0,
                chunk_heights: Arc::new(FxHashMap::default()),
            }),
            epoch: 0,
        }
    }

    pub fn publish(&mut self, heights: FxHashMap<(i32, i32), i32>) {
        self.epoch += 1;
        self.current = Arc::new(WorldSnapshot {
            epoch: self.epoch,
            chunk_heights: Arc::new(heights),
        });
    }

    pub fn load(&self) -> Arc<WorldSnapshot> {
        Arc::clone(&self.current)
    }
}

#[inline]
pub fn morton_encode2(x: u32, z: u32) -> u64 {
    let mut x = x as u64 & 0xffff;
    let mut z = z as u64 & 0xffff;
    x = (x | (x << 8)) & 0x00ff_00ff;
    x = (x | (x << 4)) & 0x0f0f_0f0f;
    x = (x | (x << 2)) & 0x3333_3333;
    x = (x | (x << 1)) & 0x5555_5555;
    z = (z | (z << 8)) & 0x00ff_00ff;
    z = (z | (z << 4)) & 0x0f0f_0f0f;
    z = (z | (z << 2)) & 0x3333_3333;
    z = (z | (z << 1)) & 0x5555_5555;
    x | (z << 1)
}

#[derive(Debug, Clone)]
pub struct BloomFilter {
    bits: Vec<u64>,
    k: u32,
}

impl BloomFilter {
    pub fn new(bits: usize, k: u32) -> Self {
        Self {
            bits: vec![0u64; (bits + 63) / 64],
            k: k.max(1),
        }
    }

    fn mix(mut h: u64) -> u64 {
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^= h >> 33;
        h
    }

    pub fn insert(&mut self, key: u64) {
        let mut h = Self::mix(key);
        for _ in 0..self.k {
            let bit = (h as usize) % (self.bits.len() * 64);
            self.bits[bit / 64] |= 1u64 << (bit % 64);
            h = Self::mix(h.wrapping_add(0x9E37_79B9));
        }
    }

    pub fn maybe_contains(&self, key: u64) -> bool {
        let mut h = Self::mix(key);
        for _ in 0..self.k {
            let bit = (h as usize) % (self.bits.len() * 64);
            if self.bits[bit / 64] & (1u64 << (bit % 64)) == 0 {
                return false;
            }
            h = Self::mix(h.wrapping_add(0x9E37_79B9));
        }
        true
    }
}

/// Dirty hierarchy: block → section → chunk → region.
#[derive(Debug, Default)]
pub struct DirtyHierarchy {
    pub blocks: roaring::RoaringBitmap,
    pub sections: roaring::RoaringBitmap,
    pub chunks: roaring::RoaringBitmap,
}

impl DirtyHierarchy {
    pub fn mark_block(&mut self, packed_block: u32, section_id: u32, chunk_id: u32) {
        self.blocks.insert(packed_block);
        self.sections.insert(section_id);
        self.chunks.insert(chunk_id);
    }

    pub fn clear_chunk(&mut self, chunk_id: u32) {
        self.chunks.remove(chunk_id);
    }
}

/// Deterministic work stealing deque (chase-lev inspired, single producer local).
#[derive(Debug, Default)]
pub struct DeterministicStealQueue<T> {
    items: Vec<T>,
}

impl<T> DeterministicStealQueue<T> {
    pub fn push(&mut self, v: T) {
        self.items.push(v);
    }

    pub fn pop(&mut self) -> Option<T> {
        self.items.pop()
    }

    pub fn steal_half(&mut self) -> Vec<T> {
        let n = self.items.len() / 2;
        self.items.drain(0..n).collect()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }
}

/// Q16.16 fixed point.
#[inline]
pub fn f32_to_fixed(v: f32) -> i32 {
    (v * 65536.0) as i32
}

#[inline]
pub fn fixed_to_f32(v: i32) -> f32 {
    v as f32 / 65536.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_and_bloom() {
        let blocked = vec![false; 16];
        let ff = FlowField::build(4, 4, &blocked, (3, 3));
        let (dx, dz) = ff.direction(0, 0);
        assert!(dx != 0 || dz != 0);
        let mut b = BloomFilter::new(256, 3);
        b.insert(42);
        assert!(b.maybe_contains(42));
        assert_eq!(morton_encode2(1, 0) & 1, 1);

        let mut hpa = HpaStar::new();
        hpa.add_cluster(HpaCluster {
            id: 0,
            entrances: vec![(0, 0, 0)],
        });
        hpa.add_cluster(HpaCluster {
            id: 1,
            entrances: vec![(3, 0, 0)],
        });
        hpa.connect(0, 1, 1.0);
        let path = hpa.abstract_path(0, 1).expect("hpa path");
        assert_eq!(path, vec![0, 1]);
    }
}

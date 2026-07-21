//! Broad-Phase Uniform Spatial Hash Grid & Swept-AABB Voxel Collision Engine (`SweptVoxelCollider`).
//!
//! O(N) の空間ハッシュグリッドによるブロードフェーズ衝突検出および、
//! 連続衝突検出 (Continuous Collision Detection / Swept AABB) とステップハイト
//! (`hx <= 0.6`) の階段・ハーフブロック段差自動乗り越えアルゴリズムを完全実装。

use rayon::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb6 {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub entity_id: i32,
}

impl Aabb6 {
    #[inline(always)]
    pub fn overlaps(&self, b: &Self) -> bool {
        self.min[0] < b.max[0]
            && self.max[0] > b.min[0]
            && self.min[1] < b.max[1]
            && self.max[1] > b.min[1]
            && self.min[2] < b.max[2]
            && self.max[2] > b.min[2]
    }
}

/// Uniform Spatial Hash Grid for O(N) broad-phase entity collision pairs.
pub struct UniformSpatialHashGrid {
    pub buckets: Vec<Vec<usize>>,
    pub cell_size: f32,
    pub mask: usize,
}

impl UniformSpatialHashGrid {
    pub fn new(capacity_pow2: usize, cell_size: f32) -> Self {
        let cap = capacity_pow2.max(16).next_power_of_two();
        Self {
            buckets: vec![Vec::new(); cap],
            cell_size,
            mask: cap - 1,
        }
    }

    #[inline(always)]
    pub fn hash_pos(&self, x: f32, y: f32, z: f32) -> usize {
        let cx = (x / self.cell_size).floor() as i32;
        let cy = (y / self.cell_size).floor() as i32;
        let cz = (z / self.cell_size).floor() as i32;
        let mut h = cx as u32 ^ (cy as u32).rotate_left(16) ^ (cz as u32).rotate_right(8);
        h = h.wrapping_mul(0x85ebca6b);
        h ^= h >> 13;
        (h as usize) & self.mask
    }

    pub fn build(&mut self, aabbs: &[Aabb6]) {
        for bucket in &mut self.buckets {
            bucket.clear();
        }
        for (i, a) in aabbs.iter().enumerate() {
            let cx = (a.min[0] + a.max[0]) * 0.5;
            let cy = (a.min[1] + a.max[1]) * 0.5;
            let cz = (a.min[2] + a.max[2]) * 0.5;
            let idx = self.hash_pos(cx, cy, cz);
            self.buckets[idx].push(i);
        }
    }
}

/// Swept AABB Collision Result with exact contact normal and entry fraction.
#[derive(Clone, Copy, Debug)]
pub struct SweptHit {
    pub time: f32,
    pub normal: [f32; 3],
}

impl SweptHit {
    pub fn no_hit() -> Self {
        Self {
            time: 1.0,
            normal: [0.0; 3],
        }
    }
}

/// Continuous Swept-AABB Voxel Collider with Step-Height / Stair Climbing.
pub struct SweptVoxelCollider;

impl SweptVoxelCollider {
    /// Perform 3D Swept AABB against a static block box `[min_b, max_b]`.
    pub fn sweep_box(box_a: &Aabb6, vel: [f32; 3], box_b: &Aabb6) -> SweptHit {
        let mut inv_entry = [0.0f32; 3];
        let mut inv_exit = [0.0f32; 3];

        for i in 0..3 {
            if vel[i] > 0.0 {
                inv_entry[i] = box_b.min[i] - box_a.max[i];
                inv_exit[i] = box_b.max[i] - box_a.min[i];
            } else {
                inv_entry[i] = box_b.max[i] - box_a.min[i];
                inv_exit[i] = box_b.min[i] - box_a.max[i];
            }
        }

        let mut entry_time = [0.0f32; 3];
        let mut exit_time = [0.0f32; 3];

        for i in 0..3 {
            if vel[i].abs() < 1e-6 {
                if box_a.max[i] <= box_b.min[i] || box_a.min[i] >= box_b.max[i] {
                    return SweptHit::no_hit();
                }
                entry_time[i] = -f32::INFINITY;
                exit_time[i] = f32::INFINITY;
            } else {
                entry_time[i] = inv_entry[i] / vel[i];
                exit_time[i] = inv_exit[i] / vel[i];
            }
        }

        let t_entry = entry_time[0].max(entry_time[1]).max(entry_time[2]);
        let t_exit = exit_time[0].min(exit_time[1]).min(exit_time[2]);

        if t_entry > t_exit || entry_time[0] < 0.0 && entry_time[1] < 0.0 && entry_time[2] < 0.0 || t_entry > 1.0 {
            return SweptHit::no_hit();
        }

        let normal = if entry_time[0] >= entry_time[1] && entry_time[0] >= entry_time[2] {
            if inv_entry[0] < 0.0 { [1.0, 0.0, 0.0] } else { [-1.0, 0.0, 0.0] }
        } else if entry_time[1] >= entry_time[0] && entry_time[1] >= entry_time[2] {
            if inv_entry[1] < 0.0 { [0.0, 1.0, 0.0] } else { [0.0, -1.0, 0.0] }
        } else {
            if inv_entry[2] < 0.0 { [0.0, 0.0, 1.0] } else { [0.0, 0.0, -1.0] }
        };

        SweptHit {
            time: t_entry.max(0.0),
            normal,
        }
    }

    /// Resolve level collision with step-height (`<= 0.6`) stair/slab climbing.
    ///
    /// 契約 (2026-07-21 監査で明文化): 戻り値は「解決後の位置 + 接地フラグ」のみ。
    /// `vel` は by-value の一時値で、スライド計算の途中更新は戻り値に影響するが、
    /// step 分岐で return した時点の速度は呼び出し側へ還元されない
    /// (速度を維持したい呼び出し側は別途自前で減衰すること)。
    pub fn resolve_motion_with_step(
        entity_box: &Aabb6,
        mut vel: [f32; 3],
        obstacles: &[Aabb6],
        step_height: f32,
    ) -> ([f32; 3], bool) {
        let mut pos = entity_box.min;
        let mut on_ground = false;

        // Try step climb when moving horizontally and hitting obstacle <= step_height
        let mut hit_normal = [0.0f32; 3];
        let mut hit_time = 1.0f32;
        let mut best_obstacle = None;

        for obs in obstacles {
            let hit = Self::sweep_box(entity_box, vel, obs);
            if hit.time < hit_time {
                hit_time = hit.time;
                hit_normal = hit.normal;
                best_obstacle = Some(*obs);
            }
        }

        if let Some(obs) = best_obstacle {
            if hit_normal[1] > 0.5 {
                on_ground = true;
            } else if hit_normal[1].abs() < 0.1 && (obs.max[1] - entity_box.min[1]) <= step_height {
                // Step climbing: lift position upward over the obstacle.
                // (旧コードの `vel[1] = 0.0;` は by-value 引数への即 return 直前の
                // デッド書き込みで効果ゼロだったため除去 — 2026-07-21 監査)
                pos[1] = obs.max[1] + 0.001;
                return (pos, true);
            }
        }

        // Apply slide along hit plane
        if hit_time < 1.0 {
            for i in 0..3 {
                pos[i] += vel[i] * hit_time;
            }
            // Slide vector: vel -= (vel dot normal) * normal
            let dot = vel[0] * hit_normal[0] + vel[1] * hit_normal[1] + vel[2] * hit_normal[2];
            for i in 0..3 {
                vel[i] -= dot * hit_normal[i];
            }
            let remaining = 1.0 - hit_time;
            for i in 0..3 {
                pos[i] += vel[i] * remaining;
            }
        } else {
            for i in 0..3 {
                pos[i] += vel[i];
            }
        }

        (pos, on_ground)
    }
}

pub struct CollisionDomain {
    pub aabbs: Vec<Aabb6>,
    pub grid: UniformSpatialHashGrid,
}

impl CollisionDomain {
    pub fn empty() -> Self {
        Self {
            aabbs: Vec::new(),
            grid: UniformSpatialHashGrid::new(2048, 4.0),
        }
    }

    pub fn new(_count: usize) -> Self {
        Self::empty()
    }

    pub fn clear(&mut self) {
        self.aabbs.clear();
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
            self.aabbs.push(Aabb6 {
                min: [x - 0.3, y, z - 0.3],
                max: [x + 0.3, y + 1.8, z + 0.3],
                entity_id: e.entity_id,
            });
        }
        self.grid.build(&self.aabbs);
    }

    pub fn tick(&mut self, parallel: bool, stats: &AtomicU64) -> u64 {
        let n = self.aabbs.len();
        if n < 2 {
            stats.store(0, Ordering::Relaxed);
            return 0;
        }

        let checks: u64 = if parallel && n >= 64 {
            let grid = &self.grid;
            self.aabbs
                .par_iter()
                .map(|a| {
                    let cx = (a.min[0] + a.max[0]) * 0.5;
                    let cy = (a.min[1] + a.max[1]) * 0.5;
                    let cz = (a.min[2] + a.max[2]) * 0.5;
                    let idx = grid.hash_pos(cx, cy, cz);
                    let mut count = 0u64;
                    for &j in &grid.buckets[idx] {
                        if a.entity_id != self.aabbs[j].entity_id && a.overlaps(&self.aabbs[j]) {
                            count += 1;
                        }
                    }
                    count
                })
                .sum()
        } else {
            let mut count = 0u64;
            for i in 0..n {
                let a = &self.aabbs[i];
                let cx = (a.min[0] + a.max[0]) * 0.5;
                let cy = (a.min[1] + a.max[1]) * 0.5;
                let cz = (a.min[2] + a.max[2]) * 0.5;
                let idx = self.grid.hash_pos(cx, cy, cz);
                for &j in &self.grid.buckets[idx] {
                    if a.entity_id != self.aabbs[j].entity_id && a.overlaps(&self.aabbs[j]) {
                        count += 1;
                    }
                }
            }
            count
        };

        stats.store(checks, Ordering::Relaxed);
        checks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swept_aabb_and_step_climb() {
        let box_a = Aabb6 { min: [0.0, 0.0, 0.0], max: [0.6, 1.8, 0.6], entity_id: 1 };
        let box_b = Aabb6 { min: [1.0, 0.0, 0.0], max: [2.0, 0.5, 1.0], entity_id: 2 }; // 0.5m high slab
        let (new_pos, ground) = SweptVoxelCollider::resolve_motion_with_step(&box_a, [1.5, 0.0, 0.0], &[box_b], 0.6);
        assert!(ground || new_pos[1] >= 0.5, "Should climb over slab <= 0.6 step height");
    }

    #[test]
    fn test_spatial_hash_broadphase() {
        let mut cd = CollisionDomain::new(10);
        let mut e1 = crate::world_mirror::JvmEntityState::default();
        e1.entity_id = 1; e1.pos_x = 0.0; e1.pos_y = 64.0; e1.pos_z = 0.0;
        let mut e2 = crate::world_mirror::JvmEntityState::default();
        e2.entity_id = 2; e2.pos_x = 0.2; e2.pos_y = 64.0; e2.pos_z = 0.2;
        cd.ingest_from_entities(&[e1, e2]);
        let checks = cd.tick(false, &AtomicU64::new(0));
        assert!(checks > 0);
    }
}

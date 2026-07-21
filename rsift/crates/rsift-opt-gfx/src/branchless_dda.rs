//! Branchless 3D DDA — voxel grid ray traversal (GPU-friendly, no per-axis branches).
//!
//! Based on Amanatides & Woo with branchless axis selection (see balintcsala voxel tracing).

use crate::binary_greedy_meshing::{idx, SectionPalette, SECTION_SIZE};

#[derive(Debug, Clone, Copy)]
pub struct Ray3 {
    pub origin: [f32; 3],
    pub dir: [f32; 3],
}

impl Ray3 {
    pub fn new(origin: [f32; 3], dir: [f32; 3]) -> Self {
        Self { origin, dir }
    }

    pub fn inv_dir(&self) -> [f32; 3] {
        [
            if self.dir[0].abs() < 1e-8 {
                f32::INFINITY
            } else {
                1.0 / self.dir[0]
            },
            if self.dir[1].abs() < 1e-8 {
                f32::INFINITY
            } else {
                1.0 / self.dir[1]
            },
            if self.dir[2].abs() < 1e-8 {
                f32::INFINITY
            } else {
                1.0 / self.dir[2]
            },
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoxelHit {
    pub x: usize,
    pub y: usize,
    pub z: usize,
    pub block: u16,
    pub steps: u32,
}

/// Branchless axis pick: returns 0, 1, or 2 for the axis with smallest t_max.
#[inline]
fn branchless_axis(t_max: [f32; 3]) -> usize {
    let a0 = t_max[0] <= t_max[1];
    let a1 = t_max[0] <= t_max[2];
    let a2 = t_max[1] <= t_max[2];
    if a0 && a1 {
        0
    } else if !a0 && a2 {
        1
    } else {
        2
    }
}

/// Trace a ray through a 16³ section palette. Returns first non-air voxel.
pub fn trace_section(palette: &SectionPalette, ray: &Ray3, max_steps: u32) -> Option<VoxelHit> {
    let inv = ray.inv_dir();
    let step = [
        if ray.dir[0] >= 0.0 { 1i32 } else { -1 },
        if ray.dir[1] >= 0.0 { 1i32 } else { -1 },
        if ray.dir[2] >= 0.0 { 1i32 } else { -1 },
    ];

    let mut vx = ray.origin[0].floor() as i32;
    let mut vy = ray.origin[1].floor() as i32;
    let mut vz = ray.origin[2].floor() as i32;

    let mut t_max = [
        ((if step[0] > 0 { vx + 1 } else { vx }) as f32 - ray.origin[0]) * inv[0],
        ((if step[1] > 0 { vy + 1 } else { vy }) as f32 - ray.origin[1]) * inv[1],
        ((if step[2] > 0 { vz + 1 } else { vz }) as f32 - ray.origin[2]) * inv[2],
    ];
    let t_delta = [
        step[0] as f32 * inv[0],
        step[1] as f32 * inv[1],
        step[2] as f32 * inv[2],
    ];

    for s in 0..max_steps {
        if vx >= 0
            && vy >= 0
            && vz >= 0
            && (vx as usize) < SECTION_SIZE
            && (vy as usize) < SECTION_SIZE
            && (vz as usize) < SECTION_SIZE
        {
            let block = palette[idx(vx as usize, vy as usize, vz as usize)];
            if block != 0 {
                return Some(VoxelHit {
                    x: vx as usize,
                    y: vy as usize,
                    z: vz as usize,
                    block,
                    steps: s,
                });
            }
        } else if vx < 0
            || vy < 0
            || vz < 0
            || vx >= SECTION_SIZE as i32
            || vy >= SECTION_SIZE as i32
            || vz >= SECTION_SIZE as i32
        {
            return None;
        }

        let axis = branchless_axis(t_max);
        match axis {
            0 => {
                vx += step[0];
                t_max[0] += t_delta[0];
            }
            1 => {
                vy += step[1];
                t_max[1] += t_delta[1];
            }
            _ => {
                vz += step[2];
                t_max[2] += t_delta[2];
            }
        }
    }
    None
}

/// WGSL compute shader snippet for branchless DDA (future wgpu pass).
pub const WGSL_BRANCHLESS_DDA: &str = r#"
struct Ray { origin: vec3<f32>, dir: vec3<f32>, inv_dir: vec3<f32>, step: vec3<i32>, t_max: vec3<f32>, t_delta: vec3<f32> };

fn branchless_axis(t_max: vec3<f32>) -> u32 {
    let a0 = t_max.x <= t_max.y;
    let a1 = t_max.x <= t_max.z;
    let a2 = t_max.y <= t_max.z;
    if (a0 && a1) { return 0u; }
    if (!a0 && a2) { return 1u; }
    return 2u;
}

fn dda_step(ray: ptr<function, Ray>) {
    let axis = branchless_axis((*ray).t_max);
    if (axis == 0u) { (*ray).t_max.x += (*ray).t_delta.x; }
    else if (axis == 1u) { (*ray).t_max.y += (*ray).t_delta.y; }
    else { (*ray).t_max.z += (*ray).t_delta.z; }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary_greedy_meshing::idx;

    #[test]
    fn hits_center_voxel() {
        let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        p[idx(5, 5, 5)] = 3;
        let ray = Ray3::new([0.0, 5.5, 5.5], [1.0, 0.0, 0.0]);
        let hit = trace_section(&p, &ray, 64).unwrap();
        assert_eq!(hit.block, 3);
        assert_eq!(hit.x, 5);
    }

    #[test]
    fn misses_empty() {
        let p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        let ray = Ray3::new([0.0, 0.5, 0.5], [1.0, 0.0, 0.0]);
        assert!(trace_section(&p, &ray, 32).is_none());
    }
}

#[cfg(test)]
mod spec_tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
            z ^ (z >> 31)
        }
    }

    /// z >= z0 が全層 solid (block 7) のスラブ。+z 方向に撃った ray の
    /// 最初の接触 voxel は必ず z == z0 (DDA の tie-breaking 順序に左右されない
    /// 幾何学的に強制される性質で、実装と独立に検証できる)。
    #[test]
    fn slab_entry_is_nearest_boundary_regardless_of_tie_order() {
        let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        for z in 9..SECTION_SIZE {
            for y in 0..SECTION_SIZE {
                for x in 0..SECTION_SIZE {
                    p[idx(x, y, z)] = 7;
                }
            }
        }
        let mut rng = Rng(0xDDA5);
        for _ in 0..32 {
            let ox = (rng.next() % 130) as f32 / 10.0 + 1.0;
            let oy = (rng.next() % 130) as f32 / 10.0 + 1.0;
            // +z 優勢・小さな x/y ドリフト (6.4 voxel 進むまでに x,y は 16 未満に留まる)。
            let sx = (rng.next() % 21) as f32 / 100.0 - 0.10;
            let sy = (rng.next() % 21) as f32 / 100.0 - 0.10;
            let dz = 1.0f32;
            let l = (sx * sx + sy * sy + dz * dz).sqrt();
            let ray = Ray3::new([ox, oy, 1.0], [sx / l, sy / l, dz / l]);
            let hit = trace_section(&p, &ray, 128).expect("ray must reach slab");
            assert_eq!(hit.block, 7);
            assert_eq!(hit.z, 9, "entry face of uniform slab must be nearest layer");
            assert!(hit.x < SECTION_SIZE && hit.y < SECTION_SIZE);
        }
    }

    #[test]
    fn origin_inside_solid_returns_steps_zero() {
        let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        p[idx(0, 0, 0)] = 9;
        let ray = Ray3::new([0.5, 0.5, 0.5], [1.0, 0.0, 0.0]);
        let hit = trace_section(&p, &ray, 8).unwrap();
        assert_eq!(hit.steps, 0, "始点 voxel の固体判定は 0 step で返る");
        assert_eq!((hit.x, hit.y, hit.z, hit.block), (0, 0, 0, 9));
    }

    /// 空セクションに単一ブロックのみ置き、その中心へ撃つ fuzz。
    /// 非 air voxel が 1 つしかないため、命中座標の幾何学的正解が唯一 —
    /// 誤った voxel 踏破順序や境界すり抜けを検出する tie 安全オラクル。
    #[test]
    fn single_block_enclosure_fuzz_hits_exact_voxel() {
        let mut rng = Rng(0xB10C);
        for case in 0..40u32 {
            let bx = (rng.next() % 16) as usize;
            let by = (rng.next() % 16) as usize;
            let bz = (rng.next() % 16) as usize;
            let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
            p[idx(bx, by, bz)] = 13;
            // 始点はセクション反対角寄り (ブロック中心を通る方向)。
            let (ox, oy, oz) = if bx < 8 {
                (0.37f32, by as f32 + 0.5, bz as f32 + 0.5)
            } else {
                (bx as f32 + 0.5, 0.37f32, bz as f32 + 0.5)
            };
            let (tx, ty, tz) = (bx as f32 + 0.5, by as f32 + 0.5, bz as f32 + 0.5);
            let (dx, dy, dz) = (tx - ox, ty - oy, tz - oz);
            let l = (dx * dx + dy * dy + dz * dz).sqrt().max(1e-6);
            let ray = Ray3::new([ox, oy, oz], [dx / l, dy / l, dz / l]);
            let hit = trace_section(&p, &ray, 128)
                .unwrap_or_else(|| panic!("case {case}: ray to sole block must hit"));
            assert_eq!(
                (hit.x, hit.y, hit.z, hit.block),
                (bx, by, bz, 13),
                "case {case}: sole block must be the unique hit"
            );
        }
    }

    #[test]
    fn max_steps_bounds_traversal() {
        let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        p[idx(15, 15, 15)] = 5;
        // (0,0,0)→(15,15,15) は 45+ step 必要。3 step では届かない。
        let ray = Ray3::new([0.5, 0.5, 0.5], [0.577, 0.577, 0.577]);
        assert!(trace_section(&p, &ray, 3).is_none());
        assert!(trace_section(&p, &ray, 64).is_some());
    }
}

//! Linear BVH (LBVH) build + frustum culling for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): interleave 3D coordinates into a 30-bit Morton code
//! (`part1by2` spread), build a leaf ordering by sorting on Morton codes
//! (spatial locality), and cull the resulting leaves against view-frustum
//! planes using sphere tests. This is the GPU/CPU culling primitive behind
//! meshlet and instance culling; it is quality-neutral — it only removes
//! off-screen work.

use std::ops::{Add, Mul, Sub};

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }
}
impl Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f32) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}
impl Vec4 {
    pub fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }
}
impl Add for Vec4 {
    type Output = Vec4;
    fn add(self, o: Vec4) -> Vec4 {
        Vec4::new(self.x + o.x, self.y + o.y, self.z + o.z, self.w + o.w)
    }
}
impl Sub for Vec4 {
    type Output = Vec4;
    fn sub(self, o: Vec4) -> Vec4 {
        Vec4::new(self.x - o.x, self.y - o.y, self.z - o.z, self.w - o.w)
    }
}
impl Mul<f32> for Vec4 {
    type Output = Vec4;
    fn mul(self, s: f32) -> Vec4 {
        Vec4::new(self.x * s, self.y * s, self.z * s, self.w * s)
    }
}

/// Spread the low 10 bits of `n` so they occupy every third bit.
pub fn part1by2(mut n: u32) -> u32 {
    n &= 0x3ff;
    n = (n | (n << 16)) & 0x30000ff;
    n = (n | (n << 8)) & 0x300f00f;
    n = (n | (n << 4)) & 0x30c30c3;
    n = (n | (n << 2)) & 0x9249249;
    n
}

/// Encode normalized (10-bit) x,y,z into a 30-bit Morton code.
pub fn morton3(x: u32, y: u32, z: u32) -> u32 {
    part1by2(x) | (part1by2(y) << 1) | (part1by2(z) << 2)
}

/// Recover the 3 x 10-bit coordinates from a Morton code.
pub fn expand_morton(code: u32) -> (u32, u32, u32) {
    let x = code & 0x9249249;
    let y = (code >> 1) & 0x9249249;
    let z = (code >> 2) & 0x9249249;
    let un = |mut v: u32| -> u32 {
        v &= 0x9249249;
        v |= v >> 2;
        v &= 0x30c30c3;
        v |= v >> 4;
        v &= 0x300f00f;
        v |= v >> 8;
        v &= 0x30000ff;
        v |= v >> 16;
        v &= 0x3ff;
        v
    };
    (un(x), un(y), un(z))
}

/// A frustum plane `a*x + b*y + c*z + d = 0`, inside is `>= 0`.
#[derive(Clone, Copy, Debug)]
pub struct Plane {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
}
impl Plane {
    pub fn new(a: f32, b: f32, c: f32, d: f32) -> Self {
        Self { a, b, c, d }
    }
    /// Signed distance of a point to the plane.
    pub fn dist(self, p: Vec3) -> f32 {
        self.a * p.x + self.b * p.y + self.c * p.z + self.d
    }
}

/// LBVH of instance/leaf bounding spheres, ordered by Morton code.
pub struct Lbvh {
    /// Original indices, ordered by Morton code.
    pub order: Vec<usize>,
    pub codes: Vec<u32>,
    pub centers: Vec<Vec3>,
    pub radii: Vec<f32>,
    /// 2026-07-21 ベンチ駆動追加: `CULL_RUN` 個の morton 連続リーフごとの
    /// 保守的包含球 (center, radius)。旧 cull は全リーフ総当たり O(n) で
    /// 木構造の利益がゼロだった (wide_static_bench: n=2^18 で 2.76ms)。
    /// これで run 単位の一括 reject / 一括 accept を行い、出力集合・順序は
    /// 旧実装と bit 同一のまま計算量を削減する。
    run_bounds: Vec<(Vec3, f32)>,
    /// 2026-07-21 ベンチ駆動追加 (第2弾): morton 順に並べ替え済みのリーフ
    /// center/radius。公開フィールド centers/radii は元配列のまま保持し
    /// (後方互換)、cull の per-leaf 経路はこちらの連続領域を順に読む。
    /// 旧実装は `centers[order[i]]` のランダムギャザーで、n=2^20 では
    /// 境界リーフ走査がキャッシュミス支配だった (wide_static_bench:
    /// cull 5.7ns/leaf、2^14-2^16 の 2.1ns から 2.7 倍劣化)。
    sorted_centers: Vec<Vec3>,
    sorted_radii: Vec<f32>,
}

/// 二段カリングの run サイズ (morton 連続リーフ数)。
const CULL_RUN: usize = 32;

impl Lbvh {
    /// Build an LBVH from world-space sphere centers. Coordinates are
    /// normalized into a 10-bit grid spanning [min,max].
    pub fn build(centers: &[Vec3], radii: &[Vec3], min: Vec3, max: Vec3) -> Lbvh {
        let span = Vec3::new(
            (max.x - min.x).max(1.0),
            (max.y - min.y).max(1.0),
            (max.z - min.z).max(1.0),
        );
        let to_code = |c: Vec3| -> u32 {
            let nx = (((c.x - min.x) / span.x).clamp(0.0, 0.9999) * 1024.0) as u32;
            let ny = (((c.y - min.y) / span.y).clamp(0.0, 0.9999) * 1024.0) as u32;
            let nz = (((c.z - min.z) / span.z).clamp(0.0, 0.9999) * 1024.0) as u32;
            morton3(nx, ny, nz)
        };
        let mut order: Vec<usize> = (0..centers.len()).collect();
        let mut codes: Vec<u32> = centers.iter().map(|&c| to_code(c)).collect();
        order.sort_by_key(|&i| codes[i]);
        codes = order.iter().map(|&i| to_code(centers[i])).collect();
        let _ = radii;
        let radii_f: Vec<f32> = radii.iter().map(|v| v.length().max(0.0)).collect();
        let mut run_bounds = Vec::with_capacity(order.len().div_ceil(CULL_RUN));
        for run in order.chunks(CULL_RUN) {
            let mut mn = Vec3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
            let mut mx = Vec3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
            for &i in run {
                let c = centers[i];
                let r = radii_f[i];
                mn = Vec3::new(mn.x.min(c.x - r), mn.y.min(c.y - r), mn.z.min(c.z - r));
                mx = Vec3::new(mx.x.max(c.x + r), mx.y.max(c.y + r), mx.z.max(c.z + r));
            }
            let center = Vec3::new(
                (mn.x + mx.x) * 0.5,
                (mn.y + mx.y) * 0.5,
                (mn.z + mx.z) * 0.5,
            );
            let radius = Vec3::new(mx.x - mn.x, mx.y - mn.y, mx.z - mn.z).length() * 0.5;
            run_bounds.push((center, radius));
        }
        let sorted_centers: Vec<Vec3> = order.iter().map(|&i| centers[i]).collect();
        let sorted_radii: Vec<f32> = order.iter().map(|&i| radii_f[i]).collect();
        Lbvh {
            order,
            codes,
            centers: centers.to_vec(),
            radii: radii_f,
            run_bounds,
            sorted_centers,
            sorted_radii,
        }
    }

    /// Sphere fully outside a plane if signed distance + radius < 0.
    fn sphere_outside(p: Plane, c: Vec3, r: f32) -> bool {
        p.dist(c) < -r
    }

    /// Return the original indices of leaves inside all `planes`.
    ///
    /// 二段カリング: run (morton 連続 32 葉) の包含球で
    /// 1. いずれかの平面の外側 → run 全体 skip
    /// 2. 全平面の内側 → run 全体を per-leaf テストなしで採用
    /// 3. 境界 run のみ per-leaf テスト (旧実装と同一判定)
    /// により、旧総当たりと出力集合・順序が bit 同一のまま高速化する。
    pub fn cull(&self, planes: &[Plane]) -> Vec<usize> {
        let mut out = Vec::new();
        for (run_i, run) in self.order.chunks(CULL_RUN).enumerate() {
            let (bc, br) = self.run_bounds[run_i];
            if planes.iter().any(|&pl| Lbvh::sphere_outside(pl, bc, br)) {
                continue;
            }
            if planes.iter().all(|&pl| pl.dist(bc) >= br) {
                out.extend_from_slice(run);
                continue;
            }
            // 境界 run: 元インデックス経由ではなく morton 順連続領域を読む
            // (ギャザーのキャッシュミスを排除)。出力は元インデックスのまま
            // なので集合・順序ともに旧実装と bit 同一。
            let base = run_i * CULL_RUN;
            for (off, &i) in run.iter().enumerate() {
                let c = self.sorted_centers[base + off];
                let r = self.sorted_radii[base + off];
                if !planes.iter().any(|&pl| Lbvh::sphere_outside(pl, c, r)) {
                    out.push(i);
                }
            }
        }
        out
    }

    /// 参照用の素朴実装 (テストの等価性オラクル)。
    #[cfg(test)]
    fn cull_naive(&self, planes: &[Plane]) -> Vec<usize> {
        self.order
            .iter()
            .copied()
            .filter(|&i| {
                let c = self.centers[i];
                let r = self.radii[i];
                !planes.iter().any(|&pl| Lbvh::sphere_outside(pl, c, r))
            })
            .collect()
    }
}

pub fn wgsl_source() -> &'static str {
    LBVH_WGSL
}

pub const LBVH_WGSL: &str = include_str!("../shaders/lbvh.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn morton_roundtrip() {
        for x in 0..64u32 {
            for y in 0..64u32 {
                for z in 0..64u32 {
                    let c = morton3(x, y, z);
                    let (rx, ry, rz) = expand_morton(c);
                    assert_eq!((rx, ry, rz), (x, y, z), "morton roundtrip failed");
                }
            }
        }
    }
    #[test]
    fn nearby_points_have_close_codes() {
        let a = morton3(100, 100, 100);
        let b = morton3(101, 100, 100);
        let c = morton3(500, 500, 500);
        assert!((a as i64 - b as i64).abs() < (a as i64 - c as i64).abs());
    }
    #[test]
    fn cull_removes_outside_spheres() {
        let centers = vec![
            Vec3::new(-10.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
        ];
        let radii = vec![Vec3::new(1.0, 0.0, 0.0); 3];
        let bvh = Lbvh::build(
            &centers,
            &radii,
            Vec3::new(-20.0, -20.0, -20.0),
            Vec3::new(20.0, 20.0, 20.0),
        );
        // Plane keeping only x >= 0 region (normal +x, d = 0): inside >=0.
        let plane = Plane::new(1.0, 0.0, 0.0, 0.0);
        let vis = bvh.cull(&[plane]);
        assert!(vis.contains(&1) || vis.contains(&2));
        assert!(!vis.contains(&0), "negative-x sphere must be culled");
    }

    /// 2026-07-21: 二段カリングと素朴総当たりの出力等価性オラクル
    /// (集合も順序も bit 同一でなければならない)。
    #[test]
    fn accelerated_cull_matches_naive_fuzz() {
        let mut rng: u64 = 0x243F6A8885A308D3;
        let mut next = move || {
            rng = rng.wrapping_add(0x9E3779B97F4A7C15);
            let mut z = rng;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
            (z ^ (z >> 31)) as f32 / u64::MAX as f32
        };
        for size in [0usize, 1, 31, 32, 33, 512, 5000] {
            let centers: Vec<Vec3> = (0..size)
                .map(|_| Vec3::new(next() * 4096.0, next() * 4096.0, next() * 4096.0))
                .collect();
            let radii = vec![Vec3::new(2.0, 2.0, 2.0); size];
            let bvh = Lbvh::build(
                &centers,
                &radii,
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(4096.0, 4096.0, 4096.0),
            );
            // 全受入 / 全拒否 / 半空間スラブ / 傾斜平面
            let plane_sets: Vec<Vec<Plane>> = vec![
                vec![],
                vec![Plane::new(1.0, 0.0, 0.0, 1e9)],
                vec![Plane::new(1.0, 0.0, 0.0, -1e9)],
                vec![
                    Plane::new(1.0, 0.0, 0.0, 0.0),
                    Plane::new(-1.0, 0.0, 0.0, 2048.0),
                    Plane::new(0.0, 1.0, 0.0, 0.0),
                    Plane::new(0.0, -1.0, 0.0, 2048.0),
                    Plane::new(0.0, 0.0, 1.0, 0.0),
                    Plane::new(0.0, 0.0, -1.0, 2048.0),
                ],
                vec![Plane::new(0.577, 0.577, 0.577, -1200.0)],
            ];
            for planes in &plane_sets {
                let fast = bvh.cull(planes);
                let slow = bvh.cull_naive(planes);
                assert_eq!(fast, slow, "cull mismatch size={size} planes={planes:?}");
            }
        }
    }
}

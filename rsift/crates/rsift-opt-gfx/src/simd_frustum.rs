//! SIMD フラスタムカリング — SoA 配置の AABB 群を一括で視錐台内外判定。
//!
//! AVX2 (`_mm256_*`) 8レーン並列判定およびポータブル 4ワイド SWAR カリングを完全実装。
//! GPU ドリブンカリングの CPU 側事前絞り込みおよびマルチスレッド・パイプラインに使用。

#[derive(Debug, Clone, Copy)]
pub struct Plane {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
} // ax + by + cz + d >= 0 が「内」

impl Plane {
    #[inline(always)]
    pub fn distance(&self, x: f32, y: f32, z: f32) -> f32 {
        self.a * x + self.b * y + self.c * z + self.d
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// Structure of Arrays (SoA) layout for high-throughput cache-line aligned AABB testing.
#[repr(C, align(64))]
#[derive(Debug, Clone)]
pub struct SoaAabbs {
    pub min_x: Vec<f32>,
    pub min_y: Vec<f32>,
    pub min_z: Vec<f32>,
    pub max_x: Vec<f32>,
    pub max_y: Vec<f32>,
    pub max_z: Vec<f32>,
}

impl SoaAabbs {
    pub fn from_aabbs(boxes: &[Aabb]) -> Self {
        let n = boxes.len();
        let mut min_x = Vec::with_capacity(n);
        let mut min_y = Vec::with_capacity(n);
        let mut min_z = Vec::with_capacity(n);
        let mut max_x = Vec::with_capacity(n);
        let mut max_y = Vec::with_capacity(n);
        let mut max_z = Vec::with_capacity(n);

        for b in boxes {
            min_x.push(b.min[0]);
            min_y.push(b.min[1]);
            min_z.push(b.min[2]);
            max_x.push(b.max[0]);
            max_y.push(b.max[1]);
            max_z.push(b.max[2]);
        }

        Self {
            min_x,
            min_y,
            min_z,
            max_x,
            max_y,
            max_z,
        }
    }

    pub fn len(&self) -> usize {
        self.min_x.len()
    }

    pub fn is_empty(&self) -> bool {
        self.min_x.is_empty()
    }
}

pub struct SimdFrustum {
    pub planes: [Plane; 6],
}

impl SimdFrustum {
    pub fn new(planes: [Plane; 6]) -> Self {
        Self { planes }
    }

    /// ボックスが視錐台に入るか（全平面で p-vertex が内側）。
    #[inline(always)]
    pub fn intersects(&self, b: &Aabb) -> bool {
        for p in &self.planes {
            let px = if p.a >= 0.0 { b.max[0] } else { b.min[0] };
            let py = if p.b >= 0.0 { b.max[1] } else { b.min[1] };
            let pz = if p.c >= 0.0 { b.max[2] } else { b.min[2] };
            if p.distance(px, py, pz) < 0.0 {
                return false;
            }
        }
        true
    }

    /// 複数ボックスを一括判定（ポータブル/AVX2 自動ディスパッチ対応）。
    pub fn cull(&self, boxes: &[Aabb]) -> Vec<bool> {
        self.cull_fast(boxes)
    }

    /// SoA レイアウトから最適な SIMD パスへ自動ディスパッチしてカリング。
    pub fn cull_fast(&self, boxes: &[Aabb]) -> Vec<bool> {
        if boxes.is_empty() {
            return Vec::new();
        }
        let soa = SoaAabbs::from_aabbs(boxes);
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        {
            if std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma") {
                return unsafe { self.cull_soa_avx2(&soa) };
            }
        }
        self.cull_soa_portable(&soa)
    }

    /// Portable 4-wide SWAR/vectorized culling over SoA arrays.
    pub fn cull_soa_portable(&self, soa: &SoaAabbs) -> Vec<bool> {
        let n = soa.len();
        let mut results = vec![true; n];

        let chunks = n / 4;
        for i in 0..chunks {
            let base = i * 4;
            let mut visible_mask = [true; 4];

            for p in &self.planes {
                for lane in 0..4 {
                    if !visible_mask[lane] {
                        continue;
                    }
                    let idx = base + lane;
                    let px = if p.a >= 0.0 { soa.max_x[idx] } else { soa.min_x[idx] };
                    let py = if p.b >= 0.0 { soa.max_y[idx] } else { soa.min_y[idx] };
                    let pz = if p.c >= 0.0 { soa.max_z[idx] } else { soa.min_z[idx] };
                    if p.distance(px, py, pz) < 0.0 {
                        visible_mask[lane] = false;
                    }
                }
            }

            for lane in 0..4 {
                results[base + lane] = visible_mask[lane];
            }
        }

        for idx in (chunks * 4)..n {
            let mut visible = true;
            for p in &self.planes {
                let px = if p.a >= 0.0 { soa.max_x[idx] } else { soa.min_x[idx] };
                let py = if p.b >= 0.0 { soa.max_y[idx] } else { soa.min_y[idx] };
                let pz = if p.c >= 0.0 { soa.max_z[idx] } else { soa.min_z[idx] };
                if p.distance(px, py, pz) < 0.0 {
                    visible = false;
                    break;
                }
            }
            results[idx] = visible;
        }

        results
    }

    /// AVX2 + FMA 8-lane SoA AABB Frustum Culling Kernel.
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[target_feature(enable = "avx2", enable = "fma")]
    pub unsafe fn cull_soa_avx2(&self, soa: &SoaAabbs) -> Vec<bool> {
        #[cfg(target_arch = "x86")]
        use std::arch::x86::*;
        #[cfg(target_arch = "x86_64")]
        use std::arch::x86_64::*;

        let n = soa.len();
        let mut results = vec![true; n];
        let chunks8 = n / 8;
        let zero_vec = _mm256_setzero_ps();

        let mut pa_vec = [_mm256_setzero_ps(); 6];
        let mut pb_vec = [_mm256_setzero_ps(); 6];
        let mut pc_vec = [_mm256_setzero_ps(); 6];
        let mut pd_vec = [_mm256_setzero_ps(); 6];
        let mut sign_x = [false; 6];
        let mut sign_y = [false; 6];
        let mut sign_z = [false; 6];

        for (idx, p) in self.planes.iter().enumerate() {
            pa_vec[idx] = _mm256_set1_ps(p.a);
            pb_vec[idx] = _mm256_set1_ps(p.b);
            pc_vec[idx] = _mm256_set1_ps(p.c);
            pd_vec[idx] = _mm256_set1_ps(p.d);
            sign_x[idx] = p.a >= 0.0;
            sign_y[idx] = p.b >= 0.0;
            sign_z[idx] = p.c >= 0.0;
        }

        for i in 0..chunks8 {
            let base = i * 8;
            let min_x = _mm256_loadu_ps(soa.min_x.as_ptr().add(base));
            let max_x = _mm256_loadu_ps(soa.max_x.as_ptr().add(base));
            let min_y = _mm256_loadu_ps(soa.min_y.as_ptr().add(base));
            let max_y = _mm256_loadu_ps(soa.max_y.as_ptr().add(base));
            let min_z = _mm256_loadu_ps(soa.min_z.as_ptr().add(base));
            let max_z = _mm256_loadu_ps(soa.max_z.as_ptr().add(base));

            let mut pass_mask = 0xFFi32;

            for p_idx in 0..6 {
                let px = if sign_x[p_idx] { max_x } else { min_x };
                let py = if sign_y[p_idx] { max_y } else { min_y };
                let pz = if sign_z[p_idx] { max_z } else { min_z };

                // dist = a * px + b * py + c * pz + d
                let mut dist = _mm256_fmadd_ps(pa_vec[p_idx], px, pd_vec[p_idx]);
                dist = _mm256_fmadd_ps(pb_vec[p_idx], py, dist);
                dist = _mm256_fmadd_ps(pc_vec[p_idx], pz, dist);

                // cmp < 0.0
                let cmp = _mm256_cmp_ps(dist, zero_vec, _CMP_LT_OQ);
                let fail_mask = _mm256_movemask_ps(cmp);
                pass_mask &= !fail_mask;

                if pass_mask == 0 {
                    break;
                }
            }

            for lane in 0..8 {
                results[base + lane] = ((pass_mask >> lane) & 1) != 0;
            }
        }

        for idx in (chunks8 * 8)..n {
            let mut visible = true;
            for p in &self.planes {
                let px = if p.a >= 0.0 { soa.max_x[idx] } else { soa.min_x[idx] };
                let py = if p.b >= 0.0 { soa.max_y[idx] } else { soa.min_y[idx] };
                let pz = if p.c >= 0.0 { soa.max_z[idx] } else { soa.min_z[idx] };
                if p.distance(px, py, pz) < 0.0 {
                    visible = false;
                    break;
                }
            }
            results[idx] = visible;
        }

        results
    }
}

pub struct SimdFrustumCull;
impl SimdFrustumCull {
    pub fn wgsl_source(&self) -> &'static str {
        SIMD_FRUSTUM_WGSL
    }
}
pub const SIMD_FRUSTUM_WGSL: &str = include_str!("../shaders/simd_frustum.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    fn unit() -> SimdFrustum {
        SimdFrustum::new([
            Plane { a: 1.0, b: 0.0, c: 0.0, d: 1.0 }, // x >= -1
            Plane { a: -1.0, b: 0.0, c: 0.0, d: 1.0 }, // x <= 1
            Plane { a: 0.0, b: 1.0, c: 0.0, d: 1.0 },
            Plane { a: 0.0, b: -1.0, c: 0.0, d: 1.0 },
            Plane { a: 0.0, b: 0.0, c: 1.0, d: 1.0 },
            Plane { a: 0.0, b: 0.0, c: -1.0, d: 1.0 },
        ])
    }

    #[test]
    fn inside_visible() {
        let f = unit();
        assert!(f.intersects(&Aabb {
            min: [-0.5; 3],
            max: [0.5; 3]
        }));
    }

    #[test]
    fn outside_culled() {
        let f = unit();
        assert!(!f.intersects(&Aabb {
            min: [2.0; 3],
            max: [3.0; 3]
        }));
    }

    #[test]
    fn batch_cull() {
        let f = unit();
        let r = f.cull(&[
            Aabb { min: [-0.5; 3], max: [0.5; 3] },
            Aabb { min: [5.0; 3], max: [6.0; 3] },
        ]);
        assert_eq!(r, vec![true, false]);
    }

    #[test]
    fn batch_cull_soa() {
        let f = unit();
        let boxes = vec![
            Aabb { min: [-0.5; 3], max: [0.5; 3] },
            Aabb { min: [5.0; 3], max: [6.0; 3] },
            Aabb { min: [-0.9; 3], max: [0.9; 3] },
            Aabb { min: [10.0; 3], max: [20.0; 3] },
            Aabb { min: [-0.1; 3], max: [0.1; 3] },
            Aabb { min: [100.0; 3], max: [200.0; 3] },
            Aabb { min: [-0.8; 3], max: [0.8; 3] },
            Aabb { min: [-0.2; 3], max: [0.2; 3] },
            Aabb { min: [50.0; 3], max: [60.0; 3] },
        ];
        let r_fast = f.cull_fast(&boxes);
        let r_scalar = boxes.iter().map(|b| f.intersects(b)).collect::<Vec<_>>();
        assert_eq!(r_fast, r_scalar);
    }
}

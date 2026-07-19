//! Phase 7 — Advanced Optimization: CPU SIMD AVX2 Frustum Culling.

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

/// 8 chunk AABBs stored in Structure-of-Arrays (SoA) format for AVX2 processing.
#[derive(Clone, Default)]
pub struct ChunkAabbSoa8 {
    pub min_x: [f32; 8],
    pub min_y: [f32; 8],
    pub min_z: [f32; 8],
    pub max_x: [f32; 8],
    pub max_y: [f32; 8],
    pub max_z: [f32; 8],
}

/// A standard plane equation (ax + by + cz + d = 0).
pub struct Plane {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
}

impl ChunkAabbSoa8 {
    /// AVX2 highly optimized frustum culling.
    /// Tests 8 AABBs simultaneously against 6 frustum planes.
    /// Returns an 8-bit mask where bit `i` is 1 if the chunk is visible.
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    pub unsafe fn cull_frustum_avx2(&self, planes: &[Plane; 6]) -> u8 {
        let mut visible_mask = _mm256_set1_epi32(-1);

        let min_x = _mm256_loadu_ps(self.min_x.as_ptr());
        let min_y = _mm256_loadu_ps(self.min_y.as_ptr());
        let min_z = _mm256_loadu_ps(self.min_z.as_ptr());
        let max_x = _mm256_loadu_ps(self.max_x.as_ptr());
        let max_y = _mm256_loadu_ps(self.max_y.as_ptr());
        let max_z = _mm256_loadu_ps(self.max_z.as_ptr());

        for plane in planes.iter() {
            let pa = _mm256_set1_ps(plane.a);
            let pb = _mm256_set1_ps(plane.b);
            let pc = _mm256_set1_ps(plane.c);
            let pd = _mm256_set1_ps(plane.d);

            // Determine which corner is most aligned with the plane normal
            let x_sel = _mm256_cmp_ps(pa, _mm256_setzero_ps(), _CMP_GT_OQ);
            let px = _mm256_blendv_ps(min_x, max_x, x_sel);

            let y_sel = _mm256_cmp_ps(pb, _mm256_setzero_ps(), _CMP_GT_OQ);
            let py = _mm256_blendv_ps(min_y, max_y, y_sel);

            let z_sel = _mm256_cmp_ps(pc, _mm256_setzero_ps(), _CMP_GT_OQ);
            let pz = _mm256_blendv_ps(min_z, max_z, z_sel);

            // Calculate plane distance: a*px + b*py + c*pz + d
            let mut dist = _mm256_mul_ps(pa, px);
            dist = _mm256_fmadd_ps(pb, py, dist);
            dist = _mm256_fmadd_ps(pc, pz, dist);
            dist = _mm256_add_ps(dist, pd);

            // If distance >= 0, it's inside or intersecting this plane
            let inside = _mm256_cmp_ps(dist, _mm256_setzero_ps(), _CMP_GE_OQ);
            visible_mask = _mm256_and_si256(visible_mask, _mm256_castps_si256(inside));
        }

        let mask = _mm256_movemask_ps(_mm256_castsi256_ps(visible_mask));
        mask as u8
    }
}

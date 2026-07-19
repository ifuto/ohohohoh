//! Portable SIMD-ish kernels for culling / greedy runs / popcount (Tier 3).
//! Stable Rust: u64 bit tricks + rayon; no nightly `std::simd`.

use rayon::prelude::*;

#[inline]
pub fn popcount_u64(x: u64) -> u32 {
    x.count_ones()
}

pub fn popcount_opaque_mask(mask: &[u64]) -> u32 {
    mask.iter().map(|w| w.count_ones()).sum()
}

/// Parallel popcount for large bitmasks (chunk columns).
pub fn popcount_opaque_mask_par(mask: &[u64]) -> u32 {
    if mask.len() < 64 {
        return popcount_opaque_mask(mask);
    }
    mask.par_chunks(64)
        .map(|c| c.iter().map(|w| w.count_ones()).sum::<u32>())
        .sum()
}

/// Merge consecutive set bits in a 16-wide row into (start, len) runs — greedy meshing helper.
pub fn merge_face_runs(row_mask: u16) -> Vec<(u8, u8)> {
    let mut runs = Vec::new();
    let mut i = 0u8;
    while i < 16 {
        if (row_mask >> i) & 1 == 0 {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < 16 && (row_mask >> i) & 1 != 0 {
            i += 1;
        }
        runs.push((start, i - start));
    }
    runs
}

/// Batch merge for 16 rows (one face slice) using u16 masks.
pub fn merge_face_runs_batch(rows: &[u16; 16]) -> Vec<(u8, u8, u8)> {
    // (row, start, len)
    let mut out = Vec::new();
    for (r, &mask) in rows.iter().enumerate() {
        for (start, len) in merge_face_runs(mask) {
            out.push((r as u8, start, len));
        }
    }
    out
}

#[derive(Clone, Copy, Debug)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// Six frustum planes as (nx,ny,nz,d) with outward normals; point visible if all ax+by+cz+d >= 0.
pub type FrustumPlanes = [[f32; 4]; 6];

#[inline]
fn aabb_outside_plane(aabb: &Aabb, p: &[f32; 4]) -> bool {
    // Positive vertex along plane normal
    let x = if p[0] >= 0.0 { aabb.max[0] } else { aabb.min[0] };
    let y = if p[1] >= 0.0 { aabb.max[1] } else { aabb.min[1] };
    let z = if p[2] >= 0.0 { aabb.max[2] } else { aabb.min[2] };
    p[0] * x + p[1] * y + p[2] * z + p[3] < 0.0
}

pub fn frustum_aabb_visible(aabb: &Aabb, planes: &FrustumPlanes) -> bool {
    for p in planes {
        if aabb_outside_plane(aabb, p) {
            return false;
        }
    }
    true
}

/// Batch frustum test — returns bitset of visible indices (bit i = aabbs[i] visible).
pub fn frustum_aabb_batch(aabbs: &[Aabb], planes: &FrustumPlanes) -> Vec<u64> {
    let words = (aabbs.len() + 63) / 64;
    let mut bits = vec![0u64; words.max(1)];
    for (i, aabb) in aabbs.iter().enumerate() {
        if frustum_aabb_visible(aabb, planes) {
            bits[i / 64] |= 1u64 << (i % 64);
        }
    }
    bits
}

pub fn frustum_aabb_batch_par(aabbs: &[Aabb], planes: &FrustumPlanes) -> Vec<bool> {
    aabbs
        .par_iter()
        .map(|a| frustum_aabb_visible(a, planes))
        .collect()
}

/// Alias used by low-spec stack.
#[inline]
pub fn aabb_in_frustum(aabb: &Aabb, planes: &FrustumPlanes) -> bool {
    frustum_aabb_visible(aabb, planes)
}

/// Extract 6 outward frustum planes from a row-major view-projection matrix
/// (`mul(float4(p,1), vp)` convention — matches terrain VS).
pub fn frustum_planes_from_view_proj(vp: &[[f32; 4]; 4]) -> FrustumPlanes {
    // Columns of the matrix (row-vector * M → treat rows as basis).
    let r0 = [vp[0][0], vp[1][0], vp[2][0], vp[3][0]];
    let r1 = [vp[0][1], vp[1][1], vp[2][1], vp[3][1]];
    let r2 = [vp[0][2], vp[1][2], vp[2][2], vp[3][2]];
    let r3 = [vp[0][3], vp[1][3], vp[2][3], vp[3][3]];
    let mut planes = [
        add4(r3, r0), // left
        sub4(r3, r0), // right
        add4(r3, r1), // bottom
        sub4(r3, r1), // top
        add4(r3, r2), // near
        sub4(r3, r2), // far
    ];
    for p in &mut planes {
        let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt().max(1e-8);
        p[0] /= len;
        p[1] /= len;
        p[2] /= len;
        p[3] /= len;
    }
    planes
}

#[inline]
fn add4(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]]
}

#[inline]
fn sub4(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_and_frustum() {
        assert_eq!(merge_face_runs(0b0000_0000_0011_1100), vec![(2, 4)]);
        let aabb = Aabb {
            min: [-1.0, -1.0, -1.0],
            max: [1.0, 1.0, 1.0],
        };
        // Open frustum (all planes far)
        let planes = [[0.0, 0.0, 1.0, 10.0]; 6];
        assert!(frustum_aabb_visible(&aabb, &planes));
        assert_eq!(popcount_opaque_mask(&[0b1111]), 4);
    }
}

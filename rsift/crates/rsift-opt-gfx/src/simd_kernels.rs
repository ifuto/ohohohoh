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

/// Six frustum planes as (nx,ny,nz,d); 平面の内側が dist = ax+by+cz+d >= 0。
/// 法線は**内向き** (Gribb/Hartmann の r3±r0 抽出は内向きを与える —
/// 旧 doc の "outward normals" 記述は偽だった、2026-07-22 wave 27 で訂正)。
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

/// Extract 6 inward-facing frustum planes from a row-major view-projection
/// matrix with **row-vector** convention `clip = mul(float4(p,1), vp)`
/// (= world_column_store::TerrainFrameConstants の view_proj。列ベクトル
/// M·p 規約の行列 (frame_pipeline::build_view_proj 等) をそのまま流すと
/// 転置ずれで誤カリングする — M·p 系は frame_worldgen::frustum_planes
/// (2026-07-21 監査で実検証済み) を使うこと。両者は相互に供給禁止)。
///
/// Near plane は `r2` 単体: wgpu の z_ndc ∈ [0,1] では視体積は
/// clip.z >= 0 ⟺ col2·p >= 0。旧実装の add4(r3, r2) は z >= -w の
/// GL 式体積で「誤カリングしないが近段子抜け」という保守側の逸脱だった
/// (frame_worldgen::frustum_planes との対称性・厳密値テストで確定、
/// 2026-07-22 wave 27 で根治)。Far は sub4(r3, r2) (z <= w) で旧来正しい。
pub fn frustum_planes_from_view_proj(vp: &[[f32; 4]; 4]) -> FrustumPlanes {
    // Columns of the matrix (row-vector p·M ⇒ clip.x = p·col0, clip.w = p·col3).
    let r0 = [vp[0][0], vp[1][0], vp[2][0], vp[3][0]];
    let r1 = [vp[0][1], vp[1][1], vp[2][1], vp[3][1]];
    let r2 = [vp[0][2], vp[1][2], vp[2][2], vp[3][2]];
    let r3 = [vp[0][3], vp[1][3], vp[2][3], vp[3][3]];
    let mut planes = [
        add4(r3, r0), // left   (w + x >= 0)
        sub4(r3, r0), // right  (w - x >= 0)
        add4(r3, r1), // bottom (w + y >= 0)
        sub4(r3, r1), // top    (w - y >= 0)
        r2,           // near   (wgpu z in [0,1]: z >= 0)
        sub4(r3, r2), // far    (w - z >= 0)
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

    /// wave 27-1: 単位行列 (clip = p そのもの) からの 6 平面が手計算の
    /// 整数係数と厳密一致 (正規化 len = 1 で変化しない値)。特に near 平面が
    /// (0,0,1,0) = z >= 0 であることが wgpu z∈[0,1] 規則の機械ピン — 旧実装の
    /// GL 式 (0,0,1,1) = z >= -w ではこのテストは確実に赤になる。
    #[test]
    fn identity_view_proj_planes_exact() {
        let vp = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let p = frustum_planes_from_view_proj(&vp);
        for (i, want) in [
            [1.0f32, 0.0, 0.0, 1.0], // left:   x >= -1
            [-1.0, 0.0, 0.0, 1.0],   // right:  x <= 1
            [0.0, 1.0, 0.0, 1.0],    // bottom: y >= -1
            [0.0, -1.0, 0.0, 1.0],   // top:    y <= 1
            [0.0, 0.0, 1.0, 0.0],    // near:   z >= 0 (wgpu)
            [0.0, 0.0, -1.0, 1.0],   // far:    z <= 1
        ]
        .iter()
        .enumerate()
        {
            for c in 0..4 {
                assert_eq!(p[i][c].to_bits(), want[c].to_bits(), "plane {i}[{c}]");
            }
        }
    }

    /// wave 27-2: 実透視行列 (fov 90, aspect 1, near 1, far 10) からの 6 平面が
    /// Gribb/Hartmann 手計算 (正規化後) と厳密一致。係数は exact rational で
    /// 厳密導出 (float64 近似禁止、W-3 教訓)。near が (0,0,-1,-1) となるのは
    /// near 平面 dist = -z_ndc_view_len で z_view=-1 (near) に置くと 0 となる
    /// 符号規則の実証 (plane の内側が dist>=0)。
    #[test]
    fn perspective_planes_match_hand_derived_gribb() {
        // world_column_store::perspective_rh 相当の row-major (p·M) 行列
        let a = 10.0f32 / (1.0 - 10.0); // -1.1111112
        let vp = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, a, -1.0],
            [0.0, 0.0, a, 0.0],
        ];
        let p = frustum_planes_from_view_proj(&vp);
        let want: [[u32; 4]; 6] = [
            [0x3f3504f3, 0x00000000, 0xbf3504f3, 0x00000000], // left
            [0xbf3504f3, 0x00000000, 0xbf3504f3, 0x00000000], // right
            [0x00000000, 0x3f3504f3, 0xbf3504f3, 0x00000000], // bottom
            [0x00000000, 0xbf3504f3, 0xbf3504f3, 0x00000000], // top
            [0x00000000, 0x00000000, 0xbf800000, 0xbf800000], // near  (0,0,-1,-1)
            [0x00000000, 0x00000000, 0x3f800000, 0x411ffffc], // far   (0,0,1,9.9999962)
        ];
        for i in 0..6 {
            for c in 0..4 {
                assert_eq!(p[i][c].to_bits(), want[i][c], "plane {i}[{c}] exact bits");
            }
        }
        // near 面の意味: z_view = -near の点で dist = 0、それより奥で正
        let at_near = [0.0f32, 0.0, -1.0, 1.0];
        let dist = (0..4).map(|c| p[4][c] * at_near[c]).sum::<f32>();
        assert_eq!(dist.to_bits(), 0, "z=-near is exactly on the near plane");
    }

    /// wave 27-3: merge/popcount の境界ケース厳密ピン。
    #[test]
    fn merge_runs_edge_cases_exact() {
        assert_eq!(
            merge_face_runs(0x0000),
            Vec::<(u8, u8)>::new(),
            "empty mask"
        );
        assert_eq!(merge_face_runs(0xffff), vec![(0, 16)], "full row");
        assert_eq!(merge_face_runs(0x8000), vec![(15, 1)], "top bit only");
        assert_eq!(merge_face_runs(0x0001), vec![(0, 1)], "bottom bit only");
        assert_eq!(
            merge_face_runs(0b0101_0101_0101_0101),
            vec![
                (0, 1),
                (2, 1),
                (4, 1),
                (6, 1),
                (8, 1),
                (10, 1),
                (12, 1),
                (14, 1)
            ],
            "alternating"
        );
        let rows: [u16; 16] = {
            let mut r = [0u16; 16];
            r[3] = 0b0000_0000_0111_1000;
            r[15] = 0x0001;
            r
        };
        assert_eq!(
            merge_face_runs_batch(&rows),
            vec![(3, 3, 4), (15, 0, 1)],
            "batch must preserve row index and order"
        );
    }

    /// wave 27-4: 並列/逐次 popcount は全サイズで一致 (閾値 64 の分岐含む)。
    #[test]
    fn popcount_par_matches_seq_at_threshold() {
        for n in [0usize, 1, 63, 64, 65, 130] {
            let mask: Vec<u64> = (0..n as u64)
                .map(|i| i.wrapping_mul(0x9E3779B97F4A7C15))
                .collect();
            assert_eq!(
                popcount_opaque_mask(&mask),
                popcount_opaque_mask_par(&mask),
                "n={n}"
            );
            let want: u32 = mask.iter().map(|w| w.count_ones()).sum();
            assert_eq!(popcount_opaque_mask_par(&mask), want, "n={n} exact");
        }
    }

    /// wave 27-5: batch ビット集合レイアウトと逐次判定の一致 (64 境界跨ぎ)、
    /// および AABB の「跨ぎ = 可視 / 完全外 = カリング」意味論ピン。
    #[test]
    fn batch_bitset_layout_and_straddle_semantics() {
        let vp = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let planes = frustum_planes_from_view_proj(&vp);
        let inside = Aabb {
            min: [-0.5, -0.5, 0.25],
            max: [0.5, 0.5, 0.75],
        };
        let straddle = Aabb {
            min: [-1.5, 0.0, 0.25],
            max: [-0.5, 0.5, 0.75],
        };
        let outside = Aabb {
            min: [-3.0, 0.0, 0.25],
            max: [-2.0, 0.5, 0.75],
        };
        let behind = Aabb {
            min: [-0.5, -0.5, -0.75],
            max: [0.5, 0.5, -0.25],
        };
        assert!(frustum_aabb_visible(&inside, &planes), "inside");
        assert!(frustum_aabb_visible(&straddle, &planes), "straddler stays");
        assert!(
            !frustum_aabb_visible(&outside, &planes),
            "fully left culled"
        );
        assert!(
            !frustum_aabb_visible(&behind, &planes),
            "behind near culled (wgpu z>=0)"
        );
        // 130 個 (2 word + 余り) で bitset レイアウトと逐次一致
        let mut aabbs = Vec::new();
        for i in 0..130 {
            aabbs.push(if i % 3 == 0 { outside } else { inside });
        }
        let bits = frustum_aabb_batch(&aabbs, &planes);
        assert_eq!(bits.len(), 3, "(130+63)/64 words");
        for (i, a) in aabbs.iter().enumerate() {
            let want = frustum_aabb_visible(a, &planes) as u64;
            assert_eq!((bits[i / 64] >> (i % 64)) & 1, want, "bit layout i={i}");
        }
        let par = frustum_aabb_batch_par(&aabbs, &planes);
        for (i, v) in par.iter().enumerate() {
            assert_eq!(*v, frustum_aabb_visible(&aabbs[i], &planes), "par==seq {i}");
        }
    }
}

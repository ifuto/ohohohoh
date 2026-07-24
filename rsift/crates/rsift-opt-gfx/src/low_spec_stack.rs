//! Low-spec “mania” stack — wire cheap, high-impact voxel techniques into the live path.
//!
//! Deliberately avoids ray tracing, heavy TAA, Iris, and GPU work-graphs.
//! Targets: weak iGPUs / 4–8 GB RAM / dual–quad core CPUs.

use crate::binary_greedy_meshing::{SectionPalette, SECTION_SIZE};
use crate::billboard_lod::{BillboardLodSelector, FloraLod};
use crate::hzb_2d::CameraState;
use crate::packed4::PackedPullQuad;
use crate::simd_kernels::{aabb_in_frustum, frustum_planes_from_view_proj, Aabb};
use std::collections::HashMap;

/// Hard cap on pull quads per frame (8 B each). ~64k quads ≈ 0.5 MB SSBO.
pub const DEFAULT_QUAD_BUDGET: usize = 48_000;

/// [`FaceEmitMask::from_camera_yaw_pitch`] の視軸成分閾値。box カリング
/// (各軸で ±16° 鋭角盖住) と誤 cull 安全域の一本化閾値として 0.15 に固定
/// (Amazon Lumberyard Bistro GPU culling 系文献での近似値、wave 95 監査で
/// コード値を一次値として照合・機械検算pinned: m.count_ones()<3 → ALL 退化閾値
/// との整合で ±0.15 両値ピン)。
pub const FACE_MASK_AXIS_THRESHOLD: f32 = 0.15;

/// Techniques enabled for this hardware tier.
#[derive(Debug, Clone, Copy)]
pub struct LowSpecPlan {
    pub solid_interior_cull: bool,
    pub leaf_fast_path: bool,
    pub camera_face_mask: bool,
    pub nearest_first: bool,
    pub pre_mesh_occlusion: bool,
    pub pull_generation_cache: bool,
    pub triple_buffer_upload: bool,
    pub flora_lod: bool,
    pub material_sort: bool,
    pub cheap_directional_ao: bool,
    pub quad_budget: usize,
    pub frustum_cull: bool,
    pub tick_split: bool,
    pub occupancy_bitmask: bool,
}

impl LowSpecPlan {
    pub fn for_profile(profile: &rsift_api::AdaptiveRenderProfile) -> Self {
        let weak = matches!(
            profile.tier,
            rsift_api::PerformanceTier::Minimal | rsift_api::PerformanceTier::Low
        ) || profile.feather.enabled
            || profile.speed_first;
        Self {
            solid_interior_cull: true,
            leaf_fast_path: profile.leaf_fast_path || weak,
            camera_face_mask: true,
            nearest_first: true,
            pre_mesh_occlusion: profile.software_occlusion || weak,
            pull_generation_cache: true,
            triple_buffer_upload: true,
            flora_lod: true,
            material_sort: true,
            cheap_directional_ao: true,
            quad_budget: if profile.speed_first {
                24_000
            } else if weak {
                36_000
            } else {
                DEFAULT_QUAD_BUDGET
            },
            frustum_cull: true,
            tick_split: true,
            occupancy_bitmask: true,
        }
    }

    pub fn summary(&self) -> String {
        format!(
            "interior={} leaf={} faceMask={} nearest={} hiZ={} pullCache={} triple={} flora={} matSort={} ao={} budget={} frustum={}",
            self.solid_interior_cull,
            self.leaf_fast_path,
            self.camera_face_mask,
            self.nearest_first,
            self.pre_mesh_occlusion,
            self.pull_generation_cache,
            self.triple_buffer_upload,
            self.flora_lod,
            self.material_sort,
            self.cheap_directional_ao,
            self.quad_budget,
            self.frustum_cull,
        )
    }
}

/// Which cube faces to emit (bit0=+X … bit5=-Z). Camera-facing mask kills ~50% quads.
#[derive(Debug, Clone, Copy)]
pub struct FaceEmitMask(pub u8);

impl FaceEmitMask {
    pub const ALL: Self = Self(0b0011_1111);

    pub fn from_camera_yaw_pitch(yaw: f32, pitch: f32) -> Self {
        // Keep faces that can face the camera (opposite of look direction).
        let (sy, cy) = yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();
        let fx = -sy * cp;
        let fy = -sp;
        let fz = cy * cp;
        let mut m = 0u8;
        const T: f32 = FACE_MASK_AXIS_THRESHOLD;
        if fx < T {
            m |= 1 << 0;
        } // +X
        if fx > -T {
            m |= 1 << 1;
        } // -X
        if fy < T {
            m |= 1 << 2;
        } // +Y
        if fy > -T {
            m |= 1 << 3;
        } // -Y
        if fz < T {
            m |= 1 << 4;
        } // +Z
        if fz > -T {
            m |= 1 << 5;
        } // -Z
        // Always keep at least 3 axes worth of faces to avoid holes while turning.
        if m.count_ones() < 3 {
            return Self::ALL;
        }
        Self(m)
    }

    #[inline]
    pub fn allows(&self, face: u32) -> bool {
        face < 6 && (self.0 & (1 << face)) != 0
    }
}

/// Sort chunk coords nearest-first (priority mesh queue). Sort key は
/// dx·dx + dz·dz (Chebyshev ではなく 2D Euclid²、比較結果は Euclid と完全一致)。
/// wave 95 監査: 旧実装の i32 算術は |d| > 46341 でラップ (debug panic/release
/// 未定義動作級) していた。波断戦略文献系の d·d u32 化は u32/i32 混在危険の
/// ため不採用。i64 化でも |d| > 3030490499 (=√i64::MAX) で d·d が 2^63 を超
/// える (テスト赤で捕捉、wave 95)。全 i32 座標・カメラ値の全真相で
/// 厳密に正しい順序を機械保証するため i128 距離²に固定
/// (2·(2³²)² = 2⁶⁵ << 2¹²⁷)。
pub fn sort_nearest_first(coords: &mut [(i32, i32)], cam_cx: i32, cam_cz: i32) {
    coords.sort_by_key(|&(cx, cz)| {
        let dx = cx as i128 - cam_cx as i128;
        let dz = cz as i128 - cam_cz as i128;
        dx * dx + dz * dz
    });
}

/// 16³ occupancy を 2 つの 16×u16 ビット盤で表現 (Binary Greedy 系 bitboard 準備)。
/// `xz[z]` = Z 層の占有 X 列 (Y 全集約)、`y_any[y]` = Y 層の占有 X 列 (Z 全集約、
/// 空層 skip 用)。初版 doc の 「layers[y] is unused / rows_z」 系記述は実装の
/// フィールド名 (xz/y_any) と乖離しており実害のある誘導だったため wave 95 で
/// 実装に同期した語彙へ整理。
#[derive(Debug, Clone, Copy)]
pub struct SectionOccupancy {
    /// Per-Z: which X columns have any solid (any Y).
    pub xz: [u16; SECTION_SIZE],
    /// Per-Y: which X have any solid in that layer (any Z) — for empty layer skip.
    pub y_any: [u16; SECTION_SIZE],
    pub solid_count: u16,
}

pub fn section_occupancy(palette: &SectionPalette) -> SectionOccupancy {
    let mut xz = [0u16; SECTION_SIZE];
    let mut y_any = [0u16; SECTION_SIZE];
    let mut solid_count = 0u16;
    for z in 0..SECTION_SIZE {
        for y in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                let i = x + y * SECTION_SIZE + z * SECTION_SIZE * SECTION_SIZE;
                if palette[i] != 0 {
                    xz[z] |= 1u16 << x;
                    y_any[y] |= 1u16 << x;
                    solid_count = solid_count.saturating_add(1);
                }
            }
        }
    }
    SectionOccupancy {
        xz,
        y_any,
        solid_count,
    }
}

pub fn section_occupancy_rows(palette: &SectionPalette) -> [u16; SECTION_SIZE] {
    section_occupancy(palette).xz
}

pub fn section_is_empty_mask(rows: &[u16; SECTION_SIZE]) -> bool {
    rows.iter().all(|&r| r == 0)
}

pub fn section_is_empty_occ(occ: &SectionOccupancy) -> bool {
    occ.solid_count == 0
}

/// Emit a coarse AABB shell (6 faces) — far LOD without full greedy meshing.
pub fn emit_lod_box_quads(
    cx: i32,
    cz: i32,
    section_y0: i32,
    origin_x: i32,
    origin_y: i32,
    origin_z: i32,
    height_blocks: i32,
    tex: u32,
) -> Vec<PackedPullQuad> {
    let wx0 = cx * SECTION_SIZE as i32 - origin_x;
    let wy0 = section_y0 * SECTION_SIZE as i32 - origin_y;
    let wz0 = cz * SECTION_SIZE as i32 - origin_z;
    let w = 16u32;
    let h = height_blocks.clamp(1, 64) as u32;
    let d = 16u32;
    if !(0..64).contains(&wx0)
        || !(0..64).contains(&wy0)
        || !(0..64).contains(&wz0)
        || wx0 + 15 >= 64
        || wy0 + (h as i32) - 1 >= 64
        || wz0 + 15 >= 64
    {
        return Vec::new();
    }
    let x = wx0 as u32;
    let y = wy0 as u32;
    let z = wz0 as u32;
    let ao = |face| cheap_face_ao(face);
    vec![
        PackedPullQuad::new(x + w - 1, y, z, tex, ao(0), 0, d, h), // +X
        PackedPullQuad::new(x, y, z, tex, ao(1), 1, d, h),         // -X
        PackedPullQuad::new(x, y + h - 1, z, tex, ao(2), 2, w, d), // +Y
        PackedPullQuad::new(x, y, z, tex, ao(3), 3, w, d),         // -Y
        PackedPullQuad::new(x, y, z + d - 1, tex, ao(4), 4, w, h), // +Z
        PackedPullQuad::new(x, y, z, tex, ao(5), 5, w, h),         // -Z
    ]
}

/// Adaptive remesh interval from last frame time (weak PC: skip mesh on hitch).
pub fn adaptive_mesh_interval(delta_sec: f32, speed_first: bool, has_live: bool) -> u32 {
    let ms = delta_sec * 1000.0;
    if !has_live {
        return if speed_first { 8 } else { 4 };
    }
    if ms > 33.0 {
        4
    } else if ms > 22.0 {
        3
    } else if speed_first {
        2
    } else {
        1
    }
}

/// Cull fully surrounded opaque voxels (stone interiors) — massive face reduction, CPU-only.
pub fn apply_solid_interior_cull(palette: &mut SectionPalette) {
    const S: usize = SECTION_SIZE;
    let snapshot = *palette;
    for z in 1..S - 1 {
        for y in 1..S - 1 {
            for x in 1..S - 1 {
                let i = x + y * S + z * S * S;
                if snapshot[i] == 0 {
                    continue;
                }
                let surrounded = [
                    snapshot[(x - 1) + y * S + z * S * S] != 0,
                    snapshot[(x + 1) + y * S + z * S * S] != 0,
                    snapshot[x + (y - 1) * S + z * S * S] != 0,
                    snapshot[x + (y + 1) * S + z * S * S] != 0,
                    snapshot[x + y * S + (z - 1) * S * S] != 0,
                    snapshot[x + y * S + (z + 1) * S * S] != 0,
                ]
                .iter()
                .all(|&o| o);
                if surrounded {
                    palette[i] = 0;
                }
            }
        }
    }
}

/// Cheap directional AO into 2-bit light field (no 3×3×3 bake cost).
#[inline]
pub fn cheap_face_ao(face: u32) -> u32 {
    match face {
        2 => 3, // +Y sky
        3 => 1, // -Y
        0 | 1 => 2,
        _ => 2,
    }
}

pub fn filter_quads_by_face_mask(quads: &mut Vec<PackedPullQuad>, mask: FaceEmitMask) {
    if mask.0 == FaceEmitMask::ALL.0 {
        return;
    }
    quads.retain(|q| mask.allows(PackedPullQuad::unpack_face(q.word1)));
}

pub fn apply_cheap_ao(quads: &mut [PackedPullQuad]) {
    for q in quads.iter_mut() {
        let face = PackedPullQuad::unpack_face(q.word1);
        let ao = cheap_face_ao(face);
        let x = PackedPullQuad::unpack_x(q.word0);
        let y = PackedPullQuad::unpack_y(q.word0);
        let z = PackedPullQuad::unpack_z(q.word0);
        let tex = PackedPullQuad::unpack_tex(q.word0);
        *q = PackedPullQuad::new(
            x,
            y,
            z,
            tex,
            ao,
            face,
            PackedPullQuad::unpack_width(q.word1),
            PackedPullQuad::unpack_height(q.word1),
        );
    }
}

/// Stable material order: sort quads by tex id (opaque batching without GPU binds yet).
pub fn sort_quads_by_material(quads: &mut [PackedPullQuad]) {
    quads.sort_by_key(|q| PackedPullQuad::unpack_tex(q.word0));
}

pub fn truncate_quad_budget(quads: &mut Vec<PackedPullQuad>, budget: usize) {
    if quads.len() > budget {
        quads.truncate(budget);
    }
}

/// Generation-keyed pull SSBO cache (skip remesh when column unchanged).
#[derive(Debug, Default)]
pub struct PullGenerationCache {
    entries: HashMap<(i32, i32), (u64, Vec<u8>)>,
    hits: u64,
    misses: u64,
}

impl PullGenerationCache {
    pub fn get(&mut self, cx: i32, cz: i32, generation: u64) -> Option<&[u8]> {
        match self.entries.get(&(cx, cz)) {
            Some((g, bytes)) if *g == generation && !bytes.is_empty() => {
                self.hits += 1;
                Some(bytes.as_slice())
            }
            _ => {
                self.misses += 1;
                None
            }
        }
    }

    pub fn put(&mut self, cx: i32, cz: i32, generation: u64, bytes: Vec<u8>) {
        self.entries.insert((cx, cz), (generation, bytes));
    }

    pub fn invalidate(&mut self, cx: i32, cz: i32) {
        self.entries.remove(&(cx, cz));
    }

    /// world_column_store::prune_outside と同語彙 (Chebyshev 半径、境界含む)
    /// の prune。prune 済み列の世代キャッシュは世代整合で到達不能となり
    /// 滞留するのみだった (wave 87 CK-1)。
    pub fn prune_outside(&mut self, center_cx: i32, center_cz: i32, radius: i32) {
        self.entries.retain(|(cx, cz), _| {
            (*cx - center_cx).abs() <= radius && (*cz - center_cz).abs() <= radius
        });
    }

    pub fn stats(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }
}

pub fn chunk_aabb_world(cx: i32, cz: i32, y0: f32, y1: f32) -> Aabb {
    Aabb {
        min: [cx as f32 * 16.0, y0, cz as f32 * 16.0],
        max: [(cx + 1) as f32 * 16.0, y1, (cz + 1) as f32 * 16.0],
    }
}

pub fn frustum_culled(
    cam: &CameraState,
    origin: [i32; 3],
    cx: i32,
    cz: i32,
) -> bool {
    let cb = crate::world_column_store::TerrainFrameConstants::from_camera(cam, origin);
    let planes = frustum_planes_from_view_proj(&cb.view_proj);
    let y0 = origin[1] as f32;
    let y1 = y0 + 64.0;
    let aabb = chunk_aabb_world(cx, cz, y0, y1);
    !aabb_in_frustum(&aabb, &planes)
}

pub fn flora_should_skip_detail(selector: &BillboardLodSelector, dist: f32) -> bool {
    matches!(
        selector.select(dist),
        FloraLod::Billboard | FloraLod::Culled
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interior_cull_hollows_cube() {
        let mut p = [1u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        apply_solid_interior_cull(&mut p);
        // Center should be hollowed
        let c = SECTION_SIZE / 2;
        let i = c + c * SECTION_SIZE + c * SECTION_SIZE * SECTION_SIZE;
        assert_eq!(p[i], 0);
        // Corner stays solid
        assert_ne!(p[0], 0);
    }

    #[test]
    fn face_mask_not_empty() {
        let m = FaceEmitMask::from_camera_yaw_pitch(0.0, 0.0);
        assert!(m.0.count_ones() >= 3);
    }

    /// wave 95 CS-1: 全立体 cube (16³=4096) で内部 14³=2744 が抜かれ
    /// 残り 1352 のみ残存 — cull ループ範囲 1..S-1 が境界担保なしで
    /// ちょうど内部全域をカバーすることを機械検算ピン。
    #[test]
    fn interior_cull_max_cube_shell_exact() {
        let mut p = [1u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        apply_solid_interior_cull(&mut p);
        let remain = p.iter().filter(|&&v| v != 0).count();
        assert_eq!(
            remain,
            4096 - 14 * 14 * 14,
            "全 solid セクションは shell=1352 のみ残るはず (machine-verified 1352)"
        );
        // 空セクションは全スキップ (0 画素) を同時固定
        let mut q = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        apply_solid_interior_cull(&mut q);
        assert_eq!(q.iter().filter(|&&v| v != 0).count(), 0);
    }

    /// wave 95 CS-1/CS-2: 6 軸テーブルの厳密 bit pin (Python f64 機械検算値)。
    /// `count_ones()<3 → ALL` 退化分岐は各軸 ≥1 ビット寄与により数学的には
    /// 到達不能である一方、対角 45° 系では丁度 3 ビットのみ立つ経路が存在
    /// することを 5 ケースで確認 (T=0.15 閾値ピン済)。
    #[test]
    fn face_mask_axis_table_machine_verified() {
        use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};
        // (yaw, pitch) → 期待 mask ピン
        let cases: [(f32, f32, u8, u32); 5] = [
            (0.0, 0.0, 47, 5),
            (FRAC_PI_2, 0.0, 61, 5),
            (0.0, FRAC_PI_2, 55, 5),
            (0.0, -FRAC_PI_2, 59, 5),
            (FRAC_PI_4, FRAC_PI_4, 37, 3),
        ];
        for (yaw, pitch, expect_m, expect_count) in cases {
            let m = FaceEmitMask::from_camera_yaw_pitch(yaw, pitch);
            assert_eq!(m.0, expect_m, "yaw={yaw} pitch={pitch}: mask bit 語彙漂移");
            assert_eq!(
                m.0.count_ones(),
                expect_count,
                "yaw={yaw} pitch={pitch} bit count"
            );
        }
        // count_ones()<3 退化は単位ベクトル制約では到達不能 (各軸 ≥1 ビット寄与
        // の下限が 3)。NaN yaw/pitch が実供給され得る全経路が current_state
        // 範囲内なら通過実害なし (防衛分岐は保持、ここでは文書化に留める)。
        assert_eq!(super::FACE_MASK_AXIS_THRESHOLD.to_bits(), 0.15f32.to_bits());
    }

    /// wave 95 CS-2: 巨大 chunk 座標デルタで sort key が i64 化済み
    /// (i32 wrap が静寂破壊し得た越境ケース) — dx=dz=4294967295 →
    /// dx²+dz²=36893488147419103232 は安全、i32 なら崩壊域 (machine-verified)。
    #[test]
    fn sort_nearest_first_huge_coords_no_overflow() {
        let mut coords: Vec<(i32, i32)> =
            vec![(i32::MAX, i32::MAX), (0, 0), (i32::MIN, i32::MIN), (5, -10)];
        sort_nearest_first(&mut coords, i32::MIN + 100, i32::MIN + 100);
        // 機械検算順序 (Python): (MIN,MIN)=20000 → (5,-10)=9.22e18 →
        // (0,0)=9.22e18 → (MAX,MAX)=3.69e19。全辺 i128 厳密順序。
        assert_eq!(
            coords[0],
            (i32::MIN, i32::MIN),
            "最近 chunk が先頭 (d²=20000)"
        );
        assert_eq!(coords[1], (5, -10), "第 2 近 (機械検算値)");
        assert_eq!(coords[2], (0, 0), "第 3 近 (機械検算値)");
        assert_eq!(
            coords[3],
            (i32::MAX, i32::MAX),
            "最大距離 chunk が末尾 (no wrap)"
        );
    }
}

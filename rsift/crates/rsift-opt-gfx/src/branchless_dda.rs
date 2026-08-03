//! Branchless 3D DDA — voxel grid ray traversal (GPU-friendly, no per-axis branches).
//!
//! Based on Amanatides & Woo with branchless axis selection (see balintcsala voxel tracing).

use crate::binary_greedy_meshing::{idx, SectionPalette, SECTION_SIZE};
// wave HQ (2026-08-03): cpu_saver の歩進/選択/プリフェッチプリミティブを本来の
// 消費者 (本 DDA) へ真配線 (§7「消費者ゼロ保持」違反の根治)。詳細は
// docs/internal/AUDIT_* wave HQ 節。
use crate::cpu_saver::{branchless_select_f32, BranchlessVoxelStepper, CacheLinePrefetcher};

#[derive(Debug, Clone, Copy)]
pub struct Ray3 {
    pub origin: [f32; 3],
    pub dir: [f32; 3],
}

impl Ray3 {
    pub fn new(origin: [f32; 3], dir: [f32; 3]) -> Self {
        Self { origin, dir }
    }

    /// 各軸の逆方向成分。`|d| < 1e-8` は「動かない軸」(tiny-dir 無視帯) とみなし
    /// **符号保持の ±INF** を返す (16³ 最長踏破 ≒ 48 voxel で drift ≦ 4.8e-7 voxel = 観測不能)。
    /// 旧実装は +INF 固定で、負の tiny dir では step=-1 × inv=+INF → t_delta=-INF を
    /// 生み t_max が負方向へ暴走する誤動作経路があった (DE-1)。
    /// なお -0.0 は「符号を持たないゼロ」として +INF (immobilize 一貫性優先)。
    /// wave HQ: tiny 帯の ±INF 選択を cpu_saver::branchless_select_f32 (bit 選択、
    /// NaN/-0.0/±inf の bit 保持・DN-2 契約) へ真配線。`if d<0{NEG_INF}else{POS_INF}`
    /// と bit 厳密同値 (-0.0/0.0 は d<0.0=false → +INF、tiny 負は -INF)。
    pub fn inv_dir(&self) -> [f32; 3] {
        #[inline]
        fn axis_inv(d: f32) -> f32 {
            if d.abs() < 1e-8 {
                branchless_select_f32(d < 0.0, f32::NEG_INFINITY, f32::INFINITY)
            } else {
                1.0 / d
            }
        }
        [
            axis_inv(self.dir[0]),
            axis_inv(self.dir[1]),
            axis_inv(self.dir[2]),
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoxelHit {
    /// 命中 voxel 座標 (0..SECTION_SIZE)
    pub x: usize,
    pub y: usize,
    pub z: usize,
    /// 命中 block id (非 air、すなわち 1..=u16::MAX)
    pub block: u16,
    /// 始点 voxel からの軸遷移回数 (始点 voxel 自体の命中は 0)
    pub steps: u32,
}

/// Branchless axis pick: returns 0, 1, or 2 for the axis with smallest t_max.
/// 同値 (tie) は小さい index 優先 (X > Y > Z の固定優先度で argmin の最小添字が選ばれる。
/// 面ちょうどの入射では幾何学的にどの軸を選んでも正しい)。NaN lane 混入時は IEEE の
/// 比較全 false で構造的に軸が歪むが、有限入力では t_max が NaN になる経路は無い
/// (DE-1 の immobilize で 0·INF=NaN も遮断) — DE-4 注記。
///
/// wave HQ (2026-08-03): 本体は [`cpu_saver::BranchlessVoxelStepper::advance_axis`]
/// へ委譲。旧局所実装 (a0/a1/a2 boolean 算術) と**全入力で厳密同値** (同値タイ優先
/// x>y>z の完全一致): advance_axis の `step_x=tmx<=tmy&&tmx<=tmz` が旧 `a0&&a1` に、
/// `step_y=!step_x&&tmy<=tmz` が旧 `!a0&&a2` に (t1<=t2 下での (t0>t1)||(t0>t2) ⟹ t0>t1
/// で一致)、`step_z` が残りの z 最小に対応。cpu_saver 343 網羅 strict pin + 本モジュール
/// 既存 fuzz (single_block_enclosure / slab_entry 等) で二重保証。局所再実装廃止 =
/// cpu_saver::advance_axis の真消費者化 (§7 根治)。
#[inline]
fn branchless_axis(t_max: [f32; 3]) -> usize {
    let (sx, sy, _sz) = BranchlessVoxelStepper::advance_axis(t_max[0], t_max[1], t_max[2]);
    if sx {
        0
    } else if sy {
        1
    } else {
        2
    }
}

/// Trace a ray through a 16³ section palette. Returns first non-air voxel.
/// - `max_steps` は voxel サンプル回数の上限 (0 なら即 None)。
/// - 始点 voxel 自体の判定は steps=0 として返る (steps = 始点から hit までの軸遷移回数)。
/// - 始点が 16³ 範囲外なら移動せず即 None (踏破中に範囲外へ出た場合も同様)。air = block id 0。
pub fn trace_section(palette: &SectionPalette, ray: &Ray3, max_steps: u32) -> Option<VoxelHit> {
    // DE-2: NaN/±INF の origin/dir は受理しない。floor()→as i32 は NaN→0 へ静寂飽和し、
    // (0,0,0) 起点の虚偽 trace を生む。debug_assert (dev/test で fail-loud、release は
    // コンパイルアウトのため bench 経路の bit・計時と無干渉)。
    debug_assert!(
        ray.origin.iter().all(|v| v.is_finite()) && ray.dir.iter().all(|v| v.is_finite()),
        "trace_section: 非有限の origin/dir は受理しない (NaN floor→0 静寂化の防止)"
    );
    let inv = ray.inv_dir();
    // wave HQ: 各軸の歩進符号を cpu_saver::BranchlessVoxelStepper::step_direction
    // (branchless・DN-3 契約) で統一。旧 `if d>=0{1}else{-1}` と**観測等価**:
    // 差分は d==0/-0.0 のみ (旧=+1, 新=0) だが、これらは |d|<1e-8 の tiny 帯 →
    // t_max/t_delta が INF で軸不動化 (DE-1) され step 値は一切使われないため結果不変。
    // 非 tiny 軸 (|d|>=1e-8) は d≠0 で step∈{±1} = 旧式と一致。branchless_select_i32
    // も step_direction 経由で伝播消費化 (§7)。
    let step = [
        BranchlessVoxelStepper::step_direction(ray.dir[0]),
        BranchlessVoxelStepper::step_direction(ray.dir[1]),
        BranchlessVoxelStepper::step_direction(ray.dir[2]),
    ];

    let mut vx = ray.origin[0].floor() as i32;
    let mut vy = ray.origin[1].floor() as i32;
    let mut vz = ray.origin[2].floor() as i32;

    // DE-1: tiny-dir lane (|d|<1e-8) は t_max/t_delta を直接 +INF に固定して軸を不動化する。
    // 旧実装は (b-o)·inv を無条件に計算していたため、負 tiny で (b-o)<0 × +INF=-INF
    // (軸が負方向へ暴走) や、整数境界始点で 0·INF=NaN (比較が全 false 化し無関係な軸を
    // 踏み続ける) に陥った。正・負・ゼロの全 tiny で lane 一貫の不動化となる。
    let tiny = [
        ray.dir[0].abs() < 1e-8,
        ray.dir[1].abs() < 1e-8,
        ray.dir[2].abs() < 1e-8,
    ];
    let mut t_max = [
        if tiny[0] {
            f32::INFINITY
        } else {
            ((if step[0] > 0 { vx + 1 } else { vx }) as f32 - ray.origin[0]) * inv[0]
        },
        if tiny[1] {
            f32::INFINITY
        } else {
            ((if step[1] > 0 { vy + 1 } else { vy }) as f32 - ray.origin[1]) * inv[1]
        },
        if tiny[2] {
            f32::INFINITY
        } else {
            ((if step[2] > 0 { vz + 1 } else { vz }) as f32 - ray.origin[2]) * inv[2]
        },
    ];
    let t_delta = [
        if tiny[0] {
            f32::INFINITY
        } else {
            step[0] as f32 * inv[0]
        },
        if tiny[1] {
            f32::INFINITY
        } else {
            step[1] as f32 * inv[1]
        },
        if tiny[2] {
            f32::INFINITY
        } else {
            step[2] as f32 * inv[2]
        },
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
        } else {
            // DE-3: 内部/外部判定は排反完備 (全軸 0<=v<16 の否定 ≡ 何れかの軸で
            // v<0 || v>=16) なので同値条件の再走査は不要で else で良い (旧 else if は冗長)。
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
        // wave HQ: 次反復の読出し対象を先行プリフェッチ (cpu_saver::CacheLinePrefetcher
        // の真消費者化・§7)。DDA は反復毎に palette[1 entry] を読むため 1 歩先のエントリを
        // 暖めるのは正統なレイテンシ隠蔽 (モジュール目的「CPU Overhead & Cache Optimization」)。
        // 座標は境界脱出後も clamp で常時 [0,16)^3 の有効 index → in-bounds ポインタ生成
        // (UB 無し)。効果はハードウェア状態依存で非観測 (DN-4 契約: 検出不能=中性) だが、
        // 偽配線ではなく実呼出としての真消費者化。x86/x86_64 では更にフォールトフリー。
        let pi = idx(
            vx.clamp(0, SECTION_SIZE as i32 - 1) as usize,
            vy.clamp(0, SECTION_SIZE as i32 - 1) as usize,
            vz.clamp(0, SECTION_SIZE as i32 - 1) as usize,
        );
        CacheLinePrefetcher::prefetch_read(palette.as_ptr().wrapping_add(pi));
    }
    None
}

/// WGSL compute shader snippet for branchless DDA (future wgpu pass)。
/// 消費者ゼロの scaffold (将来の GPU パス配線用に温存 — 削除方針外)。
/// 正直注記: dda_step は t_max のみ進行し、CPU 版の `vx += step` 相当の voxel 座標
/// 更新を含まない (Ray struct に voxel フィールドが無い) = 不完全対称。配線時に統一すること。
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

    /// DE-1 根治ピン: 負の tiny dir (|d|<1e-8) は「動かない軸」。
    /// 旧実装は t_delta=-INF で y 軸が負方向へ暴走し、本来命中するブロックを取り逃がした。
    #[test]
    fn negative_tiny_dir_keeps_axis_immobile() {
        let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        p[idx(5, 15, 0)] = 11;
        // d=[+1, -1e-9, 0]: y の負ドリフトは無視帯。x 行軍のみで (5,15,0) へ。
        let ray = Ray3::new([0.5, 15.5, 0.5], [1.0, -1e-9, 0.0]);
        let hit = trace_section(&p, &ray, 64).expect("負 tiny は無視帯: x 行軍で命中");
        assert_eq!((hit.x, hit.y, hit.z, hit.block), (5, 15, 0, 11));
        assert_eq!(hit.steps, 5, "vx: 0→5");
    }

    /// DE-1 根治ピン: 整数境界始点 × 負 tiny。旧実装は t_max=0·INF=NaN で
    /// branchless_axis の比較が全 false 化 (a0=false, a2=false) → 無関係な z 軸を
    /// 踏み続けて範囲外脱出していた。固定後は NaN 経路が遮断され x 行軍で命中する。
    #[test]
    fn integer_origin_negative_tiny_no_nan_axis_hijack() {
        let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        p[idx(8, 10, 5)] = 12;
        let ray = Ray3::new([5.5, 10.0, 5.5], [1.0, -1e-9, 0.0]);
        let hit = trace_section(&p, &ray, 64).expect("y 不動で x 行軍命中");
        assert_eq!((hit.x, hit.y, hit.z, hit.block), (8, 10, 5, 12));
        assert_eq!(hit.steps, 3, "vx: 5→8");
    }

    /// wave HQ (2026-08-03): `branchless_axis` は `cpu_saver::BranchlessVoxelStepper::
    /// advance_axis` へ委譲し**全入力で厳密同値** (局所再実装を廃し cpu_saver を真消費者化)。
    /// 同値タイ優先 x>y>z を含む広域 bit-pattern fuzz + 明示タイケースで固定。
    /// (adversarial A: advance_axis の `<=`→`<` 変異で本 pin が RED → 配線の真性を逆証明。)
    #[test]
    fn branchless_axis_delegates_to_cpu_saver_advance_axis_equivalence() {
        use crate::cpu_saver::BranchlessVoxelStepper;
        let mut s = 0xC0FFEEu64;
        let mut rng = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        for _ in 0..500 {
            let t = [
                f32::from_bits(rng() as u32),
                f32::from_bits(rng() as u32),
                f32::from_bits(rng() as u32),
            ];
            let axis = branchless_axis(t);
            let (sx, sy, _sz) = BranchlessVoxelStepper::advance_axis(t[0], t[1], t[2]);
            let from_advance = if sx { 0 } else if sy { 1 } else { 2 };
            assert_eq!(axis, from_advance, "branchless_axis ≠ advance_axis: t={t:?}");
        }
        // 明示タイケース (同値タイ優先 x>y>z)。
        assert_eq!(branchless_axis([1.0, 1.0, 2.0]), 0, "x=y<z → x 優先");
        assert_eq!(branchless_axis([3.0, 1.0, 1.0]), 1, "y=z<x → y 優先");
        assert_eq!(branchless_axis([2.0, 3.0, 1.0]), 2, "z 最小");
        assert_eq!(branchless_axis([5.0, 5.0, 5.0]), 0, "3 軸同一 → x 優先");
    }

    /// DE-1: inv_dir の符号・eps 境界セマンティクス厳密ピン。
    #[test]
    fn inv_dir_tiny_sign_and_boundary_pins() {
        let inv = Ray3::new([0.0; 3], [1e-9, -1e-9, -0.0]).inv_dir();
        assert_eq!(inv[0], f32::INFINITY, "tiny 正 → +INF");
        assert_eq!(inv[1], f32::NEG_INFINITY, "tiny 負 → -INF (符号保持)");
        assert_eq!(inv[2], f32::INFINITY, "-0.0 は immobilize 一貫で +INF");
        // eps 境界: 1e-8 未満=無視帯、以上=通常逆数
        let below = Ray3::new([0.0; 3], [9.9e-9, 0.0, 0.0]).inv_dir()[0];
        assert_eq!(below, f32::INFINITY, "9.9e-9 < 1e-8");
        let above = Ray3::new([0.0; 3], [1.1e-8, 0.0, 0.0]).inv_dir()[0];
        assert!((above > 9.0e7) && (above < 9.2e7), "1/1.1e-8 ≈ 9.09e7 台");
    }

    /// DE-5: 負方向対称性の strict ピン (スラブ -z 入射、正方向 spec テストの対)。
    #[test]
    fn negative_direction_slab_entry_symmetric() {
        let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        for z in 0..=6usize {
            for y in 0..SECTION_SIZE {
                for x in 0..SECTION_SIZE {
                    p[idx(x, y, z)] = 7;
                }
            }
        }
        let ray = Ray3::new([8.5, 8.5, 15.5], [0.0, 0.0, -1.0]);
        let hit = trace_section(&p, &ray, 64).expect("-z 行軍で slab 上面に命中");
        assert_eq!((hit.x, hit.y, hit.z, hit.block), (8, 8, 6, 7));
        assert_eq!(hit.steps, 9, "vz: 15→6");
    }

    /// DE-2: 非有限入力は dev/test プロファイルで panic する (release は無干渉)。
    #[test]
    #[should_panic(expected = "非有限")]
    fn non_finite_origin_panics_in_debug() {
        let p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        let ray = Ray3::new([f32::NAN, 0.5, 0.5], [1.0, 0.0, 0.0]);
        let _ = trace_section(&p, &ray, 8);
    }
}

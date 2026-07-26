//! Clustered (forward+) light assignment.
//!
//! 視錐台ではなく **正規化単位立方体 [0,1]³** を 3D グリッドに分割し、
//! 各クラスタに球-AABB 交差するライトを列挙する参照実装 (透視分割・指数
//! z スライスは無い様式化)。forward シェーダが全ライトでなくピクセルの
//! クラスタ内少数だけを回す forward+ の概念検証。
//!
//! 【誠実注記 wave 134 EH-1】wiring の実供給 (full_graph_wiring) は
//! **セクション局所座標 [0,16) のボクセル位置と輝度レベル半径 1..15** を
//! そのまま [0,1]³ グリッドへ流すため、空間割当は座標フレーム不一致の
//! まま計算される (近原点・高輝度は立方体全体を飲み込み、遠方・低輝度は
//! 全ミス)。消費者は `_max_cluster_load` 集計破棄のみ = 「評価実効・
//! 消費は集計型」の中間構造 (DY-2 と同型)。view/NDC 正規化の実配線は
//! 設計判断として引継ぎ。WGSL 側は同一式の件数集計のみの参照パス
//! (Rust 側は index リスト、3 連鎖語彙は球-AABB 式の同形維持)。

#[derive(Clone, Copy, Debug)]
pub struct Light {
    pub position: [f32; 3],
    pub radius: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct ClusterGrid {
    pub tiles_x: u32,
    pub tiles_y: u32,
    pub slices: u32,
}

impl ClusterGrid {
    /// # Contract
    /// 全次元 >= 1 かつ総クラスタ数が u32 指標域に収まること。
    /// 零次元は全クエリを静寂に空化する堕落形のため fail-loud で拒否し、
    /// 積の u64 検査で `index` の折り畳み衝突 (u32 wrap) を構造的に排除する。
    pub fn new(tiles_x: u32, tiles_y: u32, slices: u32) -> Self {
        assert!(
            tiles_x >= 1 && tiles_y >= 1 && slices >= 1,
            "ClusterGrid: all dims must be >= 1 (got {tiles_x}x{tiles_y}x{slices})"
        );
        let total = u64::from(tiles_x) * u64::from(tiles_y) * u64::from(slices);
        assert!(
            total <= u64::from(u32::MAX),
            "ClusterGrid: total clusters {total} exceeds u32 index domain"
        );
        Self {
            tiles_x,
            tiles_y,
            slices,
        }
    }

    /// Linear index of the cluster at `(cx, cy, cz)`.
    /// 範囲外座標は他クラスタのスロットへの静寂折り畳みとなるため fail-loud
    /// で拒否。`new` の総数保証により cz*ty*tx+.. は u32 を超えない (wrap 不出)。
    pub fn index(&self, cx: u32, cy: u32, cz: u32) -> usize {
        assert!(
            cx < self.tiles_x && cy < self.tiles_y && cz < self.slices,
            "ClusterGrid::index: ({cx},{cy},{cz}) out of {}x{}x{}",
            self.tiles_x,
            self.tiles_y,
            self.slices
        );
        (cz * self.tiles_y * self.tiles_x + cy * self.tiles_x + cx) as usize
    }

    /// World-space AABB (in a unit cube [0,1]^3) of cluster `(cx,cy,cz)`.
    pub fn aabb(&self, cx: u32, cy: u32, cz: u32) -> ([f32; 3], [f32; 3]) {
        let sx = 1.0 / self.tiles_x as f32;
        let sy = 1.0 / self.tiles_y as f32;
        let sz = 1.0 / self.slices as f32;
        let min = [cx as f32 * sx, cy as f32 * sy, cz as f32 * sz];
        let max = [(cx + 1) as f32 * sx, (cy + 1) as f32 * sy, (cz + 1) as f32 * sz];
        (min, max)
    }

    /// Assign each light to every cluster whose AABB it intersects.
    /// Returns `lights_per_cluster[index]` = list of light indices.
    ///
    /// 計算量は O(L × N) の全走査 (L=ライト数, N=クラスタ数)。範囲制限走査
    /// への置換は f32 境界判定の bit 同一性証明を伴う設計判断のため引継ぎ
    /// (wave 134 EH-5 棚卸し、EC-3 と同型)。ライト指標の型域は u32
    /// (GPU packed 配布前提)。
    pub fn assign_lights(&self, lights: &[Light]) -> Vec<Vec<u32>> {
        let n = (self.tiles_x * self.tiles_y * self.slices) as usize;
        let mut out: Vec<Vec<u32>> = vec![Vec::new(); n];
        for (li, light) in lights.iter().enumerate() {
            for cz in 0..self.slices {
                for cy in 0..self.tiles_y {
                    for cx in 0..self.tiles_x {
                        let (mn, mx) = self.aabb(cx, cy, cz);
                        if sphere_intersects_aabb(light.position, light.radius, mn, mx) {
                            out[self.index(cx, cy, cz)].push(li as u32);
                        }
                    }
                }
            }
        }
        out
    }

    pub fn wgsl_source(&self) -> &'static str {
        CLUSTERED_LIGHTING_WGSL
    }
}

/// 球 (c, r) と AABB [mn, mx] の交差。最近接点距離二乗 d <= r*r の**包含**
/// 判定 — 境界面に接する球は**両隣クラスタの双方に帰属**する conservative
/// 設計。非有限の c/r は判定 false で静寂 drop (観測欠測は drop)。
fn sphere_intersects_aabb(c: [f32; 3], r: f32, mn: [f32; 3], mx: [f32; 3]) -> bool {
    let mut d = 0.0f32;
    for i in 0..3 {
        let v = c[i].clamp(mn[i], mx[i]);
        let diff = c[i] - v;
        d += diff * diff;
    }
    d <= r * r
}

pub const CLUSTERED_LIGHTING_WGSL: &str = include_str!("../shaders/clustered_lighting.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn index_is_unique_and_in_range() {
        let g = ClusterGrid::new(4, 4, 4);
        let mut seen = std::collections::HashSet::new();
        for cz in 0..4u32 {
            for cy in 0..4u32 {
                for cx in 0..4u32 {
                    let i = g.index(cx, cy, cz);
                    assert!(i < 64);
                    assert!(seen.insert(i));
                }
            }
        }
        assert_eq!(seen.len(), 64);
    }
    #[test]
    fn light_assigned_to_overlapping_clusters_only() {
        let g = ClusterGrid::new(4, 4, 4);
        // unit cube split into 4 per axis => each cluster is 0.25 wide.
        // A light at center (0.5,0.5,0.5) radius 0.1 hits only the middle cluster.
        let lights = vec![Light {
            position: [0.5, 0.5, 0.5],
            radius: 0.1,
        }];
        let assigned = g.assign_lights(&lights);
        let middle = g.index(2, 2, 2); // 0.5..0.75
        assert!(assigned[middle].contains(&0));
        // a far cluster must NOT contain it
        let far = g.index(0, 0, 0);
        assert!(!assigned[far].contains(&0));
    }
    #[test]
    fn big_light_fills_many_clusters() {
        let g = ClusterGrid::new(4, 4, 4);
        let lights = vec![Light {
            position: [0.5, 0.5, 0.5],
            radius: 10.0,
        }];
        let assigned = g.assign_lights(&lights);
        let count: usize = assigned.iter().map(|v| v.len()).sum();
        assert_eq!(count, 64); // covers everything
    }
    #[test]
    fn sphere_aabb_edge_cases() {
        assert!(sphere_intersects_aabb([0.0, 0.0, 0.0], 1.0, [0.5, 0.5, 0.5], [1.0, 1.0, 1.0]));
        assert!(!sphere_intersects_aabb([0.0, 0.0, 0.0], 0.1, [0.5, 0.5, 0.5], [1.0, 1.0, 1.0]));
    }

    /// EH-2: 零次元は全クエリ静寂空化の堕落形のため契約拒否。
    #[test]
    #[should_panic(expected = "must be >= 1")]
    fn new_rejects_zero_tiles_x() {
        let _ = ClusterGrid::new(0, 4, 4);
    }
    #[test]
    #[should_panic(expected = "must be >= 1")]
    fn new_rejects_zero_tiles_y() {
        let _ = ClusterGrid::new(4, 0, 4);
    }
    #[test]
    #[should_panic(expected = "must be >= 1")]
    fn new_rejects_zero_slices() {
        let _ = ClusterGrid::new(4, 4, 0);
    }

    /// EH-2: 総数が u32 指標域を超える巨大グリッドも拒否 (index 折り畳み衝突の防止)。
    #[test]
    #[should_panic(expected = "u32 index domain")]
    fn new_rejects_u32_overflowing_total() {
        let _ = ClusterGrid::new(65536, 65536, 65536); // 2^48 > u32::MAX
    }

    /// EH-3: 非対称グリッドの全掃引単射 + 厳密ストライド (rq 導出 104/105)。
    #[test]
    fn index_injective_full_sweep_asymmetric() {
        let g = ClusterGrid::new(3, 5, 7);
        assert_eq!(g.index(2, 4, 6), 104); // rq: 6*(5*3) + 4*3 + 2
        let mut seen = std::collections::HashSet::new();
        for cz in 0..7u32 {
            for cy in 0..5u32 {
                for cx in 0..3u32 {
                    let i = g.index(cx, cy, cz);
                    assert!(i < 105);
                    assert!(seen.insert(i));
                }
            }
        }
        assert_eq!(seen.len(), 105);
    }

    /// EH-3: 範囲外座標は fail-loud (他クラスタへの静寂折り畳み拒否)。
    #[test]
    #[should_panic(expected = "out of")]
    fn index_rejects_out_of_range() {
        let g = ClusterGrid::new(4, 4, 4);
        let _ = g.index(4, 0, 0);
    }

    /// EH-4: aabb は乗算 1 発のため丸め 1 回に確定する厳密 bit 契約 (rq 導出)。
    #[test]
    fn aabb_exact_bits() {
        let g4 = ClusterGrid::new(4, 4, 4);
        let (mn, mx) = g4.aabb(2, 2, 2);
        assert_eq!(mn[0].to_bits(), 0x3F000000); // 0.5
        assert_eq!(mx[0].to_bits(), 0x3F400000); // 0.75
        let g6 = ClusterGrid::new(6, 6, 6);
        assert_eq!((1.0f32 / 6.0).to_bits(), 0x3E2AAAAB);
        let (mn5, mx5) = g6.aabb(5, 5, 5);
        // 5*(1/6) は 0x3F555555 では **なく** 0x3F555556 (rq 実導出、暗算禁止の理由)
        assert_eq!(mn5[0].to_bits(), 0x3F555556);
        assert_eq!(mx5[0].to_bits(), 0x3F800000); // 6*(1/6) は丸めで厳密 1.0 (rq)
    }

    /// EH-4: 接する球は交差扱い (d <= r*r の包含境界、両方向を bits 固定)。
    #[test]
    fn sphere_tangent_is_inclusive() {
        // 最近接距離 0.5 → d = 0.25、r = 0.5 → r*r = 0.25 → 等値で交差真
        assert!(sphere_intersects_aabb(
            [0.0, 0.0, 0.0],
            0.5,
            [0.5, 0.0, 0.0],
            [1.0, 1.0, 1.0]
        ));
        // r を 1 ulp 下げる (0x3EFFFFFF) と r*r < d で不交差
        let below = f32::from_bits(0x3EFFFFFF);
        assert!(!sphere_intersects_aabb(
            [0.0, 0.0, 0.0],
            below,
            [0.5, 0.0, 0.0],
            [1.0, 1.0, 1.0]
        ));
    }

    /// EH-4: 境界面上のライトは両隣クラスタの双方に帰属する (conservative)。
    #[test]
    fn boundary_face_light_claimed_by_both_clusters() {
        let g = ClusterGrid::new(4, 4, 4);
        // x=0.25 はクラスタ 0 ([0,0.25]) と 1 ([0.25,0.5]) の共有面。
        // y,z はクラスタ内部 (0.375) に据えて x 軸のみの 2 帰属に限定する。
        let lights = vec![Light {
            position: [0.25, 0.375, 0.375],
            radius: 0.01,
        }];
        let a = g.assign_lights(&lights);
        assert!(a[g.index(0, 1, 1)].contains(&0));
        assert!(a[g.index(1, 1, 1)].contains(&0));
        let total: usize = a.iter().map(|v| v.len()).sum();
        assert_eq!(total, 2);
    }

    /// EH-4: 非有限は観測欠測として drop、r*r = inf は全域支配。
    #[test]
    fn non_finite_inputs_contract() {
        assert!(!sphere_intersects_aabb(
            [f32::NAN, 0.0, 0.0],
            1.0,
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0]
        ));
        assert!(!sphere_intersects_aabb(
            [0.5, 0.5, 0.5],
            f32::NAN,
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0]
        ));
        // r = 1e30 は r*r が inf に飽和 → d <= inf で全域支配 (d 自体も inf、inf<=inf は真)
        assert!(sphere_intersects_aabb(
            [1e30, 1e30, 1e30],
            1e30,
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0]
        ));
    }

    /// EH-1: wiring 実供給形状 (セクション局所 [0,16) 座標 × 輝度半径) の
    /// 座標フレーム不一致 soak ピン。(2,2,2) r=15 は全点まで sqrt(12)<15 で
    /// **全 128,640 クラスタを飲み込み**、(15,15,15) r=1 は最近隅 (1,1,1) まで
    /// sqrt(588)>1 で**全域ミス** (rq eh_clustered.rq 導出)。
    #[test]
    fn wiring_shape_coordinate_frame_soak() {
        let g = ClusterGrid::new(120, 67, 16); // full_graph_wiring の実引数形状
        let flood = g.assign_lights(&[Light {
            position: [2.0, 2.0, 2.0],
            radius: 15.0,
        }]);
        let total: usize = flood.iter().map(|v| v.len()).sum();
        assert_eq!(total, 120 * 67 * 16); // 128,640
        let miss = g.assign_lights(&[Light {
            position: [15.0, 15.0, 15.0],
            radius: 1.0,
        }]);
        let total2: usize = miss.iter().map(|v| v.len()).sum();
        assert_eq!(total2, 0);
    }
}

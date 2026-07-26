//! Clustered (forward+) light assignment.
//!
//! 視錐台ではなく **正規化単位立方体 [0,1]³** を 3D グリッドに分割し、
//! 各クラスタに球-AABB 交差するライトを列挙する参照実装 (透視分割・指数
//! z スライスは無い様式化)。forward シェーダが全ライトでなくピクセルの
//! クラスタ内少数だけを回す forward+ の概念検証。
//!
//! 【根治 wave 135 EI-1 (旧 EH-1)】wiring の実供給 (full_graph_wiring) は
//! wave 134 時点でセクション局所座標 [0,16) × 半径 1..15 をそのまま
//! [0,1]³ グリッドへ流す座標フレーム不一致だったが、wave 135 で
//! **/16 正規化 (2 の冪除算・f32 無丸め)** に根治した。消費者も
//! `_max_cluster_load` 破棄から FrameWiringReport の
//! `cluster_max_load` / `cluster_lit_clusters` 実フィールドへ配線済み
//! (新指令 §7 未配線・消費者なし禁止の消化)。WGSL 側は同一式の件数集計
//! のみの参照パス (Rust 側は index リスト、3 連鎖語彙は球-AABB 式の同形
//! 維持)。view/NDC 実空間からの変換配線は描画本線の設計判断として残る。

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
    /// 【wave 135 EI-2】O(L × N) 全走査から**範囲制限走査**へ置換
    /// (旧 EH-5 棚卸し消化・新指令 §7)。出力は旧実装と**完全同一**
    /// (ライト順の pushes・クラスタ順の走査順序を保つ)。健全性は
    /// `axis_range` の包含証明に帰着: 範囲は全ての真の帰属クラスタを
    /// 必ず含む (超集合) ため、範囲外の評価省略は結果に影響しない。
    /// 等価性は mod tests の旧実装忠実オラクルとの fuzz 突合で機械固定。
    /// ライト指標の型域は u32 (GPU packed 配布前提)。
    pub fn assign_lights(&self, lights: &[Light]) -> Vec<Vec<u32>> {
        let n = (self.tiles_x * self.tiles_y * self.slices) as usize;
        let mut out: Vec<Vec<u32>> = vec![Vec::new(); n];
        for (li, light) in lights.iter().enumerate() {
            let (x0, x1) = axis_range(light.position[0], light.radius, self.tiles_x);
            let (y0, y1) = axis_range(light.position[1], light.radius, self.tiles_y);
            let (z0, z1) = axis_range(light.position[2], light.radius, self.slices);
            for cz in z0..=z1 {
                for cy in y0..=y1 {
                    for cx in x0..=x1 {
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

/// ライト 1 個の帰属候補クラスタ範囲 (軸方向 [lo, hi] 両端包含)。
///
/// **包含証明 (真の帰属集合の超集合であること)**:
/// 判定は f32 のクラスタ端 fl(cx·sx) で行われる。範囲は f64 で
/// floor((p∓r)/sx) を取り、両側へ pad 個広げる。
/// (i) pad の外側のクラスタ cx (> hi+pad) は、f32 端との差が
///     E = pad·sx − |fl 端の丸め誤差| だけ実距離で r を超える。
///     端の丸めは |x| ≲ 1.6 の領域で 2^-22 (≈2.4e-7、安全率込み) 未満、
///     pad = ceil(2^-22/sx)+1 ⟹ pad·sx ≥ 2^-22 + sx > 誤差 となり
///     f32 判定でも交差是不可能。
/// (ii) クランプが飽和する領域 (大きい |p±r| や大半径) では範囲が
///     [0, tiles-1] の端に貼り付くため自明に超集合。
/// (iii) 非有限の p/r は全範囲へ退化させ naive と同一評価経路に載せる
///     (NaN は判定側で落ちる、r*r=inf は全域支配、`inf<=inf` は真)。
/// 負半径は判定が |r| と同値 (r*r) なので abs で正規化してから床を取る。
fn axis_range(p: f32, r: f32, tiles: u32) -> (u32, u32) {
    if !p.is_finite() || !r.is_finite() {
        return (0, tiles - 1);
    }
    // (iv) r*r が f32 で inf に飽和するなら判定は d <= inf で全域真となる
    // (d は平方和で NaN 不出、d=inf でも inf<=inf は真) — naive は全
    // クラスタに帰属させるため、範囲も全域に退化させないと乖離する。
    // (捕捉 54: 飽和クラスの乖離を adversarial 想定外の入力テスト赤が捕捉)
    if (r * r).is_infinite() {
        return (0, tiles - 1);
    }
    let sx = 1.0f64 / f64::from(tiles);
    let rr = f64::from(r.abs());
    let pp = f64::from(p);
    let pad = ((4.0 / (sx * (1u64 << 22) as f64)).ceil() as i64 + 1).min(i64::from(tiles));
    // i64 キャスト前に f64 を予備 clamp (極端入力の飽和→wrapping 減算を根絶、
    // `as` は i64 域外で飽和するため ±1e15 を超える値は全て端の貼り付きに揃う)。
    let lo = ((pp - rr) / sx).floor().clamp(-1e15, 1e15) as i64 - pad;
    let hi = ((pp + rr) / sx).floor().clamp(-1e15, 1e15) as i64 + pad;
    let t = i64::from(tiles);
    // rr >= 0 より生範囲は単調 (lo <= hi) で、clamp は単調性を保つため
    // lo2 <= hi2 が常に成立する (空範囲分岐は到達不能)。
    (lo.clamp(0, t - 1) as u32, hi.clamp(0, t - 1) as u32)
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

    /// EI-2 の参照オラクル: wave 134 までの O(L × N) 全走査の忠実ミラー。
    /// (旧実装対照のため mod tests に保持 — DX-1 と同型の「テスト専用保持」)。
    fn assign_lights_naive(g: &ClusterGrid, lights: &[Light]) -> Vec<Vec<u32>> {
        let n = (g.tiles_x * g.tiles_y * g.slices) as usize;
        let mut out: Vec<Vec<u32>> = vec![Vec::new(); n];
        for (li, light) in lights.iter().enumerate() {
            for cz in 0..g.slices {
                for cy in 0..g.tiles_y {
                    for cx in 0..g.tiles_x {
                        let (mn, mx) = g.aabb(cx, cy, cz);
                        if sphere_intersects_aabb(light.position, light.radius, mn, mx) {
                            out[g.index(cx, cy, cz)].push(li as u32);
                        }
                    }
                }
            }
        }
        out
    }

    /// EI-2: 範囲制限走査は旧全走査と**出力完全同一** (push 順まで含め)。
    /// 乱択グリッド・乱択/特別ライトで fuzz 突合。
    #[test]
    fn range_restricted_matches_naive_oracle_fuzz() {
        let mut seed = 0x243F6A8885A308D3u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let grids = [
            (1u32, 1u32, 1u32),
            (2, 3, 5),
            (3, 5, 7),
            (4, 4, 4),
            (16, 9, 4),
            (120, 120, 4),
            (120, 67, 16),
        ];
        let special_r = [
            0.0f32,
            1e-3,
            0.05,
            -0.25,
            1.0,
            30.0,
            1e30,
            f32::NAN,
            f32::INFINITY,
        ];
        for (gi, &(tx, ty, tz)) in grids.iter().enumerate() {
            let g = ClusterGrid::new(tx, ty, tz);
            for round in 0..24 {
                let nl = if tx >= 120 { 4usize } else { 9 };
                let mut lights = Vec::new();
                for _ in 0..nl {
                    let pick = next();
                    let r = if pick % 3 == 0 {
                        special_r[(pick % (special_r.len() as u64)) as usize]
                    } else {
                        ((pick % 400) as f32) * 0.01
                    };
                    lights.push(Light {
                        position: [
                            ((next() % 5000) as f32) * 0.001 - 2.0,
                            ((next() % 5000) as f32) * 0.001 - 2.0,
                            ((next() % 5000) as f32) * 0.001 - 2.0,
                        ],
                        radius: r,
                    });
                }
                // 端ピッタリ狙いの決定的ケース (face の f32 端を aabb から取得)
                if round % 6 == 0 {
                    let (mn0, _) = g.aabb(0, 0, 0);
                    let (_, mx0) = g.aabb(tx - 1, 0, 0);
                    lights.push(Light {
                        position: [mx0[0], mn0[1], 0.5],
                        radius: 0.0,
                    });
                }
                let fast = g.assign_lights(&lights);
                let slow = assign_lights_naive(&g, &lights);
                assert_eq!(
                    fast, slow,
                    "grid={gi} round={round}: 範囲制限は全走査と完全同一でなければならない"
                );
            }
        }
    }

    /// 極端入力でも範囲計算が panic しない (i64 予備 clamp の堅牢性)。
    #[test]
    fn axis_range_extreme_inputs_no_panic() {
        let g = ClusterGrid::new(120, 67, 16);
        for r in [0.0f32, 1.0, 3.4e38] {
            for p in [-3.4e38f32, -1.0, 1e30, 3.4e38] {
                let out = g.assign_lights(&[Light {
                    position: [p, p, p],
                    radius: r,
                }]);
                let total: usize = out.iter().map(|v| v.len()).sum();
                if (r * r).is_infinite() {
                    // r*r = inf → 判定 d <= inf は全域真 → naive と同一の全域帰属
                    assert_eq!(total, 120 * 67 * 16, "p={p} r={r}: inf 半径は全域支配");
                } else {
                    assert_eq!(total, 0, "p={p} r={r}: グリッド外に帰属なし");
                }
            }
        }
        // 非有限は全範囲退化だが判定側で落ちる (r=NaN) / 全域支配 (r=inf)
        let g4 = ClusterGrid::new(4, 4, 4);
        let nan_out = g4.assign_lights(&[Light {
            position: [0.5, 0.5, 0.5],
            radius: f32::NAN,
        }]);
        assert_eq!(nan_out.iter().map(|v| v.len()).sum::<usize>(), 0);
        let inf_out = g4.assign_lights(&[Light {
            position: [0.5, 0.5, 0.5],
            radius: f32::INFINITY,
        }]);
        assert_eq!(inf_out.iter().map(|v| v.len()).sum::<usize>(), 64);
    }

    /// EI-2: face 上の点ライトは両隣に帰属 (計算端 = f32 の真の面)。
    /// aabb の端をそのまま位置に使うことで「リテラルの 0.4 は面ではない」
    /// (48*(1/120) = 0x3ECCCCNE vs 0.4 = 0x3ECCCNCD、rq 導出) の罠を構造的に回避。
    #[test]
    fn point_on_computed_face_claimed_by_face_neighbors() {
        let g = ClusterGrid::new(120, 120, 4);
        // 共用面はクラスタ 47|48 と 71|72 の間 = 47/71 の max 端。
        let edge_x = g.aabb(47, 0, 0).1[0]; // mx.x = 48*(1/120)
        let edge_y = g.aabb(0, 71, 0).1[1]; // mx.y = 72*(1/120) = 0.6 厳密 (rq)
        let lights = [Light {
            position: [edge_x, edge_y, 0.3],
            radius: 0.0,
        }];
        let a = g.assign_lights(&lights);
        for (cx, cy) in [(47u32, 71u32), (48, 71), (47, 72), (48, 72)] {
            assert!(
                a[g.index(cx, cy, 1)].contains(&0),
                "face 共有のクラスタ ({cx},{cy},1) に帰属必須"
            );
        }
        let total: usize = a.iter().map(|v| v.len()).sum();
        assert_eq!(total, 4, "r=0 の面上帰属はちょうど x2・y2 の4クラスタ");
    }

    /// EI-1: 正規化後の wiring 形状ピン — ボクセル (2,2,2) 輝度 15 は
    /// (0.125, 0.125, 0.125) r = 15/16 (= 0x3F700000、f32 無丸め、rq 導出)。
    /// 旧不一致形状の全氾濫は解消されつつ、ライト実在クラスタ (15,8,2) を含む
    /// 意味ある帰属になること。
    #[test]
    fn wiring_normalized_shape_pin() {
        let g = ClusterGrid::new(120, 67, 16);
        let lights = [Light {
            position: [2.0 / 16.0, 2.0 / 16.0, 2.0 / 16.0],
            radius: 15.0 / 16.0,
        }];
        let a = g.assign_lights(&lights);
        assert_eq!(lights[0].position[0].to_bits(), 0x3E000000); // 0.125
        assert_eq!(lights[0].radius.to_bits(), 0x3F700000); // 15/16
        let home = g.index(15, 8, 2); // floor(0.125*{120,67,16})
        assert!(a[home].contains(&0), "実在クラスタ (15,8,2) への帰属");
        let total: usize = a.iter().map(|v| v.len()).sum();
        let full = 120 * 67 * 16;
        assert!(
            total > 0 && total < full,
            "全氾濫でも全ミスでもないこと: {total}"
        );
        assert_eq!(total, 95_608, "機械導出 golden (EI-1、2026-07-26 実測)");
    }
}

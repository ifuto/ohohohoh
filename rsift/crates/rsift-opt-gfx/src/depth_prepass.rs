//! Depth prepass + translucent / water dedicated passes (Tier 4).
//!
//! 誠実注記 (wave 144 ER-3):
//! 1. `plan().estimated_cost` は**静的推定定数** (実測プロファイル非接続)。
//!    prepass 経路は gross +4 (3+4 vs 5) で early-Z reject により実測では
//!    相殺される設計意図 — 低スペック (UserSystemPrompt §2 制約) では
//!    余分なジオメトリパスが逆効果のため `for_low_spec` は prepass 省略
//!    (本コメントの旧来記述と整合)。
//! 2. 半透明ソートは**クアッド中心のみ**で半径非考慮 (近い大クワッドが
//!    遠い小クワッドを遮る典型ケースで中心ベースの限界が出る標準的近似)。
//! 3. NaN center は `partial_cmp` 失敗で **Equal fallback** — 任意要素と
//!    Equal の「橋」を形成し、比較系列の走査打止め箇所によっては
//!    **有限要素間の降順すら崩壊**する (挿入ソート系列で実測
//!    [0,1,2] 維持 = back-to-front 破壊、strict pin が機械記録)。
//!    fail-loud (panic) 化しない判断: ソート中破壊より退化順序で
//!    描画継続する方が総被害小 (±NaN=観測欠測は描画段で drop 済の
//!    契約前提、付録 A-8 哲学)。
//! 4. `dist2` は sqrt なし距離² (単調同値でソート順は正確、丸めの差は
//!    ソート比較に影響しない — 比較のみ用途)。
//! 5. render_pipeline:122 の `depth_plan` 保持・254 初期化は
//!    **plan() 未消費** (フレーム描画パス組立への導線は今後の実接続
//!    対象) — wave 144 ER-1 は wiring 側から plan() の実消費者を追加
//!    して §7 の「実装済み未配線」状態を解消 (census grep 機械確定)。

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PassId {
    DepthPrepass,
    OpaqueColor,
    Water,
    TranslucentBackToFront,
    Particles,
    Composite,
}

#[derive(Debug, Clone)]
pub struct PlannedPass {
    pub id: PassId,
    pub estimated_cost: u32,
    pub writes_color: bool,
    pub writes_depth: bool,
}

#[derive(Debug)]
pub struct DepthPrepassPlanner {
    pub enable_depth_prepass: bool,
    pub dedicated_water: bool,
    pub sort_translucent: bool,
}

impl DepthPrepassPlanner {
    pub fn new(enable_depth_prepass: bool, dedicated_water: bool) -> Self {
        Self {
            enable_depth_prepass,
            dedicated_water,
            sort_translucent: true,
        }
    }

    pub fn for_low_spec() -> Self {
        // Skip depth prepass on weak GPUs (extra geometry pass hurts more than it helps)
        Self::new(false, true)
    }

    pub fn for_high_spec() -> Self {
        Self::new(true, true)
    }

    pub fn plan(&self) -> Vec<PlannedPass> {
        let mut passes = Vec::new();
        if self.enable_depth_prepass {
            passes.push(PlannedPass {
                id: PassId::DepthPrepass,
                estimated_cost: 3,
                writes_color: false,
                writes_depth: true,
            });
            passes.push(PlannedPass {
                id: PassId::OpaqueColor,
                estimated_cost: 4,
                writes_color: true,
                writes_depth: false, // early-Z reject
            });
        } else {
            passes.push(PlannedPass {
                id: PassId::OpaqueColor,
                estimated_cost: 5,
                writes_color: true,
                writes_depth: true,
            });
        }
        if self.dedicated_water {
            passes.push(PlannedPass {
                id: PassId::Water,
                estimated_cost: 3,
                writes_color: true,
                writes_depth: true,
            });
        }
        passes.push(PlannedPass {
            id: PassId::TranslucentBackToFront,
            estimated_cost: if self.sort_translucent { 4 } else { 2 },
            writes_color: true,
            writes_depth: false,
        });
        passes.push(PlannedPass {
            id: PassId::Particles,
            estimated_cost: 2,
            writes_color: true,
            writes_depth: false,
        });
        passes.push(PlannedPass {
            id: PassId::Composite,
            estimated_cost: 1,
            writes_color: true,
            writes_depth: false,
        });
        passes
    }

    pub fn total_cost(&self) -> u32 {
        self.plan().iter().map(|p| p.estimated_cost).sum()
    }
}

/// Sort translucent quad centers back-to-front relative to camera.
pub fn sort_translucent_indices(
    centers: &[[f32; 3]],
    cam: [f32; 3],
    indices: &mut [u32],
) {
    indices.sort_by(|&a, &b| {
        let da = dist2(centers[a as usize], cam);
        let db = dist2(centers[b as usize], cam);
        db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
    });
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    dx * dx + dy * dy + dz * dz
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_skips_prepass() {
        let p = DepthPrepassPlanner::for_low_spec().plan();
        assert!(!p.iter().any(|x| x.id == PassId::DepthPrepass));
    }

    #[test]
    fn sort_back_to_front() {
        let centers = [[0.0, 0.0, 0.0], [0.0, 0.0, 10.0], [0.0, 0.0, 5.0]];
        let mut idx = [0u32, 1, 2];
        sort_translucent_indices(&centers, [0.0, 0.0, -1.0], &mut idx);
        assert_eq!(idx[0], 1); // farthest first
    }
}

#[cfg(test)]
mod strict_tests {
    use super::*;

    /// ER-4: plan() 構造 golden (rq er_dp 導出) — 4 構成形 exhaustive:
    /// pass 列・cost・writes フラグ真理値表 (prepass 経路では
    /// OpaqueColor が writes_depth=false = early-Z 設計の核心)。
    #[test]
    fn plan_structure_golden_exhaustive() {
        let low = DepthPrepassPlanner::for_low_spec().plan();
        let low_ids: Vec<PassId> = low.iter().map(|p| p.id).collect();
        assert_eq!(
            low_ids,
            vec![
                PassId::OpaqueColor,
                PassId::Water,
                PassId::TranslucentBackToFront,
                PassId::Particles,
                PassId::Composite,
            ],
            "low: 5 passes (prepass なし = 低スペック方針)"
        );
        assert_eq!(low.iter().map(|p| p.estimated_cost).sum::<u32>(), 15);
        assert_eq!(DepthPrepassPlanner::for_low_spec().total_cost(), 15);
        // writes 真理値表 (low): Opaque は writes_depth=true (prepass 不在)
        assert!(low[0].writes_color && low[0].writes_depth, "low Opaque");
        // Translucent/Particles/Composite は depth 非書込
        assert!(!low[2].writes_depth && !low[3].writes_depth && !low[4].writes_depth);

        let high = DepthPrepassPlanner::for_high_spec().plan();
        assert_eq!(high.len(), 6, "high: prepass 追加で 6 passes");
        assert_eq!(high.iter().map(|p| p.estimated_cost).sum::<u32>(), 17);
        assert_eq!(DepthPrepassPlanner::for_high_spec().total_cost(), 17);
        assert_eq!(high[0].id, PassId::DepthPrepass);
        assert!(
            !high[0].writes_color && high[0].writes_depth,
            "prepass: depth のみ"
        );
        assert!(
            high[1].id == PassId::OpaqueColor && high[1].writes_color && !high[1].writes_depth,
            "prepass 経路: Opaque は depth 非書込 (early-Z reject)"
        );

        let ff = DepthPrepassPlanner::new(false, false).plan();
        assert_eq!(ff.len(), 4, "water なし");
        assert_eq!(ff.iter().map(|p| p.estimated_cost).sum::<u32>(), 12);
        assert!(!ff.iter().any(|p| p.id == PassId::Water));

        let tf = DepthPrepassPlanner::new(true, false).plan();
        assert_eq!(tf.len(), 5);
        assert_eq!(tf.iter().map(|p| p.estimated_cost).sum::<u32>(), 14);
    }

    /// ER-4: sort_translucent=false で Translucent cost が 4→2 に変化
    /// (pub フィールドの実契約 pin)。
    #[test]
    fn sort_disabled_cost_pin() {
        let mut p = DepthPrepassPlanner::for_low_spec();
        p.sort_translucent = false;
        let plan = p.plan();
        assert_eq!(plan[2].estimated_cost, 2, "unsorted → cost 2");
        assert_eq!(p.total_cost(), 13, "5+3+2+2+1 = 13 (rq 導出)");
    }

    /// ER-4: new() は sort_translucent=true 既定 (半透明の順序保証が既定
    /// = 半透明の視覚正しさ優先)。
    #[test]
    fn new_defaults_sorted_pin() {
        let p = DepthPrepassPlanner::new(false, true);
        assert!(p.sort_translucent && p.dedicated_water && !p.enable_depth_prepass);
    }

    /// ER-4: back-to-front 正確 golden (rq er_dp 導出、f32 exact 整数
    /// 経路): centers=[0,0,0],[0,0,10],[1,1,1], cam=[0,0,-1] → dist2 =
    /// 1/121/6 → 降順 [1, 2, 0]。
    #[test]
    fn sort_exact_order_golden() {
        let centers = [[0.0, 0.0, 0.0], [0.0, 0.0, 10.0], [1.0, 1.0, 1.0]];
        let mut idx = [0u32, 1, 2];
        sort_translucent_indices(&centers, [0.0, 0.0, -1.0], &mut idx);
        assert_eq!(idx, [1, 2, 0], "dist2 121 > 6 > 1 → [1,2,0]");
    }

    /// ER-4: 空/単一入力は恒等 (契約域端点)。
    #[test]
    fn sort_empty_and_single_identity() {
        let centers: [[f32; 3]; 0] = [];
        let mut idx: [u32; 0] = [];
        sort_translucent_indices(&centers, [0.0; 3], &mut idx);
        let c1 = [[1.0, 2.0, 3.0]];
        let mut i1 = [7u32];
        sort_translucent_indices(&c1, [0.0; 3], &mut i1);
        assert_eq!(i1, [7]);
    }

    /// ER-3 (b) pin: NaN center は partial_cmp=None → **Equal fallback**。
    /// 実測の重要帰結: NaN は任意要素と Equal の「橋」を形成し、
    /// 挿入ソート型の比較系列 (cmp(1,0)=Equal → cmp(2,1)=Equal で打
    /// 止め、cmp(2,0) は走査されない) では**有限要素間の降順すら
    /// 崩壊**する (本例: 最遠 index 2 (dist2=121) が index 0 の後に
    /// 残る [0,1,2] = back-to-front 破壊)。fail-loud 化せず順序保証
    /// 喪失を受容する設計の誠実 pin (got 値機械記録)。
    #[test]
    fn sort_nan_equal_fallback_pin() {
        let centers = [[0.0, 0.0, 0.0], [f32::NAN, 0.0, 1.0], [0.0, 0.0, 10.0]];
        let mut idx = [0u32, 1, 2];
        sort_translucent_indices(&centers, [0.0, 0.0, -1.0], &mut idx);
        assert_eq!(
            idx,
            [0, 1, 2],
            "NaN の Equal 橋で降順崩壊 ([0,1,2] 維持、注記 (b) 機械記録)"
        );
    }

    /// ER-3 (c) pin: dist2 は sqrt なし距離² — 単調同値でソート順正確
    /// (-0.0/+0.0 も partial_cmp では別順序を生まない)。
    #[test]
    fn dist2_monotone_sqrt_less_pin() {
        assert_eq!(dist2([3.0, 4.0, 0.0], [0.0; 3]), 25.0, "3-4-5 の二乗");
        assert_eq!(dist2([0.0; 3], [0.0; 3]), 0.0);
        // 符号対称: dist2(a,b) == dist2(b,a)
        assert_eq!(
            dist2([1.0, 2.0, 3.0], [4.0, 5.0, 6.0]),
            dist2([4.0, 5.0, 6.0], [1.0, 2.0, 3.0])
        );
    }
}

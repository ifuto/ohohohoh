//! Sodium Extra 逆輸入 — パーティクル予算・間引き・距離減衰。
//!
//! 「量制限(パーティクル総数)」「カテゴリ別最適化」「一定間隔で規則的に残す間引き」。
//! ハッシュ間引きで潮ジッターを防ぐ (乱数ではなく ID に基づく決定論).

/// カテゴリはパーティクル種類に 1:1 は対応せず、大雑把に分類する (Sodium Extra 同様)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParticleKind {
    Explosion,
    Smoke,
    Weather,
    Magic,
    Fire,
    Splash,
    Ambient,
    BlockBreak,
    Combat,
    Other,
}

#[derive(Debug, Clone, Copy)]
pub struct ParticleBudget {
    /// 同時アクティブ総数の上限 (超えたら間引き)
    pub total_max: u32,
    pub per_kind_max: [u32; 10],
    /// これより近い粒子は間引きしない (爆発等の近距離体感維持)
    pub protect_near_distance: f32,
    /// カテゴリ別の視認距離上限倍率 (1.0 = フルレンダ距離)
    pub per_kind_distance: [f32; 10],
}

impl Default for ParticleBudget {
    fn default() -> Self {
        Self {
            total_max: 4000,
            per_kind_max: [256; 10],
            protect_near_distance: 3.0,
            per_kind_distance: [0.7, 1.0, 1.0, 0.8, 0.8, 0.6, 0.5, 1.0, 1.0, 1.0],
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ParticleRequest {
    pub id: u64,
    pub kind_idx: usize, // ParticleKind as usize
    pub pos: [f32; 3],
    pub velocity_mag: f32,
}

pub struct ParticleController {
    budget: ParticleBudget,
    active_total: u32,
    active_per_kind: [u32; 10],
    tick: u64,
}

impl ParticleController {
    /// 【呼出プロトコル (監査 2026-07-26 DR-1 で strict 化)】
    /// 各 tick で `begin_tick()` → **`reset_counts()`** → `allow()` の順で
    /// 呼ぶこと。`begin_tick` は間引きハッシュの回転 (tick++) のみを行い、
    /// カウント類はリセットしない。reset を怠ると active 数が単調累積して
    /// 予算解釈が「常時アクティブ数」から「累積総数」へ**破壊的に変質**する
    /// (wiring での実被害事例)。また `allow()` は許可系決定 (Allow/
    /// AllowProtected/AllowDecimated) 時に**内部で note_active 済み**のため、
    /// 呼出側が成功時にもう一度 note_active を呼ぶと**二重カウント**になる
    /// (実効予算が意図の半量へ縮退した wiring:1392 系の旧欠陥)。
    pub fn new(budget: ParticleBudget) -> Self {
        Self {
            budget,
            active_total: 0,
            active_per_kind: [0; 10],
            tick: 0,
        }
    }

    pub fn begin_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    pub fn note_active(&mut self, kind_idx: usize) {
        if kind_idx < 10 {
            self.active_per_kind[kind_idx] = self.active_per_kind[kind_idx].saturating_add(1);
            self.active_total = self.active_total.saturating_add(1);
        }
    }

    pub fn reset_counts(&mut self) {
        self.active_total = 0;
        self.active_per_kind = [0; 10];
    }

    /// スポーン要求を受けるか。true = spawn 可。
    ///
    /// ## 短絡順序の契約 (監査 2026-07-26 DR-3)
    /// 評価は (1) 距離カリング → (2) 近距離保護 → (3) 総数予算 → (4) カテゴリ
    /// 予算の順。**総数予算超過時はカテゴリ予算を評価しない** (1/8 通過なら
    /// カテゴリ満杯でも AllowDecimated = バイパス)。距離カリング、近距離保護
    /// 以外は allow 成功時に内部でカウント (呼出側の再カウントは二重計上)。
    /// * `kind_idx >= 10` は **Other (9) へ静寂クランプ** (panic しない)。
    ///   直接の `note_active(10+)` は無視 (allow 経路では 9 として計上)。
    /// * `dist == max_d` は**カリングしない** (厳密 `>`)、
    ///   `dist <= protect_near_distance` は保護 (境界包含)。
    /// * NaN 位置 (dist=NaN) は両比較 false で**予算評価へ通常通り進む**
    ///   (拒否しない fail-visible = NaN のまま Allow 経路)。
    pub fn allow(
        &mut self,
        req: &ParticleRequest,
        cam: [f32; 3],
        max_render_distance: f32,
    ) -> SpawnDecision {
        let kind = req.kind_idx.min(9);
        let dist = distance(req.pos, cam);

        // 視認距離: カテゴリ別距離上限
        let max_d = max_render_distance * self.budget.per_kind_distance[kind];
        if dist > max_d {
            return SpawnDecision::CullTooFar;
        }

        // 近距離保護
        if dist <= self.budget.protect_near_distance {
            self.note_active(kind);
            return SpawnDecision::AllowProtected;
        }

        // 総数予算・カテゴリ予算
        if self.active_total >= self.budget.total_max {
            // 予算外でも決定論 1/8 間引きで残す (突然消えないように)
            if !deterministic_keep(req.id, self.tick, 8) {
                return SpawnDecision::CullTotalBudget;
            }
            self.note_active(kind);
            return SpawnDecision::AllowDecimated;
        }
        if self.active_per_kind[kind] >= self.budget.per_kind_max[kind] {
            if !deterministic_keep(req.id, self.tick, 4) {
                return SpawnDecision::CullKindBudget;
            }
            self.note_active(kind);
            return SpawnDecision::AllowDecimated;
        }

        self.note_active(kind);
        SpawnDecision::Allow
    }

    pub fn budget(&self) -> &ParticleBudget {
        &self.budget
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnDecision {
    Allow,
    AllowProtected,
    AllowDecimated,
    CullTooFar,
    CullTotalBudget,
    CullKindBudget,
}

/// id と tick のハッシュで 1/n を残す決定論間引き。
/// 同じ id は同じ tick 内で同じ判定になる (フリッカーしない)。tick ごとに 1/n 巡回。
fn deterministic_keep(id: u64, tick: u64, n: u32) -> bool {
    let n = n.max(1) as u64;
    // FNV-1a 64
    let mut h: u64 = 0xcbf29ce484222325;
    for b in id.to_le_bytes().iter().chain(tick.to_le_bytes().iter()) {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    (h % n) == 0
}

#[inline]
fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(id: u64) -> ParticleRequest {
        // カメラ原点から距離 10 (protect_near より遠く max_distance 64 より近い)
        ParticleRequest {
            id,
            kind_idx: 0,
            pos: [10.0, 0.0, 0.0],
            velocity_mag: 1.0,
        }
    }

    #[test]
    fn near_is_protected() {
        let mut c = ParticleController::new(ParticleBudget {
            total_max: 1,
            ..Default::default()
        });
        let r = ParticleRequest {
            id: 1,
            kind_idx: 0,
            pos: [1.0, 0.0, 0.0],
            velocity_mag: 0.0,
        };
        assert_eq!(c.allow(&r, [0.0; 3], 64.0), SpawnDecision::AllowProtected);
    }

    #[test]
    fn far_is_culled() {
        let mut c = ParticleController::new(ParticleBudget::default());
        let r = ParticleRequest {
            id: 1,
            kind_idx: 7,
            pos: [200.0, 0.0, 0.0],
            velocity_mag: 0.0,
        };
        assert_eq!(c.allow(&r, [0.0; 3], 64.0), SpawnDecision::CullTooFar);
    }

    #[test]
    fn deterministic_decimation_stable_within_tick() {
        // same id & tick = same outcome (no flicker)
        let a = deterministic_keep(12345, 50, 8);
        let b = deterministic_keep(12345, 50, 8);
        assert_eq!(a, b);
    }

    #[test]
    fn kind_budget_enforcement() {
        let mut c = ParticleController::new(ParticleBudget {
            total_max: 10000,
            per_kind_max: [1, 1, 1, 1, 1, 1, 1, 1, 1, 1],
            ..Default::default()
        });
        let r1 = req(111);
        c.reset_counts();
        let _ = c.allow(&r1, [0.0; 3], 64.0);
        // 2nd same-kind: may be culled or decimated
        let r2 = req(222);
        let d = c.allow(&r2, [0.0; 3], 64.0);
        assert!(matches!(
            d,
            SpawnDecision::CullKindBudget | SpawnDecision::AllowDecimated
        ));
    }

    // ------------------------------------------------- 監査 2026-07-26 DR 追加分

    /// DR-1: プロトコル契約 pin — allow() は許可時に内部で 1 回だけ計上し、
    /// begin_tick はカウントをリセットしない (= reset_counts は呼出側責務)。
    #[test]
    fn count_protocol_contract() {
        let mut c = ParticleController::new(ParticleBudget::default());
        let r = req(1);
        assert_eq!(
            c.allow(&r, [0.0; 3], 64.0),
            SpawnDecision::Allow,
            "予算内・距離内の初回"
        );
        // 内部計上は高々 1 回/許可 (wiring 旧欠陥の意図的保護)。
        assert_eq!(
            c.active_total, 1,
            "二重計上しない (旧 wiring 欠陥の回帰があれば 2 になる)"
        );
        assert_eq!(c.active_per_kind[0], 1);
        // begin_tick は tick のみ回転、カウントは据置き。
        c.begin_tick();
        assert_eq!(c.active_total, 1, "begin_tick はカウントをリセットしない");
        c.reset_counts();
        assert_eq!(c.active_total, 0);
        assert_eq!(c.active_per_kind[0], 0);
    }

    /// DR-2: kind_idx の静寂クランプと note_active 直接呼出の非対称契約。
    #[test]
    fn kind_idx_clamp_and_note_active_asymmetry() {
        let mut c = ParticleController::new(ParticleBudget::default());
        // allow(kind_idx=10) は clamp して kind 9 (Other) 評価で計上も 9。
        let r = ParticleRequest {
            id: 1,
            kind_idx: 10,
            pos: [10.0, 0.0, 0.0],
            velocity_mag: 0.0,
        };
        assert_eq!(c.allow(&r, [0.0; 3], 64.0), SpawnDecision::Allow);
        assert_eq!(c.active_per_kind[9], 1, "kind_idx=10 → Other(9) として計上");
        assert_eq!(c.active_total, 1);
        // 直接 note_active(10) は無視 (範囲外を記録しない = 呼出 discipline)。
        c.note_active(10);
        assert_eq!(c.active_per_kind[9], 1, "直接 note_active(10) は no-op");
        assert_eq!(c.active_total, 1);
    }

    /// DR-3: 短絡順序の厳密 pin — 総数超過はカテゴリ予算をバイパス、
    /// 保護は計上する (保護粒子が予算を消費 = far 粒子を飢えさせうる意図設計)。
    #[test]
    fn order_bypass_and_protected_accounting_contract() {
        // 総数 2・カテゴリ 1。kind0 を 2 個 (両 budget 満杯) → 3 個目は
        // 総数超過のため 1/8 判定。id=5 (keep(5,0,8)=true) ならカテゴリ評価
        // 抜きの AllowDecimated = バイパス (id は Python FNV シムで事前選定)。
        let mut c = ParticleController::new(ParticleBudget {
            total_max: 2,
            per_kind_max: [1; 10],
            ..Default::default()
        });
        c.note_active(0);
        c.note_active(0);
        let r = ParticleRequest {
            id: 5,
            kind_idx: 0,
            pos: [10.0, 0.0, 0.0],
            velocity_mag: 0.0,
        };
        assert_eq!(
            c.allow(&r, [0.0; 3], 64.0),
            SpawnDecision::AllowDecimated,
            "総数超過 1/8 通過 → カテゴリ予算 (1) を評価せずバイパス"
        );
        // 同一状況で 1/8 非通過 (id=6) → CullTotalBudget。
        let r2 = ParticleRequest {
            id: 6,
            kind_idx: 0,
            pos: [10.0, 0.0, 0.0],
            velocity_mag: 0.0,
        };
        assert_eq!(c.allow(&r2, [0.0; 3], 64.0), SpawnDecision::CullTotalBudget);
        // 保護粒子は予算を消費する (保護のまま満杯にできる)。
        let mut p = ParticleController::new(ParticleBudget {
            total_max: 1,
            ..Default::default()
        });
        let near = ParticleRequest {
            id: 1,
            kind_idx: 0,
            pos: [1.0, 0.0, 0.0],
            velocity_mag: 0.0,
        };
        assert_eq!(
            p.allow(&near, [0.0; 3], 64.0),
            SpawnDecision::AllowProtected
        );
        assert_eq!(p.active_total, 1, "保護粒子も budget 計上する (DR-3 公表)");
    }

    /// DR-3: 距離境界 (dist==max_d はカリングしない) と NaN fail-visible。
    #[test]
    fn distance_boundary_and_nan_contract() {
        let b = ParticleBudget::default();
        // == max_d → 距離カリングしない (厳密 `>`)。Ambien (kind 6) は
        // 距離倍率 0.5: max_d = 64*0.5 = 32。
        let mut c = ParticleController::new(b);
        let r = ParticleRequest {
            id: 1,
            kind_idx: 6,
            pos: [32.0, 0.0, 0.0],
            velocity_mag: 0.0,
        };
        let d = c.allow(&r, [0.0; 3], 64.0);
        assert!(matches!(
            d,
            SpawnDecision::Allow | SpawnDecision::AllowProtected | SpawnDecision::AllowDecimated
        ));
        // ε 超過 → CullTooFar (計上されない)。
        let mut c2 = ParticleController::new(b);
        let r2 = ParticleRequest {
            id: 1,
            kind_idx: 6,
            pos: [32.0001, 0.0, 0.0],
            velocity_mag: 0.0,
        };
        assert_eq!(c2.allow(&r2, [0.0; 3], 64.0), SpawnDecision::CullTooFar);
        assert_eq!(c2.active_total, 0, "TooFar カリングは計上しない");
        // NaN 位置 → 距離比較全 false で予算評価へ進む (保護でもカリング
        // でもない、fail-visible の通常評価)。
        let mut c3 = ParticleController::new(b);
        let rn = ParticleRequest {
            id: 1,
            kind_idx: 0,
            pos: [f32::NAN, 0.0, 0.0],
            velocity_mag: 0.0,
        };
        assert_eq!(
            c3.allow(&rn, [0.0; 3], 64.0),
            SpawnDecision::Allow,
            "NaN dist ↔ 両比較 false → 予算内の通常 Allow (拒否しない)"
        );
    }

    /// DR-4: FNV-1a 間引きの厳密 pin (Python 独立シムで事前導出) と
    /// 決定的分布 (連続 id 640 個で n=8 → 厳密 80 個 = 1/8 ちょうど)。
    #[test]
    fn deterministic_keep_exact_and_distribution_pin() {
        assert!(!deterministic_keep(12345, 50, 8), "h%8=6 (tick=50)");
        assert!(!deterministic_keep(12345, 51, 8), "h%8=7 (tick=51)");
        // id 固定・tick 0..=4 の系列を厳密固定 (h は tick に敏感)。
        let series: Vec<bool> = (0..5).map(|t| deterministic_keep(12345, t, 8)).collect();
        assert_eq!(series, vec![false, false, false, false, true]);
        // n=max(1) の床: n=0/1 は常に残す。
        assert!(deterministic_keep(42, 0, 0));
        assert!(deterministic_keep(42, 0, 1));
        // 連続 id の決定的分布: 640 中 80 = 1/8 ちょうど (FNV の均質性)。
        let c8 = (0u64..640).filter(|&i| deterministic_keep(i, 0, 8)).count();
        assert_eq!(c8, 80, "n=8 で厳密に 80/640 (均質分布)");
        let c4 = (0u64..640).filter(|&i| deterministic_keep(i, 0, 4)).count();
        assert_eq!(c4, 160, "n=4 で厳密に 160/640");
    }

    /// DR-5: 既存 kind_budget_enforcement の loose `matches!` を確定値へ強化 —
    /// 2nd (id=222, tick=0) は keep(222,0,4)=false のため厳密に CullKindBudget
    /// を取る (randomness 不在なので flag 変化はない、fail-visible pin)。
    #[test]
    fn kind_budget_decimation_exact_landing() {
        let mut c = ParticleController::new(ParticleBudget {
            total_max: 10000,
            per_kind_max: [1; 10],
            ..Default::default()
        });
        let r1 = req(111);
        c.reset_counts();
        let _ = c.allow(&r1, [0.0; 3], 64.0);
        let r2 = req(222);
        assert_eq!(
            c.allow(&r2, [0.0; 3], 64.0),
            SpawnDecision::CullKindBudget,
            "keep(222,0,4)=false の決定的着地点 (FNV シム事前導出)"
        );
        // id を 1/4 通過 (keep(?,0,4)=true) 系にすると同一構成で decimate。
        // シムで選定: id=5 は keep(5,0,4)=true。
        assert!(
            deterministic_keep(5, 0, 4),
            "シム事前確認: id=5 は n=4 を通過"
        );
        let r3 = req(5);
        assert_eq!(
            c.allow(&r3, [0.0; 3], 64.0),
            SpawnDecision::AllowDecimated,
            "keep(5,0,4)=true → カテゴリ超過でも 1/4 通過"
        );
    }

    /// DR-4 (adversarial (b) で発見した検出空白の強化): total 間引きの
    /// 呼出側レート自体 (1/8) をピン化。kind 側の budget を潰した構成で
    /// 640 の連続 id を要求すると AllowDecimated icensus は厳密 80
    /// (= FNV ids 0..640 の 1/8 通過数、分布ピンと閉形式一致)。
    /// 1/16 への変化は 40 になり厳密に検出できる (当時 (b) は id=5/6 の
    /// 2 値標本では検出不能だった = 誠実記録)。
    #[test]
    fn total_budget_call_site_rate_census_pin() {
        let mut c = ParticleController::new(ParticleBudget {
            total_max: 2,
            per_kind_max: [10000; 10],
            ..Default::default()
        });
        c.note_active(0);
        c.note_active(1); // total 満杯、kind は潰れ状態で無効
        let mut decimated = 0u32;
        for i in 0..640u64 {
            let r = ParticleRequest {
                id: i,
                kind_idx: 2,
                pos: [10.0, 0.0, 0.0],
                velocity_mag: 0.0,
            };
            if c.allow(&r, [0.0; 3], 64.0) == SpawnDecision::AllowDecimated {
                decimated += 1;
            }
        }
        assert_eq!(
            decimated, 80,
            "total 満杯で 640 ids = 厳密 80 (1/8 の呼出側レート pin、ids 0..640 の FNV 集計と一致)"
        );
    }
}

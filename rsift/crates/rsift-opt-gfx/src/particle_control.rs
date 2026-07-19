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
    pub fn allow(&mut self, req: &ParticleRequest, cam: [f32; 3], max_render_distance: f32) -> SpawnDecision {
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
        ParticleRequest { id, kind_idx: 0, pos: [10.0, 0.0, 0.0], velocity_mag: 1.0 }
    }

    #[test]
    fn near_is_protected() {
        let mut c = ParticleController::new(ParticleBudget { total_max: 1, ..Default::default() });
        let r = ParticleRequest { id: 1, kind_idx: 0, pos: [1.0, 0.0, 0.0], velocity_mag: 0.0 };
        assert_eq!(c.allow(&r, [0.0; 3], 64.0), SpawnDecision::AllowProtected);
    }

    #[test]
    fn far_is_culled() {
        let mut c = ParticleController::new(ParticleBudget::default());
        let r = ParticleRequest { id: 1, kind_idx: 7, pos: [200.0, 0.0, 0.0], velocity_mag: 0.0 };
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
}

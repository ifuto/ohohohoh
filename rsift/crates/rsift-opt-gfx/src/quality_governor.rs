//! # QualityGovernor — UE DynamicRes 式「連続オーバーバジェット → panic 降段」+
//! ヒステリシス昇段の適応品質ガバナ
//!
//! 出典: Unreal Engine Dynamic Resolution
//! 「`r.DynamicRes.MaxConsecutiveOverbudgetGPUFrameCount` フレーム連続で
//!   予算超過したら即座に解像度を落とし、履歴をリセットする」。
//!
//! Rsift では解像度だけでなく複数ノブ（シャドウ解像度・描画距離・AO・粒子）
//! を段階的に昇降させる。ピンポン防止のため降段と昇段に別々の閾値と
//! クールダウンを持つ（ヒステリシス）。

/// 品質ノブ（降段順 = 視覚影響の小さい順）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityKnob {
    RenderScale,
    ShadowResolution,
    Particles,
    AmbientOcclusion,
    RenderDistance,
    Foliage,
}

impl QualityKnob {
    /// 降段優先度（小さいほど先に落とす）。
    pub fn downshift_order() -> &'static [QualityKnob] {
        &[
            QualityKnob::ShadowResolution,
            QualityKnob::RenderScale,
            QualityKnob::AmbientOcclusion,
            QualityKnob::Particles,
            QualityKnob::Foliage,
            QualityKnob::RenderDistance,
        ]
    }

    /// 各ノブの段数。
    pub fn steps(&self) -> u8 {
        match self {
            QualityKnob::RenderScale => 6,       // 100, 90, 80, 70, 60, 50%
            QualityKnob::ShadowResolution => 4,  // 2048, 1024, 512, off
            QualityKnob::Particles => 4,         // all, decreased, minimal, off
            QualityKnob::AmbientOcclusion => 3,  // full, half-res, off
            QualityKnob::RenderDistance => 8,    // 32..=5
            QualityKnob::Foliage => 3,           // fancy, fast, off
        }
    }
}

/// ノブごとの現在段（0=最高、steps-1=最低）。
#[derive(Debug, Clone, Copy)]
pub struct QualityState {
    pub levels: [u8; 6],
}

impl Default for QualityState {
    fn default() -> Self {
        Self { levels: [0; 6] }
    }
}

impl QualityState {
    fn idx(k: QualityKnob) -> usize {
        match k {
            QualityKnob::RenderScale => 0,
            QualityKnob::ShadowResolution => 1,
            QualityKnob::Particles => 2,
            QualityKnob::AmbientOcclusion => 3,
            QualityKnob::RenderDistance => 4,
            QualityKnob::Foliage => 5,
        }
    }
    pub fn level(&self, k: QualityKnob) -> u8 {
        self.levels[Self::idx(k)]
    }
    /// render scale を % で（ベンチ・UI 表示用）。
    pub fn render_scale_pct(&self) -> u32 {
        let l = self.level(QualityKnob::RenderScale) as u32;
        [100u32, 90, 80, 70, 60, 50][l.min(5) as usize]
    }
}

/// ガバナ設定。
#[derive(Debug, Clone, Copy)]
pub struct GovernorConfig {
    /// 目標フレーム・マイクロ秒（例: 60fps → 16666）
    pub target_frame_us: u32,
    /// この連続オーバー数で降段（panic 相当）
    pub max_consecutive_over: u32,
    /// この連続クリア数で昇段を検討
    pub min_consecutive_good: u32,
    /// 昇段後の降段禁止クールダウン（フレーム数）
    pub up_cooldown_frames: u32,
    /// オーバー判定の倍率（target * X を超えたらオーバー）
    pub over_factor: f32,
    /// クリア判定の倍率（target * Y 未満なら余裕あり）
    pub good_factor: f32,
}

impl Default for GovernorConfig {
    fn default() -> Self {
        Self {
            target_frame_us: 16_666,
            max_consecutive_over: 12,
            min_consecutive_good: 240,
            up_cooldown_frames: 90,
            over_factor: 1.15,
            good_factor: 0.80,
        }
    }
}

/// 適応品質ガバナ。
pub struct QualityGovernor {
    pub cfg: GovernorConfig,
    pub state: QualityState,
    consecutive_over: u32,
    consecutive_good: u32,
    cooldown: u32,
    /// 直近 N フレームの移動平均（EMA）
    ema_us: f64,
    frames: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GovernorAction {
    None,
    Downshift(QualityKnob, u8),
    Upshift(QualityKnob, u8),
}

impl QualityGovernor {
    pub fn new(cfg: GovernorConfig) -> Self {
        Self {
            ema_us: cfg.target_frame_us as f64,
            cfg,
            state: QualityState::default(),
            consecutive_over: 0,
            consecutive_good: 0,
            cooldown: 0,
            frames: 0,
        }
    }

    /// フレーム計測値を食わせて行動を返す（毎フレーム冒頭に 1 回）。
    pub fn observe(&mut self, frame_us: u32) -> GovernorAction {
        self.frames += 1;
        // EMA α=0.06（約 16 フレームで半減期）
        self.ema_us += 0.06 * (frame_us as f64 - self.ema_us);
        let target = self.cfg.target_frame_us as f64;
        let over_thresh = target * self.cfg.over_factor as f64;
        let good_thresh = target * self.cfg.good_factor as f64;

        if self.cooldown > 0 {
            self.cooldown -= 1;
        }

        if self.ema_us > over_thresh {
            self.consecutive_over += 1;
            self.consecutive_good = 0;
        } else if self.ema_us < good_thresh {
            self.consecutive_good += 1;
            self.consecutive_over = 0;
        } else {
            self.consecutive_over = 0;
            self.consecutive_good = 0;
        }

        // 降段判定（panic 降段: UE と同じく連続オーバーで即応答）
        if self.consecutive_over >= self.cfg.max_consecutive_over {
            if let Some((knob, lvl)) = self.next_downshift() {
                self.state.levels[QualityState::idx(knob)] = lvl;
                self.consecutive_over = 0;
                self.consecutive_good = 0;
                // 降段時も EMA をリセットして過去の悪さを引きずらない
                self.ema_us = frame_us as f64;
                return GovernorAction::Downshift(knob, lvl);
            }
        }

        // 昇段判定（慎重: クールダウン中は禁止、逆順で戻す）
        if self.cooldown == 0 && self.consecutive_good >= self.cfg.min_consecutive_good {
            if let Some((knob, lvl)) = self.next_upshift() {
                self.state.levels[QualityState::idx(knob)] = lvl;
                self.consecutive_good = 0;
                self.cooldown = self.cfg.up_cooldown_frames;
                return GovernorAction::Upshift(knob, lvl);
            }
        }
        GovernorAction::None
    }

    fn next_downshift(&self) -> Option<(QualityKnob, u8)> {
        for &k in QualityKnob::downshift_order() {
            let cur = self.state.level(k);
            if cur + 1 < k.steps() {
                return Some((k, cur + 1));
            }
        }
        None
    }

    fn next_upshift(&self) -> Option<(QualityKnob, u8)> {
        // 昇段は降段の逆順（最後に落としたものから戻す）
        for &k in QualityKnob::downshift_order().iter().rev() {
            let cur = self.state.level(k);
            if cur > 0 {
                return Some((k, cur - 1));
            }
        }
        None
    }

    /// ベンチ用: 現在の品質スコア（0-100, 高いほど高品質）。
    pub fn quality_score(&self) -> u32 {
        let mut total_steps = 0u32;
        let mut used = 0u32;
        for &k in QualityKnob::downshift_order() {
            total_steps += (k.steps() - 1) as u32;
            used += (k.steps() - 1 - self.state.level(k)) as u32;
        }
        if total_steps == 0 {
            100
        } else {
            (used * 100 / total_steps) as u32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downshifts_under_sustained_load() {
        let mut g = QualityGovernor::new(GovernorConfig {
            max_consecutive_over: 4,
            ..Default::default()
        });
        let mut actions = Vec::new();
        for _ in 0..40 {
            let a = g.observe(40_000); // ずっと重い (2.4x 予算)
            if a != GovernorAction::None {
                actions.push(a);
            }
        }
        assert!(!actions.is_empty(), "must downshift under sustained load");
        // 最初に落ちるのは ShadowResolution のはず
        match actions[0] {
            GovernorAction::Downshift(k, _) => {
                assert_eq!(k, QualityKnob::ShadowResolution)
            }
            _ => panic!("first action must be downshift"),
        }
        assert!(g.quality_score() < 100);
    }

    #[test]
    fn no_downshift_on_spike_only() {
        let mut g = QualityGovernor::new(GovernorConfig {
            max_consecutive_over: 8,
            ..Default::default()
        });
        // EMA が立ち上がる前に連続オーバーしない程度のスパイクだけ
        for i in 0..30 {
            let us = if i % 10 == 0 { 60_000 } else { 14_000 };
            assert_eq!(g.observe(us), GovernorAction::None);
        }
    }

    #[test]
    fn upshift_after_long_good_run_with_cooldown() {
        let mut g = QualityGovernor::new(GovernorConfig {
            max_consecutive_over: 4,
            min_consecutive_good: 20,
            up_cooldown_frames: 5,
            ..Default::default()
        });
        // まず落とす
        for _ in 0..20 {
            g.observe(40_000);
        }
        assert!(g.quality_score() < 100);
        // 長いクリア期間
        let mut ups = 0;
        for _ in 0..200 {
            if let GovernorAction::Upshift(_, _) = g.observe(8_000) {
                ups += 1;
            }
        }
        assert!(ups >= 1, "should upshift back after good run");
        assert!(g.quality_score() <= 100);
    }

    #[test]
    fn bottom_clamp_no_panic() {
        let mut g = QualityGovernor::new(GovernorConfig {
            max_consecutive_over: 2,
            ..Default::default()
        });
        for _ in 0..4000 {
            g.observe(999_999);
        }
        // 全ノブが底まで落ちても None が続くだけ（パニックしない）
        for &k in QualityKnob::downshift_order() {
            assert_eq!(g.state.level(k), k.steps() - 1);
        }
        assert_eq!(g.quality_score(), 0);
    }
}

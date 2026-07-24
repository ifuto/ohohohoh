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

/// EMA 平滑化係数 α (wave 97 CU-1: 旧インライン 0.06 を pub const 化)。
/// 半減期 = ln(0.5)/ln(1−α) ≈ 11.20 フレーム。時定数 τ = 1/α ≈ 16.67 フレーム。
/// (旧 observe() コメント「約 16 フレームで半減期」は時定数との混同:
///  n=16 では残存 (1−α)^16 = 37.16% で半減には n≈11.2 が必要 — Python f64 機械検算)
pub const EMA_ALPHA: f64 = 0.06;

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

impl GovernorConfig {
    /// wave 97 CU-2: 設定不変条件の fail-loud 検査。
    /// NaN/非有限 factor や反転帯 (good_factor ≥ over_factor) は observe 内の
    /// 大小比較を両方不成立にしてガバナを静寂沈黙させる (else 分岐で両カウンタを
    /// 恒常リセット → 降段も昇段も永久不発)。0 閾値は毎フレーム発火の病理。
    /// 構築時に根絶する (呼び出し側分岐なしの契約: QualityGovernor::new が強制)。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.target_frame_us == 0 {
            return Err("target_frame_us must be > 0");
        }
        if self.max_consecutive_over == 0 {
            return Err("max_consecutive_over must be >= 1 (0 は毎フレーム降段の病理)");
        }
        if self.min_consecutive_good == 0 {
            return Err("min_consecutive_good must be >= 1 (0 は毎フレーム昇段の病理)");
        }
        if !self.good_factor.is_finite() || !self.over_factor.is_finite() {
            return Err("over_factor/good_factor must be finite (NaN/inf は比較を全不成立化)");
        }
        if self.good_factor <= 0.0 {
            return Err("good_factor must be > 0");
        }
        if self.good_factor >= self.over_factor {
            return Err("good_factor must be < over_factor (判定帯反転の防止)");
        }
        Ok(())
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
        // wave 97 CU-2: 不変条件は構築時に fail-loud 強制 (静寂沈黙ガバナの根絶)。
        if let Err(e) = cfg.validate() {
            panic!("GovernorConfig invalid: {e}");
        }
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

    /// 累計観測フレーム数 (wave 97 CU-3: ベンチの fps/期間算出用に公開。
    /// 旧 private フィールド `frames` は書込のみで読み手ゼロだった → 消費者追加)。
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// フレーム計測値を食わせて行動を返す（毎フレーム冒頭に 1 回）。
    pub fn observe(&mut self, frame_us: u32) -> GovernorAction {
        self.frames += 1;
        // EMA α=EMA_ALPHA (半減期 ≈11.20 フレーム — CU-1 で機械ピン) (時定数≈16.67)。
        self.ema_us += EMA_ALPHA * (frame_us as f64 - self.ema_us);
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
        // 昇段は降段順の逆優先度で 1 段ずつ戻す (視覚影響の大きい
        // RenderDistance から先に)。履歴スタックではなく静的優先度の逆走査 —
        // 旧コメント「最後に落としたものから戻す」は未実装の履歴トラッキングを
        // 示唆する虚偽だったため訂正 (wave 97 CU-4、順序はテストで機械ピン)。
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
            // wave 97 CU-4: levels は pub で外部改竄し得る (level > steps-1 で
            // u8 アンダーフロー → debug panic / release ラップで score 破壊)
            // → saturating で 0..=100 の契約を堅持 (bench 経路の継続性優先)。
            used += (k.steps() - 1).saturating_sub(self.state.level(k)) as u32;
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

    /// wave 97 CU-1: 旧コメント「約 16 フレームで半減期」の誤記を f64 厳密ピンで置換。
    /// 観測 0 連続供給で EMA は 0.94^n 減衰: n=11 で 8437.97 > 8333 (半分耐え)、
    /// n=12 で 7931.69 < 8333 (半減) → 半減期は (11,12] ∈ ≈11.20 フレーム。
    /// bits は Python IEEE754 同順再現値 (recurrence の逐次丸めを忠実反映)。
    #[test]
    fn ema_half_life_bit_exact() {
        let mut g = QualityGovernor::new(GovernorConfig::default());
        for _ in 0..11 {
            g.observe(0);
        }
        assert_eq!(g.ema_us.to_bits(), 0x40c07afba3552504, "n=11 f64 bits");
        assert!(g.ema_us > 8333.0, "n=11 では半分未満に達しない");
        g.observe(0);
        assert_eq!(g.ema_us.to_bits(), 0x40befbb01e95d4f3, "n=12 f64 bits");
        assert!(g.ema_us < 8333.0, "n=12 で半減 → 半減期 ∈ (11,12] ≈ 11.20");
    }

    /// wave 97 CU-1: 降段時の EMA リセットは「最新観測値」への厳密代入
    /// (40000us 継続で n=2 から over 蓄積、max_consecutive_over=4 により
    ///  厳密に 5 回目の observe で初回降段 ShadowResolution→1)。
    #[test]
    fn downshift_resets_ema_to_last_frame_bit_exact() {
        let mut g = QualityGovernor::new(GovernorConfig {
            max_consecutive_over: 4,
            ..Default::default()
        });
        let mut first: Option<(usize, GovernorAction)> = None;
        for i in 1..=10usize {
            let a = g.observe(40_000);
            if a != GovernorAction::None {
                first = Some((i, a));
                break;
            }
        }
        let (i, a) = first.expect("must downshift under sustained load");
        assert_eq!(i, 5, "over は n=2 から蓄積し 4 連続で厳密 n=5 (機械検算)");
        match a {
            GovernorAction::Downshift(k, l) => {
                assert_eq!(k, QualityKnob::ShadowResolution);
                assert_eq!(l, 1);
            }
            _ => panic!("first action must be downshift"),
        }
        assert_eq!(
            g.ema_us.to_bits(),
            0x40e3880000000000,
            "降段後 EMA は最新観測 40000.0 への bit 厳密代入"
        );
    }

    /// wave 97 CU-2: 反転帯/NaN/非有限 factor は new() で fail-loud。
    /// (旧実装は大小比較が両方不成立 → カウンタ恒常リセットでガバナ静寂沈黙)
    #[test]
    fn config_validate_rejects_inverted_or_nan_factors() {
        for (g, o) in [
            (0.90f32, 0.80f32),    // 反転帯
            (f32::NAN, 1.15),      // NaN good
            (0.80, f32::INFINITY), // 非有限 over
            (0.0, 1.15),           // good <= 0
        ] {
            let cfg = GovernorConfig {
                good_factor: g,
                over_factor: o,
                ..Default::default()
            };
            assert!(cfg.validate().is_err(), "g={g} o={o} は拒否のはず");
            let r = std::panic::catch_unwind(|| {
                let _ = QualityGovernor::new(cfg);
            });
            assert!(r.is_err(), "new() は fail-loud のはず g={g} o={o}");
        }
        assert!(GovernorConfig::default().validate().is_ok());
    }

    /// wave 97 CU-2: 0 閾値 (毎フレーム発火の病理) と 0 target を拒否。
    #[test]
    fn config_validate_rejects_zero_thresholds() {
        let bad1 = GovernorConfig {
            max_consecutive_over: 0,
            ..Default::default()
        };
        let bad2 = GovernorConfig {
            min_consecutive_good: 0,
            ..Default::default()
        };
        let bad3 = GovernorConfig {
            target_frame_us: 0,
            ..Default::default()
        };
        for cfg in [bad1, bad2, bad3] {
            assert!(cfg.validate().is_err());
            assert!(std::panic::catch_unwind(|| {
                let _ = QualityGovernor::new(cfg);
            })
            .is_err());
        }
    }

    /// wave 97 CU-3: frames() accessor は observe 毎に厳密 +1 (未読フィールドの
    /// 消費者追加 — fps/期間算出の一次情報)。
    #[test]
    fn frames_accessor_monotone() {
        let mut g = QualityGovernor::new(GovernorConfig::default());
        assert_eq!(g.frames(), 0);
        for i in 1..=7u64 {
            g.observe(16_000);
            assert_eq!(g.frames(), i);
        }
    }

    /// wave 97 CU-4: 昇段は履歴スタックではなく静的逆優先度
    /// (RenderDistance → … → Shadow) に従うことを機械ピン。
    /// 初期レベル Shadow=3, RenderDistance=1 を直接改竄後 good 連続供給:
    /// (10,Upshift(RD,0)) → (15,Shadow→2) → (20,Shadow→1) → (25,Shadow→0) の
    /// 厳密列 (Python 全挙動シミュレーションで導出: cooldown=5 は次回 upshift
    /// まで厳密 5 フレーム間隔、good 判定は frame 8 から成立 ema<13332.80…)。
    #[test]
    fn upshift_reverse_priority_bit_exact() {
        let mut g = QualityGovernor::new(GovernorConfig {
            min_consecutive_good: 3,
            up_cooldown_frames: 5,
            ..Default::default()
        });
        g.state.levels[1] = 3; // ShadowResolution=3 (先に深く落ちた体)
        g.state.levels[4] = 1; // RenderDistance=1
        let mut ups: Vec<(u64, QualityKnob, u8)> = Vec::new();
        for i in 1..=80u64 {
            if let GovernorAction::Upshift(k, l) = g.observe(8_000) {
                ups.push((i, k, l));
            }
        }
        let want = [
            (10u64, QualityKnob::RenderDistance, 0u8),
            (15, QualityKnob::ShadowResolution, 2),
            (20, QualityKnob::ShadowResolution, 1),
            (25, QualityKnob::ShadowResolution, 0),
        ];
        assert_eq!(ups.len(), want.len(), "upshift 総数 4 のはず: {ups:?}");
        for (got, w) in ups.iter().zip(want.iter()) {
            assert_eq!(got.0, w.0, "frame 番号ピン");
            assert_eq!(got.1, w.1, "knob ピン (RD 先返上=静的逆優先度)");
            assert_eq!(got.2, w.2, "level ピン");
        }
    }

    /// wave 97 CU-4: quality_score は外部改竄 levels (level > steps-1) でも
    /// saturating で 0..=100 契約維持 (旧実装は u8 アンダーフローで debug panic)。
    /// Shadow=3, RD=1 の正規改竄時は used=5+0+3+2+6+2=18 → 18*100/22=81。
    #[test]
    fn quality_score_saturates_on_tampered_levels() {
        let mut g = QualityGovernor::new(GovernorConfig::default());
        g.state.levels = [200; 6]; // 全面改竄 (最大 steps=8 を全超過)
        assert_eq!(g.quality_score(), 0, "saturating で底張り付き (panic なし)");
        let mut g2 = QualityGovernor::new(GovernorConfig::default());
        g2.state.levels[1] = 3;
        g2.state.levels[4] = 1;
        assert_eq!(g2.quality_score(), 81, "used=18, total=22 → 81 (機械検算)");
    }
}

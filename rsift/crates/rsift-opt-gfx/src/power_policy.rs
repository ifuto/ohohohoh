//! Dynamic FPS 逆輸入 — 非フォーカス/最小化/放置時の電力ポリシー。
//!
//! Dynamic FPS の中核は「フォーカス状態 + 放置タイマー + ヒステリシス」順に
//! ターゲット FPS を落とすこと。切替チャタリングを防ぐため上下の閾値を分ける。
//!
//! 【wave 154 EZ 監査注記】
//! 1. **捕捉 70 [中] (`render` 常時 true の定数退化)**: 旧 `mode_tick` は
//!    全経路 `FrameDecision{render: true}` 固定返却で、wiring の
//!    `report.power_skip_extra = !power_decision.render` は恒 false の
//!    全沈黙観測面だった (描画 skip の真判定 = `frame_gate` が配線では
//!    legacy `tick_frame` の `let _` 破棄のみ + 捕捉 62 同型 ms 誤供給)。
//!    根治: `render` フィールド削除 + wiring `tick_world` で frame_gate
//!    を秒化 (`delta_ms/1000.0`) 実駆動し skip 判定を一本化 (§7 消化 17)。
//! 2. **捕捉 69 [中] (ヒステリシス機構欠落)**: doc 明記「上下の閾値を
//!    分ける」「アイドル解除のヒステリシス (fps)。上方向へ抜けるには
//!    target 差がこれ以上必要」に対し `hysteresis_fps` は dead field で
//!    実装に閾値機構が存在しなかった。根治: 上向き (target 上昇) 遷移で
//!    差 < hysteresis_fps (双方非無制限) なら mode 保留、境界差 == 閾値は
//!    反映 (「これ以上」)、0=無制限との出入りは差定義外で即反映、下向き
//!    は閾値非適用 (TDD RED 1 機械記録、残 3 規則は新旧一致の構造的緑)。
//! 3. **捕捉 68 [小] (`with_frame_dt(_dt)` no-op fake API)**: mode_tick が
//!    計算した frame_dt を捨てる装飾 method だった → FrameDecision に
//!    `frame_dt` を真保持 (target=0=無制限 → 0.0 契約)。**smoothed_dt
//!    dead state**: new で 1/60 初期化後に一切更新されなかった →
//!    frame_gate 呼出毎に EMA (α=0.1、real_dt 秒契約) で真更新 (TDD RED
//!    1)、getter は wiring `report.power_smoothed_dt` へ配線 (全値
//!    delta_ms 系列のみ由来 = 完全決定的、det subset 登録が正当)。
//! 4. **契約注記**: (a) real_dt は秒契約 (need=1/target_fps [s]、旧 legacy
//!    tick_frame は ms 誤供給=捕捉 62 同型、wiring 秒化済)。(b) EMA α=0.1
//!    は定数選択 (重い平滑側)、gate の ε=1e-5 は f32 蓄積誤差吸収用
//!    (4·(1/120)==1/30 bits 一致区間では不発 = rq ez_pp 機械確認)。(c)
//!    `on_input` は wiring で GC-3 の camera_dir 変化検出点に真接続済
//!    (Idle 解除が実入力駆動)。(d) hysteresis は default limits では遷移
//!    差 ≥ 13 > 閾値 4 で非活性 (構造注記、実効回路は custom limits で
//!    strict pin)。
//! 5. **`tick_frame` (wiring legacy) 不在化**、`mode()` getter 削除
//!    (FrameDecision::mode と冗長、消費者ゼロ census)。
//!    `power_frame_dt`/`power_mode` は wall-clock 由来 mode 経路の値なので
//!    det subset には登録しない (power_skip_extra と同グループ、wiring
//!    test の Active 内 golden で pin)。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerMode {
    /// フォーカスあり・入力あり
    Active,
    /// フォーカス喪失 (alt-tab 等)
    Unfocused,
    /// 最小化/画面オフ
    Minimized,
    /// フォーカスあり + 入力無しが連続
    Idle,
}

#[derive(Debug, Clone, Copy)]
pub struct PowerLimits {
    pub active_fps: u32,
    pub unfocused_fps: u32,
    pub minimized_fps: u32,
    pub idle_fps: u32,
    /// 入力無し→アイドル移行までの秒
    pub idle_after_secs: f32,
    /// アイドル解除のヒステリシス (fps)。上方向へ抜けるには target 差がこれ以上必要
    pub hysteresis_fps: u32,
}

impl Default for PowerLimits {
    fn default() -> Self {
        Self {
            active_fps: 0, // 0=制限なし
            unfocused_fps: 15,
            minimized_fps: 1,
            idle_fps: 30,
            idle_after_secs: 60.0,
            hysteresis_fps: 4,
        }
    }
}

pub struct PowerPolicy {
    limits: PowerLimits,
    mode: PowerMode,
    last_input_time: f64,
    smoothed_dt: f32,
    battery_saver: bool,
}

impl PowerPolicy {
    pub fn new(limits: PowerLimits) -> Self {
        Self {
            limits,
            mode: PowerMode::Active,
            last_input_time: 0.0,
            smoothed_dt: 1.0 / 60.0,
            battery_saver: false,
        }
    }

    pub fn set_battery_saver(&mut self, enabled: bool) {
        self.battery_saver = enabled;
    }

    pub fn on_input(&mut self, now_secs: f64) {
        self.last_input_time = now_secs;
    }

    /// フレームごとに呼ぶ。モードを更新して target FPS 判決を返す。
    /// 描画 skip の真判定は `frame_gate` (捕捉 70 根治で一本化)。
    pub fn mode_tick(&mut self, now_secs: f64, focused: bool, minimized: bool) -> FrameDecision {
        let raw_mode = if minimized {
            PowerMode::Minimized
        } else if !focused {
            PowerMode::Unfocused
        } else if now_secs - self.last_input_time > self.limits.idle_after_secs as f64 {
            PowerMode::Idle
        } else {
            PowerMode::Active
        };

        // EZ-2 (捕捉 69 根治): doc 明記のヒステリシスを真実装 — 上向き
        // (target 上昇) の mode 遷移は差が hysteresis_fps 未満なら保留
        // (チャタリング防止)。下向きは閾値非適用、0=無制限との出入りは
        // 差定義外として即反映 (注記 4d: default では非活性)。
        if raw_mode != self.mode {
            let cur = self.fps_for(self.mode);
            let new = self.fps_for(raw_mode);
            let hold = cur != 0 && new != 0 && new > cur && new - cur < self.limits.hysteresis_fps;
            if !hold {
                self.mode = raw_mode;
            }
        }

        let target = self.current_target_fps();
        let frame_dt = if target == 0 {
            0.0 // 0=無制限契約 (捕捉 68 根治: frame_dt 真保持)
        } else {
            1.0f32 / target as f32
        };
        FrameDecision {
            target_fps: target,
            mode: self.mode,
            frame_dt,
        }
    }

    /// 実測 dt (秒契約) を与えると次フレームを描画するかを決める (貯金法)。
    /// 溜めた時間が目標 dt に達してから描画。余剰分は次に持ち越し。
    /// EZ-3: 呼出毎に smoothed_dt を EMA (α=0.1) 真更新 (dead state 根治)。
    pub fn frame_gate(&mut self, accumulated_dt: &mut f32, real_dt: f32, target_fps: u32) -> bool {
        self.smoothed_dt = 0.9 * self.smoothed_dt + 0.1 * real_dt;
        if target_fps == 0 || self.mode == PowerMode::Active {
            *accumulated_dt = 0.0;
            return true;
        }
        *accumulated_dt += real_dt;
        let need = 1.0f32 / target_fps as f32;
        if *accumulated_dt + 1e-5 >= need {
            // 余剰は残すが、暴走時 (長いスタッター) はリセット
            *accumulated_dt = (*accumulated_dt - need).min(need);
            true
        } else {
            false
        }
    }

    pub fn current_target_fps(&self) -> u32 {
        self.fps_for(self.mode)
    }

    fn fps_for(&self, mode: PowerMode) -> u32 {
        let mut fps = match mode {
            PowerMode::Active => self.limits.active_fps,
            PowerMode::Unfocused => self.limits.unfocused_fps,
            PowerMode::Minimized => self.limits.minimized_fps,
            PowerMode::Idle => self.limits.idle_fps,
        };
        if self.battery_saver && (fps == 0 || fps > 30) {
            fps = 30;
        }
        fps
    }

    pub fn smoothed_dt(&self) -> f32 {
        self.smoothed_dt
    }
}

/// mode_tick の判決。描画 skip 判定は mode だけでは決まらない (frame_gate
/// が真判定、捕捉 70 根治で render フィールドは削除)。
#[derive(Debug, Clone, Copy)]
pub struct FrameDecision {
    pub target_fps: u32,
    pub mode: PowerMode,
    /// 【捕捉 68 根治】1/target_fps [s] (旧 with_frame_dt no-op の真保持化)。
    /// target=0=無制限 → 0.0 契約。
    pub frame_dt: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unfocused_caps_fps() {
        let mut p = PowerPolicy::new(PowerLimits::default());
        p.on_input(0.0);
        let d = p.mode_tick(10.0, false, false);
        assert_eq!(d.mode, PowerMode::Unfocused);
        assert_eq!(p.current_target_fps(), 15);
    }

    #[test]
    fn idle_after_timeout() {
        let mut p = PowerPolicy::new(PowerLimits::default());
        p.on_input(0.0);
        let d = p.mode_tick(120.0, true, false);
        assert_eq!(d.mode, PowerMode::Idle);
        assert_eq!(p.current_target_fps(), 30);
    }

    #[test]
    fn gate_accumulates() {
        let mut p = PowerPolicy::new(PowerLimits::default());
        p.on_input(0.0);
        p.mode_tick(120.0, true, false); // Idle → 30fps
        let mut acc = 0.0f32;
        let dt = 1.0 / 120.0;
        let mut rendered = 0;
        for _ in 0..12 {
            if p.frame_gate(&mut acc, dt, 30) {
                rendered += 1;
            }
        }
        // 12 frames @120Hz ≒ 3 renders @30Hz
        assert!((2..=4).contains(&rendered), "{rendered}");
    }

    #[test]
    fn active_zero_unlimited() {
        let mut p = PowerPolicy::new(PowerLimits::default());
        p.on_input(0.0);
        p.mode_tick(1.0, true, false);
        let mut acc = 999.0f32;
        assert!(p.frame_gate(&mut acc, 0.001, 0));
    }

    /// 【wave 154 EZ-2 捕捉 69 TDD RED】doc 明記「上下の閾値を分ける/上方向
    /// へ抜けるには target 差がこれ以上必要」のヒステリシスが実装に存在
    /// しない (hysteresis_fps は dead field)。custom limits で上向き差 3 <
    /// 閾値 4 の遷移は**保留** (mode 維持) が正しい振る舞い。
    #[test]
    fn hysteresis_holds_upward_step_strict() {
        let limits = PowerLimits {
            active_fps: 0,
            unfocused_fps: 13,
            minimized_fps: 10,
            idle_fps: 30,
            idle_after_secs: 60.0,
            hysteresis_fps: 4,
        };
        let mut p = PowerPolicy::new(limits);
        p.on_input(0.0);
        let d1 = p.mode_tick(1.0, false, true); // Minimized (10)
        assert_eq!(d1.mode, PowerMode::Minimized);
        let d2 = p.mode_tick(2.0, true, false); // raw=Active… ではなく focused かつ last_input 経過 1.0 < 60 → Active
                                                // Active は無制限 (0) → 即反映 (注記: unlimited 出入りは保留しない)
        assert_eq!(d2.mode, PowerMode::Active);
        // Unfocused(13)→Minimized(10): 下向き → 即反映
        let d3 = p.mode_tick(3.0, false, false);
        assert_eq!(d3.mode, PowerMode::Unfocused);
        let d4 = p.mode_tick(4.0, false, true);
        assert_eq!(d4.mode, PowerMode::Minimized, "下向き 13→10 は即反映");
        // Minimized(10)→Unfocused(13): 上向き差 3 < 閾値 4 → **保留** (捕捉 69 RED 点)
        let d5 = p.mode_tick(5.0, false, false);
        assert_eq!(
            d5.mode,
            PowerMode::Minimized,
            "上向き差 3 < hysteresis 4 → mode 保留 (doc ヒステリシス、旧実装は即反映で RED)"
        );
    }

    /// 【wave 154 EZ-2】境界規則 pin: 差 == 閾値 → doc「これ以上」⇒ 反映。
    /// 下向きは閾値非適用で即反映 (rq ez_pp (5))。
    #[test]
    fn hysteresis_boundary_equal_releases_pin() {
        let limits = PowerLimits {
            active_fps: 0,
            unfocused_fps: 14,
            minimized_fps: 10,
            idle_fps: 30,
            idle_after_secs: 60.0,
            hysteresis_fps: 4,
        };
        let mut p = PowerPolicy::new(limits);
        p.on_input(0.0);
        p.mode_tick(1.0, false, true); // Minimized (10)
        let d = p.mode_tick(2.0, false, false); // 上向き差 14−10=4 == 閾値
        assert_eq!(
            d.mode,
            PowerMode::Unfocused,
            "差 == 閾値 → 反映 (「これ以上」)"
        );
    }

    /// 【wave 154 EZ-2】無制限 (0) との出入りは差定義外 → 即反映 pin。
    #[test]
    fn hysteresis_unlimited_pass_through_pin() {
        let mut p = PowerPolicy::new(PowerLimits::default());
        p.on_input(0.0);
        p.mode_tick(1.0, false, true); // Minimized (1)
        let d = p.mode_tick(2.0, true, false); // → Active (0=無制限)
        assert_eq!(d.mode, PowerMode::Active, "無制限への出入りは即反映");
    }

    /// 【wave 154 EZ-3 捕捉 71 系 TDD RED】smoothed_dt は new 後に一切更新
    /// されない dead state だった (getter のみ、消費者ゼロ)。EMA
    /// s ← 0.9·s + 0.1·real_dt (α=0.1、real_dt 秒契約) の真更新へ:
    /// s0=1/60 → dt=0.016 で s1=0x3C87FCBA、s2=0x3C877EE6 (rq ez_pp)。
    #[test]
    fn smoothed_dt_ema_converges_strict() {
        let mut p = PowerPolicy::new(PowerLimits::default());
        p.on_input(0.0);
        p.mode_tick(120.0, true, false); // Idle
        let mut acc = 0.0f32;
        let dt = 16.0f32 / 1000.0;
        p.frame_gate(&mut acc, dt, 30);
        assert_eq!(p.smoothed_dt().to_bits(), 0x3C87_FCBAu32, "s1 (rq ez_pp)");
        p.frame_gate(&mut acc, dt, 30);
        assert_eq!(p.smoothed_dt().to_bits(), 0x3C87_7EE6u32, "s2 (rq ez_pp)");
    }

    /// 【wave 154 EZ-3】貯金法の厳密値 pin (rq ez_pp (3)): dt=1/120×12 で
    /// rendered==3 かつ acc 残 0.0 exact (4·(1/120) == 1/30 bits 一致、
    /// ε=1e-5 は含まずとも成立する区間)。
    #[test]
    fn gate_accumulates_exact_count_strict() {
        let mut p = PowerPolicy::new(PowerLimits::default());
        p.on_input(0.0);
        p.mode_tick(120.0, true, false); // Idle → 30fps
        let mut acc = 0.0f32;
        let dt = 1.0f32 / 120.0;
        let mut rendered = 0;
        for _ in 0..12 {
            if p.frame_gate(&mut acc, dt, 30) {
                rendered += 1;
            }
        }
        assert_eq!(rendered, 3, "12@120Hz → 3@30Hz exact");
        assert_eq!(acc.to_bits(), 0, "acc 残 0 exact (rq ez_pp)");
    }

    /// 【wave 154 EZ-1 捕捉 68 TDD】frame_dt 真保持の golden (rq ez_pp (2)):
    /// Unfocused(15) → 1/15 = 0x3D888889、Minimized(1) → 1.0、
    /// Active(0=無制限) → 0.0 契約。旧 with_frame_dt(_dt) no-op では
    /// フィールド自体が不在 (compile RED 記録)。
    #[test]
    fn frame_dt_truthful_golden_strict() {
        let mut p = PowerPolicy::new(PowerLimits::default());
        p.on_input(0.0);
        let d = p.mode_tick(10.0, false, false); // Unfocused (15)
        assert_eq!(d.frame_dt.to_bits(), 0x3D88_8889u32, "1/15 (rq ez_pp)");
        assert_eq!(d.target_fps, 15);
        let d = p.mode_tick(11.0, false, true); // Minimized (1)
        assert_eq!(d.frame_dt.to_bits(), 0x3F80_0000u32, "1/1 = 1.0 exact");
        let d = p.mode_tick(12.0, true, false); // Active (0=無制限)
        assert_eq!(d.frame_dt.to_bits(), 0u32, "無制限 → 0.0 契約");
    }

    /// 【wave 154 EZ-1 捕捉 70 根治 pin】FrameDecision に render 不在 =
    /// mode_tick は mode/target のみ返し、skip 判定は frame_gate が担う
    /// (構造的一本化、wiring gate 実駆動の module 側契約)。
    #[test]
    fn gate_is_sole_skip_decider_contract_pin() {
        let mut p = PowerPolicy::new(PowerLimits::default());
        p.on_input(0.0);
        p.mode_tick(120.0, true, false); // Idle (30)
        let mut acc = 0.0f32;
        // need=1/30≈0.0333 に対し 0.016 は 1・2 回で未達 (0.032<1/30)、
        // 3 回 (0.048) で到達 (rq ez_pp (6): 私の初 pin 2 回は暗算誤りで
        // rq 訂正 — 数値手繰り禁止規律どおり機械確認)。
        assert!(!p.frame_gate(&mut acc, 0.016, 30), "1 回未達");
        assert!(!p.frame_gate(&mut acc, 0.016, 30), "2 回未達 (0.032<1/30)");
        assert!(p.frame_gate(&mut acc, 0.016, 30), "3 回到達 (0.048>=1/30)");
    }

    /// 【wave 154 EZ-3】暴走 (長スタッター) clamp pin: acc=1.0 到達時は
    /// min(余剰, need) で貯金 1 枚分に頭打ち (0.966…>need → need、rq (4))。
    #[test]
    fn gate_stutter_clamps_acc_strict() {
        let mut p = PowerPolicy::new(PowerLimits::default());
        p.on_input(0.0);
        p.mode_tick(120.0, true, false); // Idle → 30fps (need=1/30)
        let mut acc = 0.0f32;
        assert!(p.frame_gate(&mut acc, 1.0, 30), "巨 dt は即描画");
        assert_eq!(
            acc.to_bits(),
            0x3D08_8889u32,
            "acc = min(1.0−1/30, 1/30) = 1/30 = 0x3D088889 (rq ez_pp)"
        );
    }
}

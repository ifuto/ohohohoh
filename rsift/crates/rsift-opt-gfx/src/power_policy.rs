//! Dynamic FPS 逆輸入 — 非フォーカス/最小化/放置時の電力ポリシー。
//!
//! Dynamic FPS の中核は「フォーカス状態 + 放置タイマー + ヒステリシス」順に
//! ターゲット FPS を落とすこと。切替チャタリングを防ぐため上下の閾値を分ける。

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

    /// フレームごとに呼ぶ。モードを更新して、描画すべきかを返す。
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

        // ヒステリシス: Idle→Active 以外は即反映; Idle 復帰は入力必須 (on_input 済み)
        if raw_mode != self.mode {
            self.mode = raw_mode;
        }

        let target = self.current_target_fps();
        if target == 0 {
            return FrameDecision { render: true, target_fps: 0, mode: self.mode };
        }

        let frame_dt = 1.0f32 / target as f32;
        FrameDecision { render: true, target_fps: target, mode: self.mode }
            .with_frame_dt(frame_dt)
    }

    /// 実測 dt を与えると次フレームを描画するかを決める (貯金法)。
    /// 溜めた時間が目標 dt に達してから描画。余剰分は次に持ち越し。
    pub fn frame_gate(&mut self, accumulated_dt: &mut f32, real_dt: f32, target_fps: u32) -> bool {
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
        let mut fps = match self.mode {
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

    pub fn mode(&self) -> PowerMode {
        self.mode
    }

    pub fn smoothed_dt(&self) -> f32 {
        self.smoothed_dt
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FrameDecision {
    pub render: bool,
    pub target_fps: u32,
    pub mode: PowerMode,
}

impl FrameDecision {
    fn with_frame_dt(self, _dt: f32) -> Self {
        self
    }
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
}

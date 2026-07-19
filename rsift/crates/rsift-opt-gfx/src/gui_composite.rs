//! Exordium 逆輸入 — 3D は高 FPS のまま、GUI/HUD を別レートのオフスクリーン面で描く。
//!
//! 中核はデュアルレート・スケジューラ:
//! - GUI 面は `gui_fps` (既定 30) でだけ再描画
//! - 入力イベントやアニメ時計があればそのフレームで即無効化 (入力遅延ゼロ)
//! - 3D 側の始終は毎フレーム走り、GUI 面は「前回キャッシュを再合成」で済ませる

#[derive(Debug, Clone, Copy)]
pub struct GuiRates {
    pub gui_fps: f32,
    /// アニメ CSV (hotbar scroll 等) の持続中に無効化する最大 FPS
    pub anim_burst_fps: f32,
}

impl Default for GuiRates {
    fn default() -> Self {
        Self { gui_fps: 30.0, anim_burst_fps: 60.0 }
    }
}

pub struct GuiCompositeClock {
    rates: GuiRates,
    gui_accum: f32,
    last_gui_render_time: f64,
    /// input invalidation カウンタ (このフレームに 1 回以上の入力イベント)
    pending_invalidations: u32,
    anim_active_until: f64,
    surface_valid: bool,
}

impl GuiCompositeClock {
    pub fn new(rates: GuiRates) -> Self {
        Self {
            rates,
            gui_accum: 0.0,
            last_gui_render_time: 0.0,
            pending_invalidations: 0,
            anim_active_until: 0.0,
            surface_valid: false,
        }
    }

    /// マウス移動・クリック・キー入力時に呼ぶ (GUI 遅延を生まないため即時無効化)。
    pub fn on_input_event(&mut self) {
        self.pending_invalidations += 1;
    }

    /// ホットバースクロールやチャットフェードのような「時間駆動アニメ」が
    /// 起きてる間は anim_until を延長する (Exordium の animated hijack 相当)。
    pub fn on_animation_window(&mut self, now: f64, secs: f32) {
        self.anim_active_until = (now + secs as f64).max(self.anim_active_until);
    }

    /// フレーム毎の判定。true = このフレームで GUI オフスクリーン面を再描画。
    pub fn should_render_gui(&mut self, now: f64, real_dt: f32) -> GuiDecision {
        let animating = now < self.anim_active_until;
        let target_fps = if animating {
            self.rates.anim_burst_fps
        } else {
            self.rates.gui_fps
        };

        self.gui_accum += real_dt;
        let need = 1.0f32 / target_fps.max(1.0);

        let invalidated_now = self.pending_invalidations > 0;
        let due = self.gui_accum >= need;

        if !self.surface_valid || invalidated_now || due {
            self.gui_accum = if due { self.gui_accum - need } else { 0.0 };
            if self.gui_accum > need {
                self.gui_accum = 0.0; // スタッター暴走防止
            }
            self.pending_invalidations = 0;
            self.surface_valid = true;
            self.last_gui_render_time = now;
            return GuiDecision {
                render_gui_surface: true,
                reuse_cached_scene: true,
                fps_now: target_fps,
                invalidated: invalidated_now,
            };
        }

        GuiDecision {
            render_gui_surface: false,
            reuse_cached_scene: true,
            fps_now: target_fps,
            invalidated: false,
        }
    }

    /// 合成段で GUI 面のアルファ (フェード用)。
    pub fn surface_valid(&self) -> bool {
        self.surface_valid
    }

    pub fn invalidate(&mut self) {
        self.surface_valid = false;
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GuiDecision {
    pub render_gui_surface: bool,
    pub reuse_cached_scene: bool,
    pub fps_now: f32,
    pub invalidated: bool,
}

/// timescale に関係なく GUI 面を主画面へ貼る際の矩形 (ピクセル精确)。
#[derive(Debug, Clone, Copy)]
pub struct GuiBlit {
    pub src_size: (u32, u32),
    pub dst_size: (u32, u32),
}

impl GuiBlit {
    /// 計算領域を保ったままスケール値だけ返す (= cost: 画面対応 1 draw)。
    pub fn scale(&self) -> (f32, f32) {
        (
            self.dst_size.0 as f32 / self.src_size.0.max(1) as f32,
            self.dst_size.1 as f32 / self.src_size.1.max(1) as f32,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steady_state_is_lowrate() {
        let mut c = GuiCompositeClock::new(GuiRates::default());
        // 初期 invalid + 30fps: 120Hz で走ると GUI 描画は ~1/4
        let dt = 1.0 / 120.0;
        let mut renders = 0;
        let mut now = 0.0;
        for _ in 0..120 {
            now += dt as f64;
            if c.should_render_gui(now, dt).render_gui_surface {
                renders += 1;
            }
        }
        assert!((28..=33).contains(&renders), "{renders}");
    }

    #[test]
    fn input_invalidates_immediately() {
        let mut c = GuiCompositeClock::new(GuiRates::default());
        let dt = 1.0 / 120.0;
        c.should_render_gui(0.0, dt); // valid
        c.on_input_event();
        let d = c.should_render_gui(dt as f64, dt);
        assert!(d.render_gui_surface);
        assert!(d.invalidated);
    }

    #[test]
    fn animation_window_boosts_rate() {
        let mut c = GuiCompositeClock::new(GuiRates::default());
        c.on_animation_window(0.0, 0.5);
        let d = c.should_render_gui(0.001, 1.0 / 120.0);
        assert_eq!(d.fps_now, 60.0);
    }
}

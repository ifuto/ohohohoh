//! Exordium 逆輸入 — 3D は高 FPS のまま、GUI/HUD を別レートのオフスクリーン面で描く。
//!
//! 中核はデュアルレート・スケジューラ:
//! - GUI 面は `gui_fps` (既定 30) でだけ再描画
//! - 入力イベントがあればそのフレームで即無効化 (入力遅延ゼロ)
//! - 3D 側の始終は毎フレーム走り、GUI 面は「前回キャッシュを再合成」で済ませる
//!
//! 【wave 149 GC-5 誠実注記 5 項】
//! 1. 捕捉 62 [高]: 旧 wiring:542 は `real_dt` (秒契約) に `inputs.delta_ms`
//!    (ms = 16.0) を誤供給していた。gui_accum が 480 倍速で蓄積され
//!    `gui_accum >= need` が毎 tick 真となり、30fps デュアルレート機構は
//!    wiring 経由では構造的に全沈黙 (毎フレーム再描画判定) だった。旧来は
//!    decision が `let _gui_decision` で破棄され観測経路が無かったため
//!    実害は潜在化していたが、§7 配線と同時根治しないと「配線した途端に
//!    省電力機構が無効化状態で実害化」する二重構造だった (call site で
//!    `delta_ms / 1000.0` 秒化に根治、`delta_ms` 名称は ms の一次情報)。
//! 2. anim 系 (GuiRates::anim_burst_fps / on_animation_window /
//!    anim_active_until) の削除証明: Exordium 由来の「時間駆動 GUI アニメ
//!    中のバーストレート」機構だが、FrameWiringInputs にホットバー
//!    スクロール等の GUI アニメ事件源が存在せず、census grep で wiring・
//!    本番・テストの全消費者ゼロを機械確定。wiring から虚偽イベントを
//!    捏造する配線は【偽装禁止】に抵触するため、不可能証明の上で削除
//!    (§7 「接続か削除か」の後者)。`now: f64` 引数は anim 比較と dead
//!    store `last_gui_render_time` への代入が唯一の消費地だったため、
//!    両者の削除に伴い引数ごと撤去 — GUI 時計は inputs のみ駆動の完全
//!    決定的機構となった (wall-clock 非依存、det subset 登録が正当)。
//! 3. GuiBlit/scale の削除証明: GUI オフスクリーン面の実体 (src 寸法) は
//!    本 crate が管理しておらず src/dst の実データ源が wiring に存在
//!    しない (census 全消費者ゼロ機械確定)。同一 sizes の退化配線は
//!    lattice 退化と異なり値が恒等 1.0 の偽装になるため削除で処置。
//! 4. 実駆動設計: `on_input_event` は camera_dir 変化 (視点操作=入力駆動)
//!    を wiring が prev 照合で実検出して駆動、`invalidate()` は
//!    screen_w/h 変化 (= GUI 面破棄の実事象) で駆動、`surface_valid()`
//!    は report.gui_surface_valid 観測面が真の消費地。dead store だった
//!    `last_gui_render_time` は「GUI 面年齢」配線には wall-clock `now`
//!    が必要で決定性汚染のため設計不能 → read ゼロの census 証明で削除。
//! 5. 契約注記: `fps_now` は生レート報告 (need の `max(1.0)` 底上げは
//!    内部判定のみに効き fps_now には反映されない)。`real_dt <= 0` や
//!    NaN は accum にそのまま蓄積・伝播する (due は比較 false で静寂
//!    不発、fail-loud しない設計: タイミング系は観測経路で咎める)。
//!    anti-stutter (render 後 accum > need → 0 リセット) は大スタッター
//!    時の追従を 1 周期分までに制限する意図的上限 (0.05 溜りでは残存、
//!    0.1 溜りでリセット — rq gc_gui 機械確定)。

#[derive(Debug, Clone, Copy)]
pub struct GuiRates {
    pub gui_fps: f32,
}

impl Default for GuiRates {
    fn default() -> Self {
        Self { gui_fps: 30.0 }
    }
}

pub struct GuiCompositeClock {
    rates: GuiRates,
    gui_accum: f32,
    /// input invalidation カウンタ (このフレームに 1 回以上の入力イベント)
    pending_invalidations: u32,
    surface_valid: bool,
}

impl GuiCompositeClock {
    pub fn new(rates: GuiRates) -> Self {
        Self {
            rates,
            gui_accum: 0.0,
            pending_invalidations: 0,
            surface_valid: false,
        }
    }

    /// マウス移動・クリック・キー入力時に呼ぶ (GUI 遅延を生まないため即時無効化)。
    /// wiring では camera_dir 変化の実検出から駆動される (GC-4 注記 4)。
    pub fn on_input_event(&mut self) {
        self.pending_invalidations += 1;
    }

    /// フレーム毎の判定。`real_dt` は **秒** (捕捉 62: ms 供給で機構沈黙の前科)。
    /// GuiDecision.render_gui_surface = このフレームで GUI オフスクリーン面を再描画。
    pub fn should_render_gui(&mut self, real_dt: f32) -> GuiDecision {
        let target_fps = self.rates.gui_fps;

        self.gui_accum += real_dt;
        let need = 1.0f32 / target_fps.max(1.0);

        let invalidated_now = self.pending_invalidations > 0;
        let due = self.gui_accum >= need;

        if !self.surface_valid || invalidated_now || due {
            self.gui_accum = if due { self.gui_accum - need } else { 0.0 };
            if self.gui_accum > need {
                self.gui_accum = 0.0; // スタッター暴走防止 (注記 5)
            }
            self.pending_invalidations = 0;
            self.surface_valid = true;
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

    /// 合成段で GUI 面のアルファ (フェード用)。wiring では
    /// report.gui_surface_valid 観測面が消費地 (GC-2)。
    pub fn surface_valid(&self) -> bool {
        self.surface_valid
    }

    /// GUI 面の外部破棄 (wiring では screen_w/h 変化の実検出から駆動、GC-4)。
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steady_state_is_lowrate() {
        let mut c = GuiCompositeClock::new(GuiRates::default());
        // 初期 invalid + 30fps: 120Hz で走ると GUI 描画は ~1/4
        let dt = 1.0 / 120.0;
        let mut renders = 0;
        for _ in 0..120 {
            if c.should_render_gui(dt).render_gui_surface {
                renders += 1;
            }
        }
        assert!((28..=33).contains(&renders), "{renders}");
    }

    #[test]
    fn input_invalidates_immediately() {
        let mut c = GuiCompositeClock::new(GuiRates::default());
        let dt = 1.0 / 120.0;
        c.should_render_gui(dt); // valid
        c.on_input_event();
        let d = c.should_render_gui(dt);
        assert!(d.render_gui_surface);
        assert!(d.invalidated);
    }

    // ---- wave 149 GC-6 strict 群 (全値 rq gc_gui 事前導出) ----

    /// 補間系の golden: dt=0.02 での実系列。render t ∈ {1,3,5,7,8,10}
    /// (10 tick 頭出し、rq gc_gui 確定 — t7→t8 連続 render は accum 残りが
    /// 境界 gap 2^-27 を跨ぐ f32 蓄積の非自明挙動、単純交互では再現不可)。
    #[test]
    fn gc_gui_carry_series_dt20_golden() {
        const EXPECT: [bool; 10] = [
            true, false, true, false, true, false, true, true, false, true,
        ];
        let mut c = GuiCompositeClock::new(GuiRates::default());
        for (i, &exp) in EXPECT.iter().enumerate() {
            let d = c.should_render_gui(0.02);
            assert_eq!(d.render_gui_surface, exp, "tick {} (rq gc_gui)", i + 1);
        }
    }

    /// invalidated カウンタは 1 回の判定で消費され、accum は due=false
    /// 経路で 0.0 に正規化される (翌 tick 非 due)。複数イベントは 1 回に畳込。
    #[test]
    fn gc_gui_invalidated_consumes_counter() {
        let mut c = GuiCompositeClock::new(GuiRates::default());
        let dt = 1.0 / 120.0;
        c.should_render_gui(dt); // valid 化
        c.on_input_event();
        c.on_input_event();
        c.on_input_event();
        let d = c.should_render_gui(dt);
        assert!(
            d.invalidated,
            "3 連イベントも 1 回の invalidated として消費"
        );
        let d2 = c.should_render_gui(dt);
        assert!(!d2.invalidated, "カウンタは消費済み");
        assert!(
            !d2.render_gui_surface,
            "accum=0 起点 + dt=1/120 では非 due (rq)"
        );
    }

    /// need 底上げ契約: gui_fps=0 → need=1.0 (max(1.0)) で殆ど due せず、
    /// fps_now は生レート (0.0) を報告する (注記 5 の契約 pin)。
    #[test]
    fn gc_gui_fps_floor_contract() {
        let mut c = GuiCompositeClock::new(GuiRates { gui_fps: 0.0 });
        let mut renders = 0;
        let mut fps0 = false;
        for _ in 0..10 {
            let d = c.should_render_gui(0.016);
            if d.render_gui_surface {
                renders += 1;
            }
            fps0 |= d.fps_now == 0.0;
        }
        assert_eq!(renders, 1, "need=1.0 では 10 tick で t1 のみ (rq)");
        assert!(fps0, "fps_now は生レート報告 (内部 need clamp 非反映)");
    }

    /// NaN 契約: gui_fps=NaN → max 規律で need=1.0 (f32::max は NaN を
    /// もう一方へ流す)・real_dt=NaN は accum を永久汚染 (due 不発継続)。
    /// 共に fail-loud しない静寂設計の誠実 pin (注記 5)。
    #[test]
    fn gc_gui_nan_propagation_contract() {
        let mut c = GuiCompositeClock::new(GuiRates { gui_fps: f32::NAN });
        let d0 = c.should_render_gui(0.016);
        assert!(d0.render_gui_surface, "t1 は invalid 表面で render");
        assert!(d0.fps_now.is_nan(), "fps_now 生 NaN 伝播");
        let mut renders = 0;
        for _ in 0..10 {
            if c.should_render_gui(0.016).render_gui_surface {
                renders += 1;
            }
        }
        assert_eq!(renders, 0, "NaN need → 内部 max で 1.0 → 非 due 継続");

        let mut c2 = GuiCompositeClock::new(GuiRates::default());
        c2.should_render_gui(0.016); // valid 化
        let dn = c2.should_render_gui(f32::NAN);
        assert!(
            !dn.render_gui_surface,
            "NaN accum → due 比較 false 静寂不発"
        );
        let d_after = c2.should_render_gui(0.016);
        assert!(
            !d_after.render_gui_surface,
            "accum=NaN は後続 tick でも不発 (伝播)"
        );
    }

    /// anti-stutter 境界 (rq gc_gui): 大 dt=10.0 の spike では render 後
    /// accum を 0 リセット (0.1 級の溜りは破棄)。リセットの観測は後続
    /// dt=0.016 系列が 0 起点になること (t1 後 2 回非 due、3 回目 due:
    /// 0.032f32=0x3D03126F < 0x3D088889 の 2 tick 目境界、rq 機械確定)。
    #[test]
    fn gc_gui_anti_stutter_reset() {
        let mut c = GuiCompositeClock::new(GuiRates::default());
        assert!(c.should_render_gui(0.016).render_gui_surface, "t1 valid 化");
        assert!(
            c.should_render_gui(10.0).render_gui_surface,
            "spike で render"
        );
        const EXPECT: [bool; 3] = [false, false, true];
        for (i, &exp) in EXPECT.iter().enumerate() {
            assert_eq!(
                c.should_render_gui(0.016).render_gui_surface,
                exp,
                "spike 後オフセット tick {} (rq: accum=0 起点系列)",
                i + 1
            );
        }
    }

    /// GuiRates Default 契約: (30.0 = 0x41F00000)・reuse_cached_scene は
    /// module 仕様上常時 true (wiring 契約 pin の根拠)。
    #[test]
    fn gc_gui_default_and_reuse_contract() {
        let d = GuiRates::default();
        assert_eq!(d.gui_fps.to_bits(), 0x41F0_0000, "30.0 exact (rq)");
        let mut c = GuiCompositeClock::new(d);
        for _ in 0..6 {
            assert!(c.should_render_gui(0.016).reuse_cached_scene);
        }
    }
}

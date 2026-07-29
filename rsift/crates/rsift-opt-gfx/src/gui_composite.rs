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
//!
//! 【wave 189 GI (2026-07-29)】メイン画面/ポーズ画面 (3D でない GUI) の
//! 描画高速化 = 適応静止低レート化 (`GuiAdaptive`, 既定 enabled):
//! 入力/面破棄からの連続再描画回数 `still` が 45 超で 30→12、135 超で
//! 12→4 fps へ段階退化 (u32 整数比較で確定的、f32 丸め非介在)。15 s
//! アイドルの GUI 面再描画は 750 → 199 (73.5% 削減、probe 機械確定)。
//! 入力・面委棄は判定フレーム自身で即時フルレート復帰 (0 フレーム遅延、
//! 入力即時無効化保証は不変)。`fps_now` はそのフレームの実効レートを
//! 報告 (注記 5 の生レート報告からの意図的変更 — 44 render 未満の既存
//! corpus 領域では値不変を gi_adaptive_full_rate_below_threshold で
//! 機査立証済)。閾値/分母は `GuiAdaptive` で変更可能 (既定は gui_fps=30
//! が厳密に 12.0/4.0 になる値)。

#[derive(Debug, Clone, Copy)]
pub struct GuiRates {
    pub gui_fps: f32,
    /// 【wave 189 GI】静止画面 (メイン/ポーズ等の入力不在区間) の適応
    /// デュアルレート。`adaptive.enabled = false` で wave 149 挙動と完全一致。
    pub adaptive: GuiAdaptive,
}

/// 静止判定閾値: `still` = 最後の入力/面破棄からの連続 GUI 再描画回数。
/// `still < hold_full` でフルレート、`hold_mid` まで mid、以降 floor。
/// 分母は既定 gui_fps=30 が厳密に 12.0/4.0 になる値 (2.5/7.5、probe gi_probe
/// 機械確定)。15s アイドルの GUI 再描画回数は 750 → 199 (73.5% 削減)。
#[derive(Debug, Clone, Copy)]
pub struct GuiAdaptive {
    pub enabled: bool,
    pub hold_full: u32,
    pub hold_mid: u32,
    pub mid_div: f32,
    pub floor_div: f32,
}

impl Default for GuiAdaptive {
    fn default() -> Self {
        Self {
            enabled: true,
            hold_full: 45,
            hold_mid: 135,
            mid_div: 2.5,
            floor_div: 7.5,
        }
    }
}

impl Default for GuiRates {
    fn default() -> Self {
        Self {
            gui_fps: 30.0,
            adaptive: GuiAdaptive::default(),
        }
    }
}

pub struct GuiCompositeClock {
    rates: GuiRates,
    gui_accum: f32,
    /// input invalidation カウンタ (このフレームに 1 回以上の入力イベント)
    pending_invalidations: u32,
    surface_valid: bool,
    /// 【wave 189 GI】最後の入力/面破棄からの連続再描画回数 (静止度)。
    still: u32,
}

impl GuiCompositeClock {
    pub fn new(rates: GuiRates) -> Self {
        Self {
            rates,
            gui_accum: 0.0,
            pending_invalidations: 0,
            surface_valid: false,
            still: 0,
        }
    }

    /// マウス移動・クリック・キー入力時に呼ぶ (GUI 遅延を生まないため即時無効化)。
    /// wiring では camera_dir 変化の実検出から駆動される (GC-4 注記 4)。
    pub fn on_input_event(&mut self) {
        self.pending_invalidations += 1;
    }

    /// フレーム毎の判定。`real_dt` は **秒** (捕捉 62: ms 供給で機構沈黙の前科)。
    /// GuiDecision.render_gui_surface = このフレームで GUI オフスクリーン面を再描画。
    ///
    /// 【wave 189 GI】適応静止低レート化: `still` (入力/面破棄からの連続再
    /// 描画回数) が閾値を超えると目標レートを段階的に落とす (30 → 12 → 4)。
    /// メイン画面/ポーズ画面の静止区間で GUI 面の再描画回数を ~73.5% 削減
    /// (probe gi_probe: 15 s = 3000 tick @120 Hz で 750 → 199)。入力があれば
    /// **判定フレーム自身で即時にフルレートへ復帰** (0 フレーム遅延、
    /// 既存の入力即時無効化保証を維持)。`fps_now` はそのフレームの実効
    /// 目標レートを報告する (注記 5 の「生レート報告」からの意図的変更:
    /// wiring 既存 pin が担保する 44 render 未満の領域では値は不変、
    /// 機械検証済)。threshold は f32 比較を介さず u32 整数比較で確定的。
    pub fn should_render_gui(&mut self, real_dt: f32) -> GuiDecision {
        let gui_fps = self.rates.gui_fps;
        let invalidated_now = self.pending_invalidations > 0;
        let fresh = !self.surface_valid;
        let a = &self.rates.adaptive;
        let target_fps = if !a.enabled || invalidated_now || fresh {
            gui_fps // 入力 or 面委棄 = 即時復帰 (適応一時解除)
        } else if self.still < a.hold_full {
            gui_fps
        } else if self.still < a.hold_mid {
            gui_fps / a.mid_div
        } else {
            gui_fps / a.floor_div
        };

        self.gui_accum += real_dt;
        let need = 1.0f32 / target_fps.max(1.0);

        let due = self.gui_accum >= need;

        if !self.surface_valid || invalidated_now || due {
            self.gui_accum = if due { self.gui_accum - need } else { 0.0 };
            if self.gui_accum > need {
                self.gui_accum = 0.0; // スタッター暴走防止 (注記 5)
            }
            self.pending_invalidations = 0;
            let was_fresh = !self.surface_valid;
            self.surface_valid = true;
            if was_fresh || invalidated_now {
                self.still = 0; // 新規面 or 入力 = 静止カウンタ リセット
            } else {
                self.still = self.still.saturating_add(1);
            }
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
        let mut c = GuiCompositeClock::new(GuiRates {
            gui_fps: 0.0,
            ..Default::default()
        });
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
        let mut c = GuiCompositeClock::new(GuiRates {
            gui_fps: f32::NAN,
            ..Default::default()
        });
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

    // ---- wave 189 GI strict 群 (全値 probe gi_probe 機械確定) ----

    /// 静止画面の適応低レート化 golden: dt=1/120 で 3000 tick (15 s) 入力
    /// なし → render 199/750 (73.5% 削減)、fps は 30 / 12 / 4 に段階退化。
    /// render #44/#45/#46 = t173/177/181 (fps 30、render 序列の絶対 tick)、
    /// 初の fps=12.0 render は t191、初の fps=4.0 render は t1111、
    /// 件数内訳は 30.x=46・12.x=90・4.x=63、最終 render は t2971
    /// (probe gi_probe [1] 機械確定、60 要素 Vec 走査で O(n) 完遂)。
    #[test]
    fn gi_adaptive_static_degrade_golden() {
        let dt = 1.0f32 / 120.0;
        let mut c = GuiCompositeClock::new(GuiRates::default());
        let mut renders: Vec<(usize, f32)> = Vec::new();
        for t in 1..=3000usize {
            let d = c.should_render_gui(dt);
            if d.render_gui_surface {
                renders.push((t, d.fps_now));
            }
        }
        assert_eq!(renders.len(), 199, "15 s = 199 renders (probe 確定)");
        assert_eq!(renders[43], (173, 30.0), "render #44 (probe)");
        assert_eq!(renders[44], (177, 30.0), "render #45 (probe)");
        assert_eq!(renders[45], (181, 30.0), "render #46 (probe)");
        let first12 = renders.iter().find(|&&(_, f)| f == 12.0).copied();
        let first4 = renders.iter().find(|&&(_, f)| f == 4.0).copied();
        assert_eq!(first12, Some((191, 12.0)), "first 12 fps render (probe)");
        assert_eq!(first4, Some((1111, 4.0)), "first 4 fps render (probe)");
        let n30 = renders.iter().filter(|&&(_, f)| f == 30.0).count();
        let n12 = renders.iter().filter(|&&(_, f)| f == 12.0).count();
        let n4 = renders.iter().filter(|&&(_, f)| f == 4.0).count();
        assert_eq!((n30, n12, n4), (46, 90, 63), "fps 内訳 (probe)");
        assert_eq!(
            renders.last().copied(),
            Some((2971, 4.0)),
            "最終 render (probe)"
        );
        // fps ビット厳密 (12.0=0x41400000・4.0=0x40800000、python 機械値)
        assert_eq!(12.0f32.to_bits(), 0x4140_0000);
        assert_eq!(4.0f32.to_bits(), 0x4080_0000);
    }

    /// 入力で即時フルレート復帰 golden: tick 2000 で on_input_event →
    /// 判定 t=2000 で即座に f=30.0・invalidated=true・render=true (0 フレーム
    /// 遅延)、t=2004 で再 render (再 30 fps 周期の立上がり)、総 render 294
    /// (probe gi_probe [2] 機械確定)。
    #[test]
    fn gi_adaptive_input_restore_zero_latency_golden() {
        let dt = 1.0f32 / 120.0;
        let mut c = GuiCompositeClock::new(GuiRates::default());
        let mut renders = 0usize;
        let mut log: Vec<(usize, bool, f32, bool)> = Vec::new();
        for t in 1..=3000usize {
            if t == 2000 {
                c.on_input_event();
            }
            let d = c.should_render_gui(dt);
            if d.render_gui_surface {
                renders += 1;
            }
            if (1998..=2006).contains(&t) {
                log.push((t, d.render_gui_surface, d.fps_now, d.invalidated));
            }
        }
        assert_eq!(renders, 294, "入力 1 回込みの 15 s render 数 (probe)");
        let want: [(usize, bool, f32, bool); 9] = [
            (1998, false, 4.0, false),
            (1999, false, 4.0, false),
            (2000, true, 30.0, true),
            (2001, false, 30.0, false),
            (2002, false, 30.0, false),
            (2003, false, 30.0, false),
            (2004, true, 30.0, false),
            (2005, false, 30.0, false),
            (2006, false, 30.0, false),
        ];
        assert_eq!(log, want, "入力前後 9 tick の判定系列 (probe gi [2])");
    }

    /// 既存 corpus 非破壊証明: render 44 件未満では fps_now は常に
    /// gui_fps 定数 (wave 149 挙動と完全一致、短 tick 列 pin の保証域)。
    #[test]
    fn gi_adaptive_full_rate_below_threshold() {
        let dt = 1.0f32 / 120.0;
        let mut c = GuiCompositeClock::new(GuiRates::default());
        let mut containers = Vec::new();
        for t in 1..=188usize {
            let d = c.should_render_gui(dt);
            if d.render_gui_surface {
                containers.push((t, d.fps_now));
            }
        }
        assert_eq!(containers.len(), 46, "46 render までは 30 fps 域 (probe)");
        let all30 = containers.iter().all(|&(_, f)| f == 30.0);
        assert!(all30, "46 render 以内は fps_now が常に 30.0");
    }

    /// adaptive enabled=false で wave 149 挙動と完全一致 (legacy 経路 pin)。
    #[test]
    fn gi_adaptive_disabled_is_legacy_identical() {
        let dt = 1.0f32 / 120.0;
        let mut c = GuiCompositeClock::new(GuiRates {
            gui_fps: 30.0,
            adaptive: GuiAdaptive {
                enabled: false,
                ..Default::default()
            },
        });
        let mut renders = 0usize;
        for _ in 1..=3000usize {
            if c.should_render_gui(dt).render_gui_surface {
                renders += 1;
            }
        }
        assert_eq!(renders, 750, "legacy 30 fps 固定 = 750 (probe [3])");
    }

    /// invalidate() による委棄でも still カウンタとレートが即リセット
    /// (委棄 = 面の再構築事由 = 即時フルレートで再描画開始)。
    /// probe gi_probe_extra 確定: t2001 render (fps 30・still=0)、以後
    /// 30 fps 周期で t2005/2009/2013/2017 に render (20 tick 中 5 回)。
    /// 【wave 189 自己照査】初版実装は委棄後レート復帰を持たず、本 pin
    /// が TDD RED として検出 → probe による系列確定後に根治。
    #[test]
    fn gi_adaptive_invalidate_surface_restores() {
        let dt = 1.0f32 / 120.0;
        let mut c = GuiCompositeClock::new(GuiRates::default());
        for _ in 1..=2000usize {
            c.should_render_gui(dt);
        }
        c.invalidate();
        let mut renders_in_window: Vec<usize> = Vec::new();
        for t in 2001..=2020usize {
            let d = c.should_render_gui(dt);
            if d.render_gui_surface {
                assert_eq!(d.fps_now.to_bits(), 0x41F0_0000, "t{t}: 30 fps (bits)");
                renders_in_window.push(t);
            }
        }
        assert_eq!(
            renders_in_window,
            vec![2001, 2005, 2009, 2013, 2017],
            "委棄後 20 tick の render tick 系列 (probe gi_extra 機械確定)"
        );
    }
}

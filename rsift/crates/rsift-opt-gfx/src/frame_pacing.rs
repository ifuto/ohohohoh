//! Frame pacing — 提示タイミングを vsync 境界に揃えてジャンク（カクつき）を消去。
//!
//! 内蔵GPUでは vsync に合わせて提示しないと描画が無駄になり発熱するだけなので重要。
//! また EMA でフレーム時間を平滑化し、スパイクによる誤った解像度判断を防ぐ。
//!
//! 【wave 161 FG (2026-07-28)】消費者: `FullGraphWiring` の Pacing 帳簿
//! (`new`/`record_frame`/`next_present_time`/`smoothed_frame_ms`/
//! `target_refresh_hz`) → report 実フィールド 3 件、WGSL marker は
//! `gpu_runtime::all_wgsl_sources` へ `wgsl_source()` 経由で登録。
//! 捕捉 90 [小]: 消費者完全ゼロの `FramePacing` unit-struct 完全装飾
//! (`wgsl_source(&self)`) を tbdr_hints EL-1 判例で free fn へ根治
//! (EJ-2 Vec3/Vec4 完全装飾削除と同型)。捕捉 91 [小]: §7 消化 23 で
//! 消費者ゼロだった vsync snap/平滑値へ Pacing 帳簿の実消費者を配線し、
//! `refresh_hz` を private+getter へ (pub 全書込み可能フィールドは
//! interval キャッシュとの不整合を許す契約逸脱口)。
//!
//! 【wave 186 GF (2026-07-29)】捕捉 128 [中]: `next_present_time` が
//! 「now = 計算済み第 k 境界」の丸め窓で 1 インターバル全分の提示スキップ
//! を返し得た段階丸めを、n 基準再計算へ根治 (probe gf_probe2/3 機械確定、
//! 67116 corpus 中 2574 件 + 段階加算ドリフト 4 件、60Hz でも k=62 等)。
//! 併せて loose pin 3 件を golden bit 化: (a2) 第 1 補正ループ発火窓
//! (t_k+1ulp・商下側丸め) pin、(b) EMA α module 厳密 golden。(a) ceil→floor
//! の wave 161「構造吸収」記述は機械訂正: 旧実装は bit 発散 4422/67116 で
//! golden 不在の loose 事例 (検出可能だった)、根治後は n 基準の単一計算
//! 単位で ceil≡floor の真の observational equivalence を probe 証明
//! (67116 corpus 発散 0)。power_policy 側にも ε 窓 pin (EZ (e) 回収)。

pub struct FramePacer {
    /// 目標リフレッシュレート (Hz)。private 化 (wave 161 FG 捕捉 91):
    /// pub フィールドのままでは構築後の外部書換えで `frame_interval`
    /// キャッシュと恒久的に不整合となり得た (契約逸脱口)。更新経路は
    /// 持たない設計のため、読取りは `target_refresh_hz` に限定する。
    refresh_hz: f64,
    frame_interval: f64,
    smoothed_ms: f64,
    alpha: f64,
}

impl FramePacer {
    /// `refresh_hz` は有限正であること (= frame interval が有限正に定まる契約)。
    /// 0・負・NaN では `next_present_time` が旧実装の逐次加算ループで
    /// 無限ループに陥ったため、契約として明示拒否する (fail-loud)。
    pub fn new(refresh_hz: f64) -> Self {
        let interval = 1000.0 / refresh_hz;
        assert!(
            interval.is_finite() && interval > 0.0,
            "FramePacer: refresh_hz must yield a finite positive frame interval (got {refresh_hz})"
        );
        Self {
            refresh_hz,
            frame_interval: interval,
            smoothed_ms: interval,
            alpha: 0.2,
        }
    }

    /// EMA でフレーム時間を平滑化（ノイズ除去）。
    /// 非有限 (NaN/±inf) の観測値は欠測として捨てる — 1 回の異常値で
    /// 平滑値が永久に NaN 汚染されることを防ぐ。
    pub fn record_frame(&mut self, frame_ms: f64) {
        if !frame_ms.is_finite() {
            return;
        }
        self.smoothed_ms += self.alpha * (frame_ms - self.smoothed_ms);
    }

    /// 次の提示時刻を vsync 境界にスナップ（ジャンク防止）。
    /// `last_present`, `now` はミリ秒。
    ///
    /// 除算で境界を直接推定し ±1 ステップの端数補正で「`now` 以上の最早境界」を
    /// 得るため O(1) で返る。旧実装は逐次加算ループで、ギャップが interval 比
    /// ~6e10 (タイマーリセット直後等) の入力で実質ハングした。
    /// `now` が NaN の場合は旧実装と同じく `last_present + interval` に帰着
    /// する (比較が全て false となる堕落形)。
    pub fn next_present_time(&self, last_present: f64, now: f64) -> f64 {
        let interval = self.frame_interval;
        let mut n = ((now - last_present) / interval).ceil();
        if !(n >= 1.0) {
            n = 1.0; // NaN や負ギャップは旧ループと同一の帰着 (last + 1 interval)
        }
        // 浮動小数点の端数ズレだけを n 基準で補正 (高々 2 回ずつで収束)。
        // 【wave 186 GF 捕捉 128】旧実装は段階加算 (`t += interval`) と減算
        // guard (`t - interval >= now`) の二重丸めで、「now が計算済み第 k
        // 境界そのもの」の丸め窓 (例: 23.976Hz k≡0 mod 3, 60Hz k=62 等) に
        // 落ちると 1 インターバル全分の提示スキップ (約 16.7ms@60Hz) を返し
        // 得た (probe gf_probe2/3 機械確定: corpus 67116 中 2574 件)。
        // 各反復で `last + n*interval` を再計算して段階丸めの累積を排除し、
        // 出力を「計算済み境界の最早もの」に厳密化する (新旧差分は全て
        // スキップ解消方向 = 遅れることはない、probe 機械検証済)。
        while last_present + n * interval < now {
            n += 1.0;
        }
        while n > 1.0 && last_present + (n - 1.0) * interval >= now {
            n -= 1.0;
        }
        last_present + n * interval
    }

    pub fn smoothed_frame_ms(&self) -> f64 {
        self.smoothed_ms
    }

    /// 目標リフレッシュレート (Hz、構築時検証済みの不変値)。
    pub fn target_refresh_hz(&self) -> f64 {
        self.refresh_hz
    }
}

/// WGSL 取得の唯一の公式アクセスポイント (wave 161 FG 捕捉 90)。
/// 旧 `FramePacing` unit struct ラッパ (`wgsl_source(&self)`) は crate 全体で
/// 消費者完全ゼロ (新指令 §7「消費者なし禁止」違反) で、状態を持たず
/// `&self` を使わない装飾メソッドだったため削除 (tbdr_hints EL-1 /
/// async_compute EJ-2 Vec3/Vec4 完全装飾削除と同型)。ddgi/tbdr_hints/
/// shadow_lod と同じ free fn 様式に統一し、gpu_runtime::all_wgsl_sources
/// の登録を本関数経由に一本化した。
pub fn wgsl_source() -> &'static str {
    FRAME_PACING_WGSL
}
pub const FRAME_PACING_WGSL: &str = include_str!("../shaders/frame_pacing.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_to_vsync_boundary() {
        let p = FramePacer::new(60.0); // 16.666.. ms
        let np = p.next_present_time(0.0, 10.0);
        assert!((np - 1000.0 / 60.0).abs() < 1e-3);
    }

    #[test]
    fn never_presents_in_past() {
        let p = FramePacer::new(60.0);
        let np = p.next_present_time(100.0, 1000.0);
        assert!(np >= 1000.0);
        let steps = ((np - 100.0) / p.frame_interval).round();
        assert!(steps >= 1.0);
    }

    #[test]
    fn smoothing_reduces_spike() {
        let mut p = FramePacer::new(60.0);
        p.record_frame(16.6);
        p.record_frame(16.6);
        p.record_frame(50.0); // spike
        let s = p.smoothed_frame_ms();
        assert!(s < 50.0 && s > 16.0, "smoothed={}", s);
    }

    /// 旧実装では ~6e10 回ループして実質ハングした巨大ギャップでも O(1) で
    /// 「最早の未来境界」に到達すること。
    #[test]
    fn huge_gap_is_constant_time_and_earliest() {
        let p = FramePacer::new(60.0);
        let i = 1000.0 / 60.0;
        let now = 1e12; // last=0 から interval 比 ~6e10
        let t = p.next_present_time(0.0, now);
        assert!(t >= now, "must not present in the past: {t}");
        assert!(t - i < now, "must be the earliest boundary: {t}");
    }

    /// 最早境界性の不変条件を決定的乱数で掃引: 結果は常に [now, now+interval)
    /// の半開区間に入る (最早 vsync 境界の一意特徴付け)。
    #[test]
    fn earliest_boundary_invariant_sweep() {
        let p = FramePacer::new(59.94); // 切りの悪い Hz (interval が 2 進で割り切れない)
        let i = p.frame_interval;
        let mut seed = 0x9E3779B97F4A7C15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..2000 {
            let last = (next() % 100_000) as f64 * 0.01; // 0..1000 ms
            let now = last + (next() % 40_000) as f64 * 0.01; // gap 0..400 ms (~24 intervals)
            let t = p.next_present_time(last, now);
            assert!(t >= now, "past present: t={t} now={now}");
            assert!(t - i < now, "not earliest: t={t} now={now} i={i}");
        }
    }

    /// 【wave 161 FG 捕捉 90】wgsl は設計上シェーダを持ち得ない正当 marker
    /// (mip_streaming wave 43 様式): naga parse 可能・entry point / global
    /// 変数ゼロを機械ピン。GPU は提示のスケジュールを行えない (提出自体が
    /// 既に提示であり自己参照) ので registry 内で唯一「将来もシェーダを
    /// 持ち得ない」ことの宣言。
    #[test]
    fn wgsl_is_intentionally_shader_free_marker() {
        let module = naga::front::wgsl::parse_str(FRAME_PACING_WGSL).expect("marker must parse");
        assert!(
            module.entry_points.is_empty(),
            "CPU 計時モジュールにシェーダは要らない (設計正当性は wgsl コメント参照)"
        );
        assert!(module.global_variables.iter().next().is_none());
    }

    /// 【wave 161 FG】負ギャップ (now < last) は strictly-after-last 語彙で
    /// n=1 帰着 (last 自体の境界は要求しない)。rq fg_pacing (3) + 実機
    /// probe: 200 + 50/3 = 216.666.. = 0x406b155555555555。
    #[test]
    fn negative_gap_returns_strictly_after_last() {
        let p = FramePacer::new(60.0);
        let t = p.next_present_time(200.0, 100.0);
        assert_eq!(t.to_bits(), 0x406b155555555555, "216.666.. (probe bits)");
        assert!(t > 200.0 && t >= 100.0, "strictly-after-last で now 以上");
    }

    /// 非有限の観測値は平滑値を汚染しない (bit 不変)。
    #[test]
    fn record_frame_ignores_non_finite() {
        let mut p = FramePacer::new(60.0);
        p.record_frame(16.6);
        let before = p.smoothed_frame_ms();
        p.record_frame(f64::NAN);
        p.record_frame(f64::INFINITY);
        p.record_frame(f64::NEG_INFINITY);
        assert_eq!(p.smoothed_frame_ms().to_bits(), before.to_bits());
    }

    /// NaN な now は旧実装と同じく「次の 1 境界」に帰着する (堕落形)。
    #[test]
    fn nan_now_falls_back_to_single_next_boundary() {
        let p = FramePacer::new(60.0);
        let t = p.next_present_time(100.0, f64::NAN);
        assert_eq!(t, 100.0 + 1000.0 / 60.0);
    }

    /// interval が有限正に定まらない refresh_hz は契約拒否。
    #[test]
    #[should_panic(expected = "finite positive")]
    fn new_rejects_non_positive_refresh() {
        let _ = FramePacer::new(0.0);
    }

    #[test]
    #[should_panic(expected = "finite positive")]
    fn new_rejects_nan_refresh() {
        let _ = FramePacer::new(f64::NAN);
    }

    /// 【wave 185 GE フェーズ2 回収】dead code 系 10 例目 (wave 161 FG adversarial (e)
    /// 装飾 struct 再救出 非検出、FG-1 で unit struct+装飾メソッドを EL-1 判例で free fn
    /// 一本化削除済、tbdr_hints.rs:56-58 同型判例) の lexeme pin 化。
    /// 同宣言形の将来復活を静寂に通さない。
    #[test]
    fn ge_removed_frame_pacing_unit_lexeme() {
        let src = include_str!("frame_pacing.rs");
        for lex in [
            concat!("struct ", "FramePacing"),
            concat!("impl ", "FramePacing"),
        ] {
            assert!(
                !src.contains(lex),
                "dead code 系削除語彙の宣言形復活を検出 (wave 185 GE lexeme pin)"
            );
        }
    }

    /// 【wave 186 GF 捕捉 128 TDD】now が「計算済み第 k 境界そのもの」に一致する
    /// 入力では、答えはその境界自身でなければならない (最早境界の一意性)。
    /// 旧実装は ceil 推定後の段階減算 guard (`t - interval >= now`) の二重丸めで
    /// この丸め窓に落ち、1 インターバル全分 (約 16.7ms@60Hz・41.7ms@23.976Hz) の
    /// 提示スキップを返した — 例: 23.976Hz k=27 で 0x4091988127350B89 ではなく
    /// 0x40923F568779614B (= last + 28·interval の段階加算系) を返していた
    /// (probe gf_probe2/3 機械確定、corpus 67116 中 2574 件、60Hz でも k=62 等)。
    /// golden bits は根治版 (n 基準再計算、wave 186 GF-1) の機械値。
    #[test]
    fn gf_boundary_exact_returns_computed_boundary_strict() {
        // 23.976Hz: interval = 1001/24 の f64
        let p = FramePacer::new(23.976);
        let i = p.frame_interval;
        assert_eq!(i.to_bits(), 0x4044_DAAC_088A_B856, "interval bits (probe)");
        for (k, want_now) in [
            (27u32, 0x4091_9881_2735_0B89u64),
            (30u32, 0x4093_8D01_4802_0CD1u64),
            (99u32, 0x40B0_2121_0E9B_4A93u64),
        ] {
            let now = (k as f64) * i; // 第 k 計算済み境界 (bits 一致を仕込む)
            assert_eq!(now.to_bits(), want_now, "k={k}: 境界 bits (probe)");
            let t = p.next_present_time(0.0, now);
            assert_eq!(
                t.to_bits(),
                now.to_bits(),
                "k={k}: now が計算済み境界なら答えはその境界自身 (捕捉 128 丸め窓スキップ拒否)"
            );
        }
        // 60Hz (default 経路, last=0) k=62: 旧実装は 0x4090680000000000
        // (1 interval スキップ) を返した (probe gf_probe3 機械確定)。
        let p60 = FramePacer::new(60.0);
        let i60 = p60.frame_interval;
        let now60 = 62.0f64 * i60;
        assert_eq!(
            now60.to_bits(),
            0x4090_2555_5555_5556,
            "60Hz k=62 境界 bits (probe)"
        );
        let t60 = p60.next_present_time(0.0, now60);
        assert_eq!(
            t60.to_bits(),
            now60.to_bits(),
            "60Hz k=62 でも境界自身を返す (1 インターバル全分スキップ拒否)"
        );
    }

    /// 【wave 186 GF (a2) 回収】第 1 補正ループ (undershoot 側) の発火 bit 域を
    /// pin 化 — wave 161 FG adversarial (a2) は「現 corpus 外防御」として非検出
    /// だったが、発火窓は `now = t_k + 1 ulp` かつ商丸め下側 (probe gf_probe2
    /// (ii) 機械特定)。loop1 除去変異で t = 0x40B62856C91363DB < now に退行し
    /// 本 pin が RED 化することを adversarial で実証済み。
    /// golden bits は根治版 (n 基準再計算) の機械値 (k=136, k=139)。
    #[test]
    fn gf_loop1_undershoot_window_strict() {
        let p = FramePacer::new(23.976);
        let i = p.frame_interval;
        for (k, y_bits, want_t) in [
            (136u32, 0x40B6_2856_C913_63DBu64, 0x40B6_520C_2124_794Cu64),
            (139u32, 0x40B6_A576_D146_A42Du64, 0x40B6_CF2C_2957_B99Eu64),
            (34u32, 0x4096_2856_C913_63DBu64, 0x4096_CF2C_2957_B99Eu64),
        ] {
            let y = f64::from_bits(y_bits);
            assert_eq!(y, (k as f64) * i, "k={k}: 第 k 境界と一致 (設計前提)");
            let now = f64::from_bits(y.to_bits() + 1); // t_k + 1 ulp
            let t = p.next_present_time(0.0, now);
            assert!(
                t >= now,
                "k={k}: 提示は未来になければならない (loop1 除去変異はここで RED)"
            );
            assert_eq!(
                t.to_bits(),
                want_t,
                "k={k}: 次境界 t_{{k+1}} の厳密 bits (probe gf_probe3 機械値)"
            );
        }
    }

    /// 【wave 186 GF (b) 回収】EMA α=0.2 の module 厳密 golden — wave 161 FG
    /// adversarial (b) α 0.2→0.5 は wiring bit pin のみ捕捉 (module 区間検査は
    /// 検出不能と分類) だった loose 例を module golden bit 化で回収。
    /// α 変異で s1/s2 双方が別 bits (probe gf_probe [2]: α=0.5 →
    /// s1'=0x4030A22222222222, s2'=0x4040A88888888888) となり本 pin が RED。
    #[test]
    fn gf_module_ema_golden_bits_strict() {
        let mut p = FramePacer::new(60.0);
        assert_eq!(
            p.smoothed_frame_ms().to_bits(),
            0x4030_AAAA_AAAA_AAAB,
            "初期値 = interval (1000/60, probe)"
        );
        p.record_frame(16.6);
        assert_eq!(
            p.smoothed_frame_ms().to_bits(),
            0x4030_A740_DA74_0DA8,
            "s1 = s0 + 0.2·(16.6 − s0) (probe gf_probe [2] 機械値)"
        );
        p.record_frame(50.0);
        assert_eq!(
            p.smoothed_frame_ms().to_bits(),
            0x4037_529A_485C_D7BA,
            "s2 = s1 + 0.2·(50 − s1) (probe gf_probe [2] 機械値)"
        );
    }
}

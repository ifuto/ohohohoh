//! Frame pacing — 提示タイミングを vsync 境界に揃えてジャンク（カクつき）を消去。
//!
//! 内蔵GPUでは vsync に合わせて提示しないと描画が無駄になり発熱するだけなので重要。
//! また EMA でフレーム時間を平滑化し、スパイクによる誤った解像度判断を防ぐ。

pub struct FramePacer {
    pub refresh_hz: f64,
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
        let mut t = last_present + n * interval;
        // 浮動小数点の端数ズレだけを補正 (高々 2 回ずつで収束)。
        while t < now {
            t += interval;
        }
        while n > 1.0 && t - interval >= now {
            t -= interval;
            n -= 1.0;
        }
        t
    }

    pub fn smoothed_frame_ms(&self) -> f64 {
        self.smoothed_ms
    }
}

pub struct FramePacing;
impl FramePacing {
    pub fn wgsl_source(&self) -> &'static str {
        FRAME_PACING_WGSL
    }
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
}

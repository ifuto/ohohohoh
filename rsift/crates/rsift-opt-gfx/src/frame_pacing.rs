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
    pub fn new(refresh_hz: f64) -> Self {
        let interval = 1000.0 / refresh_hz;
        Self {
            refresh_hz,
            frame_interval: interval,
            smoothed_ms: interval,
            alpha: 0.2,
        }
    }

    /// EMA でフレーム時間を平滑化（ノイズ除去）。
    pub fn record_frame(&mut self, frame_ms: f64) {
        self.smoothed_ms += self.alpha * (frame_ms - self.smoothed_ms);
    }

    /// 次の提示時刻を vsync 境界にスナップ（ジャンク防止）。
    /// `last_present`, `now` はミリ秒。
    pub fn next_present_time(&self, last_present: f64, now: f64) -> f64 {
        let mut t = last_present + self.frame_interval;
        while t < now {
            t += self.frame_interval;
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
}

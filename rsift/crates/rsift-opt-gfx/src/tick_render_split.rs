//! Fixed 20 TPS tick clock + render interpolation (Tier 3).

use std::time::Instant;

pub const TICK_HZ: f64 = 20.0;
pub const TICK_DT: f64 = 1.0 / TICK_HZ;

#[derive(Debug)]
pub struct FixedTickClock {
    started: Instant,
    accumulator: f64,
    tick_count: u64,
    max_catchup: u32,
}

impl FixedTickClock {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            accumulator: 0.0,
            tick_count: 0,
            max_catchup: 10, // avoid spiral of death on hitch
        }
    }

    pub fn with_max_catchup(mut self, n: u32) -> Self {
        self.max_catchup = n.max(1);
        self
    }

    /// Advance with wall-clock frame delta (seconds). Returns ticks to simulate.
    ///
    /// **契約**: NaN の dt は観測欠損として drop (戻り 0 tick、状態不変)。
    /// 旧実装は `clamp` が NaN を素通しするため accumulator を NaN 汚染し、
    /// 以後全フレームで `acc >= TICK_DT` が偽となり**永久凍結**した —
    /// 2026-07-22 wave 32 で根治 (drs wave 23 / frame_pacing S-3 と同契約)。
    /// ±∞ は clamp が責任を持つ (0.25 → 5 tick / 0) ので従来どおり受理。
    pub fn consume_ticks(&mut self, frame_dt_sec: f64) -> u32 {
        if frame_dt_sec.is_nan() {
            return 0;
        }
        let dt = frame_dt_sec.clamp(0.0, 0.25);
        self.accumulator += dt;
        let mut n = 0u32;
        while self.accumulator >= TICK_DT && n < self.max_catchup {
            self.accumulator -= TICK_DT;
            self.tick_count += 1;
            n += 1;
        }
        if n == self.max_catchup {
            self.accumulator = 0.0;
        }
        n
    }

    /// Interpolation alpha in [0,1) between last tick and next.
    pub fn render_alpha(&self) -> f32 {
        (self.accumulator / TICK_DT).clamp(0.0, 1.0) as f32
    }

    pub fn tick_count(&self) -> u64 {
        self.tick_count
    }

    pub fn elapsed_sec(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// Lerp helper for render interpolation.
    #[inline]
    pub fn lerp(a: f32, b: f32, alpha: f32) -> f32 {
        a + (b - a) * alpha
    }
}

impl Default for FixedTickClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twenty_tps() {
        let mut c = FixedTickClock::new();
        let n = c.consume_ticks(0.1); // 100ms → 2 ticks
        assert_eq!(n, 2);
        assert!(c.render_alpha() < 1.0);
    }

    /// wave 32-1: dt 厳密列 (tick 数・alpha・残余累積) — 0.05 は TICK_DT と
    /// bit 一致するため剰余ゼロで 1 tick に着地する (Python exact 検証済)。
    #[test]
    fn consume_ticks_exact_sequence() {
        let mut c = FixedTickClock::new();
        assert_eq!(c.consume_ticks(0.05), 1); // ちょうど 1 tick、残余 0
        assert_eq!(c.tick_count(), 1);
        assert_eq!(c.render_alpha(), 0.0);
        // 0.024 → 0 tick、alpha = 0.024/0.05 (f64 除算→f32) == 0.48f32
        // (ビット 0x3ef5c28f、exact rational で導出検証済)
        assert_eq!(c.consume_ticks(0.024), 0);
        assert_eq!(c.render_alpha(), 0.48f32);
        // さらに 0.026 足すと累積 0.05 → 1 tick (残余 ~0)
        assert_eq!(c.consume_ticks(0.026), 1);
        assert_eq!(c.tick_count(), 2);
        // 1 フレームの dt は 0.25 で頭打ち → 最大 5 tick
        assert_eq!(c.consume_ticks(9.0), 5);
        assert_eq!(c.consume_ticks(-3.0), 0);
        assert_eq!(c.tick_count(), 7);
    }

    /// wave 32-2: NaN は状態を汚染せず drop される (旧実装は永久凍結 = 赤)。
    #[test]
    fn nan_dt_is_dropped_without_poisoning() {
        let mut c = FixedTickClock::new();
        assert_eq!(c.consume_ticks(f64::NAN), 0);
        assert_eq!(c.tick_count(), 0);
        assert_eq!(c.render_alpha(), 0.0);
        // 直後の通常フレームが普通に動くこと
        assert_eq!(c.consume_ticks(0.05), 1);
        assert_eq!(c.tick_count(), 1);
    }

    /// wave 32-3: catchup 上限到達で残余を破棄 (spiral of death 防止) +
    /// with_max_catchup(0) は 1 に矯正。
    #[test]
    fn max_catchup_discards_remainder_and_zero_is_coerced() {
        let mut c = FixedTickClock::new().with_max_catchup(2);
        // 0.25s = 5 tick 分だが 2 で打ち止め、残余 0.15 は破棄される
        assert_eq!(c.consume_ticks(0.25), 2);
        assert_eq!(c.tick_count(), 2);
        assert_eq!(c.render_alpha(), 0.0, "残余破棄で alpha は 0");
        // 0 は 1 に矯正される (0 のままだと永久 0 tick 凍結するため)
        let mut z = FixedTickClock::new().with_max_catchup(0);
        assert_eq!(z.consume_ticks(0.25), 1);
        assert_eq!(z.tick_count(), 1);
    }

    /// wave 32-4: lerp 厳密値ピン。
    #[test]
    fn lerp_exact_values() {
        assert_eq!(FixedTickClock::lerp(1.0, 3.0, 0.5), 2.0);
        assert_eq!(FixedTickClock::lerp(2.0, 4.0, 0.25), 2.5);
        assert_eq!(FixedTickClock::lerp(7.0, 9.0, 0.0), 7.0);
        assert_eq!(FixedTickClock::lerp(7.0, 9.0, 1.0), 9.0);
    }
}

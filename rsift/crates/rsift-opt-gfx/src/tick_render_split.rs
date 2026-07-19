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
    pub fn consume_ticks(&mut self, frame_dt_sec: f64) -> u32 {
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
}

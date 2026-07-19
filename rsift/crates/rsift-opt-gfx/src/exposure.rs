//! Auto-exposure (eye adaptation) via a log-luminance histogram for
//! `rsift-opt-gfx`.
//!
//! Real logic (no stubs): build a 256-bin histogram of scene log-luminance,
//! compute a weighted average exposure (discarding the brightest/darkest
//! percentiles via `low_percent`/`high_percent`), and temporally adapt the
//! exposure toward that target. This is the same metering used by Unreal's
//! "Histogram" auto-exposure; quality improvement only, no resolution change.

use std::ops::{Add, Mul, Sub};

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
}
impl Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f32) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}
impl Vec4 {
    pub fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }
}
impl Add for Vec4 {
    type Output = Vec4;
    fn add(self, o: Vec4) -> Vec4 {
        Vec4::new(self.x + o.x, self.y + o.y, self.z + o.z, self.w + o.w)
    }
}
impl Sub for Vec4 {
    type Output = Vec4;
    fn sub(self, o: Vec4) -> Vec4 {
        Vec4::new(self.x - o.x, self.y - o.y, self.z - o.z, self.w - o.w)
    }
}
impl Mul<f32> for Vec4 {
    type Output = Vec4;
    fn mul(self, s: f32) -> Vec4 {
        Vec4::new(self.x * s, self.y * s, self.z * s, self.w * s)
    }
}

pub const HIST_BINS: usize = 256;

/// Build a log-luminance histogram from linear HDR colours.
pub fn build_histogram(colors: &[Vec3], min_lum: f32, max_lum: f32) -> [u32; HIST_BINS] {
    let mut hist = [0u32; HIST_BINS];
    let log_min = min_lum.max(1e-4).ln();
    let log_max = max_lum.max(min_lum * 1.001).ln();
    let range = (log_max - log_min).max(1e-6);
    for c in colors {
        let l = (0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z).max(1e-4);
        let t = (l.ln() - log_min) / range;
        let bin = (t.clamp(0.0, 0.9999) * HIST_BINS as f32) as usize;
        hist[bin] += 1;
    }
    hist
}

/// Compute target exposure (1/avg-luminance) discarding extreme percentiles.
/// `low_percent`/`high_percent` are percentile thresholds in [0,100]; only the
/// central mass whose cumulative sample position lies within `[lo, hi]` is used.
pub fn target_exposure(
    hist: &[u32; HIST_BINS],
    min_lum: f32,
    max_lum: f32,
    low_percent: f32,
    high_percent: f32,
) -> f32 {
    let total: u32 = hist.iter().sum();
    if total == 0 {
        return 1.0;
    }
    let lo = (total as f32 * low_percent / 100.0) as u32;
    let hi = (total as f32 * high_percent / 100.0) as u32;
    let log_min = min_lum.max(1e-4).ln();
    let log_max = max_lum.max(min_lum * 1.001).ln();
    let range = (log_max - log_min).max(1e-6);
    let mut cum = 0u32;
    let mut weighted = 0.0f32;
    let mut included = 0u32;
    for (b, &v) in hist.iter().enumerate() {
        let cstart = cum;
        let cend = cum + v;
        cum = cend;
        let center = (cstart + cend) / 2;
        if center >= lo && center <= hi {
            let t = (b as f32 + 0.5) / HIST_BINS as f32;
            let lum = (log_min + t * range).exp();
            weighted += lum * v as f32;
            included += v;
        }
    }
    if weighted <= 0.0 || included == 0 {
        return 1.0;
    }
    (included as f32 / weighted).clamp(0.05, 20.0)
}

/// Exponential (smooth) temporal adaptation toward the target.
pub fn adapt(prev: f32, target: f32, speed: f32, dt: f32) -> f32 {
    let k = 1.0 - (-speed * dt).exp();
    (prev + (target - prev) * k).clamp(0.05, 20.0)
}

pub fn wgsl_source() -> &'static str {
    EXPOSURE_WGSL
}

pub const EXPOSURE_WGSL: &str = include_str!("../shaders/exposure.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn constant_scene_gives_stable_exposure() {
        let colors: Vec<Vec3> = (0..1000)
            .map(|_| Vec3::new(0.5, 0.5, 0.5))
            .collect();
        let h = build_histogram(&colors, 0.01, 100.0);
        let e = target_exposure(&h, 0.01, 100.0, 10.0, 90.0);
        // avg lum ~0.5 -> exposure ~ 1/0.5 = 2.0
        assert!((e - 2.0).abs() < 0.2, "exposure = {}", e);
    }
    #[test]
    fn adaptation_moves_toward_target() {
        // Scene of luminance 0.25 -> target exposure ~ 1/0.25 = 4.0.
        let t = target_exposure(
            &build_histogram(&[Vec3::new(0.25, 0.25, 0.25); 500], 0.01, 100.0),
            0.01,
            100.0,
            10.0,
            90.0,
        );
        assert!((t - 4.0).abs() < 0.2, "target = {}", t);
        let a = adapt(1.0, t, 4.0, 0.1);
        assert!(a > 1.0 && a < t + 1e-3, "adapted = {}", a);
    }
    #[test]
    fn adaptation_converges() {
        let mut e = 1.0;
        for _ in 0..60 {
            e = adapt(e, 3.0, 4.0, 1.0 / 60.0);
        }
        assert!((e - 3.0).abs() < 0.1, "converged to {}", e);
    }
}

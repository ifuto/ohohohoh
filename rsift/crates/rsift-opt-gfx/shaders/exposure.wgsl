// rsift-opt-gfx :: auto-exposure (log-luminance histogram)
// Mirrors the Rust `build_histogram` / `target_exposure` / `adapt` pipeline,
// matching Unreal's histogram auto-exposure metering.

const HIST_BINS: u32 = 256u;

fn exposure_luma(c: vec3<f32>) -> f32 {
  return 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;
}

fn exposure_target(hist: array<f32, 256>, min_lum: f32, max_lum: f32,
                   low_percent: f32, high_percent: f32) -> f32 {
  var total = 0.0;
  for (var b: u32 = 0u; b < HIST_BINS; b = b + 1u) {
    total = total + hist[b];
  }
  if (total <= 0.0) { return 1.0; }
  let lo = ceil(total * low_percent / 100.0);
  let hi = ceil(total * (1.0 - high_percent / 100.0));
  let log_min = log(max(min_lum, 1e-4));
  let log_max = log(max(max_lum, min_lum * 1.001));
  let range = max(log_max - log_min, 1e-6);
  var count = 0.0;
  var weighted = 0.0;
  for (var b: u32 = 0u; b < HIST_BINS; b = b + 1u) {
    if (count < lo) { count = count + hist[b]; continue; }
    if (count >= hi) { break; }
    let t = (f32(b) + 0.5) / f32(HIST_BINS);
    let lum = exp(log_min + t * range);
    weighted = weighted + lum * hist[b];
    count = count + hist[b];
  }
  if (weighted <= 0.0) { return 1.0; }
  return clamp(total / weighted, 0.05, 20.0);
}

fn exposure_adapt(prev: f32, target: f32, speed: f32, dt: f32) -> f32 {
  let k = 1.0 - exp(-speed * dt);
  return clamp(prev + (target - prev) * k, 0.05, 20.0);
}

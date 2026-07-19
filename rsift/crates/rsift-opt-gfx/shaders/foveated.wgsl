// rsift-opt-gfx :: foveated VRS shading-rate field

fn foveated_rate(uv: vec2<f32>, gaze: vec2<f32>, radius: f32, min_rate: f32) -> f32 {
  let d = length(uv - gaze);
  let t = clamp(d / max(radius, 1e-4), 0.0, 1.0);
  return clamp(1.0 - t * (1.0 - min_rate), min_rate, 1.0);
}

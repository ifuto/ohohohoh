// rsift-opt-gfx :: depth-of-field (CoC disc gather)

fn circle_of_confusion(depth: f32, focus_dist: f32, scale: f32, max_coc: f32) -> f32 {
  return clamp(abs(depth - focus_dist) * scale, 0.0, max_coc);
}

fn gather_blur(uv: vec2<f32>, coc: f32, tex: texture_2d<f32>, samp: sampler) -> vec4<f32> {
  if (coc < 0.001) { return textureSampleLevel(tex, samp, uv, 0.0); }
  var acc = vec4<f32>(0.0);
  var taps = array<vec2<f32>, 8>(
    vec2<f32>(1.0, 0.0), vec2<f32>(0.7071, 0.7071), vec2<f32>(0.0, 1.0), vec2<f32>(-0.7071, 0.7071),
    vec2<f32>(-1.0, 0.0), vec2<f32>(-0.7071, -0.7071), vec2<f32>(0.0, -1.0), vec2<f32>(0.7071, -0.7071));
  for (var i = 0; i < 8; i = i + 1) {
    acc = acc + textureSampleLevel(tex, samp, uv + taps[i] * coc, 0.0);
  }
  return acc / 8.0;
}

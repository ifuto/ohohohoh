// rsift-opt-gfx :: HDR bloom
// Mirrors the Rust `prefilter` / `blur_row` / `composite` pipeline.
// Soft-knee prefilter -> separable Gaussian -> additive composite.

fn bloom_luma(c: vec3<f32>) -> f32 {
  return 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;
}

fn bloom_prefilter(c: vec3<f32>, threshold: f32, knee: f32) -> vec3<f32> {
  let l = bloom_luma(c);
  if (l <= threshold) {
    return vec3<f32>(0.0);
  }
  var f = (l - threshold) / max(threshold, 1e-4);
  if (knee > 0.0) {
    let t = clamp((l - threshold) / knee, 0.0, 1.0);
    f = f * (t * t * (3.0 - 2.0 * t));
  }
  return c * f;
}

fn bloom_blur(src: texture_2d<f32>, samp: sampler, uv: vec2<f32>,
              texel: vec2<f32>, radius: i32) -> vec3<f32> {
  let w = array<f32, 5>(0.0625, 0.25, 0.375, 0.25, 0.0625);
  let o = array<i32, 5>(-2, -1, 0, 1, 2);
  var acc = vec3<f32>(0.0);
  for (var k: i32 = 0; k < 5; k = k + 1) {
    let u = uv + texel * vec2<f32>(f32(o[k]) * f32(radius), 0.0);
    acc = acc + textureSampleLevel(src, samp, u, 0.0).rgb * w[k];
  }
  return acc;
}

fn bloom_composite(scene: vec3<f32>, bloom: vec3<f32>, intensity: f32) -> vec3<f32> {
  return clamp(scene + bloom * intensity, vec3<f32>(0.0), vec3<f32>(64.0));
}

// rsift-opt-gfx :: tile-based motion blur (velocity gather)

struct MotionBlurParams {
  samples: u32,
  max_velocity: f32,
};

fn motion_blur(uv: vec2<f32>, velocity: vec2<f32>, p: MotionBlurParams,
               tex: texture_2d<f32>, samp: sampler) -> vec4<f32> {
  let v = velocity * p.max_velocity;
  let inv = 1.0 / f32(p.samples);
  var acc = vec4<f32>(0.0);
  for (var i = 0u; i < p.samples; i = i + 1u) {
    let t = f32(i) * inv - 0.5;
    acc = acc + textureSampleLevel(tex, samp, uv + v * t, 0.0);
  }
  return acc * inv;
}

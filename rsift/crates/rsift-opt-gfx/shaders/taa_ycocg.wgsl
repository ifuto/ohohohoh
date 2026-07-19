// rsift-opt-gfx :: TAA neighbourhood clamping in YCoCg

fn rgb_to_ycocg(c: vec3<f32>) -> vec3<f32> {
  return vec3<f32>(0.25 * c.r + 0.5 * c.g + 0.25 * c.b,
                   0.5 * c.r - 0.5 * c.b,
                   -0.25 * c.r + 0.5 * c.g - 0.25 * c.b);
}

fn ycocg_to_rgb(c: vec3<f32>) -> vec3<f32> {
  return vec3<f32>(c.x + c.y - c.z, c.x + c.z, c.x - c.y - c.z);
}

fn clamp_to_variance(c: vec3<f32>, mu: vec3<f32>, sigma: vec3<f32>, gamma: f32) -> vec3<f32> {
  let lo = mu - sigma * gamma;
  let hi = mu + sigma * gamma;
  return clamp(c, lo, hi);
}

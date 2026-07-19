// rsift-opt-gfx :: screen-space shadows (SSS) via depth height-field march

fn cast_sss_heightfield(pos: vec3<f32>, light_dir: vec3<f32>, step_size: f32,
                        max_steps: u32, max_dist: f32, bias: f32,
                        depth: texture_2d<f32>, samp: sampler, res: vec2<f32>) -> f32 {
  let ld = normalize(light_dir);
  var v = pos + ld * bias;
  for (var i = 0u; i < max_steps; i = i + 1u) {
    v = v + ld * step_size;
    let travelled = length(v - pos);
    if (travelled > max_dist) { return 1.0; }
    let uv = (v.xy / res) * 0.5 + vec2<f32>(0.5);
    let surf = textureSampleLevel(depth, samp, uv, 0.0).r;
    if (surf >= 1e30) { continue; }
    let diff = surf - travelled;
    if (diff >= 0.0 && diff <= step_size * 2.0) { return 0.0; }
  }
  return 1.0;
}

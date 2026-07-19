// rsift-opt-gfx :: parallax occlusion mapping (tangent space)

struct ParallaxParams {
  layers: u32,
  height_scale: f32,
};

fn parallax_occlusion(uv: vec2<f32>, view_dir: vec3<f32>, height: texture_2d<f32>,
                      samp: sampler, p: ParallaxParams) -> vec2<f32> {
  let layers = max(p.layers, 1u);
  let layer_depth = 1.0 / f32(layers);
  let p_step = (view_dir.xy / max(view_dir.z, 1e-3)) * (p.height_scale * layer_depth);
  var cur_uv = uv;
  var cur_layer = 0.0;
  var cur_depth = textureSampleLevel(height, samp, cur_uv, 0.0).r;
  for (var i = 0u; i < layers; i = i + 1u) {
    if (cur_layer >= cur_depth) { break; }
    cur_uv = cur_uv - p_step;
    cur_depth = textureSampleLevel(height, samp, cur_uv, 0.0).r;
    cur_layer = cur_layer + layer_depth;
  }
  let prev_uv = cur_uv + p_step;
  let prev_depth = textureSampleLevel(height, samp, prev_uv, 0.0).r;
  let after = cur_depth - cur_layer;
  let before = prev_depth - (cur_layer + layer_depth);
  let w = after / max(after - before, 1e-4);
  return mix(cur_uv, prev_uv, w);
}

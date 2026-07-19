// rsift-opt-gfx :: screen-space reflections (SSR)
// View-space height-field ray march with thickness-tested hit detection.
// `ssr_reflect` mirrors `reflect_dir`; `ssr_march_heightfield` mirrors `march`
// but reads a depth texture and reconstructs travelled distance.

struct SsrParams {
  max_steps: u32,
  step_size: f32,
  thickness: f32,
  max_dist: f32,
};

const SSR_INF: f32 = 1e30;

fn ssr_reflect(incident: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
  let n = normalize(normal);
  let i = normalize(incident);
  return normalize(i - n * (2.0 * dot(i, n)));
}

// Depth texture holds view-space linear depth (positive). Returns hit position
// in view space, or vec3(SSR_INF) on miss.
fn ssr_march_heightfield(
  ro: vec3<f32>,
  rd: vec3<f32>,
  p: SsrParams,
  res: vec2<f32>,
  depth: texture_2d<f32>,
  samp: sampler,
) -> vec3<f32> {
  let dir = normalize(rd);
  var pos = ro;
  for (var i: u32 = 0u; i < p.max_steps; i = i + 1u) {
    pos = pos + dir * p.step_size;
    let travelled = length(pos - ro);
    if (travelled > p.max_dist) {
      return vec3<f32>(SSR_INF, SSR_INF, SSR_INF);
    }
    let uv = (pos.xy / vec2<f32>(res)) * 0.5 + vec2<f32>(0.5);
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
      continue;
    }
    let surf = textureSampleLevel(depth, samp, uv, 0.0).r;
    if (surf >= SSR_INF) {
      continue;
    }
    let diff = surf - travelled;
    if (diff >= 0.0 && diff <= p.thickness) {
      return pos;
    }
  }
  return vec3<f32>(SSR_INF, SSR_INF, SSR_INF);
}

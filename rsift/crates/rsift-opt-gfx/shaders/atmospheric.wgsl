// rsift-opt-gfx :: analytic atmospheric scattering (single scattering)
const PI: f32 = 3.14159265;

fn rayleigh_phase(c: f32) -> f32 {
  return 3.0 / (16.0 * PI) * (1.0 + c * c);
}

fn mie_phase(c: f32, g: f32) -> f32 {
  let g2 = g * g;
  let d = max(1.0 + g2 - 2.0 * g * c, 1e-4);
  return (1.0 - g2) / (4.0 * PI * pow(d, 1.5));
}

fn atmosphere_transmittance(dist: f32, coeff: vec3<f32>) -> vec3<f32> {
  return exp(-coeff * dist);
}

fn sky_color(ray_dir: vec3<f32>, sun_dir: vec3<f32>, rayleigh: vec3<f32>, sun_intensity: f32) -> vec3<f32> {
  let rd = normalize(ray_dir);
  let sd = normalize(sun_dir);
  let cos_t = dot(rd, sd);
  let phase = rayleigh_phase(cos_t) + 0.1 * mie_phase(cos_t, 0.76);
  var inscatter = vec3<f32>(0.0);
  let steps = 8;
  let seg = 8000.0 / f32(steps);
  var t = 0.0;
  for (var i = 0; i < steps; i = i + 1) {
    let d = t + seg * 0.5;
    let tr = exp(-rayleigh * d);
    inscatter = inscatter + rayleigh * phase * tr * seg;
    t = t + seg;
  }
  return inscatter * sun_intensity;
}

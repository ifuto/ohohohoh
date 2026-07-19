// rsift-opt-gfx :: exponential height fog with dithered raymarch

fn fog_density_at(height: f32, base: f32, scale: f32, start: f32) -> f32 {
  return base * exp(-max(height - start, 0.0) / max(scale, 1e-3));
}

fn raymarch_fog(ro: vec3<f32>, rd: vec3<f32>, dist: f32, steps: u32,
                base: f32, scale: f32, start: f32, dither: f32) -> vec3<f32> {
  if (steps == 0u || dist <= 0.0) { return vec3<f32>(1.0); }
  let seg = dist / f32(steps);
  let dir = normalize(rd);
  var t = clamp(dither, 0.0, 1.0) * seg;
  var trans = vec3<f32>(1.0);
  for (var i = 0u; i < steps; i = i + 1u) {
    let h = (ro + dir * t).y;
    let d = fog_density_at(h, base, scale, start);
    trans = trans * exp(-d * seg);
    t = t + seg;
  }
  return trans;
}

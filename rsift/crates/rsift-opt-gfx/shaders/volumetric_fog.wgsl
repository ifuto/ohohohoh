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
  // wave 150 EV 注記 1: 積連鎖 exp ≡ exp(-seg·Σd) は実数厳密、f32 では和側が
  // 丸め 1 回 (CPU ミラーと同形式に同形化、誤差構造縮小 + exp 1 回化)。
  var od = 0.0;
  for (var i = 0u; i < steps; i = i + 1u) {
    let h = (ro + dir * t).y;
    od = od + fog_density_at(h, base, scale, start);
    t = t + seg;
  }
  return vec3<f32>(exp(-od * seg));
}

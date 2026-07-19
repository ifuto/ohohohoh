// rsift-opt-gfx :: spherical-harmonics (3-band) ambient evaluation

fn sh_basis(dir: vec3<f32>) -> array<f32, 9> {
  let d = normalize(dir);
  let x = d.x; let y = d.y; let z = d.z;
  return array<f32, 9>(
    0.282095,
    0.488603 * y, 0.488603 * z, 0.488603 * x,
    1.092548 * x * y, 1.092548 * y * z, 0.315392 * (3.0 * z * z - 1.0),
    1.092548 * x * z, 0.546274 * (x * x - y * y));
}

fn evaluate_sh(coeffs: array<vec3<f32>, 9>, dir: vec3<f32>) -> vec3<f32> {
  let b = sh_basis(normalize(dir));
  var c = vec3<f32>(0.0);
  for (var i = 0; i < 9; i = i + 1) {
    c = c + coeffs[i] * b[i];
  }
  return c;
}

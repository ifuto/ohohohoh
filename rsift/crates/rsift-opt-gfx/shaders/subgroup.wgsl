// rsift-opt-gfx :: subgroup / wavefront operation helpers

const WAVE_WIDTH: u32 = 32u;

// On hardware these are single instructions; here `wave_sum` is supplied by a
// pre-pass so the shader stays a faithful mirror of the Rust impl.
fn subgroup_reduce_add(value: f32, wave_sum: f32) -> f32 {
  return wave_sum;
}

fn subgroup_ballot(cond: bool) -> u32 {
  return select(0u, 1u, cond);
}

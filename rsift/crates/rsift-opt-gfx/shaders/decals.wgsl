// rsift-opt-gfx :: projected (deferred) decals

struct Decal {
  center: vec3<f32>,
  right: vec3<f32>,
  up: vec3<f32>,
  forward: vec3<f32>,
  half_extent: vec3<f32>,
};

fn decal_local(world_pos: vec3<f32>, d: Decal) -> vec3<f32> {
  let rel = world_pos - d.center;
  let x = dot(rel, d.right);
  let y = dot(rel, d.up);
  let z = dot(rel, d.forward);
  return vec3<f32>(x / d.half_extent.x, y / d.half_extent.y, z);
}

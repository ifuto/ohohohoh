// rsift-opt-gfx :: bindless handle packing
// Layout (32 bits): [ set:4 | binding:8 | index:20 ]

fn pack_handle(set: u32, binding: u32, index: u32) -> u32 {
  return ((set & 0xFu) << 28u) | ((binding & 0xFFu) << 20u) | (index & 0xFFFFFu);
}

fn unpack_handle(h: u32) -> vec3<u32> {
  return vec3<u32>((h >> 28u) & 0xFu, (h >> 20u) & 0xFFu, h & 0xFFFFFu);
}

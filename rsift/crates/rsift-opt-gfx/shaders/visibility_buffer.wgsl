// Visibility buffer resolve (reference WGSL). The forward pass wrote
// (primitive_id, instance_id) into an 8-byte buffer; here we fetch the primitive's
// vertices and interpolate attributes with the stored barycentric weights, then
// shade. Keeps the forward pass tiny (8 B/px) so tile-based iGPUs avoid the
// 24-32 B/px G-buffer bandwidth hit.

struct U { primOffset : u32, };
@group(0) @binding(0) var<uniform> u : U;
// packed (prim, instance) per pixel
@group(0) @binding(1) var visBuf : texture_2d<u32>;
// primitive -> 3 vertex indices / attributes (srv buffer)
@group(0) @binding(2) var<storage, read> primitives : array<vec4<f32>>;
@group(0) @binding(3) var bary : texture_2d<f32>; // rgb = barycentric weights

@fragment
fn main(@builtin(position) pos : vec4<f32>) -> @location(0) vec4<f32> {
    let pix = vec2<i32>(i32(pos.x), i32(pos.y));
    let packed = textureLoad(visBuf, pix, 0).r;
    let primitive = packed & 0xFFFFu;
    let base = u.primOffset + primitive * 3u;
    let va = primitives[base + 0u];
    let vb = primitives[base + 1u];
    let vc = primitives[base + 2u];
    let w = textureLoad(bary, pix, 0).rgb;
    let color = va * w.x + vb * w.y + vc * w.z;
    return vec4<f32>(color.rgb, 1.0);
}

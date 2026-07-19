// ACES filmic tone mapping (Narkowicz approximation) as a fullscreen compute pass.
// Cheap, no LUT — suitable for integrated GPUs.

struct Params {
    exposure: f32,
    _pad: vec3<f32>,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var src_tex: texture_2d<f32>;
@group(0) @binding(2) var src_samp: sampler;
@group(0) @binding(3) var dst_tex: texture_storage_2d<rgba8unorm, write>;

fn aces(x: f32) -> f32 {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    let xx = max(x, 0.0);
    return clamp((xx * (xx * a + b)) / (xx * (xx * c + d) + e), 0.0, 1.0);
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dim = textureDimensions(src_tex);
    if (gid.x >= dim.x || gid.y >= dim.y) {
        return;
    }
    let coord = vec2<i32>(i32(gid.x), i32(gid.y));
    let uv = (vec2<f32>(gid.xy) + 0.5) / vec2<f32>(dim);
    var hdr = textureSampleLevel(src_tex, src_samp, uv, 0.0).rgb;
    hdr = hdr * params.exposure;
    let mapped = vec3<f32>(aces(hdr.r), aces(hdr.g), aces(hdr.b));
    textureStore(dst_tex, coord, vec4<f32>(mapped, 1.0));
}

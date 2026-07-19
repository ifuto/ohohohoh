// Variable Rate Shading mask generation (reference WGSL compute pass).
// One invocation per rate-tile. Averages per-pixel motion magnitude and luma
// variance, then picks a coarse/fine shading rate. Reducing the shading rate
// directly cuts pixel-shader invocations — a cheap win on integrated GPUs.

struct U {
    dims    : vec2<u32>,   // full-res pixel dims
    tile    : u32,         // tile size in pixels
    weights : vec2<f32>,   // (motion_weight, variance_weight)
};
@group(0) @binding(0) var<uniform> u : U;
@group(0) @binding(1) var motionTex : texture_2d<f32>;
@group(0) @binding(2) var varTex    : texture_2d<f32>;
@group(0) @binding(3) var outTex    : texture_storage_2d<r32uint, write>;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid : vec3<u32>) {
    let tw = (u.dims.x + u.tile - 1u) / u.tile;
    let th = (u.dims.y + u.tile - 1u) / u.tile;
    if (gid.x >= tw || gid.y >= th) { return; }

    var ms = 0.0;
    var vs = 0.0;
    var cnt = 0.0;
    for (var y : u32 = 0u; y < u.tile; y = y + 1u) {
        let py = gid.y * u.tile + y;
        if (py >= u.dims.y) { break; }
        for (var x : u32 = 0u; x < u.tile; x = x + 1u) {
            let px = gid.x * u.tile + x;
            if (px >= u.dims.x) { break; }
            let uv = (vec2<f32>(f32(px), f32(py)) + 0.5) / vec2<f32>(f32(u.dims.x), f32(u.dims.y));
            ms = ms + textureSampleLevel(motionTex, smp, uv, 0.0).r;
            vs = vs + textureSampleLevel(varTex, smp, uv, 0.0).r;
            cnt = cnt + 1.0;
        }
    }
    let motion = ms / max(cnt, 1.0);
    let variance = vs / max(cnt, 1.0);
    let score = motion * u.weights.x - variance * u.weights.y;

    var code : u32 = 1u; // default Rate1x2
    if (score > 0.6) { code = 4u; }
    else if (score > 0.3) { code = 3u; }
    else if (score > 0.05) { code = 2u; }
    else if (score <= -0.3) { code = 0u; }
    textureStore(outTex, vec2<i32>(i32(gid.x), i32(gid.y)), vec4<u32>(code, 0u, 0u, 0u));
}

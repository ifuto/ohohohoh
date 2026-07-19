// Shadow LOD selection (reference WGSL). Scales the shadow-map resolution a
// light receives by its on-screen coverage and drops sub-pixel casters, so a
// 4K cascade atlas is only spent where it is actually visible. Cheap to compute
// per light on integrated GPUs.

struct U {
    minRes : u32,
    maxRes : u32,
    minCasterPx : f32,
};
@group(0) @binding(0) var<uniform> u : U;
// Per-light coverage (x) and per-caster screen height (y), packed for the pass.
@group(0) @binding(1) var lightTex : texture_2d<f32>;
@group(0) @binding(2) var samp : sampler;

@fragment
fn main(@builtin(position) pos : vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pos.xy / vec2<f32>(textureDimensions(lightTex, 0));
    let data = textureSampleLevel(lightTex, samp, uv, 0.0).rg;
    let coverage = clamp(data.x, 0.0, 1.0);
    let casterPx = data.y;

    let res = mix(f32(u.minRes), f32(u.maxRes), coverage);
    let resRounded = floor(res / 256.0) * 256.0; // quantize to tile-friendly size

    var lod = 3.0;
    if (casterPx >= 256.0) { lod = 0.0; }
    else if (casterPx >= 64.0) { lod = 1.0; }
    else if (casterPx >= 16.0) { lod = 2.0; }

    let casts = select(0.0, 1.0, casterPx >= u.minCasterPx);
    return vec4<f32>(resRounded, lod, casts, 1.0);
}

// GTAO (Ground Truth Ambient Occlusion) — reference WGSL, single direction
// slice. Integrates the horizon angle of the depth/height profile around the
// pixel and converts the rise into an occlusion factor. The full pipeline runs
// several rotated slices and averages them. Cheaper than SSAO to get ray-traced
// quality; run at half-res on integrated GPUs.

struct U {
    invRes  : vec2<f32>,
    radius  : f32,
    power   : f32,   // occlusion contrast
};
@group(0) @binding(0) var<uniform> u : U;
// Depth (or linear height) buffer indexed by screen UV.
@group(0) @binding(1) var depthTex : texture_2d<f32>;
@group(0) @binding(2) var samp : sampler;

@fragment
fn main(@builtin(position) pos : vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pos.xy * u.invRes;
    let center = textureSampleLevel(depthTex, samp, uv, 0.0).r;

    // One direction slice; the full pass rotates this via a 2x2 rotation matrix.
    let dir = vec2<f32>(1.0, 0.0);
    var maxHorizon = -1.5707963; // -PI/2
    for (var i : i32 = 1; i <= 8; i = i + 1) {
        let t = f32(i) * (u.radius / 8.0);
        let sUV = uv + dir * (t * u.invRes);
        let h = textureSampleLevel(depthTex, samp, sUV, 0.0).r;
        let angle = atan2(h - center, max(t, 1.0e-4));
        maxHorizon = max(maxHorizon, angle);
    }
    let occlusion = clamp(maxHorizon / 1.5707963, 0.0, 1.0);
    let ao = pow(1.0 - occlusion, u.power);
    return vec4<f32>(ao, ao, ao, 1.0);
}

// Weighted Blended OIT (reference WGSL). One geometry pass, no sorting.
// Each fragment contributes (color.rgb * color.a * w, color.a * w) with a
// depth-based weight so nearer surfaces dominate. A final full-screen resolve
// divides by the summed weight. Avoids transparency sort stalls on low-spec GPUs.

struct U { near : f32, k : f32, };
@group(0) @binding(0) var<uniform> u : U;
// accumulated (rgb*a*w, a*w) — two RGBA16F render targets merged here as one
@group(0) @binding(1) var accumTex : texture_2d<f32>;
@group(0) @binding(2) var samp : sampler;

fn weight(depth : f32) -> f32 {
    let d = max(depth - u.near, 0.0);
    return 1.0 / (1.0 + d * u.k);
}

@fragment
fn main(@builtin(position) pos : vec4<f32>, @location(0) color : vec4<f32>,
        @location(1) depth : f32) -> @location(0) vec4<f32> {
    let w = weight(depth) * color.a;
    return vec4<f32>(color.rgb * w, w);
}

// Separate resolve pass:
@fragment
fn resolve(@builtin(position) pos : vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pos.xy / vec2<f32>(textureDimensions(accumTex, 0));
    let s = textureSampleLevel(accumTex, samp, uv, 0.0);
    if (s.a <= 1.0e-5) { return vec4<f32>(0.0); }
    return vec4<f32>(s.rgb / s.a, s.a);
}

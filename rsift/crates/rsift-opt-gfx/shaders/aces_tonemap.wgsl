// ACES Filmic tone mapping (Narkowicz 2015 approximation), reference WGSL.
// Maps HDR linear color into [0,1] with a filmic shoulder. Cheap enough for
// low-spec GPUs; apply after lighting, before sRGB encode.

struct U { exposure : f32, };
@group(0) @binding(0) var<uniform> u : U;
@group(0) @binding(1) var hdrTex : texture_2d<f32>;
@group(0) @binding(2) var samp : sampler;

fn aces(x : vec3<f32>) -> vec3<f32> {
    let a = 2.51; let b = 0.03; let c = 2.43; let d = 0.59; let e = 0.14;
    let num = x * (a * x + vec3<f32>(b));
    let den = x * (c * x + vec3<f32>(d)) + vec3<f32>(e);
    return clamp(num / den, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn linear_to_srgb(x : vec3<f32>) -> vec3<f32> {
    return pow(max(x, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
}

@fragment
fn main(@builtin(position) pos : vec4<f32>, @builtin(position) fc : vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pos.xy / vec2<f32>(textureDimensions(hdrTex, 0));
    let hdr = textureSampleLevel(hdrTex, samp, uv, 0.0).rgb * u.exposure;
    let mapped = aces(hdr);
    let outc = linear_to_srgb(mapped);
    return vec4<f32>(outc, 1.0);
}

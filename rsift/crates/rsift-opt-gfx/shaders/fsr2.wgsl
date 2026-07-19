// FSR2 temporal upscale (reference WGSL). One fragment invocation per OUTPUT pixel.
// Reconstructs display resolution from a lower-res current frame plus history,
// using a per-pixel motion vector to reproject and a 3x3 neighborhood clamp to
// suppress ghosting. No special hardware — works on Intel/Apple iGPUs.

struct Uniforms {
    inRes      : vec2<f32>,
    outRes     : vec2<f32>,
    invInRes   : vec2<f32>,
    invOutRes  : vec2<f32>,
    jitter     : vec2<f32>,
    reset      : f32,
};

@group(0) @binding(0) var<uniform> u : Uniforms;
@group(0) @binding(1) var currentTex : texture_2d<f32>;
@group(0) @binding(2) var historyTex : texture_2d<f32>;
@group(0) @binding(3) var motionTex  : texture_2d<f32>;
@group(0) @binding(4) var samp       : sampler;

fn luma(c : vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}

@fragment
fn main(@builtin(position) pos : vec4<f32>) -> @location(0) vec4<f32> {
    let outUV = pos.xy * u.invOutRes;

    // Build the color AABB from the 3x3 neighborhood of the CURRENT (low-res) frame.
    var mn = vec3<f32>(1.0e9, 1.0e9, 1.0e9);
    var mx = vec3<f32>(-1.0e9, -1.0e9, -1.0e9);
    for (var j : i32 = -1; j <= 1; j = j + 1) {
        for (var i : i32 = -1; i <= 1; i = i + 1) {
            let c = textureSampleLevel(currentTex, samp,
                outUV + vec2<f32>(f32(i), f32(j)) * u.invInRes, 0.0).rgb;
            mn = min(mn, c);
            mx = max(mx, c);
        }
    }

    let cur = textureSampleLevel(currentTex, samp, outUV, 0.0).rgb;

    // Motion vector is stored as (prevUV - curUV) in UV space.
    let mv = textureSampleLevel(motionTex, samp, outUV, 0.0).rg;
    let prevUV = clamp(outUV + mv, vec2<f32>(0.0), vec2<f32>(1.0));
    var hist = textureSampleLevel(historyTex, samp, prevUV, 0.0).rgb;

    // Neighborhood clamp to suppress ghosting on disocclusions.
    hist = clamp(hist, mn, mx);

    // On reset (camera cut) use pure current frame.
    let a = select(0.95, 1.0, u.reset > 0.5);
    let resolved = mix(hist, cur, a);

    // Cheap perceptual dither to break up 8-bit banding on iGPUs.
    let dith = (fract(sin(dot(pos.xy, vec2<f32>(12.9898, 78.233))) * 43758.5453) - 0.5) / 255.0;
    return vec4<f32>(resolved + vec3<f32>(dith), 1.0);
}

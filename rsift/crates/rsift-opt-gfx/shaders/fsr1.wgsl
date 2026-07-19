// FSR 1.0 — EASU (edge-aware upsample) + RCAS (sharpen) reference compute pass.
// Mirrors the CPU reference in fsr1.rs: detect the luma gradient, shift the
// sample position toward 0.5 along strong edges, then bilinear reconstruct.

struct Params { inputSize: vec2<f32>, outputSize: vec2<f32>, sharpness: f32, _pad: f32 };
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var srcTex: texture_2d<f32>;
@group(0) @binding(2) var srcSamp: sampler;
@group(0) @binding(3) var dstTex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(4) var casTex: texture_2d<f32>;
@group(0) @binding(5) var casOut: texture_storage_2d<rgba8unorm, write>;

fn luma(c: vec3<f32>) -> f32 { return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)); }

// EASU: upsample a low-res source into the (larger) destination.
@compute @workgroup_size(8, 8)
fn fsr_easu(@builtin(global_invocation_id) gid: vec3<u32>) {
    let outDim = textureDimensions(dstTex);
    if (gid.x >= outDim.x || gid.y >= outDim.y) { return; }
    let coord = vec2<i32>(i32(gid.x), i32(gid.y));

    let uv = (vec2<f32>(gid.xy) + 0.5) / params.outputSize;
    let lr = uv * params.inputSize - 0.5;
    let base = floor(lr);
    let f = lr - base;

    let p00 = textureSampleLevel(srcTex, srcSamp, (base + vec2<f32>(0.0, 0.0) + 0.5) / params.inputSize, 0.0).rgb;
    let p10 = textureSampleLevel(srcTex, srcSamp, (base + vec2<f32>(1.0, 0.0) + 0.5) / params.inputSize, 0.0).rgb;
    let p01 = textureSampleLevel(srcTex, srcSamp, (base + vec2<f32>(0.0, 1.0) + 0.5) / params.inputSize, 0.0).rgb;
    let p11 = textureSampleLevel(srcTex, srcSamp, (base + vec2<f32>(1.0, 1.0) + 0.5) / params.inputSize, 0.0).rgb;

    let gx = abs((p10.r + p11.r) - (p00.r + p01.r));
    let gy = abs((p00.r + p10.r) - (p01.r + p11.r));
    let ex = gx / (gx + 0.5);
    let ey = gy / (gy + 0.5);
    let fx2 = f.x + (0.5 - f.x) * ex;
    let fy2 = f.y + (0.5 - f.y) * ey;

    let top = mix(p00, p10, fx2);
    let bot = mix(p01, p11, fx2);
    let outc = mix(top, bot, fy2);
    textureStore(dstTex, coord, vec4<f32>(outc, 1.0));
}

// RCAS: contrast-adaptive sharpening on the upscaled result.
@compute @workgroup_size(8, 8)
fn fsr_rcas(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dim = textureDimensions(casTex);
    if (gid.x >= dim.x || gid.y >= dim.y) { return; }
    let coord = vec2<i32>(i32(gid.x), i32(gid.y));
    let c = textureLoad(casTex, coord, 0).rgb;
    let n = textureLoad(casTex, coord + vec2<i32>(0, 1), 0).rgb;
    let s = textureLoad(casTex, coord + vec2<i32>(0, -1), 0).rgb;
    let e = textureLoad(casTex, coord + vec2<i32>(1, 0), 0).rgb;
    let w = textureLoad(casTex, coord + vec2<i32>(-1, 0), 0).rgb;
    let lap = (n + s + e + w) * 0.25 - c;
    let sharp = vec3<f32>(params.sharpness);
    textureStore(casOut, coord, vec4<f32>(clamp(c + lap * sharp, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0));
}

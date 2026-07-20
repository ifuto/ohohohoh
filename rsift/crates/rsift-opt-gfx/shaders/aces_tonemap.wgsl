// ACES Filmic tone mapping (Narkowicz 2015 approximation) + 全画面三角形。
// HDR (linear) → filmic shoulder → sRGB エンコード。低スペック GPU でも 1 パス。
//
// 使い方: vs_main (バッファ無し全画面三角形) + fs_main を同一モジュールから駆動。
// NOTE: 旧版は @fragment main が @builtin(position) を2つ受け取る無効 WGSL であり、
//       実デバイス/naga では受理されなかった潜在バグがあった (本シリーズで修正)。

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

struct PostVsOut {
    @builtin(position) pos : vec4<f32>,
};

// バッファ無し全画面三角形: vid 0→(-1,-1), 1→(-1,3), 2→(3,-1)。
@vertex
fn vs_main(@builtin(vertex_index) vid : u32) -> PostVsOut {
    let x = f32(i32(vid / 2u) * 4 - 1);
    let y = f32(i32(vid % 2u) * 4 - 1);
    var out : PostVsOut;
    out.pos = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(in : PostVsOut) -> @location(0) vec4<f32> {
    let uv = in.pos.xy / vec2<f32>(textureDimensions(hdrTex, 0));
    let hdr = textureSampleLevel(hdrTex, samp, uv, 0.0).rgb * u.exposure;
    let mapped = aces(hdr);
    let outc = linear_to_srgb(mapped);
    return vec4<f32>(outc, 1.0);
}

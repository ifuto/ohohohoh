// FXAA post-process anti-aliasing (reference WGSL). One fragment per screen pixel.
// Smooths luma edges without needing a depth or geometry buffer — ideal for
// low-spec / integrated GPUs where MSAA/TAA are too costly.

struct U {
    invRes : vec2<f32>,
};
@group(0) @binding(0) var<uniform> u : U;
@group(0) @binding(1) var tex : texture_2d<f32>;
@group(0) @binding(2) var samp : sampler;

fn luma(c : vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}

@fragment
fn main(@builtin(position) pos : vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pos.xy * u.invRes;
    let c = textureSampleLevel(tex, samp, uv, 0.0).rgb;
    // 注: WGSL テクスチャ座標は +y が下向きのため、ここで "n" と名付けた
    // サンプルは視覚的には下 (south) 側。gx/gy は abs 比較のみに使うので
    // ラベルの符号反転は振る舞いに無影響 (Rust reference 側と等価)。
    let n = textureSampleLevel(tex, samp, uv + vec2<f32>(0.0,  u.invRes.y), 0.0).rgb;
    let s = textureSampleLevel(tex, samp, uv + vec2<f32>(0.0, -u.invRes.y), 0.0).rgb;
    let e = textureSampleLevel(tex, samp, uv + vec2<f32>( u.invRes.x, 0.0), 0.0).rgb;
    let w = textureSampleLevel(tex, samp, uv + vec2<f32>(-u.invRes.x, 0.0), 0.0).rgb;

    let lc = luma(c); let ln = luma(n); let ls = luma(s); let le = luma(e); let lw = luma(w);
    let lmin = min(lc, min(ln, min(ls, min(le, lw))));
    let lmax = max(lc, max(ln, max(ls, max(le, lw))));
    let contrast = lmax - lmin;
    let threshold = max(1.0 / 256.0, lmax * 0.166667);
    if (contrast < threshold) {
        return vec4<f32>(c, 1.0);
    }

    let gx = lw - le;
    let gy = ln - ls;
    var avg : vec3<f32>;
    if (abs(gx) > abs(gy)) {
        avg = 0.5 * (n + s);
    } else {
        avg = 0.5 * (e + w);
    }
    let t = clamp(contrast / (contrast + threshold), 0.0, 1.0) * 0.5;
    let outc = mix(c, avg, t);
    return vec4<f32>(outc, 1.0);
}

// SMAA edge detection pass (reference WGSL). Outputs a luma-edge factor in the
// red channel and the edge orientation (1.0 = horizontal) in green. A later pass
// uses precomputed area tables to blend, smoothing jaggies per-subpixel without
// the blur of FXAA. One fragment per screen pixel.

struct U { invRes : vec2<f32>, threshold : f32, };
@group(0) @binding(0) var<uniform> u : U;
@group(0) @binding(1) var tex : texture_2d<f32>;
@group(0) @binding(2) var samp : sampler;

fn luma(c : vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}

@fragment
fn main(@builtin(position) pos : vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pos.xy * u.invRes;
    let c  = textureSampleLevel(tex, samp, uv, 0.0).rgb;
    let n  = textureSampleLevel(tex, samp, uv + vec2<f32>(0.0,  u.invRes.y), 0.0).rgb;
    let s  = textureSampleLevel(tex, samp, uv + vec2<f32>(0.0, -u.invRes.y), 0.0).rgb;
    let e  = textureSampleLevel(tex, samp, uv + vec2<f32>( u.invRes.x, 0.0), 0.0).rgb;
    let w  = textureSampleLevel(tex, samp, uv + vec2<f32>(-u.invRes.x, 0.0), 0.0).rgb;

    let lc = luma(c); let ln = luma(n); let ls = luma(s); let le = luma(e); let lw = luma(w);
    let lmax = max(lc, max(ln, max(ls, max(le, lw))));
    let lmin = min(lc, min(ln, min(ls, min(le, lw))));
    let contrast = lmax - lmin;
    if (contrast < max(1.0 / 256.0, lmax * u.threshold)) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let gx = lw - le;
    let gy = ln - ls;
    let horizontal = abs(gx) < abs(gy);
    let strength = select(abs(gx), abs(gy), horizontal);
    return vec4<f32>(strength, select(0.0, 1.0, horizontal), 0.0, 1.0);
}

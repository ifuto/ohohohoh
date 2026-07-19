// AMD FidelityFX-inspired Contrast Adaptive Sharpening (CAS) — Tier 4.
// Sharpen + optional bilinear upscale from internal DRS resolution to display.

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var input_samp: sampler;

struct CasParams {
    // sharpness 0..1, inverted to AMD const0 style
    sharpness: f32,
    display_w: f32,
    display_h: f32,
    _pad: f32,
};
@group(0) @binding(2) var<uniform> params: CasParams;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    var pos = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    var o: VsOut;
    o.clip = vec4<f32>(pos[vid], 0.0, 1.0);
    o.uv = pos[vid] * 0.5 + vec2<f32>(0.5, 0.5);
    return o;
}

fn cas_weight(a: f32, b: f32) -> f32 {
    return 1.0 / (1.0 + abs(a - b));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tex_size = vec2<f32>(textureDimensions(input_tex));
    let uv = in.uv;
    // 3x3 neighborhood
    let o = 1.0 / tex_size;
    let a = textureSample(input_tex, input_samp, uv + vec2<f32>(-o.x, -o.y)).rgb;
    let b = textureSample(input_tex, input_samp, uv + vec2<f32>( 0.0, -o.y)).rgb;
    let c = textureSample(input_tex, input_samp, uv + vec2<f32>( o.x, -o.y)).rgb;
    let d = textureSample(input_tex, input_samp, uv + vec2<f32>(-o.x,  0.0)).rgb;
    let e = textureSample(input_tex, input_samp, uv).rgb;
    let f = textureSample(input_tex, input_samp, uv + vec2<f32>( o.x,  0.0)).rgb;
    let g = textureSample(input_tex, input_samp, uv + vec2<f32>(-o.x,  o.y)).rgb;
    let h = textureSample(input_tex, input_samp, uv + vec2<f32>( 0.0,  o.y)).rgb;
    let i = textureSample(input_tex, input_samp, uv + vec2<f32>( o.x,  o.y)).rgb;

    let mn = min(e, min(min(b, d), min(f, h)));
    let mx = max(e, max(max(b, d), max(f, h)));
    // Adaptive amount — less sharpen where contrast already high
    let amp = clamp(min(mn, 1.0 - mx) / (mx + 1e-4), 0.0, 1.0);
    let peak = -0.125 - (params.sharpness * 0.25);
    let w = amp * peak;

    var sharp = e;
    sharp = (b + d + f + h) * w + e;
    sharp = sharp / (1.0 + 4.0 * w);

    // Soft mix with cross taps for stability
    let cross = (a + c + g + i) * 0.05 + sharp * 0.8;
    let out_rgb = mix(e, cross, clamp(params.sharpness * 1.2, 0.0, 1.0));
    return vec4<f32>(out_rgb, 1.0);
}

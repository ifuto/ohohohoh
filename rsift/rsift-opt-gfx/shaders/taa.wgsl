// Lightweight TAA — history reprojection + 3x3 neighborhood clamp (Tier 6).

@group(0) @binding(0) var current_tex: texture_2d<f32>;
@group(0) @binding(1) var history_tex: texture_2d<f32>;
@group(0) @binding(2) var velocity_tex: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;

struct TaaParams {
    blend: f32,
    _p0: f32,
    _p1: f32,
    _p2: f32,
};
@group(0) @binding(4) var<uniform> params: TaaParams;

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

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let cur = textureSample(current_tex, samp, uv).rgb;
    let vel = textureSample(velocity_tex, samp, uv).rg;
    let hist_uv = uv - vel;
    var hist = textureSample(history_tex, samp, hist_uv).rgb;

    let tex_size = vec2<f32>(textureDimensions(current_tex));
    let o = 1.0 / tex_size;
    var mn = cur;
    var mx = cur;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let s = textureSample(current_tex, samp, uv + vec2<f32>(f32(x), f32(y)) * o).rgb;
            mn = min(mn, s);
            mx = max(mx, s);
        }
    }
    hist = clamp(hist, mn, mx);
    let out_rgb = mix(cur, hist, params.blend);
    return vec4<f32>(out_rgb, 1.0);
}

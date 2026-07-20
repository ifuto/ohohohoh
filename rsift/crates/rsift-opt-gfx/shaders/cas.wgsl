// rsift-opt-gfx :: Contrast Adaptive Sharpening (実 dispatch 版)
//
// CPU ミラー (完全一致): src/frame_postfx.rs の cas_run_cpu
// (= src/cas.rs の cas_sample を全画素に適用。演算順も同一)
//   min_c/max_c : 上下左右 4 近傍 (ボーダーは端へ clamp) の成分別 min/max
//   contour = clamp(1.0 - (mx - mn), 0, 1)
//   peaking = 1.0 / (4.0 * (mx - mn) + 1.0)
//   amp     = clamp(contour * peaking * sharpness, 0, 1)
//   out     = c * (1.0 - amp) + ((mn + mx) * 0.5) * amp   (成分毎)
//
// iGPU コア: バッファのみ (sampler / storage texture 不使用)。
// mul/add/div/clamp/min/max のみで IEEE 厳密丸め、GPU==CPU bitwise を保つ
// (旧版は fs_main の vec3/scalar 混在 clamp で naga 検証に失敗していた)。

struct CasParams {
    width: u32,
    height: u32,
    sharpness: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> params: CasParams;
@group(0) @binding(1) var<storage, read> src: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> dst: array<vec4<f32>>;

fn px(x: i32, y: i32) -> vec3<f32> {
    let cx = clamp(x, 0, i32(params.width) - 1);
    let cy = clamp(y, 0, i32(params.height) - 1);
    return src[u32(cy) * params.width + u32(cx)].xyz;
}

fn cas_run(x: i32, y: i32) -> vec3<f32> {
    let n = px(x, y - 1);
    let s = px(x, y + 1);
    let e = px(x + 1, y);
    let w = px(x - 1, y);
    let c = px(x, y);
    let mn = min(min(n, s), min(e, w));
    let mx = max(max(n, s), max(e, w));
    let contour = clamp(vec3<f32>(1.0) - (mx - mn), vec3<f32>(0.0), vec3<f32>(1.0));
    let peaking = vec3<f32>(1.0) / ((mx - mn) * 4.0 + vec3<f32>(1.0));
    let amp = clamp(contour * peaking * params.sharpness, vec3<f32>(0.0), vec3<f32>(1.0));
    let avg = (mn + mx) * 0.5;
    return c * (vec3<f32>(1.0) - amp) + avg * amp;
}

@compute @workgroup_size(8, 8, 1)
fn cs_cas(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.width || gid.y >= params.height) {
        return;
    }
    let rgb = cas_run(i32(gid.x), i32(gid.y));
    let idx = gid.y * params.width + gid.x;
    dst[idx] = vec4<f32>(rgb, src[idx].w);
}

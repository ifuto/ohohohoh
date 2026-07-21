// rsift-opt-gfx :: FidelityFX Contrast Adaptive Sharpening (実 dispatch 版)
//
// 公式リファレンス: GPUOpen-Effects/FidelityFX-CAS ffx-cas/ffx_cas.h
//   CasFilter (noScaling) — cross 5 タップ、CAS_SLOW (チャネル毎ウェイト) 高品質パス
//   mn/mx   = min/max(cross + center)     ← 公式は中心を含む
//   amp     = sqrt(sat(min(mn, 1-mx) / mx))
//   w       = amp * (-1 / lerp(8, 5, sat(sharpness)))   (CasSetup const1.x)
//   out     = sat((c + (n+s+e+w)·w) / (1+4w))
// 旧実装 (contour/peaking で平均へ混合) はコントラストを下げる向きで CAS では
// なかったため 2026-07-21 に本式へ修正。
//
// CPU ミラー (完全一致): src/frame_postfx.rs の cas_run_cpu
// (= src/cas.rs の cas_sample を全画素に適用。演算順も同一)
//
// iGPU コア: バッファのみ (sampler / storage texture 不使用)。
// mul/add/div/sqrt/clamp/min/max のみ。sqrt は Vulkan コア精度 (正確丸め 1 ULP 内)、
// Rust 側 f32::sqrt は IEEE 正確丸め — 既存 mirror テストが担保する範囲で
// GPU==CPU bitwise を狙う (MX_FLOOR は c=0 の 0/0 NaN 回避の決定的ガード:
// mx<=MX_FLOOR では mn==mx となりフィルタが恒等に退化するため実害無し)。

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
    // 公式: 中心を含む cross の soft min/max
    let mn = min(min(min(n, s), min(e, w)), c);
    let mx = max(max(max(n, s), max(e, w)), c);
    // CasSetup: sharp = -1 / lerp(8, 5, sat(sharpness))
    let peak = -1.0 / (8.0 + (5.0 - 8.0) * clamp(params.sharpness, 0.0, 1.0));
    // amp = sqrt(sat(min(mn, 1-mx) / mx))   (MX_FLOOR: 1.0e-30 = CPU 側 cas::MX_FLOOR と同一)
    let rcp_m = vec3<f32>(1.0) / max(mx, vec3<f32>(1.0e-30));
    let amp = sqrt(clamp(min(mn, vec3<f32>(1.0) - mx) * rcp_m, vec3<f32>(0.0), vec3<f32>(1.0)));
    // 負ローブカーネル: out = sat((c + (n+s+e+w)·w) / (1+4w))
    let wg = amp * vec3<f32>(peak);
    let rcp_w = vec3<f32>(1.0) / (vec3<f32>(1.0) + vec3<f32>(4.0) * wg);
    return clamp((c + (n + w + e + s) * wg) * rcp_w, vec3<f32>(0.0), vec3<f32>(1.0));
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

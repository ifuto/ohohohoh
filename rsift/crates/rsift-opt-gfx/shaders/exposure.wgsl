// rsift-opt-gfx :: auto-exposure (実 dispatch 版)
//
// 役割分担 (決定性設計):
//  - GPU: 画素毎の luma 計算 (Rec.709、mul/add のみで IEEE 厳密) と
//    露出スカラの画素適用 (clamp 付き乗算)。どちらも GPU==CPU bitwise。
//  - CPU: ヒストグラム集計 + 目標露出 + 適応 (log/exp は WGSL 実装定義のため
//    GPU には置かない — src/exposure.rs の build_histogram / target_exposure /
//    adapt をそのまま使用。luma 入力が bitwise 一致するので計量結果も一致)。
//
// CPU ミラー (完全一致): src/frame_postfx.rs の luma_run_cpu / apply_run_cpu
//   luma(c) = 0.2126 * r + 0.7152 * g + 0.0722 * b   (この加算順で固定)
//   apply(c, e) = clamp(c * e, 0.0, 1.0)             (成分毎、alpha 透過)
// (旧版は `target` 予約語違反 + ヒストグラムを GPU 側で log/exp 使いで
//  非決定的だった問題を、上記の責務分離で解消)

struct ExposureParams {
    count: u32,     // 画素数
    exposure: f32,  // cs_apply で使うスカラ (CPU 計量結果を uniform で供給)
    _pad0: u32,
    _pad1: u32,
};

@group(0) @binding(0) var<uniform> params: ExposureParams;
@group(0) @binding(1) var<storage, read> src: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> luma_out: array<f32>;   // cs_luma
@group(0) @binding(3) var<storage, read_write> dst: array<vec4<f32>>;  // cs_apply

@compute @workgroup_size(64, 1, 1)
fn cs_luma(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.count) {
        return;
    }
    let c = src[gid.x].xyz;
    luma_out[gid.x] = 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
}

@compute @workgroup_size(64, 1, 1)
fn cs_apply(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.count) {
        return;
    }
    let c = src[gid.x];
    let rgb = clamp(c.xyz * params.exposure, vec3<f32>(0.0), vec3<f32>(1.0));
    dst[gid.x] = vec4<f32>(rgb, c.w);
}

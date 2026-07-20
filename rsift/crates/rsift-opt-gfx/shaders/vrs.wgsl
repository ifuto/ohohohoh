// Variable Rate Shading mask generation (実 dispatch 版)。
//
// CPU ミラー (完全一致): src/frame_postfx.rs の vrs_run_cpu
// (= src/vrs.rs の Vrs::build_mask と同一規則)
//   score = clamp(motion, 0, 1) * motion_weight - clamp(variance, 0, 1) * variance_weight
//   code  : >0.6 → 4 / >0.3 → 3 / >0.05 → 2 / >-0.3 → 1 / それ以外 → 0
//
// iGPU コア: バッファのみ。旧版は sampler 宣言 `smp` 欠落 + サンプラ双一次で
// GPU==CPU bitwise 不可能だったため、f32 バッファ直接読みに置き換え
// (モーション/分散フィールドはホストが実フレームから生成したものを upload)。
// ハードウェア VRS は wgpu/WebGPU に存在しないため、マスク生成が本モジュールの
// 実効果 (粗レート画素の削減率メトリクスと可視化に実還元される)。

struct VrsParams {
    dims: vec2<u32>,   // full-res pixel dims
    tile: u32,         // tile size in pixels
    motion_weight: f32,
    variance_weight: f32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

@group(0) @binding(0) var<uniform> params: VrsParams;
@group(0) @binding(1) var<storage, read> motion_in: array<f32>;
@group(0) @binding(2) var<storage, read> var_in: array<f32>;
@group(0) @binding(3) var<storage, read_write> mask_out: array<u32>;

fn select_code(motion: f32, variance: f32, mw: f32, vw: f32) -> u32 {
    let score = clamp(motion, 0.0, 1.0) * mw - clamp(variance, 0.0, 1.0) * vw;
    if (score > 0.6) {
        return 4u;
    } else if (score > 0.3) {
        return 3u;
    } else if (score > 0.05) {
        return 2u;
    } else if (score > -0.3) {
        return 1u;
    }
    return 0u;
}

@compute @workgroup_size(8, 8, 1)
fn cs_vrs_mask(@builtin(global_invocation_id) gid: vec3<u32>) {
    let tw = (params.dims.x + params.tile - 1u) / params.tile;
    let th = (params.dims.y + params.tile - 1u) / params.tile;
    if (gid.x >= tw || gid.y >= th) {
        return;
    }
    let y0 = gid.y * params.tile;
    let y1 = min((gid.y + 1u) * params.tile, params.dims.y);
    let x0 = gid.x * params.tile;
    let x1 = min((gid.x + 1u) * params.tile, params.dims.x);
    var ms = 0.0;
    var vs = 0.0;
    var cnt = 0u;
    for (var y = y0; y < y1; y = y + 1u) {
        for (var x = x0; x < x1; x = x + 1u) {
            let i = y * params.dims.x + x;
            ms = ms + motion_in[i];
            vs = vs + var_in[i];
            cnt = cnt + 1u;
        }
    }
    let motion = ms / f32(cnt);
    let variance = vs / f32(cnt);
    mask_out[gid.y * tw + gid.x] = select_code(motion, variance, params.motion_weight, params.variance_weight);
}

// Checkerboard reconstruction — 未描画 (奇数チェッカ) 画素を 4 つの描画済み
// 斜め近傍から再構成する (実 dispatch 版)。
//
// CPU ミラー (完全一致): src/frame_postfx.rs の checker_run_cpu
//   描画済み: (x + y) & 1 == 0 (= Checkerboard::is_rendered)
//   再構成  : (nw + ne + sw + se) * 0.25 (加算は左結合、Checkerboard::reconstruct と同一)
//   境界    : 斜め近傍が範囲外なら端へ clamp (CPU ミラーと同一規則)
//
// iGPU コア: バッファのみ (旧版は暗黙 return 省略で構文エラー、
// storage texture も write-only 制約のためバッファ化)。

struct CbParams {
    width: u32,
    height: u32,
    _pad0: u32,
    _pad1: u32,
};

@group(0) @binding(0) var<uniform> params: CbParams;
@group(0) @binding(1) var<storage, read> src: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> dst: array<vec4<f32>>;

fn px(x: i32, y: i32) -> vec4<f32> {
    let cx = clamp(x, 0, i32(params.width) - 1);
    let cy = clamp(y, 0, i32(params.height) - 1);
    return src[u32(cy) * params.width + u32(cx)];
}

@compute @workgroup_size(8, 8, 1)
fn cs_checkerboard(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.width || gid.y >= params.height) {
        return;
    }
    let x = i32(gid.x);
    let y = i32(gid.y);
    let idx = gid.y * params.width + gid.x;
    if ((gid.x + gid.y) & 1u) == 0u {
        dst[idx] = src[idx]; // 描画済み画素はそのまま
        return;
    }
    let nw = px(x - 1, y - 1);
    let ne = px(x + 1, y - 1);
    let sw = px(x - 1, y + 1);
    let se = px(x + 1, y + 1);
    dst[idx] = (nw + ne + sw + se) * 0.25;
}

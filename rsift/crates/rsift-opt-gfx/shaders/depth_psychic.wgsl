// RsGraphics Phase C: 主深度 (Depth32Float) → Hi-Z 64x64 保守ダウンサンプル。
//
// Hi-Z カリングの第一段: セル内の **最遠深度 (max)** を保持する。
// 後段の hiz_raster.wgsl は「AABB の最手前深度 z_min > セル最遠深度」なら
// そのセルは完全遮蔽と安全に判定できる (保守方向は常に「描きすぎる」側)。
//
// iGPU 第一級設計: `texture_depth_2d` + `textureLoad` と write-only storage
// テクスチャのみ使用 (いずれも WebGPU core 機能 — filterable depth、
// storage-read texture、アトミック、subgroup は一切要求しない)。

struct Params {
    src_dim: vec2<f32>, // 主深度の画素寸法
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var src_depth: texture_depth_2d;
@group(0) @binding(2) var hiz_out: texture_storage_2d<r32float, write>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let out_dim = textureDimensions(hiz_out);
    if (gid.x >= out_dim.x || gid.y >= out_dim.y) { return; }

    // このセルが代表するソース画素範囲 [x0,x1) × [y0,y1)。
    // floor/ceil で隣接セルと 1px 重なりうるが、max は冪等なので保守的に正しい。
    let sw = params.src_dim.x / f32(out_dim.x);
    let sh = params.src_dim.y / f32(out_dim.y);
    let x0 = u32(floor(f32(gid.x) * sw));
    let y0 = u32(floor(f32(gid.y) * sh));
    let x1 = min(u32(ceil(f32(gid.x + 1u) * sw)), u32(params.src_dim.x));
    let y1 = min(u32(ceil(f32(gid.y + 1u) * sh)), u32(params.src_dim.y));

    var m = 0.0;
    for (var y = y0; y < y1; y = y + 1u) {
        for (var x = x0; x < x1; x = x + 1u) {
            m = max(m, textureLoad(src_depth, vec2<i32>(i32(x), i32(y)), 0));
        }
    }
    textureStore(hiz_out, vec2<i32>(i32(gid.x), i32(gid.y)), vec4<f32>(m, 0.0, 0.0, 1.0));
}

// RsGraphics Phase C: Hi-Z 64x64 のカラー可視化 (検証・デバッグ用)。
// 最遠深度が手前 (遮蔽物あり) ほど明るく、未被覆 (1.0) は暗くなる。
// tint で色付け (デフォルトシアン系)。write-only storage のみ使用 (iGPU core)。

struct DbgParams {
    tint: vec4<f32>,
};

@group(0) @binding(0) var<uniform> params: DbgParams;
@group(0) @binding(1) var hiz: texture_2d<f32>;
@group(0) @binding(2) var out_img: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let d = textureDimensions(out_img);
    if (gid.x >= d.x || gid.y >= d.y) { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));
    let depth = textureLoad(hiz, pos, 0).r;
    let lum = max(1.0 - depth, 0.0); // 手前ほど明るい
    let col = params.tint.rgb * lum;
    textureStore(out_img, pos, vec4<f32>(col, 1.0));
}

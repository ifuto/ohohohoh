// Coarse 3D noise grid + trilinear upsample (実 dispatch 版)。
//
// CPU ミラー (完全一致): src/frame_worldgen.rs の noise_coarse_cpu /
// noise_fill_cpu。
//
// 正準ノイズ = 整数ラティス **値ノイズ** (vhash)。全演算は整数 hash + f32
// 厳密演算 (mul/add/div、sqrt/log/pow/trig 不使用) → GPU==CPU bitwise。
//
// 発見済みバグ (AUDIT 追記8): 旧 CPU 側 perlin3d_dense は i32 引数のみで
// 勾配ノイズを評価するため、全整数ボクセルでラティス零点 (g·0=0) に退化し
// **定数 0.5 を返していた** (render_pipeline が実使用)。また旧 WGSL は
// Rust 風クロージャ構文 `|cx, cy, cz| -> f32 {}` で構文エラー、さらに
// hash3 に f32 を渡す型バグも抱えていた。本ファイルの値ノイズを正準とし、
// CPU 側に同一演算のミラーを新設 (旧 CPU 関数は消費者契約維持のため不触)。

struct UpsampleParams {
    stride: u32,
    seed: u32,
    origin_x: i32,
    origin_y: i32,
    origin_z: i32,
    size_x: u32,
    size_y: u32,
    size_z: u32,
    cave_threshold: f32,
    _pad: u32,
};

@group(0) @binding(0) var<uniform> params: UpsampleParams;
@group(0) @binding(1) var<storage, read_write> coarse: array<f32>;
@group(0) @binding(2) var<storage, read_write> dense: array<f32>;

fn hash3u(x: u32, y: u32, z: u32, seed: u32) -> u32 {
    var h = seed ^ x * 0x9E3779B9u ^ y * 0x85EBCA6Bu ^ z * 0xC2B2AE35u;
    h = h ^ (h >> 16u);
    h = h * 0x7FEB352Du;
    h = h ^ (h >> 15u);
    h = h * 0x846CA68Bu;
    h = h ^ (h >> 16u);
    return h;
}

// ラティス 1 点のハッシュ値 [0, 1]。CPU ミラーと同一 (u32 厳密 → f32 変換)。
fn vhash(ix: i32, iy: i32, iz: i32, seed: u32) -> f32 {
    return f32(hash3u(u32(ix & 255), u32(iy & 255), u32(iz & 255), seed) & 0xFFu) / 255.0;
}

fn coarse_dims() -> vec3<u32> {
    return vec3<u32>(
        (params.size_x / params.stride) + 1u,
        (params.size_y / params.stride) + 1u,
        (params.size_z / params.stride) + 1u,
    );
}

// CPU: noise_coarse_cpu と同一規則。density - cave * threshold。
@compute @workgroup_size(4, 4, 4)
fn cs_coarse_noise(@builtin(global_invocation_id) gid: vec3<u32>) {
    let cd = coarse_dims();
    if (gid.x >= cd.x || gid.y >= cd.y || gid.z >= cd.z) {
        return;
    }
    let wx = params.origin_x + i32(gid.x * params.stride);
    let wy = params.origin_y + i32(gid.y * params.stride);
    let wz = params.origin_z + i32(gid.z * params.stride);
    let density = vhash(wx, wy, wz, params.seed);
    let cave = vhash(wx + 97, wy + 53, wz + 31, params.seed ^ 0xCAFEu);
    coarse[gid.x + gid.y * cd.x + gid.z * cd.x * cd.y] = density - cave * params.cave_threshold;
}

// CPU: noise_fill_cpu (= noise_upsample::trilinear_upsample の補間形と同一)。
@compute @workgroup_size(4, 4, 4)
fn cs_trilinear_fill(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.size_x || gid.y >= params.size_y || gid.z >= params.size_z) {
        return;
    }
    let cd = coarse_dims();
    let stride_f = f32(params.stride);
    let lx = f32(gid.x) / stride_f;
    let ly = f32(gid.y) / stride_f;
    let lz = f32(gid.z) / stride_f;
    let x0 = u32(floor(lx));
    let y0 = u32(floor(ly));
    let z0 = u32(floor(lz));
    let x1 = min(x0 + 1u, cd.x - 1u);
    let y1 = min(y0 + 1u, cd.y - 1u);
    let z1 = min(z0 + 1u, cd.z - 1u);
    let tx = lx - f32(x0);
    let ty = ly - f32(y0);
    let tz = lz - f32(z0);
    let row = cd.x;
    let slab = cd.x * cd.y;
    let c000 = coarse[x0 + y0 * row + z0 * slab];
    let c100 = coarse[x1 + y0 * row + z0 * slab];
    let c010 = coarse[x0 + y1 * row + z0 * slab];
    let c110 = coarse[x1 + y1 * row + z0 * slab];
    let c001 = coarse[x0 + y0 * row + z1 * slab];
    let c101 = coarse[x1 + y0 * row + z1 * slab];
    let c011 = coarse[x0 + y1 * row + z1 * slab];
    let c111 = coarse[x1 + y1 * row + z1 * slab];
    let x00 = c000 + tx * (c100 - c000);
    let x10 = c010 + tx * (c110 - c010);
    let x01 = c001 + tx * (c101 - c001);
    let x11 = c011 + tx * (c111 - c011);
    let y0v = x00 + ty * (x10 - x00);
    let y1v = x01 + ty * (x11 - x01);
    dense[gid.x + gid.y * params.size_x + gid.z * params.size_x * params.size_y] = y0v + tz * (y1v - y0v);
}

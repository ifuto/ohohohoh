// Linear BVH — Morton code + frustum sphere cull (実 dispatch 版)。
//
// CPU ミラー (完全一致): src/frame_worldgen.rs の lbvh_codes_cpu /
// lbvh_cull_cpu (= src/lbvh.rs の part1by2 / morton3 / Lbvh::cull と同一規則)
//   正規化: n = u32(clamp((c - min) / span, 0.0, 0.9999) * 1024.0)  (軸毎、切捨て)
//   morton = part1by2(nx) | part1by2(ny) << 1 | part1by2(nz) << 2  (整数のみ)
//   cull   : dist = a*x + b*y + c*z + d (この加算順で固定)、dist < -r で outside
//
// iGPU コア: 整数演算 + 厳密 f32 のみ (旧版は引数の `mut` 修飾が
// 現行 WGSL で予約語違反)。優先 radius は CPU 側で長さ計算済みのものを
// 受け取る (sqrt を走査側に持ち込まない設計)。

struct LbvhParams {
    count: u32,
    plane_count: u32,
    _pad0: u32,
    _pad1: u32,
    // 正規化パラメータ (codes 用)
    min: vec3<f32>,
    _pad2: f32,
    span: vec3<f32>,
    _pad3: f32,
    // frustum planes (最大 6、vec4 = (a, b, c, d))
    planes: array<vec4<f32>, 6>,
};

@group(0) @binding(0) var<uniform> params: LbvhParams;
@group(0) @binding(1) var<storage, read> spheres: array<vec4<f32>>; // (center.xyz, radius)
@group(0) @binding(2) var<storage, read_write> codes: array<u32>;
@group(0) @binding(3) var<storage, read_write> visible: array<u32>; // 0/1 マスク

fn part1by2(n_in: u32) -> u32 {
    var n = n_in & 0x3ffu;
    n = (n | (n << 16u)) & 0x30000ffu;
    n = (n | (n << 8u)) & 0x300f00fu;
    n = (n | (n << 4u)) & 0x30c30c3u;
    n = (n | (n << 2u)) & 0x9249249u;
    return n;
}

fn morton3(x: u32, y: u32, z: u32) -> u32 {
    return part1by2(x) | (part1by2(y) << 1u) | (part1by2(z) << 2u);
}

@compute @workgroup_size(64, 1, 1)
fn cs_morton(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.count) {
        return;
    }
    let c = spheres[gid.x].xyz;
    let nx = u32(clamp((c.x - params.min.x) / params.span.x, 0.0, 0.9999) * 1024.0);
    let ny = u32(clamp((c.y - params.min.y) / params.span.y, 0.0, 0.9999) * 1024.0);
    let nz = u32(clamp((c.z - params.min.z) / params.span.z, 0.0, 0.9999) * 1024.0);
    codes[gid.x] = morton3(nx, ny, nz);
}

@compute @workgroup_size(64, 1, 1)
fn cs_cull(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.count) {
        return;
    }
    let s = spheres[gid.x];
    var vis = 1u;
    for (var i = 0u; i < params.plane_count; i = i + 1u) {
        let p = params.planes[i];
        let dist = p.x * s.x + p.y * s.y + p.z * s.z + p.w;
        if (dist < -s.w) {
            vis = 0u;
        }
    }
    visible[gid.x] = vis;
}

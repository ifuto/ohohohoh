// terrain task/mesh 等価エミュレーション (実 dispatch 版)。
//
// 経緯 (正直な記録): 旧版は `enable mesh_shader;` を使う HW mesh shader
// もどきだったが、**mesh shader は WGSL/wgpu/WebGPU に存在しない**
// (naga が構文拒否 → どのドライバでもコンパイル不能の死にシェーダ)。
// そのため rsift の task/mesh 戦略は、HW mesh shader 相当の処理を
// 「compute meshlet カリング → (terrain_task_emulation.wgsl) indirect 化 →
// (terrain_vertex_pull.wgsl) vertex pull 描画」の 3 段エミュレーションで
// 実現する。本ファイルはその **1 段目**: meshlet 視錐台カリング。
//
// CPU ミラー (完全一致): src/frame_worldgen.rs の meshlet_cull_cpu
//   dist = a*x + b*y + c*z + d (この加算順で固定)、dist < -r で outside
//
// iGPU コア: 厳密 f32 のみ (sqrt/log/pow/trig 不使用、整数決定的ループ)。

struct MeshletParams {
    count: u32,
    plane_count: u32,
    _pad0: u32,
    _pad1: u32,
    planes: array<vec4<f32>, 6>, // (a, b, c, d) × plane_count
};

@group(0) @binding(0) var<uniform> params: MeshletParams;
@group(0) @binding(1) var<storage, read> meshlets: array<vec4<f32>>; // (center.xyz, radius)
@group(0) @binding(2) var<storage, read_write> visible: array<u32>;  // 0/1 マスク

@compute @workgroup_size(64, 1, 1)
fn cs_meshlet_cull(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.count) {
        return;
    }
    let s = meshlets[gid.x];
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

// rsift-opt-gfx :: bindless handle pack/unpack (実 dispatch 版)
//
// CPU ミラー (完全一致): src/bindless.rs の pack_handle / unpack_handle
//   pack   = ((set & 0xF) << 28) | ((binding & 0xFF) << 20) | (index & 0xFFFFF)
//   unpack = ((h >> 28) & 0xF, (h >> 20) & 0xFF, h & 0xFFFFF)
// Layout (32 bits): [ set:4 | binding:8 | index:20 ]
//
// iGPU コア: 整数演算のみ (bitwise 決定的)。バインドレス配列機能は
// wgpu/WebGPU コアに存在しないため、本 pass は「ハンドル ⇔ (set,binding,index)
// テーブルの実変換」を担い、テクスチャ実体は texture_atlas のアトラス側で
// 解決する (ハンドルの index がアトラススロットを指す設計)。

struct BindlessParams {
    count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

@group(0) @binding(0) var<uniform> params: BindlessParams;
@group(0) @binding(1) var<storage, read> handles: array<u32>;
@group(0) @binding(2) var<storage, read_write> unpacked: array<u32>; // 3 words/handle

fn pack_handle(set_idx: u32, binding_idx: u32, index: u32) -> u32 {
    return ((set_idx & 0xFu) << 28u) | ((binding_idx & 0xFFu) << 20u) | (index & 0xFFFFFu);
}

fn unpack_handle(h: u32) -> vec3<u32> {
    return vec3<u32>((h >> 28u) & 0xFu, (h >> 20u) & 0xFFu, h & 0xFFFFFu);
}

// CPU: unpack_handle(h) と同一。roundtrip 検証 (pack(unpack(h)) == h) を
// CPU 側 (frame_worldgen::bindless_unpack_cpu) が実 assert する。
@compute @workgroup_size(64, 1, 1)
fn cs_unpack_handles(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.count) {
        return;
    }
    let h = handles[gid.x];
    let u = unpack_handle(h);
    let base = gid.x * 3u;
    unpacked[base] = u.x;
    unpacked[base + 1u] = u.y;
    unpacked[base + 2u] = u.z;
}

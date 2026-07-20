// RsGraphics Phase D1 — Voxel Cone Tracing 実 dispatch シェーダ (iGPU コア機能のみ)。
//
// CPU 参照 (完全一致を目指すミラー):
//   - ツリー降下: src/svo.rs::sample_lod (from_column 軸別半分割ツリー、最深 1x1x1 葉)
//   - コーンループ: src/voxel_cone_tracing.rs::VoxelConeTracing::trace_diffuse_cone
// 語列レイアウト: src/svo.rs::to_gpu_words (GPU_NODE_STRIDE = 10 u32/node)
//   word0 = 0:Empty / 1:Branch / (2+b):Uniform(block b)、words 1..=8 = 子 (dz*4+dy*2+dx)
//
// iGPU 要件: compute 64x1x1・read-only storage (nodes/cones)・read_write storage
// (出力) のみ。storage texture / アトミック / subgroups / bindless 不使用。
// ループは WGSL 現行仕様の loop+break (while は削除済み)。
// f32 演算順は CPU と同一に固定し、lod は floor(log2) をビット抽出で
// 実装 (correctly-rounded log2f と一致、実装定義 libm 誤差の影響を排除)。

struct VctParams {
    bounds: vec3<f32>,  // SVO ローカル論理境界 (16, 2 冪 pad 高さ, 16)
    root: u32,
    cap: u32,           // ツリー max_depth (16x64x16 → 6)
    node_count: u32,    // 監査用
    cone_count: u32,
    _pad: u32,
};

struct ConeWgsl {
    o: vec3<f32>,       // SVO ローカル座標のコーン原点
    aperture: f32,      // half-angle tan (例: 30° なら 0.577)
    d: vec3<f32>,       // 正規化済み方向 (CPU 側で正規化して送る)
    max_dist: f32,
};

@group(0) @binding(0) var<uniform> params: VctParams;
@group(0) @binding(1) var<storage, read> nodes: array<u32>;
@group(0) @binding(2) var<storage, read> cones: array<ConeWgsl>;
@group(0) @binding(3) var<storage, read_write> out_radiance: array<vec4<f32>>;

const NODE_STRIDE: u32 = 10u;
const NEUTRAL_ALBEDO: vec3<f32> = vec3<f32>(0.5, 0.5, 0.5);
const NO_HIT: vec4<f32> = vec4<f32>(0.0, 0.0, 0.0, -1.0);

// CPU: diameter.log2().clamp(0.0, 10.0) as u32 と一致。
// d > 1 の正規数では floor(log2(d)) == 指数部 - 127 が常に厳密成立する。
fn lod_from_diameter(d: f32) -> u32 {
    if (!(d > 1.0)) {
        return 0u;
    }
    let e = i32(bitcast<u32>(d) >> 23u) - 127;
    if (e <= 0) {
        return 0u;
    }
    return u32(min(e, 10));
}

// CPU: SparseVoxelOctree::sample_lod と同一規則。
// 戻り値 w = alpha、ヒット無しは w = -1.0 (Option::None 相当)。
fn svo_sample_lod(p: vec3<f32>, lod: u32) -> vec4<f32> {
    // NaN でも CPU の !(x>=0 && ...) → None と同じ結果になる否定形で判定
    if (!(p.x >= 0.0 && p.y >= 0.0 && p.z >= 0.0 &&
          p.x < params.bounds.x && p.y < params.bounds.y && p.z < params.bounds.z)) {
        return NO_HIT;
    }
    let budget = params.cap - min(lod, params.cap);
    var node_idx = params.root;
    var org = vec3<f32>(0.0, 0.0, 0.0);
    var size = params.bounds;
    // 有界 for (最新 naga validator の behavior 解析は、return のみで
    // break を持たない無限 loop を拒否するため。depth の進行は CPU と同一)。
    for (var depth = 0u; depth <= 32u; depth = depth + 1u) {
        let base = node_idx * NODE_STRIDE;
        if (base + 9u >= arrayLength(&nodes)) {
            return NO_HIT; // アクセスガード (正常ツリーでは到達しない)
        }
        let w0 = nodes[base];
        if (w0 == 0u) {
            return NO_HIT; // Empty
        }
        if (w0 >= 2u) {
            return vec4<f32>(NEUTRAL_ALBEDO, 1.0); // Uniform(block = w0-2)
        }
        // Branch
        if (depth >= budget) {
            // 粗 LOD: 直下 8 子の実占有率 (非 Empty 数 / 8)
            var solid = 0u;
            for (var i = 0u; i < 8u; i = i + 1u) {
                let c = nodes[base + 1u + i];
                let cbase = c * NODE_STRIDE;
                if (cbase + 9u < arrayLength(&nodes) && nodes[cbase] != 0u) {
                    solid = solid + 1u;
                }
            }
            if (solid > 0u) {
                return vec4<f32>(NEUTRAL_ALBEDO, f32(solid) / 8.0);
            }
            return NO_HIT;
        }
        let half = size * 0.5;
        // CPU: (x >= org + half) as usize — NaN → 0 と同じ select 規則
        let dx = select(0.0, 1.0, p.x >= org.x + half.x);
        let dy = select(0.0, 1.0, p.y >= org.y + half.y);
        let dz = select(0.0, 1.0, p.z >= org.z + half.z);
        let ci = u32(dx + dy * 2.0 + dz * 4.0);
        org = org + half * vec3<f32>(dx, dy, dz);
        size = half;
        node_idx = nodes[base + 1u + ci];
    }
    // depth 上限到達 (cap<=8 の正常ツリーでは到達しない安全弁)
    return NO_HIT;
}

// CPU: VoxelConeTracing::trace_diffuse_cone と同一ループ・同一 f32 演算順。
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.cone_count) {
        return;
    }
    let cone = cones[gid.x];
    var accum = vec3<f32>(0.0, 0.0, 0.0);
    var aa = 0.0f;
    var dist = 0.5f;
    loop {
        if (!(dist < cone.max_dist && aa < 0.99)) {
            break;
        }
        let pos = cone.o + cone.d * dist;
        let diameter = max(2.0 * cone.aperture * dist, 1.0);
        let lod = lod_from_diameter(diameter);
        let s = svo_sample_lod(pos, lod);
        if (s.w > 0.001) {
            let weight = s.w * (1.0 - aa);
            accum = accum + s.xyz * weight;
            aa = aa + weight;
        }
        dist = dist + diameter * 0.75;
    }
    out_radiance[gid.x] = vec4<f32>(accum, clamp(aa, 0.0, 1.0));
}

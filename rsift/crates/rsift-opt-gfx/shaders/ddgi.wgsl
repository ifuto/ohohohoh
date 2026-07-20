// RsGraphics Phase D2 — DDGI (Dynamic Diffuse GI) probe volume 実 dispatch。
//
// CPU 参照 (完全一致を目指すミラー): src/frame_ddgi.rs
//  - probe ray-march: SVO 占有判定 (svo_sample_lod, lod=0) による 0.5 刻み march
//  - atlas blend: octahedral 8x8texel × dot^4 重み (pow 不使用) で
//    irradiance (sky 見通し) + Chebyshev 用 depth moments を蓄積
//
// 設計上の決定性保証 (iGPU コア機能のみ, GPU==CPU bitwise のため):
//  - trig/pow 不使用: ray 方向は CPU 生成アップロード、重みは二乗 x2
//  - sqrt (normalize) / 除算は IEEE 厳密丸め (WGSL 規格)
//  - ode を跨ぐ走査共有: 以下は voxel_cone_tracing.wgsl とバイト同一領域。

// ==== SVO-TRACE SHARED REGION BEGIN ====
// この領域は shaders/ddgi.wgsl と **バイト同一** で共有される
// (frame_ddgi テストが両ファイルの共有領域一致を実 assert する。
//  片方だけを編集してはいけない — 必ず両方を同時に更新すること)。

struct VctParams {
    bounds: vec3<f32>,  // SVO ローカル論理境界 (16, 2 冪 pad 高さ, 16)
    root: u32,
    cap: u32,           // ツリー max_depth (16x64x16 → 6)
    node_count: u32,    // 監査用
    cone_count: u32,    // VCT ではコーン数、DDGI では未使用 (0)
    _pad: u32,
};

@group(0) @binding(0) var<uniform> params: VctParams;
@group(0) @binding(1) var<storage, read> nodes: array<u32>;

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
// ==== SVO-TRACE SHARED REGION END ====

// ---------- DDGI 固有: プローブ体積・ray 方向・atlas 出力 ----------

struct DdgiParams {
    origin: vec3<f32>,     // プローブ体積の SVO ローカル原点 (cell (0,0,0) の角)
    max_dist: f32,         // ray march 上限 / sky 判定距離
    cell: vec3<f32>,       // プローブ間隔 (ブロック)
    probe_count: u32,
    dims: vec3<u32>,       // (dx, dy, dz) プローブ数
    ray_count: u32,
    sky: vec3<f32>,        // miss ray (sky 見通し) の放射輝度
    oct_w: u32,            // octahedral atlas の 1 プローブあたり幅 (texel)
};

@group(0) @binding(2) var<uniform> ddgi: DdgiParams;
@group(0) @binding(3) var<storage, read> ray_dirs: array<vec3<f32>>;
@group(0) @binding(4) var<storage, read_write> rays_out: array<f32>;
@group(0) @binding(5) var<storage, read_write> atlas_irr: array<vec4<f32>>;
@group(0) @binding(6) var<storage, read_write> atlas_mom: array<vec2<f32>>;

// octahedral encode/decode (標準 diamond wrap。既存 ddgi.wgsl の実装を継承)。
// CPU ミラーは src/frame_ddgi.rs の oct_encode_wgsl/oct_decode_wgsl。
fn octEncode(n : vec3<f32>) -> vec2<f32> {
    let s = max(abs(n.x) + abs(n.y) + abs(n.z), 1e-8);
    var o = vec2<f32>(n.x / s, n.y / s);
    if (n.z < 0.0) {
        o = vec2<f32>(
            (1.0 - abs(o.y)) * select(-1.0, 1.0, o.x >= 0.0),
            (1.0 - abs(o.x)) * select(-1.0, 1.0, o.y >= 0.0));
    }
    return o;
}

fn octDecode(f : vec2<f32>) -> vec3<f32> {
    var n = vec3<f32>(f.x, f.y, 1.0 - abs(f.x) - abs(f.y));
    if (n.z < 0.0) {
        n = vec3<f32>(
            (1.0 - abs(f.y)) * select(-1.0, 1.0, f.x >= 0.0),
            (1.0 - abs(f.x)) * select(-1.0, 1.0, f.y >= 0.0),
            n.z);
    }
    return normalize(n);
}

fn chebyshev(m1 : f32, m2 : f32, d : f32) -> f32 {
    if (d <= m1) { return 1.0; }
    let variance = max(m2 - m1 * m1, 0.0);
    let diff = d - m1;
    return clamp(variance / (variance + diff * diff), 0.0, 1.0);
}

// Pass 1: プローブ毎 ray march (SVO 占有判定、出力は命中距離 / miss 時 max_dist)。
@compute @workgroup_size(64)
fn probe_rays(@builtin(global_invocation_id) gid: vec3<u32>) {
    let total = ddgi.probe_count * ddgi.ray_count;
    if (gid.x >= total) { return; }
    let p = gid.x / ddgi.ray_count;
    let r = gid.x % ddgi.ray_count;
    let dx = p % ddgi.dims.x;
    let dy = (p / ddgi.dims.x) % ddgi.dims.y;
    let dz = p / (ddgi.dims.x * ddgi.dims.y);
    let base = ddgi.origin
        + (vec3<f32>(f32(dx), f32(dy), f32(dz)) + vec3<f32>(0.5, 0.5, 0.5)) * ddgi.cell;
    let dir = ray_dirs[r];
    var t = 0.5f;
    var hit = ddgi.max_dist;
    for (var step = 0u; step <= 128u; step = step + 1u) {
        if (t >= ddgi.max_dist) { break; }
        let pos = base + dir * t;
        let s = svo_sample_lod(pos, 0u);
        if (s.w > 0.001) { hit = t; break; }
        t = t + 0.5f;
    }
    rays_out[gid.x] = hit;
}

// Pass 2: octahedral atlas blend (dot^4 重みで irradiance + moments 蓄積)。
@compute @workgroup_size(64)
fn atlas_blend(@builtin(global_invocation_id) gid: vec3<u32>) {
    let tex = ddgi.oct_w * ddgi.oct_w;
    let total = ddgi.probe_count * tex;
    if (gid.x >= total) { return; }
    let p = gid.x / tex;
    let t = gid.x % tex;
    let tx = t % ddgi.oct_w;
    let ty = t / ddgi.oct_w;
    // uv in [-1,1]: (texel + 0.5)/oct*2 - 1 — CPU ミラーと同一式
    let uv = vec2<f32>(
        (f32(tx) + 0.5) / f32(ddgi.oct_w) * 2.0 - 1.0,
        (f32(ty) + 0.5) / f32(ddgi.oct_w) * 2.0 - 1.0);
    let dir = octDecode(uv);
    var irr = vec3<f32>(0.0, 0.0, 0.0);
    var wm = 0.0f;
    var m1 = 0.0f;
    var m2 = 0.0f;
    let base_idx = p * ddgi.ray_count;
    for (var r = 0u; r < ddgi.ray_count; r = r + 1u) {
        let rd = ray_dirs[r];
        var w = clamp(dot(dir, rd), 0.0, 1.0);
        w = w * w;
        w = w * w; // dot^4 (pow 不使用: GPU 実装定義誤差の排除)
        let d = rays_out[base_idx + r];
        let sky_w = select(0.0f, 1.0f, d >= ddgi.max_dist);
        irr = irr + ddgi.sky * (sky_w * w);
        wm = wm + w;
        m1 = m1 + d * w;
        m2 = m2 + d * d * w;
    }
    if (wm > 0.0) {
        irr = irr / wm;
        m1 = m1 / wm;
        m2 = m2 / wm;
    }
    atlas_irr[gid.x] = vec4<f32>(irr, 1.0);
    atlas_mom[gid.x] = vec2<f32>(m1, m2);
}

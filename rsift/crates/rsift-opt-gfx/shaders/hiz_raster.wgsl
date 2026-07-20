// RsGraphics Phase C: AABB 群 × Hi-Z オクルージョンテスト (カバレッジ計算)。
//
// 各ブロック (AABB) を view_proj で射影し、NDC 矩形が被覆する全 Hi-Z セルを
// 走査する。「ブロックの最手前深度 z_min <= セル最遠深度 + DEPTH_EPS」なら
// そのセルは視認可能。coverage = 視認可能セル数、vis = coverage > 0。
//
// 保守性の保証:
//   - AABB は実ジオメトリを包含するので、ジオメトリが見えるなら AABB も
//     必ず coverage > 0 → 見えるものを誤カリングしない。
//     (逆に偽陽性 = 見えないのに描く、は許容: Hi-Z カリングの標準的性質)
//   - カメラ後方・画面外・far 越えなど判定不能ケースは coverage=1 で
//     保守的可視 (描画継続) に倒す。
//
// CPU 参照: `frame_reference::hiz_test_reference` が本規則の精密ミラー。
//
// iGPU 第一級設計: `@workgroup_size(1)` で 1 スレッド = 1 ブロック
// (dispatch = n_blocks)。アトミック / subgroup / storage-read texture /
// bindless / mesh shader は不使用。バッファのみの読み書きで完結する。

struct HizUniforms {
    view_proj: mat4x4<f32>,
    hiz_dim: vec2<f32>, // Hi-Z マップ寸法 (e.g. 64, 64)
    _pad: vec2<f32>,
};

struct BlockBox {
    lo: vec4<f32>, // xyz = min (w は詰め物)
    hi: vec4<f32>, // xyz = max (w は詰め物)
};

@group(0) @binding(0) var<uniform> uniforms: HizUniforms;
@group(0) @binding(1) var hiz: texture_2d<f32>;
@group(0) @binding(2) var<storage, read> blocks: array<BlockBox>;
@group(0) @binding(3) var<storage, read_write> coverage: array<u32>;
@group(0) @binding(4) var<storage, read_write> vis: array<u32>;

// 自己深度との一致面を可視扱いにするマージン (前フレーム深度で自分自身を
// テストする静止シーンで誤カリングしないため)。
const DEPTH_EPS: f32 = 1e-4;

@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&blocks)) { return; }
    let b = blocks[idx];

    // 8 角を射影して NDC bbox + 最手前深度 z_min を得る。
    var ndc_min = vec2<f32>(1e9, 1e9);
    var ndc_max = vec2<f32>(-1e9, -1e9);
    var z_min = 1e9;
    var any_invalid = false;
    for (var i = 0u; i < 8u; i = i + 1u) {
        let px = select(b.lo.x, b.hi.x, (i & 1u) != 0u);
        let py = select(b.lo.y, b.hi.y, (i & 2u) != 0u);
        let pz = select(b.lo.z, b.hi.z, (i & 4u) != 0u);
        let clip = uniforms.view_proj * vec4<f32>(px, py, pz, 1.0);
        let w = clip.w;
        // near 面より手前 (w <= 0) の角が 1 つでもある = near 跨ぎ or 後方。
        // 角を除外して続けると z_min が奥に偏り誤カリングしうるため、
        // このケースは全て判定不能 (保守的可視) に倒す。
        if (!(w > 1e-5)) {
            any_invalid = true;
            continue;
        }
        let ndc = clip.xyz / w;
        ndc_min = min(ndc_min, ndc.xy);
        ndc_max = max(ndc_max, ndc.xy);
        z_min = min(z_min, ndc.z);
    }

    // 判定不能 (near 跨ぎ / カメラ後方 / 画面外 / far 越え) → 保守的可視。
    let offscreen = ndc_max.x < -1.0 || ndc_min.x > 1.0 ||
        ndc_max.y < -1.0 || ndc_min.y > 1.0 || z_min > 1.0;
    if (any_invalid || offscreen) {
        coverage[idx] = 1u;
        vis[idx] = 1u;
        return;
    }

    // NDC → Hi-Z セル矩形 (テクスチャは上原点なので y 反転)。
    // cx1/cy1 は排他上限。幅ゼロの縮退矩形は 1 セルに拡張 (空走査で
    // coverage=0 = 誤カリングを起こさないよう下駄を履かせる)。
    let h = uniforms.hiz_dim;
    let cx0 = u32(clamp(floor((ndc_min.x * 0.5 + 0.5) * h.x), 0.0, h.x - 1.0));
    let cy0 = u32(clamp(floor((0.5 - ndc_max.y * 0.5) * h.y), 0.0, h.y - 1.0));
    let cx1 = max(cx0 + 1u, u32(clamp(ceil((ndc_max.x * 0.5 + 0.5) * h.x), 1.0, h.x)));
    let cy1 = max(cy0 + 1u, u32(clamp(ceil((0.5 - ndc_min.y * 0.5) * h.y), 1.0, h.y)));

    var cov = 0u;
    for (var cy = cy0; cy < cy1; cy = cy + 1u) {
        for (var cx = cx0; cx < cx1; cx = cx + 1u) {
            let farthest = textureLoad(hiz, vec2<i32>(i32(cx), i32(cy)), 0).r;
            if (z_min <= farthest + DEPTH_EPS) { cov = cov + 1u; }
        }
    }
    coverage[idx] = cov;
    vis[idx] = select(0u, 1u, cov > 0u);
}

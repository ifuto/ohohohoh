// SIMD Frustum AABB culling — compute kernel (1 box = 1 thread, SoA layout).
// CPU 参照実装: simd_frustum.rs (`cull_soa_portable` / `intersects`)。
//
// ## 数学的等価契約 (2026-07-22 wave 39: 旧コメントのみのスタブを実装化)
// 各スレッドは CPU 参照と**同一の式**を同一規則で評価する:
//   p-vertex 選択: px = select(min_x[i], max_x[i], pl.x >= 0.0) (成分ごと)
//   dist = pl.x*px + pl.y*py + pl.z*pz + pl.w   // 左結合 ((ax+by)+cz)+d
//   dist < 0.0 でその平面に対し outside → 0 を書いて early exit
//   全 6 平面生存で 1 を書き込む
// NaN 係数: `>= 0.0` は false → min 側選択 (CPU scalar と同一規則)。
// NaN dist: `< 0.0` は false → 可視側 (保守的、CPU と一致)。
// early exit は ok フラグが単調減少のみのため full loop と結果一致 (証明済)。
//
// ## bit 厳密性の境界 (正直な注記)
// 個々の f32 演算は IEEE-754 binary32 (左右結合も WGSL 文法で固定) だが、
// WGSL の設計は評価戦略 (reassociation / FMA fusion) の裁量を実装に
// 認めるため、CPU 参照 (丸めあり左結合) との**bit 級一致は保証しない**。
// 距離 0 ごく近傍の境界箱で判定が割れ得る。ただし乖離機構は wave 29 で
// 実機実証済み (AVX2-FMA vs scalar) の同一種であり、結果はどちら側でも
// 「真の距離が負なら必ず除去」の保守性を丸め誤差内で共有するため
// カリングとして実害はない。厳密一致が要る経路は CPU の
// `cull_soa_portable` を使うこと (full_graph_wiring:535 が実例)。

struct Params {
    count: u32,
    // 6 平面 (a,b,c,d): ax+by+cz+d >= 0 が内側 (法線内向き)
    planes: array<vec4<f32>, 6>,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> min_x: array<f32>;
@group(0) @binding(2) var<storage, read> min_y: array<f32>;
@group(0) @binding(3) var<storage, read> min_z: array<f32>;
@group(0) @binding(4) var<storage, read> max_x: array<f32>;
@group(0) @binding(5) var<storage, read> max_y: array<f32>;
@group(0) @binding(6) var<storage, read> max_z: array<f32>;
@group(0) @binding(7) var<storage, read_write> visibility: array<u32>;

@compute @workgroup_size(64)
fn cs_cull(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= params.count) {
        return;
    }
    for (var pi = 0u; pi < 6u; pi = pi + 1u) {
        let pl = params.planes[pi];
        // p-vertex: 平面法線の各成分符号で「最も内側の角」を選ぶ
        let px = select(min_x[i], max_x[i], pl.x >= 0.0);
        let py = select(min_y[i], max_y[i], pl.y >= 0.0);
        let pz = select(min_z[i], max_z[i], pl.z >= 0.0);
        // CPU 参照と同一の左結合評価
        let dist = pl.x * px + pl.y * py + pl.z * pz + pl.w;
        if (dist < 0.0) {
            visibility[i] = 0u;
            return;
        }
    }
    visibility[i] = 1u;
}

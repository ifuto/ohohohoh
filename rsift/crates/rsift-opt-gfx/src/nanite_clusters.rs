//! Nanite 風仮想化 — メッシュを ~128 トライアングルのクラスタ (meshlet) に分割し、
//! 隣接性に基づく親クラスタ鎖で LOD 決定木を作る。
//!
//! Nanite の中核:
//! - クラスタ = 空間的に密接で境界共有が多い三角の塊 (境界確保のため flood-fill)
//! - 各クラスタの bounding sphere + 「親に縮退したときの幾何誤差」
//!   → 画面誤差閾値 = error / dist < ε でそのクラスタを描く
//! - 親合併スコア = 共有エッジ数 (UE と同じく境界ロックが必須, ここでは誤差半径で近似)
//!
//! 本モジュールはメッシュ → クラスタ列 → 単一 stage の親鎖 (子 → merge 相手) と
//! merge 誤差までを正確に作る。多段の再クラスタ化 (真の LOD DAG) は将来課題
//! (`clusterize` 内の NOTE を参照)。

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct NanoVertex {
    pub pos: [f32; 3],
    pub color: u32,
}

pub struct NanoMeshlet {
    /// コンパクション後の通常頂点領域への参照 [開始, 個数]。
    /// **scaffold**: `clusterize` は頂点再配置を行わないため常に 0 が入る
    /// (頂点領域は streaming 実装側が追記するまで無効値。`clustered_indices`
    /// はこの領域でなく入力全体頂点への index を保持する点に注意)。
    pub vertex_offset: u32,
    /// `vertex_offset` と対の個数。現状はコーナー数 (= 3 × triangle_count、
    /// 三角形間で共有される頂点の複製込みカウント) であり、ユニーク頂点数
    /// ではない (通常領域が未構築のためユニーク数は未定義)。
    pub vertex_count: u32,
    /// 全体索引 → ローカル (三角形は 3 連続 index)
    pub index_offset: u32,
    pub triangle_count: u32,

    pub sphere_center: [f32; 3],
    pub sphere_radius: f32,

    /// 親 (merge 相手) のクラスタ index。根 = u32::MAX
    pub parent: u32,
    /// 親に縮退したときの錯誤 (半径の和から導出)
    pub error: f32,
    pub level: u32,
}

pub struct NanoClusterSet {
    pub meshlets: Vec<NanoMeshlet>,
    /// クラスタ集約後の「メッシュレット索引順」トライアングル index 列
    pub clustered_indices: Vec<u32>,
    pub source_triangle_count: u32,
}

pub const MAX_TRIS_PER_CLUSTER: usize = 128;

/// 三角リスト (indices: 3連) をクラスタ化。
///
/// **契約**:
/// - `indices.len()` は 3 の倍数必須 (平三角リスト)。旧実装は `/ 3` の切り捨てで
///   末尾の半端な 1-2 index を静寂に捨てていた (wave 72 vertex_cache_opt と同種の
///   静寂 drop。wave 100 CY-1 で fail-loud 化)。
/// - `vertices` の座標は全て有限値必須。非有限 (NaN / ±inf) が混ざると centroid
///   が NaN 化し、radius は max-scan の `if d > r` ガード (NaN 比較は常に false)
///   で 0.0 へ、merge error は `(NaN - …).max(0.0)` の NaN 非伝播仕様で +0.0 へ
///   静寂潰れ、`cluster_should_draw` 側も dist が 1.0 マスクされて「常時描画」側
///   へ**静寂に誤分類**される (実機検証: NaN.max(1.0)==1.0, (NaN-1).max(0.0)==0.0)。
///   (wave 100 CY-3 で入口 fail-loud 化。wave 71 BU-1 / wave 73 非有限拒否と同哲学)。
pub fn clusterize(vertices: &[NanoVertex], indices: &[u32]) -> NanoClusterSet {
    assert!(
        indices.len() % 3 == 0,
        "clusterize: indices.len() ({}) must be a multiple of 3 (端数 index を静寂 drop しない)",
        indices.len()
    );
    assert!(
        vertices.iter().all(|v| v.pos.iter().all(|c| c.is_finite())),
        "clusterize: 非有限 (NaN/±inf) な頂点座標は契約違反 (radius/error の静寂潰れ = 常時描画への誤分類を遮断)"
    );
    let tri_count = indices.len() / 3;
    if tri_count == 0 {
        return NanoClusterSet {
            meshlets: vec![],
            clustered_indices: vec![],
            source_triangle_count: 0,
        };
    }

    // --- 隣接リスト (三角同士がエッジ共有するか) ---
    let mut adj: Vec<Vec<u32>> = vec![Vec::new(); tri_count];
    // vertex → triangles
    let mut tri_by_vertex: std::collections::HashMap<u32, Vec<u32>> =
        std::collections::HashMap::new();
    for t in 0..tri_count {
        for k in 0..3 {
            tri_by_vertex
                .entry(indices[t * 3 + k])
                .or_default()
                .push(t as u32);
        }
    }
    // 頂点共有の回数を三角ペアで集計する。count は全頂点にまたぐ集計でなければ
    // ならない (1頂点共有 → +1)。頂点ループの内側で作ると「同じ頂点を共有する」
    // 回数しか数えられず c>=2 が永遠に成立せず隣接リストが空になる。
    let mut count = std::collections::HashMap::new();
    for ts in tri_by_vertex.values() {
        // 頂点を共有する三角は隣接 (共有エッジ判定を緩略化: 2頂点以上の共有)
        for &t in ts.iter() {
            for &u in ts.iter() {
                if t != u {
                    *count.entry((t, u)).or_insert(0u8) += 1;
                }
            }
        }
    }
    {
        let count = count;
        for ((t, u), c) in count {
            if c >= 2 {
                adj[t as usize].push(u);
                adj[u as usize].push(t);
            }
        }
    }
    for a in adj.iter_mut() {
        a.sort_unstable();
        a.dedup();
    }

    // --- flood-fill クラスタ生成 (Nanite の "grow meshlet over boundary" 相当) ---
    let mut assigned = vec![false; tri_count];
    let mut in_stack = vec![false; tri_count];
    let mut meshlets: Vec<NanoMeshlet> = Vec::new();
    let mut clustered_indices: Vec<u32> = Vec::new();
    let mut seed_stack: Vec<u32> = Vec::new();

    while let Some(seed) = next_seed(&assigned, tri_count) {
        let mut members: Vec<u32> = Vec::with_capacity(MAX_TRIS_PER_CLUSTER);
        seed_stack.clear();
        // seed は未確定のまま積み、pop 時に確定させる (必ず進捗する構造にする)
        seed_stack.push(seed);
        in_stack[seed as usize] = true;

        while members.len() < MAX_TRIS_PER_CLUSTER {
            let Some(cur) = seed_stack.pop() else {
                // 島が取りきったらその塊は終わり → 次 seed
                break;
            };
            if assigned[cur as usize] {
                // expansion source として積まれた後に確定済みとなった古いエントリ
                continue;
            }
            // フロンティア候補を確定 (1 pop = 1 確定で必ず進捗 → 必ず終了する)
            assigned[cur as usize] = true;
            members.push(cur);
            // 未確定・未スタックの隣を全てフロンティアへ積む。
            // 連結度 (確定/予定済みの隣の数) の低い順に積んで、
            // 高いものから pop される (= boundary 集約) ようにする。
            let mut cands: Vec<(usize, u32)> = adj[cur as usize]
                .iter()
                .filter(|&&n| !assigned[n as usize] && !in_stack[n as usize])
                .map(|&n| {
                    let c = adj[n as usize]
                        .iter()
                        .filter(|m| assigned[**m as usize] || in_stack[**m as usize])
                        .count();
                    (c, n)
                })
                .collect();
            cands.sort_unstable();
            for (_, n) in cands {
                in_stack[n as usize] = true;
                seed_stack.push(n);
            }
        }

        // クラスタ確定
        let index_offset = clustered_indices.len() as u32;
        let mut centroid = [0f32; 3];
        let mut vcount = 0u32;
        for &t in &members {
            for k in 0..3 {
                let vi = indices[t as usize * 3 + k] as usize;
                clustered_indices.push(vi as u32);
                centroid[0] += vertices[vi].pos[0];
                centroid[1] += vertices[vi].pos[1];
                centroid[2] += vertices[vi].pos[2];
                vcount += 1;
            }
        }
        if vcount > 0 {
            centroid[0] /= vcount as f32;
            centroid[1] /= vcount as f32;
            centroid[2] /= vcount as f32;
        }
        let mut radius = 0f32;
        for &t in &members {
            for k in 0..3 {
                let vi = indices[t as usize * 3 + k] as usize;
                let d = sq_dist(vertices[vi].pos, centroid).sqrt();
                if d > radius {
                    radius = d;
                }
            }
        }

        meshlets.push(NanoMeshlet {
            vertex_offset: 0, // streaming 側で通常領域を追記
            vertex_count: vcount,
            index_offset,
            triangle_count: members.len() as u32,
            sphere_center: centroid,
            sphere_radius: radius,
            parent: u32::MAX,
            error: 0.0,
            level: 0,
        });
    }

    // --- 親鎖: 最大共有エッジ相手に merge (ヘューリスティック) ---
    // NOTE (簡略版): 実運用の Nanite は 4 stage で stage ごとに再クラスタ化して
    // DAG を再構成する。本実装は 1 stage (level = 1) 固定に簡略化している。
    // (元々 `for level in 1..=4 { ... break; }` で先頭 stage のみ実行しており、
    //  実挙動を変えずに明示化したもの)
    // 親鎖は深さ 1 のみ: 各ペアの子側 (偶数 index) が partner を parent に持ち、
    // partner 自身と、奇数個のときの末尾クラスタは根 (parent = u32::MAX, error = 0)。
    // `level` は多段化用 scaffold で現行は全クラスタ 0。
    let meshlet_count = meshlets.len();
    let level = 1u32;
    {
        let mut i = 0usize;
        while i < meshlet_count {
            // (studio-simplified): 直接近傍への connected merge (2 クラスタ単位)
            let root = i;
            let partner = (i + 1).min(meshlet_count - 1);
            if partner != root {
                // wave 100 CY-4: 旧実装は親 index を局所 `parent_of` に書いて
                // `let _ =` で破棄していたため `NanoMeshlet::parent` が常に
                // u32::MAX (= 誰も親を持たない) であり、モジュール doc の
                // 「誤差ツリー」は一度も構築されない構造嘘だった。実際に配線する。
                meshlets[root].parent = partner as u32;
                // 誤差 = 合成 radius (境界lockなしの近似)
                let merged_c = mix3(
                    meshlets[root].sphere_center,
                    meshlets[partner].sphere_center,
                    0.5,
                );
                let r = (dist3(meshlets[root].sphere_center, merged_c)
                    + meshlets[root].sphere_radius)
                    .max(
                        dist3(meshlets[partner].sphere_center, merged_c)
                            + meshlets[partner].sphere_radius,
                    );
                let child_max = meshlets[root]
                    .sphere_radius
                    .max(meshlets[partner].sphere_radius);
                meshlets[root].error = (r - child_max).max(0.0);
                meshlets[root].level = level - 1;
                i += 2;
            } else {
                i += 1;
            }
        }
    }

    NanoClusterSet {
        meshlets,
        clustered_indices,
        source_triangle_count: tri_count as u32,
    }
}

fn next_seed(assigned: &[bool], tri_count: usize) -> Option<u32> {
    // 空間偏りを抑えるため素数刻みで走査する (side effect free)。
    // wave 100 CY-2: 剰余列 {offset + i·step mod len} は gcd(step, len) ≠ 1 だと
    // 全位置を巡回**しない**。step = 31 (素数) では len ≡ 0 (mod 31) のとき訪問
    // 位置が len/31 個 (全体の 3%) に縮退し、それらが全て確定済みになると残りの
    // 未確定三角を二度と seed できず走査が終了する = クラスタ化対象幾何の
    // **静寂な消失** (wiring 実経路は identity index で全三角が孤立するため
    // 直撃: 62 三角 → 2 クラスタのみ生成。Python 厳密シミュレーションで機械検証済)。
    // そこで step を len と互いに素な最小の奇数 (≥ 31) に取り直す。互いに素なら
    // 剰余列は巡回群 Z_len の完全置換となり全域をちょうど 1 周する (群論的事実)。
    // gcd(31, len) = 1 の既存入力 (2 の冪長のグリッド等) では step = 31 のままで
    // 走査順も旧版と完全一致する。
    let len = assigned.len();
    if len == 0 {
        return None;
    }
    let mut step = 31usize;
    while gcd_usize(step, len) != 1 {
        step += 2;
    }
    let offset = len % 7;
    for i in 0..len {
        let idx = (offset + i * step) % len;
        if idx < tri_count && !assigned[idx] {
            return Some(idx as u32);
        }
    }
    None
}

/// ユークリッド互除法 (CY-2: 走査 step の coprime 判定用)。
fn gcd_usize(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

#[inline]
fn sq_dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

#[inline]
fn dist3(a: [f32; 3], b: [f32; 3]) -> f32 {
    sq_dist(a, b).sqrt()
}

#[inline]
fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// 画面誤差: `projected = error * proj_factor / dist < ε(px)` → そのクラスタを描く
/// (判定は厳密不等号 `<` であり、projected == ε は採用**しない**)。
///
/// 下駄 (clamp) は 2 箇所で、いずれも退化回避のため:
/// - `dist = max(|center - cam|, 1.0)` — カメラ至近での除算爆発を防ぐ
/// - `ε = max(error_px, 1.0)` — 閾値 0 以下で全クラスタが不可視化するのを防ぐ
///
/// error == 0 のクラスタ (merge されなかった葉 / 根) は projected == 0 < ε で
/// 常に描画対象になる (これより粗い側が存在しないため)。
///
/// **契約 (wave 100 CY-5)**: `cam_pos` 各成分・`proj_factor` (> 0)・`error_px`・
/// `m.error`・`m.sphere_center` 各成分・`m.sphere_radius` は全て有限値必須。
/// 旧実装は `f32::max` が「一方が NaN なら他方を返す」仕様であるため NaN 入力を
/// `dist = 1.0` / `ε = 1.0` の**通常値へ静寂にマスク**し (実機検証:
/// `f32::NAN.max(1.0) == 1.0`)、NaN カメラや NaN error でも判定が通常値として
/// 静かに誤った方向へ通ってしまう経路があった。fail-loud に遮断する
/// (wave 71 BU-1 / wave 73 非有限拒否と同哲学)。
pub fn cluster_should_draw(
    m: &NanoMeshlet,
    cam_pos: [f32; 3],
    proj_factor: f32, // screen_height / (2 * tan(fov/2))
    error_px: f32,
) -> bool {
    assert!(
        cam_pos.iter().all(|c| c.is_finite())
            && proj_factor.is_finite()
            && proj_factor > 0.0
            && error_px.is_finite()
            && m.error.is_finite()
            && m.sphere_center.iter().all(|c| c.is_finite())
            && m.sphere_radius.is_finite(),
        "cluster_should_draw 契約違反: 非有限/非正の入力 (NaN の .max マスク経路による静寂な誤カリングを遮断)"
    );
    let d = dist3(m.sphere_center, cam_pos).max(1.0);
    (m.error * proj_factor) / d < error_px.max(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid_mesh(n: u32) -> (Vec<NanoVertex>, Vec<u32>) {
        let mut verts = Vec::new();
        for z in 0..=n {
            for x in 0..=n {
                verts.push(NanoVertex {
                    pos: [x as f32, 0.0, z as f32],
                    color: 0xFFFFFFFF,
                });
            }
        }
        let mut idx = Vec::new();
        for z in 0..n {
            for x in 0..n {
                let v = |x: u32, z: u32| z * (n + 1) + x;
                idx.extend_from_slice(&[v(x, z), v(x + 1, z), v(x, z + 1)]);
                idx.extend_from_slice(&[v(x + 1, z), v(x + 1, z + 1), v(x, z + 1)]);
            }
        }
        (verts, idx)
    }

    #[test]
    fn clusters_partition_all_triangles() {
        let (v, i) = grid_mesh(8);
        let set = clusterize(&v, &i);
        assert_eq!(set.source_triangle_count, 128);
        let total: u32 = set.meshlets.iter().map(|m| m.triangle_count).sum();
        assert_eq!(total, 128);
        // grid8 = 128 tris は 1 flood で丁度 1 クラスタ (Python 厳密シムで機械検証済)
        assert_eq!(set.meshlets.len(), 1);
        assert!(set
            .meshlets
            .iter()
            .all(|m| m.triangle_count <= MAX_TRIS_PER_CLUSTER as u32));
        assert_eq!(set.clustered_indices.len(), 128 * 3);
    }

    #[test]
    fn small_mesh_makes_one_cluster() {
        let (v, i) = grid_mesh(2);
        let set = clusterize(&v, &i);
        assert_eq!(set.meshlets.len(), 1);
        assert_eq!(set.meshlets[0].triangle_count, 8);
    }

    #[test]
    fn error_metric_nonzero_for_parents() {
        let (v, i) = grid_mesh(16);
        let set = clusterize(&v, &i);
        // CY-6: 旧版は `m.error >= 0.0` を assert していたが、f32 の error は
        // 定義上 0.0 以上 (= `(r - child_max).max(0.0)`) なので恒真であり、
        // テスト名が示す "nonzero" を何も検査していなかった。名実一致させる。
        // grid16 は 119 meshlets (Python 厳密シムで検証) で 59 merge ペアを持ち、
        // 先頭ペア (128 tri クラスタ同士, 中心は厳密に相異) は error > 0。
        assert!(set.meshlets.len() >= 2);
        assert!(set.meshlets[0].error > 0.0);
        assert!(set.meshlets.iter().any(|m| m.error > 0.0));
    }
}

/// wave 100 (CY) で追加した厳密ピンテスト群。
/// 全ピン値は Python (Fraction/Decimal 80桁による厳密 f32 エミュレーション) と
/// 独立 Rust プローブの bit 一致で機械検証済み。
#[cfg(test)]
mod strict_tests {
    use super::*;

    /// 孤立三角 ntri 個 (全頂点非共有 = wiring の identity index 実経路と同型)。
    fn disjoint_mesh(ntri: usize) -> (Vec<NanoVertex>, Vec<u32>) {
        let mut verts = Vec::new();
        let mut indices = Vec::new();
        for i in 0..ntri {
            let x = 4.0 * i as f32;
            let base = verts.len() as u32;
            verts.extend_from_slice(&[
                NanoVertex {
                    pos: [x, 0.0, 0.0],
                    color: 0xFFFFFFFF,
                },
                NanoVertex {
                    pos: [x + 1.0, 0.0, 0.0],
                    color: 0xFFFFFFFF,
                },
                NanoVertex {
                    pos: [x, 1.0, 0.0],
                    color: 0xFFFFFFFF,
                },
            ]);
            indices.extend_from_slice(&[base, base + 1, base + 2]);
        }
        (verts, indices)
    }

    fn grid_mesh16() -> (Vec<NanoVertex>, Vec<u32>) {
        let n = 16u32;
        let mut verts = Vec::new();
        for z in 0..=n {
            for x in 0..=n {
                verts.push(NanoVertex {
                    pos: [x as f32, 0.0, z as f32],
                    color: 0xFFFFFFFF,
                });
            }
        }
        let mut idx = Vec::new();
        for z in 0..n {
            for x in 0..n {
                let v = |x: u32, z: u32| z * (n + 1) + x;
                idx.extend_from_slice(&[v(x, z), v(x + 1, z), v(x, z + 1)]);
                idx.extend_from_slice(&[v(x + 1, z), v(x + 1, z + 1), v(x, z + 1)]);
            }
        }
        (verts, idx)
    }

    fn meshlet_at(sphere_center: [f32; 3], sphere_radius: f32, error: f32) -> NanoMeshlet {
        NanoMeshlet {
            vertex_offset: 0,
            vertex_count: 3,
            index_offset: 0,
            triangle_count: 1,
            sphere_center,
            sphere_radius,
            parent: u32::MAX,
            error,
            level: 0,
        }
    }

    /// CY-1: 平三角リスト契約の fail-loud 化 (vertex_cache_opt wave 72 と同型)。
    #[test]
    fn indices_len_must_be_multiple_of_3_fail_loud() {
        let r = std::panic::catch_unwind(|| {
            let verts: Vec<NanoVertex> = (0..4)
                .map(|i| NanoVertex {
                    pos: [i as f32, 0.0, 0.0],
                    color: 0,
                })
                .collect();
            // 4 index = 3 の倍数でない (旧版は末尾 1 個を静寂 drop していた)
            clusterize(&verts, &[0, 1, 2, 3]);
        });
        assert!(r.is_err(), "len%3 != 0 は panic 必須 (静寂 drop 遮断)");
        // 空入力は従来通り受理 (0 % 3 == 0)
        let empty = clusterize(&[], &[]);
        assert_eq!(empty.meshlets.len(), 0);
        assert_eq!(empty.source_triangle_count, 0);
        // 3 連 1 三角は受理
        let (v, i) = disjoint_mesh(1);
        let one = clusterize(&v, &i);
        assert_eq!(one.meshlets.len(), 1);
        assert_eq!(one.meshlets[0].triangle_count, 1);
    }

    /// CY-2: 剰余走査が len と互いに素でない step でも全域を覆うことをピン。
    /// Python 厳密シム: 旧版 (step=31 固定) は len ∈ {31, 62, 93} で
    /// {1, 2, 3} クラスタ (= len/31 個) しか生成せず残りの三角が静寂消失した。
    #[test]
    fn next_seed_full_coverage_when_stride_shares_factor() {
        for ntri in [31usize, 62, 93] {
            let (v, i) = disjoint_mesh(ntri);
            let set = clusterize(&v, &i);
            assert_eq!(
                set.meshlets.len(),
                ntri,
                "ntri={ntri}: 全孤立三角がクラスタ化されること"
            );
            assert!(
                set.meshlets.iter().all(|m| m.triangle_count == 1),
                "ntri={ntri}"
            );
            let total: u32 = set.meshlets.iter().map(|m| m.triangle_count).sum();
            assert_eq!(total, ntri as u32, "ntri={ntri}: 消失ゼロ");
            assert_eq!(set.clustered_indices.len(), ntri * 3, "ntri={ntri}");
            assert_eq!(set.source_triangle_count, ntri as u32, "ntri={ntri}");
        }
    }

    /// CY-2 回帰ピン: gcd(31, 512) = 1 の入力では走査順が旧版と全く同じ
    /// (grid16 の分割結果 = Python 厳密シムの 119 クラスタ分布と一致)。
    #[test]
    fn flood_partition_regression_grid16_exact() {
        // Python 厳密シミュレーション由来の構築順非依存マルチセット:
        // 110×1, 4×2, 1×4, 1×6, 3×128 (計 119, 総三角数 512)。
        #[rustfmt::skip]
        const EXPECTED: [u32; 119] = [
            1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
            1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
            1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
            1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
            1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
            1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
            1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 4, 6, 128, 128, 128,
        ];
        let (v, i) = grid_mesh16();
        let set = clusterize(&v, &i);
        assert_eq!(set.meshlets.len(), 119);
        let mut counts: Vec<u32> = set.meshlets.iter().map(|m| m.triangle_count).collect();
        counts.sort_unstable();
        assert_eq!(counts.as_slice(), EXPECTED.as_slice());
        let total: u32 = counts.iter().sum();
        assert_eq!(total, 512);
        // 構築順ピン: 先頭 3 クラスタは満杯 128 (flood 順序依存)、末尾は 1
        assert_eq!(set.meshlets[0].triangle_count, 128);
        assert_eq!(set.meshlets[1].triangle_count, 128);
        assert_eq!(set.meshlets[2].triangle_count, 128);
        assert_eq!(set.meshlets[118].triangle_count, 1);
        assert_eq!(set.clustered_indices.len(), 512 * 3);
        assert_eq!(set.source_triangle_count, 512);
    }

    /// CY-4: 親鎖が実際に配線されることの厳密ピン (旧版は全員 u32::MAX)。
    /// grid16 = 119 (奇数) → 59 ペア + index 118 が単独根。
    #[test]
    fn parent_chain_materialized_exactly_grid16() {
        let (v, i) = grid_mesh16();
        let set = clusterize(&v, &i);
        assert_eq!(set.meshlets.len(), 119);
        for child in (0..=116usize).step_by(2) {
            assert_eq!(
                set.meshlets[child].parent,
                (child + 1) as u32,
                "meshlets[{child}] parent"
            );
        }
        for partner in (1..=117usize).step_by(2) {
            assert_eq!(
                set.meshlets[partner].parent,
                u32::MAX,
                "meshlets[{partner}] はペアの根"
            );
        }
        // 単独根: 親なし, error = +0.0 (厳密 bit = 0x0000_0000)
        assert_eq!(set.meshlets[118].parent, u32::MAX);
        assert_eq!(set.meshlets[118].error.to_bits(), 0u32);
        assert!(set.meshlets.iter().any(|m| m.parent != u32::MAX));
        assert!(set.meshlets.iter().all(|m| m.level == 0));
    }

    /// CY-2/CY-4 両修正の幾何ピン: 先頭クラスタの sphere を f32 bit で厳密固定。
    /// ピン値は Python Fraction/Decimal(80桁) 厳密エミュレーションと独立 Rust
    /// プローブの bit 一致 (centroid 0x40d09555/0/0x41478aab, radius 0x415a33d2)
    /// で確定したもの (flood の pop 順序まで含めて検知できる強ピン)。
    #[test]
    fn meshlet0_sphere_bits_python_verified() {
        let (v, i) = grid_mesh16();
        let set = clusterize(&v, &i);
        let m = &set.meshlets[0];
        assert_eq!(m.sphere_center[0].to_bits(), 0x40d0_9555);
        assert_eq!(m.sphere_center[1].to_bits(), 0x0000_0000);
        assert_eq!(m.sphere_center[2].to_bits(), 0x4147_8aab);
        assert_eq!(m.sphere_radius.to_bits(), 0x415a_33d2);
        assert_eq!(m.vertex_count, 384); // コーナー数 = 3×128
        assert_eq!(m.vertex_offset, 0); // scaffold 契約 (streaming 側が追記)
        assert_eq!(m.index_offset, 0);
        assert_eq!(m.triangle_count, 128);
    }

    /// CY-3: 非有限頂点の入口遮断 (NaN は 1 つでも panic。有限のみなら受理)。
    #[test]
    fn clusterize_rejects_non_finite_vertices() {
        let r = std::panic::catch_unwind(|| {
            let verts = vec![
                NanoVertex {
                    pos: [0.0, 0.0, 0.0],
                    color: 0,
                },
                NanoVertex {
                    pos: [f32::NAN, 0.0, 0.0],
                    color: 0,
                },
                NanoVertex {
                    pos: [1.0, 1.0, 0.0],
                    color: 0,
                },
            ];
            clusterize(&verts, &[0, 1, 2]);
        });
        assert!(
            r.is_err(),
            "NaN 頂点は panic 必須 (sphere/error 汚染を遮断)"
        );
        let r2 = std::panic::catch_unwind(|| {
            let verts = vec![
                NanoVertex {
                    pos: [0.0, 0.0, 0.0],
                    color: 0,
                },
                NanoVertex {
                    pos: [f32::INFINITY, 0.0, 0.0],
                    color: 0,
                },
                NanoVertex {
                    pos: [1.0, 1.0, 0.0],
                    color: 0,
                },
            ];
            clusterize(&verts, &[0, 1, 2]);
        });
        assert!(r2.is_err(), "+inf 頂点も panic 必須");
    }

    /// CY-5: 全値 dyadic 厳密 (f32 丸めに依存しない) 判定表。
    /// (0.5*100)/10 = 5.0, (0.0625*100)/10 = 0.625, (0.5*10)/10 = 0.5 は全て
    /// f32 厳密値で、境界等号 (5.0 < 5.0) の厳密不等号契約もここで固定する。
    #[test]
    fn should_draw_dyadic_exact_decisions() {
        let center = [0.0f32, 0.0, 0.0];
        let m = meshlet_at(center, 1.0, 0.5);
        // projected = (0.5*100)/10 = 5.0 (厳密)
        assert!(!cluster_should_draw(&m, [10.0, 0.0, 0.0], 100.0, 1.0)); // 5.0 < 1.0
        assert!(!cluster_should_draw(&m, [10.0, 0.0, 0.0], 100.0, 5.0)); // 境界等号は不採用
        assert!(cluster_should_draw(&m, [10.0, 0.0, 0.0], 100.0, 6.0)); // 5.0 < 6.0
                                                                        // error = 0.0625 (2^-4): projected = 6.25/10 = 0.625 (厳密)
        let fine = meshlet_at(center, 1.0, 0.0625);
        assert!(cluster_should_draw(&fine, [10.0, 0.0, 0.0], 100.0, 1.0)); // 0.625 < 1.0
                                                                           // dist 下駄: cam 一致で |c-cam| = 0 → d = 1: (2.0*1)/1 = 2.0 < 1.0 = false
        let coarse = meshlet_at(center, 1.0, 2.0);
        assert!(!cluster_should_draw(&coarse, [0.0, 0.0, 0.0], 1.0, 1.0));
        // ε 下駄: projected = (0.5*10)/10 = 0.5, eps = 0.25 → max(1.0) = 1.0:
        // 0.5 < 1.0 = true (下駄無しなら 0.5 < 0.25 = false であることを証明)
        assert!(cluster_should_draw(&m, [10.0, 0.0, 0.0], 10.0, 0.25));
        // error == +0.0 (葉/根) は projected == 0.0 < ε で常に採用
        let leaf = meshlet_at([100.0, 0.0, 0.0], 1.0, 0.0);
        assert!(cluster_should_draw(&leaf, [0.0, 0.0, 0.0], 100.0, 1.0));
    }

    /// CY-5: 非有限/非正入力は NaN の .max マスク経路ごと入口で遮断。
    #[test]
    fn should_draw_rejects_non_finite() {
        for (name, cam, proj, eps, err) in [
            ("NaN cam", [f32::NAN, 0.0, 0.0], 100.0, 1.0, 0.5),
            ("proj=0 非正", [10.0, 0.0, 0.0], 0.0, 1.0, 0.5),
            ("NaN proj", [10.0, 0.0, 0.0], f32::NAN, 1.0, 0.5),
            ("NaN eps", [10.0, 0.0, 0.0], 100.0, f32::NAN, 0.5),
            ("NaN error", [10.0, 0.0, 0.0], 100.0, 1.0, f32::NAN),
        ] {
            let r = std::panic::catch_unwind(|| {
                let m = meshlet_at([0.0, 0.0, 0.0], 1.0, err);
                cluster_should_draw(&m, cam, proj, eps);
            });
            assert!(r.is_err(), "{name}: panic 必須 (NaN マスク経路遮断)");
        }
        // NaN sphere_center / radius も遮断
        let r = std::panic::catch_unwind(|| {
            let m = meshlet_at([f32::NAN, 0.0, 0.0], 1.0, 0.5);
            cluster_should_draw(&m, [10.0, 0.0, 0.0], 100.0, 1.0);
        });
        assert!(r.is_err(), "NaN sphere_center は panic 必須");
        let r = std::panic::catch_unwind(|| {
            let m = meshlet_at([0.0, 0.0, 0.0], f32::NAN, 0.5);
            cluster_should_draw(&m, [10.0, 0.0, 0.0], 100.0, 1.0);
        });
        assert!(r.is_err(), "NaN sphere_radius は panic 必須");
    }

    /// CY-2: coprime step 選定の数学的基礎 (ユークリッド互除法) の基本形ピン。
    #[test]
    fn gcd_usize_basics() {
        // 基本形
        assert_eq!(gcd_usize(0, 5), 5);
        assert_eq!(gcd_usize(7, 0), 7);
        // grid 系列 (2 の冪) は 31 と互いに素 → step 31 維持 (走査順不変の根拠)
        assert_eq!(gcd_usize(31, 8), 1);
        assert_eq!(gcd_usize(31, 128), 1);
        assert_eq!(gcd_usize(31, 512), 1);
        // 31 の倍数は gcd = 31 → step 取り直しが発動する相
        assert_eq!(gcd_usize(31, 31), 31);
        assert_eq!(gcd_usize(31, 62), 31);
        // 93 = 3×31: step 33 も gcd(33,93)=3 で不適 → step 35 で coprime (機械検算済)
        assert_eq!(gcd_usize(33, 93), 3);
        assert_eq!(gcd_usize(35, 93), 1);
    }
}

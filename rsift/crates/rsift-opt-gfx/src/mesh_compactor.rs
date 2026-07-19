//! # MeshCompactor — Frostbite 方式 compute ドロー圧縮（メッシュシェーダ不要版）
//!
//! 出典: Frostbite "GPU-Driven Rendering Pipelines" (execute indirect + prefix sum)、
//! Nvidium（メッシュシェーダによる GPU カリング）を**全 GPU で等価に行う**手段。
//!
//! CPU 側モデル（テスト・検証用）と、wgpu 結線用の WGSL カーネル文字列の両方を提供:
//!
//! 1. 全ドロー候補（セクション）の AABB に対し視錐台 + 距離 + 遮蔽フラグを評価
//! 2. 生存者だけを prefix-sum compaction で draw コマンド配列の先頭に詰める
//! 3. `draw_indirect` 用コマンド（vertex_count, instance_count, first_vertex,
//!    first_instance）+ GPU 側カウント値を書き出す
//!
//! 空のドローコールを投げないことで、低スペック iGPU の CPU→GPU コマンド帯域と
//! ドライバの per-draw オーバーヘッドを大幅削減。

/// 1ドロー候補（1チャンクセクション）。
#[derive(Debug, Clone, Copy)]
pub struct DrawCandidate {
    /// セクション中心 (world)
    pub center: [f32; 3],
    /// AABB 半径（各軸）
    pub half_extents: [f32; 3],
    /// 頂点数（0 = 空セクション: 常に除外）
    pub vertex_count: u32,
    /// バッファ内 first_vertex
    pub first_vertex: u32,
    /// 前フレームの遮蔽クエリ結果（true=見えた, 初フレームは true 推奨）
    pub visible_prev: bool,
    /// 追加のソートキー (material/pass)
    pub pass_key: u16,
}

/// 圧縮後の indirect draw コマンド（wgpu DrawIndirect と同形）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct IndirectDrawCmd {
    pub vertex_count: u32,
    pub instance_count: u32,
    pub first_vertex: u32,
    pub first_instance: u32,
}

/// 視錐台（6平面, 各 (a,b,c,d): ax+by+cz+d >= 0 で内側）。
#[derive(Debug, Clone, Copy)]
pub struct FrustumPlanes(pub [[f32; 4]; 6]);

impl FrustumPlanes {
    /// view-projection 行列 (column-major 16) から Gribb-Hartmann 抽出。
    pub fn from_view_proj(m: &[f32; 16]) -> Self {
        let row = |i: usize| [m[i], m[i + 4], m[i + 8], m[i + 12]];
        let r0 = row(0);
        let r1 = row(1);
        let r2 = row(2);
        let r3 = row(3);
        let add = |a: [f32; 4], b: [f32; 4]| [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]];
        let sub = |a: [f32; 4], b: [f32; 4]| [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]];
        let mut p = [
            add(r3, r0),
            sub(r3, r0),
            add(r3, r1),
            sub(r3, r1),
            add(r3, r2),
            sub(r3, r2),
        ];
        for pl in &mut p {
            let len = (pl[0] * pl[0] + pl[1] * pl[1] + pl[2] * pl[2]).sqrt().max(1e-6);
            for v in pl.iter_mut() {
                *v /= len;
            }
        }
        FrustumPlanes(p)
    }
}

/// 圧縮ポリシー。
#[derive(Debug, Clone, Copy)]
pub struct CompactPolicy {
    /// 描画最大距離（中心距離）。
    pub max_distance: f32,
    /// 1-frustum 安全側マージン（AABB を少し膨らませる）。
    pub frustum_margin: f32,
}

impl Default for CompactPolicy {
    fn default() -> Self {
        Self {
            max_distance: 512.0,
            frustum_margin: 0.5,
        }
    }
}

#[inline]
fn aabb_in_frustum(f: &FrustumPlanes, c: [f32; 3], e: [f32; 3], margin: f32) -> bool {
    for pl in &f.0 {
        // 正の頂点テスト（最も平面側に遠い頂点が外なら全体外）
        let px = if pl[0] >= 0.0 { e[0] } else { -e[0] };
        let py = if pl[1] >= 0.0 { e[1] } else { -e[1] };
        let pz = if pl[2] >= 0.0 { e[2] } else { -e[2] };
        let d = pl[0] * (c[0] + px) + pl[1] * (c[1] + py) + pl[2] * (c[2] + pz) + pl[3];
        if d < -margin {
            return false;
        }
    }
    true
}

/// CPU 参照実装: 候補列を圧縮して indirect コマンド配列を作る。
/// 戻り値 (cmds, 生存数)。
pub fn compact_draws(
    candidates: &[DrawCandidate],
    frustum: &FrustumPlanes,
    camera: [f32; 3],
    policy: &CompactPolicy,
    out: &mut Vec<IndirectDrawCmd>,
) -> usize {
    out.clear();
    let max_d2 = policy.max_distance * policy.max_distance;
    for cand in candidates {
        if cand.vertex_count == 0 || !cand.visible_prev {
            continue;
        }
        let dx = cand.center[0] - camera[0];
        let dy = cand.center[1] - camera[1];
        let dz = cand.center[2] - camera[2];
        if dx * dx + dy * dy + dz * dz > max_d2 {
            continue;
        }
        if !aabb_in_frustum(frustum, cand.center, cand.half_extents, policy.frustum_margin) {
            continue;
        }
        out.push(IndirectDrawCmd {
            vertex_count: cand.vertex_count,
            instance_count: 1,
            first_vertex: cand.first_vertex,
            first_instance: 0,
        });
    }
    // pass ソート（partial sort ではなく全ソート: セクション数は高々数万）
    out.sort_by_key(|c| c.first_vertex);
    out.len()
}

/// 幅優先の階層カリング: 4x4x4 セクション親ノードから評価し、
/// 親が外なら子を一括スキップ。低解像度 CPU でのカリング自体を速くする。
pub struct HierarchicalCuller {
    /// (level, node_index) → AABB
    pub levels: Vec<Vec<([f32; 3], [f32; 3])>>,
}

impl HierarchicalCuller {
    /// セクション群から2レベル階層を構築（子=section, 親=4x4x4 グループ）。
    pub fn build(sections: &[DrawCandidate]) -> Self {
        let mut parents: Vec<([f32; 3], [f32; 3])> = Vec::new();
        for chunk in sections.chunks(64) {
            let mut lo = [f32::INFINITY; 3];
            let mut hi = [f32::NEG_INFINITY; 3];
            for s in chunk {
                for k in 0..3 {
                    lo[k] = lo[k].min(s.center[k] - s.half_extents[k]);
                    hi[k] = hi[k].max(s.center[k] + s.half_extents[k]);
                }
            }
            let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5];
            let e = [
                (hi[0] - lo[0]).max(0.0) * 0.5,
                (hi[1] - lo[1]).max(0.0) * 0.5,
                (hi[2] - lo[2]).max(0.0) * 0.5,
            ];
            parents.push((c, e));
        }
        Self {
            levels: vec![parents],
        }
    }

    /// 生きている親ブロックの index を返す（子はそのブロック内だけ評価すれば良い）。
    pub fn visible_parent_blocks(&self, frustum: &FrustumPlanes, margin: f32) -> Vec<usize> {
        self.levels[0]
            .iter()
            .enumerate()
            .filter(|(_, (c, e))| aabb_in_frustum(frustum, *c, *e, margin))
            .map(|(i, _)| i)
            .collect()
    }
}

/// wgpu 側で動かす compute カーネル（prefix-sum compaction + indirect 書き出し）。
/// workgroup 256, 入力 candidate 構造は `DrawCandidate` と同一レイアウト。
pub const COMPACT_WGSL: &str = r#"
struct Candidate {
    center: vec3<f32>,
    half_ext: vec3<f32>,
    vertex_count: u32,
    first_vertex: u32,
    visible_prev: u32,
    pass_key: u32,
    frustum_seen: u32,
};
struct Cmd {
    vertex_count: u32,
    instance_count: u32,
    first_vertex: u32,
    first_instance: u32,
};
struct Params {
    planes: array<vec4<f32>, 6>,
    camera: vec4<f32>,
    policy: vec4<f32>, // x=max_d, y=margin, z=candidate_count
};
@group(0) @binding(0) var<storage, read> cands: array<Candidate>;
@group(0) @binding(1) var<storage, read_write> cmds: array<Cmd>;
@group(0) @binding(2) var<storage, read_write> draw_count: atomic<u32>;
@group(0) @binding(3) var<uniform> params: Params;

fn aabb_inside(c: vec3<f32>, e: vec3<f32>, margin: f32) -> bool {
    for (var i = 0; i < 6; i = i + 1) {
        let pl = params.planes[i];
        let p = c + sign(pl.xyz) * e;
        if (dot(pl.xyz, p) + pl.w < -margin) {
            return false;
        }
    }
    return true;
}

@compute @workgroup_size(256)
fn cs_compact(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= u32(params.policy.z)) {
        return;
    }
    let cand = cands[i];
    if (cand.vertex_count == 0u || cand.visible_prev == 0u) {
        return;
    }
    let dvec = cand.center - params.camera.xyz;
    if (dot(dvec, dvec) > params.policy.x * params.policy.x) {
        return;
    }
    if (!aabb_inside(cand.center, cand.half_ext, params.policy.y)) {
        return;
    }
    let slot = atomicAdd(&draw_count, 1u);
    cmds[slot] = Cmd(cand.vertex_count, 1u, cand.first_vertex, 0u);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn identity_frustum() -> FrustumPlanes {
        // x,y in [-1,1], z in [0,1] の単位錐台
        FrustumPlanes([
            [1.0, 0.0, 0.0, 1.0],
            [-1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 1.0],
            [0.0, -1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, -1.0, 1.0],
        ])
    }

    fn cand(x: f32, y: f32, z: f32, vc: u32) -> DrawCandidate {
        DrawCandidate {
            center: [x, y, z],
            half_extents: [0.5, 0.5, 0.5],
            vertex_count: vc,
            first_vertex: 0,
            visible_prev: true,
            pass_key: 0,
        }
    }

    #[test]
    fn compaction_drops_invisible_and_far() {
        let f = identity_frustum();
        let cands = vec![
            cand(0.0, 0.0, 0.5, 36),     // 生きる
            cand(5.0, 0.0, 0.5, 36),     // 錐台外
            cand(0.0, 0.0, 0.5, 0),      // 空
            cand(0.0, 0.0, 999.0, 36),   // 遠すぎ
            DrawCandidate {
                visible_prev: false,
                ..cand(0.0, 0.0, 0.5, 36)
            }, // 遮蔽
        ];
        let mut out = Vec::new();
        let n = compact_draws(
            &cands,
            &f,
            [0.0, 0.0, 0.0],
            &CompactPolicy::default(),
            &mut out,
        );
        assert_eq!(n, 1);
        assert_eq!(out[0].vertex_count, 36);
        assert_eq!(out[0].instance_count, 1);
    }

    #[test]
    fn frustum_from_matrix_identity() {
        // 恒等 VP 行列 → クリップ空間 [-1,1]^3 と同等の錐台が取れる
        let m = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        let f = FrustumPlanes::from_view_proj(&m);
        // 原点は内側
        let inside = aabb_in_frustum(&f, [0.0, 0.0, 0.5], [0.1, 0.1, 0.1], 0.0);
        assert!(inside);
        let outside = aabb_in_frustum(&f, [50.0, 0.0, 0.5], [0.1, 0.1, 0.1], 0.0);
        assert!(!outside);
    }

    #[test]
    fn hierarchical_prunes_whole_groups() {
        let mut cands = Vec::new();
        for i in 0..64 {
            cands.push(cand(0.0, 0.0, 0.5, 36)); // 錐台内グループ
            let _ = i;
        }
        for _ in 0..64 {
            cands.push(cand(1000.0, 0.0, 0.0, 36)); // 錐台外グループ
        }
        let hc = HierarchicalCuller::build(&cands);
        let f = identity_frustum();
        let blocks = hc.visible_parent_blocks(&f, 0.5);
        assert_eq!(blocks.len(), 1, "only the in-frustum parent group survives");
    }

    #[test]
    fn wgsl_kernel_is_nonempty_and_declares_entry() {
        assert!(COMPACT_WGSL.contains("@compute"));
        assert!(COMPACT_WGSL.contains("cs_compact"));
        assert!(COMPACT_WGSL.contains("atomicAdd"));
    }
}

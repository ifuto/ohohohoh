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
///
/// COMPACT_WGSL の `Cmd` (4 つの u32、span 16B) とバイト同一であり、
/// そのまま GPU バッファへアップロード可能 (wave 44 で機械ピン)。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct IndirectDrawCmd {
    pub vertex_count: u32,
    pub instance_count: u32,
    pub first_vertex: u32,
    pub first_instance: u32,
}

/// COMPACT_WGSL の `Candidate` と**バイト同一**のワイヤ表現
/// (2026-07-23 wave 44: 「将来の配線では明示的なワイヤ変換が必須」の closing)。
///
/// レイアウト決定根拠 (WGSL 構造体配置規則からの厳密導出、naga 実測と
/// 突合済み — `wgsl_layout_matches_pinned_offsets`):
/// - `center: vec3<f32>` @0 (size 12, align 16) → 12..16 は 4B の穴
/// - `half_ext: vec3<f32>` @16 (size 12) → 次のスカラは末尾 28 から密詰み
///   (WGSL は vec3 の trailing gap を後続スカラで再利用する。**Rust 側に
///   `_pad1` を追加すると 32 開始になり GPU 配置と永久的にズレる**)
/// - u32 群 @28,32,36,40,44、span = roundUp(16, 48) = 48
///
/// `frustum_seen` は現行 WGSL カーネル・CPU 参照の双方で読まれない
/// 予約フィールドであり、変換は常に 0 を書く (cross-frame フィードバック用)。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CandidateWire {
    pub center: [f32; 3],
    pub _pad0: u32,
    pub half_ext: [f32; 3],
    pub vertex_count: u32,
    pub first_vertex: u32,
    pub visible_prev: u32,
    pub pass_key: u32,
    pub frustum_seen: u32,
}

impl DrawCandidate {
    /// WGSL 入力レイアウトへの**全フィールド明示**変換
    /// (bool → 0/1、u16 → u32 ゼロ拡張、予約フィールド 0、パディング 0)。
    pub fn to_wire(&self) -> CandidateWire {
        CandidateWire {
            center: self.center,
            _pad0: 0,
            half_ext: self.half_extents,
            vertex_count: self.vertex_count,
            first_vertex: self.first_vertex,
            visible_prev: u32::from(self.visible_prev),
            pass_key: u32::from(self.pass_key),
            frustum_seen: 0,
        }
    }
}

/// 候補列の一括ワイヤ変換 (GPU アップロード用の決定的バイト列)。
pub fn candidates_to_wire(candidates: &[DrawCandidate]) -> Vec<CandidateWire> {
    candidates.iter().map(DrawCandidate::to_wire).collect()
}

/// COMPACT_WGSL の `Params` (span 128B) と**バイト同一**のワイヤ表現。
///
/// **契約**: `policy.z` は candidate_count を **f32 として運ぶ**ため、
/// count ≤ 2^24 (16,777,216) 必須 — 超過は f32 が整数を表現できず
/// 誤カウント化する (構築時に fail-loud、wave 44)。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ParamsWire {
    /// 6 平面 (a,b,c,d)、dist >= 0 で内側 (正規化済み前提、FrustumPlanes 参照)
    pub planes: [[f32; 4]; 6],
    /// camera.xyz + w 予約 0
    pub camera: [f32; 4],
    /// x=max_d, y=margin, z=candidate_count (f32 化), w=予約 0
    pub policy: [f32; 4],
}

impl ParamsWire {
    /// 最大許容 candidate_count (policy.z の f32 輸送で整数厳密性が
    /// 保てる上限 2^24)。
    pub const MAX_COUNT: u32 = 1 << 24;

    pub fn new(
        frustum: &FrustumPlanes,
        camera: [f32; 3],
        policy: &CompactPolicy,
        candidate_count: u32,
    ) -> Self {
        assert!(
            candidate_count <= Self::MAX_COUNT,
            "ParamsWire::new 契約違反: candidate_count {candidate_count} > 2^24 \
             (policy.z の f32 輸送で整数厳密性が破れる)"
        );
        Self {
            planes: frustum.0,
            camera: [camera[0], camera[1], camera[2], 0.0],
            policy: [
                policy.max_distance,
                policy.frustum_margin,
                candidate_count as f32,
                0.0,
            ],
        }
    }
}

/// 視錐台（6平面, 各 (a,b,c,d): ax+by+cz+d >= 0 で内側）。
#[derive(Debug, Clone, Copy)]
pub struct FrustumPlanes(pub [[f32; 4]; 6]);

impl FrustumPlanes {
    /// view-projection 行列 (column-major 16) から Gribb-Hartmann 抽出。
    ///
    /// 法線は内向き (dist = ax+by+cz+d >= 0 が視体積内側)。
    /// near は `r2` 単体: wgpu z_ndc ∈ [0,1] では視体積は clip.z >= 0 ⟺ row2·p >= 0。
    /// 旧実装の add(r3, r2) は z >= -w の GL 式体積で「誤カリングしないが
    /// near 背面を残す」保守側の逸脱だった (2026-07-22 wave 28 で根治。
    /// frame_worldgen::frustum_planes の r2 規則・simd_kernels AC-1 と統一)。
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
            r2, // near (wgpu z in [0,1]: z >= 0)
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
/// workgroup 256。
///
/// ## 正直な契約注記 (2026-07-22 wave 28 監査)
/// 1. **wire format**: 入力 `Candidate` は Rust の `DrawCandidate` とは
///    **別レイアウト** (span 48B — vec3 各 align 16 のため 12..16 に 4B 穴、
///    残り u32 群は 28 から密詰み。frustum_seen フィールド追加、
///    pass_key は u16→u32)。`DrawCandidate` は repr(Rust) かつ Pod 非実装
///    (bytemuck 不可) のため、配線はワイヤ構造体経由で行う:
///    `DrawCandidate::to_wire` / `candidates_to_wire` / `ParamsWire::new`
///    (2026-07-23 wave 44 で closing 済、`offset_of!` とバイト厳密ピンで
///    naga 実測と閉じた契約)。`wgsl_layout_matches_pinned_offsets` が
///    GPU 側の固定レイアウトを機械ピン。
/// 2. **出力順は非決定的**: GPU 側は atomicAdd の完了順に cmds を詰めるため
///    CPU 参照実装 (first_vertex で安定ソート) との逐一致合は成立しない。
///    厳密性が必要なら opaque 専用とする (半透明ソートは別経路)。
/// 3. `sign(pl.xyz) * e` と CPU の正頂点選択は、成分ゼロの平面で見掛けが
///    異なる (sign(0)=0 vs CPU は +e) が、ゼロ成分項は寄与しないため
///    **数学的に常に一致** (解析証明済み)。
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

    /// wave 44-1: ワイヤ構造体の Rust 側レイアウトを offset_of! でピンし、
    /// naga 実測ピン (wgsl_layout_matches_pinned_offsets 側) と閉じた契約にする。
    /// 両側が独立にピンされているため、どちらかがドリフトすれば即検出される。
    #[test]
    fn wire_layout_offsets_match_naga_pins() {
        use std::mem::{offset_of, size_of};
        // Candidate (naga: 0,16,28,32,36,40,44 / span 48)
        assert_eq!(offset_of!(CandidateWire, center), 0);
        assert_eq!(offset_of!(CandidateWire, half_ext), 16);
        assert_eq!(offset_of!(CandidateWire, vertex_count), 28);
        assert_eq!(offset_of!(CandidateWire, first_vertex), 32);
        assert_eq!(offset_of!(CandidateWire, visible_prev), 36);
        assert_eq!(offset_of!(CandidateWire, pass_key), 40);
        assert_eq!(offset_of!(CandidateWire, frustum_seen), 44);
        assert_eq!(size_of::<CandidateWire>(), 48);
        // Cmd (naga: 16B)
        assert_eq!(size_of::<IndirectDrawCmd>(), 16);
        assert_eq!(offset_of!(IndirectDrawCmd, first_instance), 12);
        // Params (naga: planes@0 camera@96 policy@112 / span 128)
        assert_eq!(offset_of!(ParamsWire, planes), 0);
        assert_eq!(offset_of!(ParamsWire, camera), 96);
        assert_eq!(offset_of!(ParamsWire, policy), 112);
        assert_eq!(size_of::<ParamsWire>(), 128);
        // Pod 保証 (derive 自体がパディング不在をコンパイル時証明)
        let _: &[u8] = bytemuck::bytes_of(&ParamsWire::new(
            &identity_frustum(),
            [1.0, 2.0, 3.0],
            &CompactPolicy::default(),
            7,
        ));
    }

    /// wave 44-2: to_wire のバイト厳密性 (視認性の高いパターンで全フィールド
    /// の位置・エンディアン・ゼロ穴を機械固定)。
    #[test]
    fn to_wire_is_byte_exact() {
        let c = DrawCandidate {
            center: [1.0, 2.0, 3.0],
            half_extents: [4.0, 5.0, 6.0],
            vertex_count: 0xAABB_CCDD,
            first_vertex: 0x1122_3344,
            visible_prev: true,
            pass_key: 0x5566,
        };
        let w = c.to_wire();
        let b: &[u8] = bytemuck::bytes_of(&w);
        assert_eq!(b.len(), 48);
        assert_eq!(&b[0..4], 1.0f32.to_le_bytes());
        assert_eq!(&b[4..8], 2.0f32.to_le_bytes());
        assert_eq!(&b[8..12], 3.0f32.to_le_bytes());
        assert_eq!(&b[12..16], [0, 0, 0, 0], "vec3 穴はゼロ");
        assert_eq!(&b[16..20], 4.0f32.to_le_bytes());
        assert_eq!(&b[20..24], 5.0f32.to_le_bytes());
        assert_eq!(&b[24..28], 6.0f32.to_le_bytes());
        assert_eq!(&b[28..32], 0xAABB_CCDDu32.to_le_bytes());
        assert_eq!(&b[32..36], 0x1122_3344u32.to_le_bytes());
        assert_eq!(&b[36..40], 1u32.to_le_bytes(), "bool true → 1");
        assert_eq!(&b[40..44], 0x5566u32.to_le_bytes(), "u16 ゼロ拡張");
        assert_eq!(&b[44..48], 0u32.to_le_bytes(), "frustum_seen 予約 0");
        // false → 0
        let mut c0 = c;
        c0.visible_prev = false;
        let w0 = c0.to_wire();
        assert_eq!(&bytemuck::bytes_of(&w0)[36..40], 0u32.to_le_bytes());
    }

    /// wave 44-3: Params 変換のバイト厳密性 (policy.z の f32 化を含む)。
    #[test]
    fn params_wire_is_byte_exact() {
        let f = identity_frustum();
        let p = ParamsWire::new(&f, [7.0, 8.0, 9.0], &CompactPolicy::default(), 11);
        let b: &[u8] = bytemuck::bytes_of(&p);
        assert_eq!(b.len(), 128);
        // plane 0 = [1,0,0,1] (identity_frustum の左面)
        assert_eq!(&b[0..4], 1.0f32.to_le_bytes());
        assert_eq!(&b[4..8], 0.0f32.to_le_bytes());
        assert_eq!(&b[12..16], 1.0f32.to_le_bytes());
        // camera @96
        assert_eq!(&b[96..100], 7.0f32.to_le_bytes());
        assert_eq!(&b[100..104], 8.0f32.to_le_bytes());
        assert_eq!(&b[104..108], 9.0f32.to_le_bytes());
        assert_eq!(&b[108..112], 0.0f32.to_le_bytes());
        // policy @112: x=max_d 512, y=margin 0.5, z=count 11, w=0
        assert_eq!(&b[112..116], 512.0f32.to_le_bytes());
        assert_eq!(&b[116..120], 0.5f32.to_le_bytes());
        assert_eq!(&b[120..124], 11.0f32.to_le_bytes());
        assert_eq!(&b[124..128], 0.0f32.to_le_bytes());
    }

    /// wave 44-4: 一括変換は順序・長さを保持する決定的写像。
    #[test]
    fn candidates_to_wire_preserves_order_and_len() {
        let cands = vec![
            cand(1.0, 0.0, 0.5, 36),
            cand(2.0, 0.0, 0.5, 60),
            cand(3.0, 0.0, 0.5, 0),
        ];
        let wires = candidates_to_wire(&cands);
        assert_eq!(wires.len(), 3);
        assert_eq!(wires[0].center, [1.0, 0.0, 0.5]);
        assert_eq!(wires[1].vertex_count, 60);
        assert_eq!(wires[2].vertex_count, 0);
        // 丸ごとバイト列 (3×48=144B) として決定的
        let bytes: &[u8] = bytemuck::cast_slice(&wires);
        assert_eq!(bytes.len(), 144);
    }

    /// wave 44-5: candidate_count > 2^24 は f32 輸送の整数厳密性が破れる
    /// ため構築時 fail-loud。
    #[test]
    #[should_panic(expected = "candidate_count")]
    fn params_wire_rejects_count_over_2pow24() {
        let f = identity_frustum();
        let _ = ParamsWire::new(
            &f,
            [0.0, 0.0, 0.0],
            &CompactPolicy::default(),
            (1 << 24) + 1,
        );
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

    /// wave 28-1: 単位 VP (column-major 16) からの 6 平面が整数係数と厳密一致。
    /// near が [0,0,1,0] (= z >= 0) であることが wgpu 規則の機械ピン —
    /// 旧 GL 式 [0,0,1,1] では確実に赤 (identity_frustum() の手書き定数とも一致)。
    #[test]
    fn identity_extractor_planes_exact_bits() {
        let m = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        let f = FrustumPlanes::from_view_proj(&m);
        let want: [[f32; 4]; 6] = [
            [1.0, 0.0, 0.0, 1.0],
            [-1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 1.0],
            [0.0, -1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0, 0.0], // near: z >= 0 (wgpu)
            [0.0, 0.0, -1.0, 1.0],
        ];
        for i in 0..6 {
            for c in 0..4 {
                assert_eq!(f.0[i][c].to_bits(), want[i][c].to_bits(), "plane {i}[{c}]");
            }
        }
    }

    /// wave 28-2: near 背面の箱は除かれる (wgpu z>=0 規則)。旧 GL 式では
    /// この箱は「保守側に残る」ためテストが確実に赤くなる回帰検出器。
    #[test]
    fn near_back_culled_with_wgpu_plane_rule() {
        let m = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        let f = FrustumPlanes::from_view_proj(&m);
        // z ∈ [-1.6, -0.6] — 全体が near (z=0) 背面
        let behind = aabb_in_frustum(&f, [0.0, 0.0, -1.1], [0.5, 0.5, 0.5], 0.0);
        assert!(!behind, "behind-near box must be culled under wgpu rule");
        // ちょうど跨ぐ箱 (z ∈ [-0.6, 0.4]) は残る (保守規則の証明)
        let straddle = aabb_in_frustum(&f, [0.0, 0.0, -0.1], [0.5, 0.5, 0.5], 0.0);
        assert!(straddle, "straddler across near stays visible");
    }

    /// wave 28-3: compact_draws の生存・除外・出力値・ソート安定性を厳密ピン。
    #[test]
    fn compact_draws_exact_survivors_and_stable_order() {
        let f = identity_frustum();
        let base = cand(0.0, 0.0, 0.5, 36);
        let cands = vec![
            // 0: 生存, vc=36, fv=7
            DrawCandidate {
                first_vertex: 7,
                ..base
            },
            // 1: 生存, vc=12, fv=3 (ソートで先頭に来る)
            DrawCandidate {
                vertex_count: 12,
                first_vertex: 3,
                ..base
            },
            // 2: 錐台完全外 (x=5)
            cand(5.0, 0.0, 0.5, 36),
            // 3: 空 (vc=0)
            DrawCandidate {
                vertex_count: 0,
                ..base
            },
            // 4: 前フレーム遮蔽
            DrawCandidate {
                visible_prev: false,
                ..base
            },
            // 5: 距離外 (dx=600 > 512)
            cand(600.0, 0.0, 0.5, 36),
            // 6: 生存, vc=9, fv=3 — 1 と同一キー (安定性検証用)
            DrawCandidate {
                vertex_count: 9,
                first_vertex: 3,
                ..base
            },
        ];
        let mut out = Vec::new();
        let n = compact_draws(
            &cands,
            &f,
            [0.0, 0.0, 0.0],
            &CompactPolicy::default(),
            &mut out,
        );
        assert_eq!(n, 3, "3 survivors");
        let cmd = |vertex_count, first_vertex| IndirectDrawCmd {
            vertex_count,
            instance_count: 1,
            first_vertex,
            first_instance: 0,
        };
        assert_eq!(
            out,
            vec![cmd(12, 3), cmd(9, 3), cmd(36, 7)],
            "first_vertex 昇順、同キーは入力順 (stable)"
        );
    }

    /// wave 28-4: WGSL のワイヤレイアウトを naga で実機固定
    /// (Candidate 52B / Cmd 16B / Params 128B + 各メンバ offset)。
    #[test]
    fn wgsl_layout_matches_pinned_offsets() {
        let module = naga::front::wgsl::parse_str(COMPACT_WGSL).expect("COMPACT_WGSL must parse");
        assert!(
            module.entry_points.iter().any(|f| f.name == "cs_compact"),
            "cs_compact entry missing"
        );
        let cases: &[(&str, u32, &[(&str, u32)])] = &[
            (
                // WGSL レイアウト規則で厳密導出: vec3 align=16 → center 0..12,
                // half_ext roundUp(16,12)=16..28, 以降 u32 は align 4 密詰み
                // (28,32,36,40,44)。span = roundUp(16,48) = 48。
                "Candidate",
                48,
                &[
                    ("center", 0),
                    ("half_ext", 16),
                    ("vertex_count", 28),
                    ("first_vertex", 32),
                    ("visible_prev", 36),
                    ("pass_key", 40),
                    ("frustum_seen", 44),
                ],
            ),
            (
                "Cmd",
                16,
                &[
                    ("vertex_count", 0),
                    ("instance_count", 4),
                    ("first_vertex", 8),
                    ("first_instance", 12),
                ],
            ),
            (
                "Params",
                128,
                &[("planes", 0), ("camera", 96), ("policy", 112)],
            ),
        ];
        for (name, want_span, want_members) in cases {
            let (_, ty) = module
                .types
                .iter()
                .find(|(_, t)| t.name.as_deref() == Some(*name))
                .unwrap_or_else(|| panic!("struct {name} not found"));
            let naga::TypeInner::Struct { members, span } = &ty.inner else {
                panic!("{name} is not a struct");
            };
            assert_eq!(*span, *want_span, "{name} span");
            for (mname, want_off) in *want_members {
                let m = members
                    .iter()
                    .find(|m| m.name.as_deref() == Some(*mname))
                    .unwrap_or_else(|| panic!("{name}.{mname} not found"));
                assert_eq!(m.offset, *want_off, "{name}.{mname} offset");
            }
        }
    }
}

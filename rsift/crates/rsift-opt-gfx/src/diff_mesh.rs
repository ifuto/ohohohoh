//! Section-level differential mesh updates (Tier 2).
//! Block edits dirty 16³ sections only — never rebuild the whole column.

use std::collections::{BTreeMap, HashMap};

pub const SECTION_SIZE: i32 = 16;
pub const SECTIONS_Y: i32 = 24; // -64..320 → 24 sections

/// セクションメッシュの差分パッチ (GPU アップロード単位)。
///
/// `quad_count` と `vertex_bytes` / `index_bytes` の整合 (stride 倍数、
/// quad×6 インデックス数等) は本モジュールでは**検証しない** (生成側の責務)。
/// 2026-07-22 wave 31 で契約明文化。
#[derive(Debug, Clone)]
pub struct MeshPatch {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub section_y: i32,
    pub vertex_bytes: Vec<u8>,
    pub index_bytes: Vec<u8>,
    pub quad_count: u32,
}

#[derive(Debug, Default)]
pub struct DiffMeshUpdater {
    /// (cx, cz) → bitset of dirty section indices (bit 0 = lowest Y section)
    dirty: HashMap<(i32, i32), u32>,
    /// Cached section mesh blobs for upload
    patches: Vec<MeshPatch>,
}

impl DiffMeshUpdater {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn block_to_section_y(block_y: i32) -> i32 {
        // world Y -64 → section 0
        (block_y + 64).div_euclid(SECTION_SIZE)
    }

    /// 注意 (**垂直のみ**): 本関数は垂直方向の隣接セクション伝播だけを行う。
    /// ブロックが chunk 端 (local x/z ∈ {0,15}) にある場合、隣接チャンクの
    /// セクションメッシュもこのブロックに依存し陳腐化し得るが、本 API は
    /// ブロックの chunk 内座標を受け取らないため**水平伝播は呼び出し側の
    /// 責務** (該当時は隣接 (cx±1, cz) / (cx, cz±1) にも同じ block_y で
    /// 呼ぶこと)。2026-07-22 wave 31 で契約明文化。
    pub fn mark_block_dirty(&mut self, chunk_x: i32, chunk_z: i32, block_y: i32) {
        let sy = Self::block_to_section_y(block_y);
        if sy < 0 || sy >= SECTIONS_Y {
            return;
        }
        let bits = self.dirty.entry((chunk_x, chunk_z)).or_insert(0);
        *bits |= 1u32 << sy;
        // Neighbor sections if on boundary
        let local_y = (block_y + 64).rem_euclid(SECTION_SIZE);
        if local_y == 0 && sy > 0 {
            *bits |= 1u32 << (sy - 1);
        }
        if local_y == SECTION_SIZE - 1 && sy + 1 < SECTIONS_Y {
            *bits |= 1u32 << (sy + 1);
        }
    }

    pub fn dirty_sections(&self, chunk_x: i32, chunk_z: i32) -> Vec<i32> {
        let mut out = Vec::new();
        if let Some(bits) = self.dirty.get(&(chunk_x, chunk_z)) {
            for i in 0..SECTIONS_Y {
                if bits & (1 << i) != 0 {
                    out.push(i);
                }
            }
        }
        out
    }

    pub fn clear_chunk(&mut self, chunk_x: i32, chunk_z: i32) {
        self.dirty.remove(&(chunk_x, chunk_z));
    }

    pub fn take_dirty_mask(&mut self, chunk_x: i32, chunk_z: i32) -> u32 {
        self.dirty.remove(&(chunk_x, chunk_z)).unwrap_or(0)
    }

    /// Register a rebuilt section patch for GPU upload.
    pub fn push_patch(&mut self, patch: MeshPatch) {
        // Replace existing patch for same section
        if let Some(pos) = self.patches.iter().position(|p| {
            p.chunk_x == patch.chunk_x
                && p.chunk_z == patch.chunk_z
                && p.section_y == patch.section_y
        }) {
            self.patches[pos] = patch;
        } else {
            self.patches.push(patch);
        }
    }

    pub fn drain_patches(&mut self) -> Vec<MeshPatch> {
        std::mem::take(&mut self.patches)
    }

    pub fn pending_patch_count(&self) -> usize {
        self.patches.len()
    }

    pub fn dirty_chunk_count(&self) -> usize {
        self.dirty.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_neighbors_on_boundary() {
        let mut d = DiffMeshUpdater::new();
        // Y=-64 is section 0 local_y=0 → also dirties nothing below
        d.mark_block_dirty(0, 0, -64);
        let s = d.dirty_sections(0, 0);
        assert!(s.contains(&0));
        // Y=-49 is top of section 0 → dirties section 1
        d.mark_block_dirty(0, 0, -49);
        let s = d.dirty_sections(0, 0);
        assert!(s.contains(&0) && s.contains(&1));
    }

    /// wave 31-1: block_to_section_y の厳密境界列 + 範囲外は非 dirty。
    #[test]
    fn section_y_mapping_exact_boundaries() {
        // (block_y + 64).div_euclid(16) の厳密列。
        let table = [
            (-65, -1), // 範囲外下 (拒否)
            (-64, 0),
            (-49, 0),
            (-48, 1),
            (0, 4),
            (255, 19),
            (304, 23),
            (319, 23),
            (320, 24), // 範囲外上 (拒否)
        ];
        for (y, want) in table {
            assert_eq!(DiffMeshUpdater::block_to_section_y(y), want, "y={y}");
        }
        let mut d = DiffMeshUpdater::new();
        d.mark_block_dirty(0, 0, -65);
        d.mark_block_dirty(0, 0, 320);
        assert_eq!(d.dirty_chunk_count(), 0, "範囲外 y は dirty を残さない");
    }

    /// wave 31-2: 境界伝播の bit 厳密ピン (上下端クランプ含む) と
    /// take_dirty_mask の排他消費。
    #[test]
    fn dirty_bits_exact_with_boundary_clamps_and_take_exclusive() {
        let mut d = DiffMeshUpdater::new();
        // y=0: sy=4, local 0 → sy-1=3 も伝播 → 0b11000 = 0x18
        d.mark_block_dirty(0, 0, 0);
        assert_eq!(d.take_dirty_mask(0, 0), 0x18);
        // take は排他消費: 直後は 0、別チャンクは無関係
        assert_eq!(d.take_dirty_mask(0, 0), 0);
        assert_eq!(d.take_dirty_mask(1, 1), 0);
        // y=319: sy=23, local 15 だが sy+1=24 は範囲外 → bit 23 のみ
        d.mark_block_dirty(0, 0, 319);
        assert_eq!(d.take_dirty_mask(0, 0), 1u32 << 23);
        // y=-64: sy=0, local 0 だが sy>0 不成立 → bit 0 のみ
        d.mark_block_dirty(0, 0, -64);
        assert_eq!(d.take_dirty_mask(0, 0), 1);
        // 中央 (y=8): sy=4 local 8 → bit 4 のみ
        d.mark_block_dirty(0, 0, 8);
        assert_eq!(d.take_dirty_mask(0, 0), 1 << 4);
        // 合算: y=304 (sy=23 local 0 → bit 23,22) + y=300 (sy=22 local 12 → bit 22)
        d.mark_block_dirty(0, 0, 304);
        d.mark_block_dirty(0, 0, 300);
        assert_eq!(d.take_dirty_mask(0, 0), (1 << 23) | (1 << 22));
    }

    /// wave 31-3: dirty_sections は昇順決定的。
    #[test]
    fn dirty_sections_ascending_deterministic() {
        let mut d = DiffMeshUpdater::new();
        // bit 0 (y=-64) / bit 2 (y=-32: sy=2 local 0 → bit 2,1) / bit 4 (y=0 で bit 4,3 も)
        d.mark_block_dirty(0, 0, -64); // bit 0
        d.mark_block_dirty(0, 0, -32); // sy=2 local 0 → bits 2,1
        d.mark_block_dirty(0, 0, 0); // sy=4 local 0 → bits 4,3
        assert_eq!(d.dirty_sections(0, 0), vec![0, 1, 2, 3, 4]);
        assert!(d.dirty_sections(9, 9).is_empty(), "未登録チャンクは空");
    }

    /// wave 31-4: push_patch は同一セクションを新で置換、drain で全回収。
    #[test]
    fn push_patch_replaces_same_section_and_drains_all() {
        let mut d = DiffMeshUpdater::new();
        let patch = |quad: u32, sy: i32| MeshPatch {
            chunk_x: 1,
            chunk_z: 2,
            section_y: sy,
            vertex_bytes: vec![7; 16],
            index_bytes: vec![3; 24],
            quad_count: quad,
        };
        d.push_patch(patch(10, 5));
        d.push_patch(patch(99, 5)); // 同一セクション → 置換
        d.push_patch(patch(20, 6)); // 別セクション → 追加
        assert_eq!(d.pending_patch_count(), 2);
        let out = d.drain_patches();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].quad_count, 99, "置換後は新パッチが生きる");
        assert_eq!(out[1].quad_count, 20);
        assert_eq!(d.pending_patch_count(), 0, "drain 後は空");
    }
}

// ============================================================================
// wave 215 — Per-block incremental mesh (差分描画の中核)
//
// 既存 DiffMeshUpdater は「汚染セクション集合の管理 + patch queue」層で、
// 汚染セクション当たりの再メッシュは依然として 16^3 全走査 + 全面 Tipsify
// を要求した (pseudo_mc_bench 編集ワークロード C 行で機械確定: 787 汚染
// セクション再構築 801 ms wall / Tipsify CPU 累積 967 ms ≈ 全コスト)。
// 本 DiffSectionMesh は単一/少数ブロック編集の再メッシュを、影響範囲
// 7 ブロック × 6 面の差分再評価 + dead-tri マーク + 最適化償却へ帰着
// させる。面集合は常に FULL rescan と bit 一致 (テストで機械証明)。
//
// 設計ノート (stub なし声明): kill は「位置台帳」ではなく「頂点署名
// (= 面の 4 local id) による値検索」で行う。Tipsify (optimize) は index
// 列を任意に並替えるため位置台帳は再最適化のたびに不整合になる — 署名
// 検索は台帳不変のため任意の並替と両立する (三角形 2 個 (v0,v1,v2)+
// (v0,v2,v3) を 1 パス走査で零面積化、見付からなければ内部不整合として
// assert で落とす)。コストは kill 1 件あたり O(indices) — 42 kill でも
// 全量再構築 (16^3 走査 + Tipsify 1.2 ms) の 1/10 未満。
// ============================================================================

/// bench FACES と同一の 6 面テーブル (差分パス専有。頂点並び規約:
/// クアッド (v0,v1,v2,v3) → 三角形 (0,1,2)+(0,2,3)。一致は bench 側で
/// 実行時検査する — 設計の単一源。
pub const DIFF_FACES: [([i64; 3], [[f32; 3]; 4]); 6] = [
    (
        [0, 1, 0],
        [[0., 1., 0.], [1., 1., 0.], [1., 1., 1.], [0., 1., 1.]],
    ),
    (
        [0, -1, 0],
        [[0., 0., 0.], [0., 0., 1.], [1., 0., 1.], [1., 0., 0.]],
    ),
    (
        [0, 0, 1],
        [[0., 0., 0.], [1., 0., 0.], [1., 1., 1.], [0., 1., 1.]],
    ),
    (
        [0, 0, -1],
        [[0., 0., 0.], [0., 1., 0.], [1., 1., 0.], [1., 0., 0.]],
    ),
    (
        [1, 0, 0],
        [[1., 0., 0.], [1., 1., 0.], [1., 1., 1.], [1., 0., 1.]],
    ),
    (
        [-1, 0, 0],
        [[0., 0., 0.], [0., 0., 1.], [0., 1., 1.], [0., 1., 0.]],
    ),
];

// slot key は (pos, 軸法線 dir 0..6) の直接索引。旧版は法線 27 組合せ空間
// ((n+1) の 3^3) で割付けていたが、DIFF_FACES は軸 6 法線のみを使うため
// 21/27 は永久未使用 → dir (0..6) をキーに縮約 (wave 217 HN: RD24 規模の
// 差分レーンで touched セクション x 0.53MB が 3GB sandbox の OOM になった
// ため 0.118MB へ縮小。意味論は同一 = pos+dir 単射性は保たれる)。
const SLOT_N: usize = 17 * 17 * 17 * 6;
/// 差分再最適化トリガ: 全三角形中の死三角形割合 (per-mille)
pub const REOPT_DEAD_PERMILLE: u32 = 125;
/// 差分再最適化トリガ: 連続編集回数
pub const REOPT_EDITS: u32 = 16;

/// slot エントリ: high u16 = 世代 (0 は初期未使用), low u16 = local id + 1
/// (0 = 空)。世代スタンプで 531KB の全面 memset を世代比較に置換 —
/// 全量再構築パスが汚染セクション毎に払っていた SLOT_N u32 初期化
/// (0.53 MB × 787 ≈ 417 MB のメモリ書込) を差分パスでは払わない。
#[inline]
fn slot_pack(gen: u16, id_plus1: u16) -> u32 {
    ((gen as u32) << 16) | (id_plus1 as u32)
}

/// 単一ブロック編集に対する差分セクションメッシュ状態。
/// 頂点 id は append 安定 (編集で既存頂点の並びは変化しない)、除去面は
/// dead-tri (全頂点同一の零面積三角形 = GPU が自動破棄) に退避し、穴率
/// または連続編集数が閾値を超えた時点で一次再最適化 (頂点圧縮 + Tipsify)
/// に落ちる。編集後の面集合は常に FULL rescan と集合として一致する
/// (機械証明テスト: `edit_face_set_matches_full_rescan`).
pub struct DiffSectionMesh {
    slot_tab: Vec<u32>,
    gen: u16,
    next_vert: u32,
    verts: Vec<u8>,
    /// face_key (x,y,z 各 0..15 の 4bit + dir 3bit) → 頂点署名 [lv0..lv3]
    faces: BTreeMap<u32, [u32; 4]>,
    indices: Vec<u32>,
    dead_tris: u32,
    edits_since_reopt: u32,
    // ---- 誠実計上 (bench 表示に使う公開カウンタ) ----
    pub face_evals: u64,
    pub verts_encoded: u64,
    pub reopts: u32,
    /// reoptimize が再構築した頂点バイト量の累積 (bench 誠実計上)
    pub bytes_rebuilt: u64,
}

impl DiffSectionMesh {
    /// セクション全面の初期構築 (全量再構築と同一走査・同一 slot 意味).
    /// 初期状態は Tipsify 未実行 — 呼出側が方針に応じて reoptimize する。
    pub fn build_full(
        get: &impl Fn(i64, i64, i64) -> u8,
        is_opaque: &impl Fn(u8) -> bool,
        origin: [i64; 3],
    ) -> Self {
        let mut m = Self {
            slot_tab: vec![0u32; SLOT_N],
            gen: 1,
            next_vert: 0,
            verts: Vec::new(),
            faces: BTreeMap::new(),
            indices: Vec::new(),
            dead_tris: 0,
            edits_since_reopt: 0,
            face_evals: 0,
            verts_encoded: 0,
            reopts: 0,
            bytes_rebuilt: 0,
        };
        for by in 0..16i64 {
            for bz in 0..16i64 {
                for bx in 0..16i64 {
                    let (wx, wy, wz) = (origin[0] + bx, origin[1] + by, origin[2] + bz);
                    if get(wx, wy, wz) == 0 {
                        continue;
                    }
                    for (dir, (n, _)) in DIFF_FACES.iter().enumerate() {
                        m.face_evals += 1;
                        if is_opaque(get(wx + n[0], wy + n[1], wz + n[2])) {
                            continue;
                        }
                        m.insert_face(bx, by, bz, dir);
                    }
                }
            }
        }
        m
    }

    #[inline]
    fn face_key(bx: i64, by: i64, bz: i64, dir: usize) -> u32 {
        (bx as u32) | ((by as u32) << 4) | ((bz as u32) << 8) | ((dir as u32) << 12)
    }

    /// 新規面の 4 頂点を slot dedup 付きで append し 6 index を末尾に積む。
    fn insert_face(&mut self, bx: i64, by: i64, bz: i64, dir: usize) {
        let (n, quad) = DIFF_FACES[dir];
        let mut lv = [0u32; 4];
        for (vi, v) in quad.iter().enumerate() {
            let slot = (((bx as usize + v[0] as usize) * 17 + (by as usize + v[1] as usize)) * 17
                + (bz as usize + v[2] as usize))
                * 6
                + dir;
            let ent = self.slot_tab[slot];
            if (ent >> 16) as u16 == self.gen && (ent & 0xFFFF) != 0 {
                lv[vi] = (ent & 0xFFFF) as u32 - 1;
            } else {
                let q = crate::chunk_mesh::Quantized12ByteVertex::encode(
                    bx as f32 + v[0],
                    by as f32 + v[1],
                    bz as f32 + v[2],
                    n[0] as f32,
                    n[1] as f32,
                    n[2] as f32,
                    0.5,
                    0.5,
                );
                self.verts.extend_from_slice(bytemuck::bytes_of(&q));
                let id = self.next_vert;
                debug_assert_eq!(self.verts.len() / 12 - 1, id as usize);
                assert!(
                    id + 1 < 65535,
                    "diff_mesh: vertex id overflow (force reopt)"
                );
                self.slot_tab[slot] = slot_pack(self.gen, id as u16 + 1);
                self.next_vert += 1;
                self.verts_encoded += 1;
                lv[vi] = id;
            }
        }
        self.indices
            .extend_from_slice(&[lv[0], lv[1], lv[2], lv[0], lv[2], lv[3]]);
        self.faces.insert(Self::face_key(bx, by, bz, dir), lv);
    }

    /// 既存面を dead-tri (零面積三角形) に退避。署名検索 1 パスで 2 三角形
    /// とも零面積化する (完走時 found!=2 は内部不整合 = assert)。
    /// 頂点配列は触らない (append 安定 = 共有頂点の誤切断なし)。
    fn kill_face(&mut self, key: u32, sig: [u32; 4]) {
        if self.faces.remove(&key).is_none() {
            return;
        }
        let t0 = [sig[0], sig[1], sig[2]];
        let t1 = [sig[0], sig[2], sig[3]];
        let mut found = 0u32;
        for tri in self.indices.chunks_mut(3) {
            if tri == t0 || tri == t1 {
                let v = tri[0];
                tri[0] = v;
                tri[1] = v;
                tri[2] = v;
                found += 1;
                if found == 2 {
                    break;
                }
            }
        }
        assert!(
            found == 2,
            "diff_mesh: kill 対象の三角形が見付からない (内部不整合) key={key}"
        );
        self.dead_tris += 2;
    }

    /// 単一ブロック編集 (world 座標) を差分適用。影響範囲 = 編集ブロック +
    /// 6 近傍の計 7 ブロック × 6 面 = 最大 42 候補面のみを再評価する。
    /// origin を基点とする本セクション外に基底のある面は触らない
    /// (隣接セクションの面はそのセクション側の呼出が担保 — vanilla 汚染
    /// 規約と同一)。返り値は (追加面数, 除去面数)。
    pub fn apply_edit(
        &mut self,
        get: &impl Fn(i64, i64, i64) -> u8,
        is_opaque: &impl Fn(u8) -> bool,
        origin: [i64; 3],
        wx0: i64,
        wy0: i64,
        wz0: i64,
    ) -> (u32, u32) {
        let mut added = 0u32;
        let mut removed = 0u32;
        for (ox, oy, oz) in [
            (0i64, 0, 0),
            (1, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ] {
            let (wx, wy, wz) = (wx0 + ox, wy0 + oy, wz0 + oz);
            let (bx, by, bz) = (wx - origin[0], wy - origin[1], wz - origin[2]);
            if !(0..16).contains(&bx) || !(0..16).contains(&by) || !(0..16).contains(&bz) {
                continue;
            }
            let solid_here = get(wx, wy, wz) != 0;
            for (dir, (n, _)) in DIFF_FACES.iter().enumerate() {
                self.face_evals += 1;
                let want = solid_here && !is_opaque(get(wx + n[0], wy + n[1], wz + n[2]));
                let key = Self::face_key(bx, by, bz, dir);
                match (want, self.faces.get(&key).copied()) {
                    (true, None) => {
                        self.insert_face(bx, by, bz, dir);
                        added += 1;
                    }
                    (false, Some(sig)) => {
                        self.kill_face(key, sig);
                        removed += 1;
                    }
                    _ => {}
                }
            }
        }
        self.edits_since_reopt += 1;
        (added, removed)
    }

    /// dead-tri 率または連続編集数で真の再最適化が必要か。
    pub fn needs_reopt(&self) -> bool {
        let total_tris = self.indices.len() / 3;
        if total_tris > 0
            && self.dead_tris as u64 * 1000 > total_tris as u64 * REOPT_DEAD_PERMILLE as u64
        {
            return true;
        }
        self.edits_since_reopt >= REOPT_EDITS
    }

    /// 一次再最適化: 面をキー順 (BTreeMap 決定順) に再構成し、頂点を実使用
    /// 分に圧縮 (再 encode なしの 12B バイトコピー = 決定的) → Tipsify 適用
    /// → dead 0 化。faces 台帳は署名 [lv0..lv3] へ引き直すため並替と両立。
    /// 返り値は圧縮後頂点数。
    pub fn reoptimize(&mut self, vco: &mut crate::vertex_cache_opt::VertexCacheOptimizer) -> u32 {
        self.gen = self.gen.wrapping_add(1);
        if self.gen == 0 {
            self.gen = 1;
        }
        let old_verts = std::mem::take(&mut self.verts);
        let old_faces = std::mem::take(&mut self.faces);
        let mut remap: Vec<u32> = vec![u32::MAX; self.next_vert as usize];
        // indices は再構築で全置換 (old は破棄、容量は new_indices 側で確保)
        self.indices = Vec::new();
        let mut new_indices: Vec<u32> = Vec::with_capacity(old_faces.len() * 6);
        let mut next = 0u32;
        for (key, old_lv) in old_faces.iter() {
            let mut lv = [0u32; 4];
            for (vi, &oid) in old_lv.iter().enumerate() {
                let id = if remap[oid as usize] != u32::MAX {
                    remap[oid as usize]
                } else {
                    let s = oid as usize * 12;
                    self.verts.extend_from_slice(&old_verts[s..s + 12]);
                    remap[oid as usize] = next;
                    next += 1;
                    next - 1
                };
                lv[vi] = id;
            }
            // slot_tab も新世代で引き直す (再 encode なしでも slot 参照が
            // 後続編集の dedup に効くよう整合させる)
            let (bx, by, bz, dir) = (
                (key & 15) as i64,
                ((key >> 4) & 15) as i64,
                ((key >> 8) & 15) as i64,
                (key >> 12) as usize,
            );
            let (_, quad) = DIFF_FACES[dir];
            for (vi, v) in quad.iter().enumerate() {
                let slot = (((bx as usize + v[0] as usize) * 17 + (by as usize + v[1] as usize))
                    * 17
                    + (bz as usize + v[2] as usize))
                    * 6
                    + dir;
                self.slot_tab[slot] = slot_pack(self.gen, lv[vi] as u16 + 1);
            }
            self.faces.insert(*key, lv);
            new_indices.extend_from_slice(&[lv[0], lv[1], lv[2], lv[0], lv[2], lv[3]]);
        }
        self.next_vert = next;
        self.indices = vco.optimize(&new_indices);
        self.dead_tris = 0;
        self.edits_since_reopt = 0;
        self.bytes_rebuilt += self.verts.len() as u64;
        self.reopts += 1;
        next
    }

    /// 現在の index 列への読出し (bench/計測用)。dead-tri は含まれる。
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }
    pub fn vertex_bytes(&self) -> &[u8] {
        &self.verts
    }
    pub fn vertex_count(&self) -> u32 {
        self.next_vert
    }
    pub fn live_face_count(&self) -> usize {
        self.faces.len()
    }
    pub fn dead_tri_count(&self) -> u32 {
        self.dead_tris
    }
    /// 決定性検査: 面集合照合用。
    pub fn has_face(&self, bx: i64, by: i64, bz: i64, dir: usize) -> bool {
        self.faces.contains_key(&Self::face_key(bx, by, bz, dir))
    }
    /// コスト計測 (bench 表記用): 維持 indices 長。
    pub fn indices_len(&self) -> usize {
        self.indices.len()
    }
}

#[cfg(test)]
mod wave215_tests {
    use super::*;
    use std::collections::HashSet;

    /// ミニミニチュアワールド (差分パス検証専用): 集合ベースの決定論ブロック配置.
    struct Mini(std::cell::RefCell<HashSet<(i64, i64, i64)>>);
    impl Mini {
        fn get(&self, x: i64, y: i64, z: i64) -> u8 {
            if self.0.borrow().contains(&(x, y, z)) {
                1
            } else {
                0
            }
        }
        fn set(&self, x: i64, y: i64, z: i64, solid: bool) {
            if solid {
                self.0.borrow_mut().insert((x, y, z));
            } else {
                self.0.borrow_mut().remove(&(x, y, z));
            }
        }
    }
    fn opaque(id: u8) -> bool {
        id != 0
    }

    /// 参照実装 (ナイーブ全面走査): origin 基準のセクション面集合を
    /// 毎回 16^3 × 6 で求める。差分実装の決定性真値源。
    fn ref_faces(m: &Mini, origin: [i64; 3]) -> HashSet<u32> {
        let mut out = HashSet::new();
        for by in 0..16i64 {
            for bz in 0..16i64 {
                for bx in 0..16i64 {
                    if m.get(origin[0] + bx, origin[1] + by, origin[2] + bz) == 0 {
                        continue;
                    }
                    for (dir, (n, _)) in DIFF_FACES.iter().enumerate() {
                        if opaque(m.get(
                            origin[0] + bx + n[0],
                            origin[1] + by + n[1],
                            origin[2] + bz + n[2],
                        )) {
                            continue;
                        }
                        out.insert(DiffSectionMesh::face_key(bx, by, bz, dir));
                    }
                }
            }
        }
        out
    }

    fn assert_parity(mesh: &DiffSectionMesh, m: &Mini, origin: [i64; 3], ctx: &str) {
        let rf = ref_faces(m, origin);
        assert_eq!(mesh.live_face_count(), rf.len(), "{ctx}: 面数不一致");
        // 全 key 空間を網羅走査 (16^3 x 6 = 24,576) — 両方向の集合一致を頑健に証明
        for by in 0..16i64 {
            for bz in 0..16i64 {
                for bx in 0..16i64 {
                    for dir in 0..6 {
                        let k = DiffSectionMesh::face_key(bx, by, bz, dir);
                        assert_eq!(
                            mesh.has_face(bx, by, bz, dir),
                            rf.contains(&k),
                            "{ctx}: 面の集合差 at ({bx},{by},{bz}) dir={dir}"
                        );
                    }
                }
            }
        }
    }

    const O: [i64; 3] = [0, 0, 0];

    /// HL-215-1 (核心): 連続ランダム編集の各ステッチ後で、差分適用後の面集合が
    /// ナイーブ全面走査と集合一致 (bit 一致) すること。途中で reoptimize を
    /// 挟んでも不変であること (再最適化は面集合を破壊しない)。
    #[test]
    fn edit_face_set_matches_full_rescan_through_edits_and_reopts() {
        let m = Mini(std::cell::RefCell::new(HashSet::new()));
        // 初期地形: 下半分 (y<8) を埋める + 丘 6 個
        for y in 0..8i64 {
            for z in 0..16i64 {
                for x in 0..16i64 {
                    m.set(x, y, z, true);
                }
            }
        }
        for (x, y, z) in [
            (3, 8, 3),
            (4, 8, 3),
            (3, 8, 4),
            (10, 8, 10),
            (10, 9, 10),
            (15, 8, 0),
        ] {
            m.set(x, y, z, true);
        }
        let get = |x, y, z| m.get(x, y, z);
        let mut mesh = DiffSectionMesh::build_full(&get, &opaque, O);
        assert_parity(&mesh, &m, O, "初期 build_full");
        let mut vco = crate::vertex_cache_opt::VertexCacheOptimizer::new(16);
        // 決定的編集列 (境界 + 丘 + 平地、add/remove 交互、全 40 件)
        let mut xs = 0xC0FFEEu32;
        for i in 0..40usize {
            xs = xs.wrapping_mul(1664525).wrapping_add(1013904223);
            let bx = ((xs >> 16) % 16) as i64;
            xs = xs.wrapping_mul(1664525).wrapping_add(1013904223);
            let by = 8 + ((xs >> 16) % 3) as i64;
            xs = xs.wrapping_mul(1664525).wrapping_add(1013904223);
            let bz = ((xs >> 16) % 16) as i64;
            let cur = m.get(bx, by, bz) != 0;
            m.set(bx, by, bz, !cur);
            mesh.apply_edit(&get, &opaque, O, bx, by, bz);
            let act = if cur { "remove" } else { "add" };
            assert_parity(&mesh, &m, O, &format!("edit #{i} ({bx},{by},{bz}) {act}"));
            if i % 5 == 4 || mesh.needs_reopt() {
                mesh.reoptimize(&mut vco);
                assert_eq!(mesh.dead_tri_count(), 0, "reopt 後 dead リセット");
                assert_eq!(
                    mesh.indices_len(),
                    mesh.live_face_count() * 6,
                    "reopt 後 indices==面×6"
                );
                assert_parity(&mesh, &m, O, &format!("reopt 後 (edit #{i})"));
            }
        }
    }

    /// HL-215-2: 配置→撤去往復で面集合 0、dead-tri 全数退避、頂点バイトは
    /// append 安定 (縮まない)。
    #[test]
    fn add_then_remove_yields_empty_faces_and_dead_tris() {
        let m = Mini(std::cell::RefCell::new(HashSet::new()));
        let get = |x, y, z| m.get(x, y, z);
        let mut mesh = DiffSectionMesh::build_full(&get, &opaque, O);
        assert_eq!(mesh.live_face_count(), 0);
        m.set(5, 5, 5, true);
        mesh.apply_edit(&get, &opaque, O, 5, 5, 5);
        assert_eq!(mesh.live_face_count(), 6, "孤立ブロックは 6 面");
        assert_eq!(mesh.dead_tri_count(), 0);
        let verts_after_add = mesh.vertex_bytes().len();
        assert!(verts_after_add > 0);
        m.set(5, 5, 5, false);
        mesh.apply_edit(&get, &opaque, O, 5, 5, 5);
        assert_eq!(mesh.live_face_count(), 0);
        assert_eq!(
            mesh.dead_tri_count() as usize * 3,
            mesh.indices_len(),
            "全三角形が死"
        );
        assert_eq!(
            mesh.vertex_bytes().len(),
            verts_after_add,
            "頂点バイトは append 安定"
        );
    }

    /// HL-215-3: dead 退避された三角形は厳密に零面積 (3 頂点同一) で、
    /// 生存三角形は混じらない。
    #[test]
    fn killed_triangles_are_exactly_zero_area() {
        let m = Mini(std::cell::RefCell::new(HashSet::new()));
        for z in 0..4i64 {
            for x in 0..4i64 {
                m.set(x, 0, z, true);
            }
        }
        let get = |x, y, z| m.get(x, y, z);
        let mut mesh = DiffSectionMesh::build_full(&get, &opaque, O);
        // 内部ブロック撤去で 6+ 面露出 → その内 1 ブロック分を撤去 → 再カバー
        m.set(2, 0, 2, false);
        mesh.apply_edit(&get, &opaque, O, 2, 0, 2);
        m.set(2, 0, 2, true);
        mesh.apply_edit(&get, &opaque, O, 2, 0, 2);
        let mut zero_area = 0u32;
        let mut live = 0u32;
        for tri in mesh.indices().chunks(3) {
            if tri[0] == tri[1] && tri[1] == tri[2] {
                zero_area += 1;
            } else {
                live += 1;
            }
        }
        assert_eq!(
            zero_area,
            mesh.dead_tri_count(),
            "dead 台帳と実 zero-area 数一致"
        );
        assert_eq!(
            live as usize,
            mesh.live_face_count() * 2,
            "生存三角形 = 面×2"
        );
    }

    /// HL-215-4: 頂点 dedup は「位置 x 法線」単位 — 隣接 2 ブロックの同法線面で
    /// 共有頂点が再利用される (encode 回数の厳密期待)。
    #[test]
    fn dedup_shared_vertex_slots_across_edits() {
        let m = Mini(std::cell::RefCell::new(HashSet::new()));
        for y in 0..8i64 {
            for z in 0..16i64 {
                for x in 0..16i64 {
                    m.set(x, y, z, true);
                }
            }
        }
        let get = |x, y, z| m.get(x, y, z);
        let mut mesh = DiffSectionMesh::build_full(&get, &opaque, O);
        let v0 = mesh.vertex_count();
        let f0 = mesh.live_face_count();
        // (4,8,4) 配置 (内壁から遠い孤立ブロック): 露出面 = 下以外 5 面 → 20 頂点
        // (法線違いは同一座標でも別 slot のため共有ゼロ — 壁伝播は起きない内部点)
        m.set(4, 8, 4, true);
        mesh.apply_edit(&get, &opaque, O, 4, 8, 4);
        assert_eq!(
            mesh.vertex_count() - v0,
            20,
            "孤立露出 5 面 = 20 頂点 (内部点で壁共有なし)"
        );
        assert_eq!(
            mesh.live_face_count(),
            f0 + 5 - 1,
            "5 面追加・地表トップ 1 面除去"
        );
        // (1,8,0) 配置: +Y/-Z/+Z 面は 2 頂点ずつ共有 × 3、+X 面 4 頂点新規
        // → 10 頂点 (block1 +X 面は移除されるが頂点は他面と共有され残る可能性あり —
        //    vertex_count は append 安定なので増分のみ厳密に見る)
        m.set(5, 8, 4, true);
        mesh.apply_edit(&get, &opaque, O, 5, 8, 4);
        assert_eq!(mesh.vertex_count() - v0, 30, "共有 slot 再利用で +10");
        assert_eq!(
            mesh.live_face_count(),
            f0 + 4 + 4 - 2,
            "4+4 面 (+X 接触面は双方移除・地表トップ 2 除去)"
        );
        assert_eq!(
            mesh.dead_tri_count(),
            6,
            "地表トップ 2 面 + block1 +X 面で計 6 三角形の死"
        );
    }

    /// HL-215-5: セクション外編集は外基底面を絶対に生成しない (境界分離)。
    #[test]
    fn edit_outside_section_never_inserts_outside_based_face() {
        let m = Mini(std::cell::RefCell::new(HashSet::new()));
        let get = |x, y, z| m.get(x, y, z);
        let mut mesh = DiffSectionMesh::build_full(&get, &opaque, O);
        m.set(16, 5, 5, true); // x=16 はセクション外
        mesh.apply_edit(&get, &opaque, O, 16, 5, 5);
        assert_eq!(
            mesh.live_face_count(),
            0,
            "外ブロック単体では面を持たない (内側露出のみのはず)"
        );
        assert!(!mesh.has_face(16, 5, 5, 4), "外基底 +X 面は絶対に入らない");
        // 内側ブロック (15,5,5) を置くと +X 面が外ブロックに覆われる →
        // セクション内基底のみ面化 (露出は -X/+Y/-Y/+Z/-Z)
        m.set(15, 5, 5, true);
        mesh.apply_edit(&get, &opaque, O, 15, 5, 5);
        assert!(
            !mesh.has_face(15, 5, 5, 4),
            "+X は外ブロックが不透明なため生成されない"
        );
        assert!(mesh.has_face(15, 5, 5, 0), "+Y は露出");
    }

    /// HL-215-6: reoptimize の頂点圧縮 — 純粋に孤立したブロックを殺した後、
    /// 32 頂点が参照されなくなれば next_vert は該当分だけ縮む (dedup 有の
    /// 厳密カウントはケース依存のため上下限で固定)。
    #[test]
    fn reopt_compacts_and_preserves_face_set() {
        let m = Mini(std::cell::RefCell::new(HashSet::new()));
        for y in 0..8i64 {
            for z in 0..16i64 {
                for x in 0..16i64 {
                    m.set(x, y, z, true);
                }
            }
        }
        m.set(4, 8, 4, true);
        let get = |x, y, z| m.get(x, y, z);
        let mut mesh = DiffSectionMesh::build_full(&get, &opaque, O);
        let full_v = mesh.vertex_count();
        m.set(4, 8, 4, false);
        mesh.apply_edit(&get, &opaque, O, 4, 8, 4);
        let mut vco = crate::vertex_cache_opt::VertexCacheOptimizer::new(16);
        let after = mesh.reoptimize(&mut vco);
        assert!(after <= full_v, "圧縮後は初期以下");
        assert!(
            mesh.vertex_count() >= full_v - 20,
            "孤立ブロック除去で減るのは最大 20 頂点 (その露出面由来)"
        );
        let mut zero_area = 0u32;
        for tri in mesh.indices().chunks(3) {
            if tri[0] == tri[1] && tri[1] == tri[2] {
                zero_area += 1;
            }
        }
        assert_eq!(zero_area, 0, "reopt 後に零面積は残存しない");
        // slot_tab が新世代で整合: 同位置再配置でも dedup が機能
        m.set(4, 8, 4, true);
        let ev0 = mesh.verts_encoded;
        mesh.apply_edit(&get, &opaque, O, 4, 8, 4);
        let delta = mesh.verts_encoded - ev0;
        assert!(
            delta <= 20 && delta >= 10,
            "再配置の encode 数は dedup 範囲内 (10..=20), got {delta}"
        );
    }
}

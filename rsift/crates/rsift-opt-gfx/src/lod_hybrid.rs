//! 3-tier LOD hybrid — near mesh / mid impostor / far heightmap (flat arrays, SVO far-only).

use crate::chunk_mesh::BuiltChunkMesh;
use tracing::trace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LodTier {
    /// Full greedy mesh (near).
    Near,
    /// Simplified voxel impostor quads (mid).
    Mid,
    /// 2D heightmap column strip (far / beyond detail).
    Far,
}

pub struct LodHybridSelector {
    pub enabled: bool,
    pub flat_palette_priority: bool,
    pub svo_far_only: bool,
    pub near_blocks: f32,
    pub mid_blocks: f32,
}

impl LodHybridSelector {
    pub fn new(enabled: bool, flat_palette: bool, svo_far_only: bool) -> Self {
        Self {
            enabled,
            flat_palette_priority: flat_palette,
            svo_far_only: svo_far_only,
            near_blocks: 48.0,
            mid_blocks: 96.0,
        }
    }

    /// Tier 選択 (境界は `<=` で近い側: [0,near]=Near、(near,mid]=Mid、
    /// (mid,∞)=Far)。
    ///
    /// **契約 (wave 71 BU-1)**: dist_blocks は NaN 禁止。旧実装は NaN が
    /// 全ての `<=` 比較を false にし、静寂に **Far (最低詳細)** を返していた
    /// — 距離不明を「無限遠扱い」で誤描画サイレント化するハザード。
    /// NaN=観測欠測は拒否 の哲学通り入口で fail-loud に遮断する。
    /// ±∞ は比較の責務として受理 (+∞→Far、-∞→Near は素朴な比較結果)。
    pub fn tier_for_distance(&self, dist_blocks: f32) -> LodTier {
        assert!(
            !dist_blocks.is_nan(),
            "lod_hybrid 契約違反: dist_blocks=NaN (静寂な最低詳細化を遮断)"
        );
        if !self.enabled {
            return LodTier::Near;
        }
        if dist_blocks <= self.near_blocks {
            LodTier::Near
        } else if dist_blocks <= self.mid_blocks {
            LodTier::Mid
        } else {
            LodTier::Far
        }
    }

    /// Downgrade mesh complexity for mid/far (geometry thinning, not resolution).
    pub fn simplify_mesh(&self, mut mesh: BuiltChunkMesh, tier: LodTier) -> BuiltChunkMesh {
        if tier == LodTier::Near || mesh.is_empty() {
            return mesh;
        }
        let stride = match tier {
            LodTier::Mid => 2,
            LodTier::Far => 4,
            _ => 1,
        };
        let mut new_verts = Vec::new();
        let mut new_idx = Vec::new();
        for (qi, chunk) in mesh.vertices.chunks(4).enumerate() {
            if qi % stride != 0 {
                continue;
            }
            if chunk.len() < 4 {
                continue;
            }
            let base = new_verts.len() as u32;
            new_verts.extend_from_slice(chunk);
            new_idx.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
        }
        mesh.vertices = new_verts;
        mesh.indices = new_idx;
        // wave 59 BI-A: is_empty フィールドは廃止 (BuiltChunkMesh::is_empty() が
        // vertices から常時導出) — この行は LOD 簡略化後の手動再同期ハザードだった。
        trace!(
            "[LodHybrid] {:?} chunk ({}, {}) → {} verts",
            tier,
            mesh.chunk_x,
            mesh.chunk_z,
            mesh.vertices.len()
        );
        mesh
    }

    /// Far chunks: SVO + branchless DDA instead of greedy mesh (polygon-zero path).
    ///
    /// **真値表 (wave 71 BU-3 確定 — 命名注意)**:
    /// | enabled | svo_far_only | Far  | Mid      | Near |
    /// |---------|--------------|------|----------|------|
    /// | false   | false        | ×    | ×        | ×    |
    /// | false   | true         | ○    | ○        | ×    |
    /// | true    | false        | ○    | ×        | ×    |
    /// | true    | true         | ○    | ○        | ×    |
    /// フィールド名 `svo_far_only` は「SVO を Far に限定する」に見えるが、
    /// 真の意味は **「SVO を Mid にも拡張する」スイッチ** (false でも Far は
    /// SVO 経路)。設定側 `FeatherRenderConfig` の doc「SVO/DAG only beyond
    /// render distance (not near terrain)」と合わせ、Near → SVO は全組合せで
    /// 不成立。enabled=false 呼出経路では tier は常に Near (tier_for_distance
    /// 参照) のため flag_only 行は防御的経路 (到達不能ではないが無害)。
    /// 公開フィールドのリネームは 2 クレート跨ぎ (API 契約) のため行わず、
    /// 真値表を契約として機械固定する。
    pub fn use_svo_encoding(&self, tier: LodTier) -> bool {
        if !self.svo_far_only && !self.enabled {
            return false;
        }
        tier == LodTier::Far || (self.svo_far_only && tier == LodTier::Mid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk_mesh::Quantized12ByteVertex;

    fn mesh_with_quads(n_quads: usize) -> BuiltChunkMesh {
        let mut vertices = Vec::with_capacity(n_quads * 4);
        let mut indices = Vec::with_capacity(n_quads * 6);
        for q in 0..n_quads {
            let base = vertices.len() as u32;
            for k in 0..4 {
                vertices.push(Quantized12ByteVertex::encode(
                    q as f32, k as f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0,
                ));
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
        }
        BuiltChunkMesh {
            chunk_x: 0,
            chunk_z: 0,
            vertices,
            indices,
        }
    }

    #[test]
    fn tier_thresholds_and_disabled() {
        let off = LodHybridSelector::new(false, false, false);
        assert_eq!(off.tier_for_distance(1e9), LodTier::Near);
        let on = LodHybridSelector::new(true, false, false);
        assert_eq!(on.tier_for_distance(0.0), LodTier::Near);
        assert_eq!(on.tier_for_distance(48.0), LodTier::Near); // 境界は <= で Near
        assert_eq!(on.tier_for_distance(48.1), LodTier::Mid);
        assert_eq!(on.tier_for_distance(96.0), LodTier::Mid); // <= で Mid
        assert_eq!(on.tier_for_distance(96.1), LodTier::Far);
    }

    #[test]
    fn simplify_near_passthrough() {
        let sel = LodHybridSelector::new(true, false, false);
        let m = mesh_with_quads(2);
        let (vb, ib) = (m.vertices.len(), m.indices.len());
        let m = sel.simplify_mesh(m, LodTier::Near);
        assert_eq!((m.vertices.len(), m.indices.len()), (vb, ib));
    }

    #[test]
    fn simplify_mid_keeps_even_quads_reindexing() {
        let sel = LodHybridSelector::new(true, false, false);
        // 3 クアッド (12 vert): Mid stride 2 → quad 0, 2 を保持して再 index。
        let m = sel.simplify_mesh(mesh_with_quads(3), LodTier::Mid);
        assert_eq!(m.vertices.len(), 8);
        assert_eq!(m.indices, vec![0, 1, 2, 2, 3, 0, 4, 5, 6, 6, 7, 4]);
        assert!(!m.is_empty());
    }

    #[test]
    fn simplify_far_stride_and_partial_quad_drop() {
        let sel = LodHybridSelector::new(true, false, false);
        // 8 クアッド: Far stride 4 → quad 0, 4 のみ保持。
        let m = sel.simplify_mesh(mesh_with_quads(8), LodTier::Far);
        assert_eq!(m.vertices.len(), 8);
        // 完全クアッド無し (3 vert) → 空メッシュへ降格。
        let mut m3 = mesh_with_quads(1);
        m3.vertices.truncate(3);
        let m3 = sel.simplify_mesh(m3, LodTier::Mid);
        assert!(m3.is_empty());
    }

    #[test]
    fn svo_encoding_truth_table() {
        let off = LodHybridSelector::new(false, false, false);
        assert!(!off.use_svo_encoding(LodTier::Far));
        let on = LodHybridSelector::new(true, false, false);
        assert!(on.use_svo_encoding(LodTier::Far));
        assert!(!on.use_svo_encoding(LodTier::Near));
        assert!(!on.use_svo_encoding(LodTier::Mid)); // svo_far_only=false → Mid 対象外
        let svo = LodHybridSelector::new(true, false, true);
        assert!(svo.use_svo_encoding(LodTier::Mid)); // svo_far_only → Mid も符号化
                                                     // svo_far_only 単独 (enabled=false) でも Far 符号化は有効 (実装規約)。
        let flag_only = LodHybridSelector::new(false, false, true);
        assert!(flag_only.use_svo_encoding(LodTier::Far));
    }

    /// wave 71 BU-3: use_svo_encoding の全組合せ真値表 (12 エントリ) を
    /// 機械固定 — 命名 (「far に限定する」) と実意味 (「Mid にも拡張する」)
    /// の乖離を含め、仕様を後付けでも**決定的に固定**する。
    #[test]
    fn svo_encoding_full_truth_table() {
        #[rustfmt::skip]
        let want: [((bool, bool), [bool; 3]); 4] = [
            // (enabled, svo_far_only) → [Far, Mid, Near]
            ((false, false), [false, false, false]),
            ((false, true),  [true,  true,  false]),
            ((true,  false), [true,  false, false]),
            ((true,  true),  [true,  true,  false]),
        ];
        for ((en, svo), w) in want {
            let sel = LodHybridSelector::new(en, false, svo);
            for (tier, expect) in [
                (LodTier::Far, w[0]),
                (LodTier::Mid, w[1]),
                (LodTier::Near, w[2]),
            ] {
                assert_eq!(
                    sel.use_svo_encoding(tier),
                    expect,
                    "truth table ({en},{svo}) × {tier:?}"
                );
            }
        }
    }

    /// wave 71 BU-1: NaN 距離は静寂な最低詳細化 (全 <= 比較が false → Far)
    /// を入口 assert で遮断。±∞ は比較結果のまま受理 (+∞→Far、-∞→Near)。
    #[test]
    #[should_panic(expected = "lod_hybrid 契約違反")]
    fn nan_distance_is_rejected() {
        let sel = LodHybridSelector::new(true, false, false);
        let _ = sel.tier_for_distance(f32::NAN);
    }

    #[test]
    fn infinite_distance_uses_comparison_result() {
        let sel = LodHybridSelector::new(true, false, false);
        assert_eq!(sel.tier_for_distance(f32::INFINITY), LodTier::Far);
        assert_eq!(sel.tier_for_distance(f32::NEG_INFINITY), LodTier::Near);
    }

    /// wave 71 BU-4: 空メッシュの簡略化は早期 passthrough (頂点無しに
    /// 再 index を走らせない契約)。
    #[test]
    fn simplify_empty_mesh_passthrough() {
        let sel = LodHybridSelector::new(true, false, false);
        let m = BuiltChunkMesh {
            chunk_x: 1,
            chunk_z: 2,
            vertices: vec![],
            indices: vec![],
        };
        let m = sel.simplify_mesh(m, LodTier::Far);
        assert!(m.is_empty());
        assert_eq!((m.chunk_x, m.chunk_z), (1, 2), "identity fields preserved");
    }
}

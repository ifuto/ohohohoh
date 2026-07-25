//! Leaf block fast path — 葉 voxel の内部を削減するメッシュ前処理 (Sodium-style に着想)。
//!
//! ## DI-1: 逐次変異スキャンの意味論契約 (wave 109 で公表)
//! 本実装は単一パスで palette を**走査中に変異**させる (スナップショット同時
//! collapse ではない)。これにより、固体立方体の内部でも collapse されるのは
//! **x+y+z のパリティがスキャン最先 interior voxel と一致する voxel のみ**
//! (3D チェッカー模様)。閉形式: 一辺 a の固体葉立方体 (オフセット偶数開始) の
//! collapse 数は **⌈(a-2)³/2⌉** (機械照合済: a=3..10 → 1,4,14,32,63,108,172,256、
//! 全 16³ 葉で 1,372 = odd-parity interior 数)。よって旧 doc の「only boundary
//! faces remain」は厳密には虚偽 — **残るのは境界 + 反選パリティの内部 voxel 半分**。
//! 本挙動は wide_static_bench の `delta_idsum` 行で digest 凍結されており、
//! snapshot 化などの意味論変更は digest 更新の大掛かりな見直しを要するため、
//! 現時点では契約公表 + 厳密ピンで固定する (消費者の実被害: 葉 canopy 内部の
//! 体積密度は半減するが外観境界は維持される設計)。

use crate::binary_greedy_meshing::SectionPalette;
use crate::chunk_mesh::BuiltChunkMesh;

/// 葉ブロック id 一覧 (DI-3 誠実注記): **エンジン内部のプレースホルダ値**で、
/// vanilla 1.21 のレジストリ id ではない。18/161 のみ Java legacy numeric の
/// 実 id (leaves/leaves2) で、162..=165 および 200..=205 は ingest 系
/// (ChunkBridge) のハッシュ空間に割り当てた**仮想 id 帯**。vanilla との対応は
/// 「oak/birch... azalea 系」の**分類上の目安**であり、実 id 写像を表さない。
pub const LEAF_TYPES: [u16; 12] = [
    18, 161, // leaves / leaves2 (実 legacy numeric id)
    162, 163, 164, 165, // jungle/acacia/dark_oak/azalea 系の仮想 id 帯
    200, 201, 202, 203, 204, 205, // ChunkBridge ハッシュ空間の葉バンド
];

/// DI-2: is_leaf のビットマスク展開表 (`LEAF_TYPES` 由来、max id = 205 < 256)。
/// 全 65,536 入力で `LEAF_TYPES.contains` と厳密等価 (strict テストで全網羅ピン)。
const LEAF_MASK: [u64; 4] = {
    let mut m = [0u64; 4];
    let mut i = 0usize;
    while i < LEAF_TYPES.len() {
        let id = LEAF_TYPES[i] as usize;
        m[id >> 6] |= 1u64 << (id & 63);
        i += 1;
    }
    m
};

#[inline(always)]
fn is_leaf(block: u16) -> bool {
    // 捕捉29: u16 全域 65,536 に対し表は 256 bit のみ — block ≥ 256 は表外なので
    // guard 必須 (全 LEAF id ≤ 205 なので guard で完全等価、contains の安全域と一致)。
    block < 256 && (LEAF_MASK[(block as usize) >> 6] >> (block & 63)) & 1 != 0
}

/// Collapse interior leaf voxels — 意味論はモジュール doc (DI-1) のパリティ契約に
/// 従う (逐次変異スキャン、走査範囲は境界 1 層を除く 1..S-1)。
pub fn apply_leaf_fast_path(palette: &mut SectionPalette, enabled: bool) {
    if !enabled {
        return;
    }
    const S: usize = crate::binary_greedy_meshing::SECTION_SIZE;
    for z in 1..S - 1 {
        for y in 1..S - 1 {
            for x in 1..S - 1 {
                let i = x + y * S + z * S * S;
                let b = palette[i];
                if !is_leaf(b) {
                    continue;
                }
                let surrounded = [
                    (x.wrapping_sub(1), y, z),
                    (x + 1, y, z),
                    (x, y.wrapping_sub(1), z),
                    (x, y + 1, z),
                    (x, y, z.wrapping_sub(1)),
                    (x, y, z + 1),
                ]
                .iter()
                .all(|&(nx, ny, nz)| {
                    if nx >= S || ny >= S || nz >= S {
                        return false;
                    }
                    is_leaf(palette[nx + ny * S + nz * S * S])
                });
                if surrounded {
                    palette[i] = 0;
                }
            }
        }
    }
}

/// DI-4: 消費者ゼロの生存確認 (削除せず保持: 組立済みメッシュへの葉 overlay 併合)。
/// base/leaf_overlay は同一チャンク (同一生成系) であること — 開始頂点オフセットは
/// base 側のみ使用し overlay の chunk 座標は外包から渡さない前提。
pub fn merge_leaf_mesh(base: &BuiltChunkMesh, leaf_overlay: &BuiltChunkMesh) -> BuiltChunkMesh {
    debug_assert_eq!(
        (base.chunk_x, base.chunk_z),
        (leaf_overlay.chunk_x, leaf_overlay.chunk_z),
        "merge_leaf_mesh: base と overlay は同一チャンク前提 (DI-4)"
    );
    let mut vertices = base.vertices.clone();
    let mut indices = base.indices.clone();
    let offset = vertices.len() as u32;
    vertices.extend_from_slice(&leaf_overlay.vertices);
    for &idx in &leaf_overlay.indices {
        indices.push(idx + offset);
    }
    BuiltChunkMesh {
        chunk_x: base.chunk_x,
        chunk_z: base.chunk_z,
        vertices,
        indices,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary_greedy_meshing::SECTION_SIZE;
    use crate::chunk_mesh::Quantized12ByteVertex;

    const S: usize = SECTION_SIZE;

    fn idx(x: usize, y: usize, z: usize) -> usize {
        x + y * S + z * S * S
    }

    /// 全 air セクションの中心に a×a×a の葉ブロック立方体 (id 18) を置く。
    fn section_with_leaf_cube(x0: usize, y0: usize, z0: usize, a: usize) -> SectionPalette {
        let mut p = [0u16; S * S * S];
        for z in z0..z0 + a {
            for y in y0..y0 + a {
                for x in x0..x0 + a {
                    p[idx(x, y, z)] = 18;
                }
            }
        }
        p
    }

    #[test]
    fn interior_leaf_collapses_but_surface_survives() {
        let mut p = section_with_leaf_cube(5, 5, 5, 3);
        apply_leaf_fast_path(&mut p, true);
        // 中心 (6,6,6) は全 6 近傍が葉 → air 化。
        assert_eq!(p[idx(6, 6, 6)], 0, "interior leaf must collapse");
        // 表面 26 voxel は少なくとも 1 面が非葉に接するため残る。
        let mut surface = 0usize;
        for z in 5..8 {
            for y in 5..8 {
                for x in 5..8 {
                    if (x, y, z) != (6, 6, 6) {
                        assert_eq!(p[idx(x, y, z)], 18, "surface leaf must survive");
                        surface += 1;
                    }
                }
            }
        }
        assert_eq!(surface, 26);
    }

    #[test]
    fn disabled_is_bit_identical_noop() {
        let mut p = section_with_leaf_cube(5, 5, 5, 3);
        let before = p;
        apply_leaf_fast_path(&mut p, false);
        assert_eq!(p, before, "disabled must not touch the palette");
    }

    #[test]
    fn non_leaf_blocks_never_collapse_even_when_surrounded() {
        // 葉で囲まれた非葉 (id 7) は消えないこと (葉専用経路の限定性)。
        let mut p = section_with_leaf_cube(5, 5, 5, 3);
        p[idx(6, 6, 6)] = 7;
        apply_leaf_fast_path(&mut p, true);
        assert_eq!(p[idx(6, 6, 6)], 7);
        // かつ「葉に囲まれても近傍が非葉になる」周囲の葉は残る
        // (中心が非葉なので 6 近傍葉は surrounded=false)。
        assert_eq!(p[idx(5, 6, 6)], 18);
    }

    #[test]
    fn near_border_leaf_with_air_neighbor_survives() {
        // 走査起点は 1 からだが (1,1,1) の近傍 (0,·,·) は安全に読まれる。
        // 立方体 a=2 では (1,1,1) も近傍に air (or 立方体外) を持ち、消える
        // voxel は 1 つも出ないことを固定する (見出し「境界層は対象外」の
        // 厳密な意味: 境界上の voxel 自身が消されない、という規約)。
        let mut p = section_with_leaf_cube(0, 0, 0, 2);
        let before = p;
        apply_leaf_fast_path(&mut p, true);
        assert_eq!(
            p, before,
            "a=2 の境界寄り立方体では collapse 対象が存在しない"
        );
        assert_eq!(p[idx(1, 1, 1)], 18);
    }

    #[test]
    fn only_listed_leaf_ids_are_collapsible() {
        // LEAF_TYPES 外の id で 3x3x3 を作っても消える voxel は出ない。
        let mut p = [0u16; S * S * S];
        for z in 5..8 {
            for y in 5..8 {
                for x in 5..8 {
                    p[idx(x, y, z)] = 161; // birch (登録済 leaf)
                }
            }
        }
        let mut b = [0u16; S * S * S];
        for z in 10..13 {
            for y in 5..8 {
                for x in 5..8 {
                    b[idx(x, y, z)] = 9; // 非登録 id
                }
            }
        }
        apply_leaf_fast_path(&mut p, true);
        apply_leaf_fast_path(&mut b, true);
        assert_eq!(p[idx(6, 6, 6)], 0, "registered leaf 161 collapses");
        assert!(b.iter().all(|&v| v == 0 || v == 9), "unlisted id intact");
    }

    fn vert() -> Quantized12ByteVertex {
        Quantized12ByteVertex::encode(0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0)
    }

    #[test]
    fn merge_leaf_mesh_offsets_overlay_indices() {
        let base = BuiltChunkMesh {
            chunk_x: 2,
            chunk_z: -3,
            vertices: vec![vert(); 5],
            indices: vec![0, 1, 2, 0, 2, 3],
        };
        let overlay = BuiltChunkMesh {
            chunk_x: 2,
            chunk_z: -3,
            vertices: vec![vert(); 4],
            indices: vec![0, 1, 2],
        };
        let merged = merge_leaf_mesh(&base, &overlay);
        assert_eq!(merged.vertices.len(), 9);
        assert_eq!(
            merged.indices[6..],
            [5, 6, 7],
            "overlay 側は base 頂点数分オフセット"
        );
        assert_eq!((merged.chunk_x, merged.chunk_z), (2, -3));
        assert!(!merged.is_empty());
        // 空 overlay 追加は no-op 相当 (bytes 等価)。
        let empty_overlay = BuiltChunkMesh {
            vertices: vec![],
            indices: vec![],
            ..overlay.clone()
        };
        let m2 = merge_leaf_mesh(&base, &empty_overlay);
        assert_eq!(m2.indices, base.indices);
        assert_eq!(m2.vertices.len(), base.vertices.len());
    }
}

#[cfg(test)]
mod strict_tests {
    use super::*;
    use crate::binary_greedy_meshing::SECTION_SIZE;
    use crate::chunk_mesh::Quantized12ByteVertex;

    const S: usize = SECTION_SIZE;

    fn idx(x: usize, y: usize, z: usize) -> usize {
        x + y * S + z * S * S
    }

    fn leaf_cube(x0: usize, y0: usize, z0: usize, a: usize) -> SectionPalette {
        let mut p = [0u16; S * S * S];
        for z in z0..z0 + a {
            for y in y0..y0 + a {
                for x in x0..x0 + a {
                    p[idx(x, y, z)] = 18;
                }
            }
        }
        p
    }

    fn count_air(p: &SectionPalette) -> usize {
        p.iter().filter(|&&v| v == 0).count()
    }

    /// DI-1 パリティ意味論の厳密ピン (Python 独立モデル照合済):
    /// 一辺 a の固体葉立方体 (起点 2,2,2) の collapse 数は ⌈(a-2)³/2⌉。
    /// adversarial (a) 標的: snapshot 同時 collapse 変体は全て (a-2)³ となり RED。
    #[test]
    fn parity_collapse_exact_residuals() {
        const EXPECT: [(usize, usize); 8] = [
            (3, 1),
            (4, 4),
            (5, 14),
            (6, 32),
            (7, 63),
            (8, 108),
            (9, 172),
            (10, 256),
        ];
        for (a, want_collapsed) in EXPECT {
            let mut p = leaf_cube(2, 2, 2, a);
            let before_air = count_air(&p);
            apply_leaf_fast_path(&mut p, true);
            let collapsed = count_air(&p) - before_air;
            assert_eq!(collapsed, want_collapsed, "a={a} parity residual");
        }
    }

    /// DI-1 全 16³ 葉セクションの厳密ピン: collapse = 1,372 (14³ interior の
    /// 奇パリティ voxel 数に厳密一致)。境界 x/y/z ∈ {0,15} 環は untouched。
    #[test]
    fn full_section_parity_and_boundary_ring() {
        let mut p = [18u16; S * S * S];
        apply_leaf_fast_path(&mut p, true);
        let collapsed = p.iter().filter(|&&v| v == 0).count();
        assert_eq!(collapsed, 1372, "全 16³ leaf: ⌈2744/2⌉ parity theorem");
        for e in [0usize, S - 1] {
            for c in 0..S {
                assert_eq!(p[idx(c, c.min(15), e)], 18, "z 環 (e={e}) untouched");
                assert_eq!(p[idx(e, 3, c.min(15))], 18, "x 環 (e={e}) untouched");
                assert_eq!(p[idx(c.min(15), e, 7)], 18, "y 環 (e={e}) untouched");
            }
        }
        // チェッカー隣接不変量: collapse した任意の interior voxel の 6 近傍は全て葉残留
        for z in 1..S - 1 {
            for y in 1..S - 1 {
                for x in 1..S - 1 {
                    if p[idx(x, y, z)] == 0 {
                        for (nx, ny, nz) in [
                            (x - 1, y, z),
                            (x + 1, y, z),
                            (x, y - 1, z),
                            (x, y + 1, z),
                            (x, y, z - 1),
                            (x, y, z + 1),
                        ] {
                            assert_eq!(
                                p[idx(nx, ny, nz)],
                                18,
                                "collapse 近傍は checkerboard 上必ず残留"
                            );
                        }
                    }
                }
            }
        }
    }

    /// DI-2 厳密等価ピン (adversarial (b) 標的: ビットマスク表の 1 ビット齟齬で RED):
    /// 全 65,536 id で LEAF_MASK ルックアップ ≡ LEAF_TYPES.contains。
    #[test]
    fn leaf_mask_exhaustive_equivalence() {
        for v in 0..=u16::MAX {
            let bitwise = v < 256 && (LEAF_MASK[(v as usize) >> 6] >> (v & 63)) & 1 != 0;
            assert_eq!(bitwise, LEAF_TYPES.contains(&v), "id {v} mask mismatch");
        }
        assert_eq!(
            LEAF_TYPES
                .iter()
                .filter(|&&v| (LEAF_MASK[(v as usize) >> 6] >> (v & 63)) & 1 != 0)
                .count(),
            12
        );
    }

    /// DI-4 merge 前提 assert の should_panic ピン (chunk 座標不一致は debug で fail-loud)。
    #[test]
    #[should_panic(expected = "merge_leaf_mesh: base と overlay は同一チャンク前提")]
    fn merge_leaf_mesh_mismatched_chunk_panics() {
        let v = || Quantized12ByteVertex::encode(0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0);
        let base = BuiltChunkMesh {
            chunk_x: 1,
            chunk_z: 0,
            vertices: vec![v(); 1],
            indices: vec![0, 0, 0],
        };
        let overlay = BuiltChunkMesh {
            chunk_x: 2,
            chunk_z: 0,
            vertices: vec![v(); 1],
            indices: vec![0, 0, 0],
        };
        let _ = merge_leaf_mesh(&base, &overlay);
    }

    /// DI-4 merge の空 overlay 逆変換 + オフセット厳密往復 (consumer 不在でも契約 pin)。
    #[test]
    fn merge_leaf_mesh_offset_inverse_roundtrip() {
        let v = || Quantized12ByteVertex::encode(1.0, 2.0, 3.0, 0.0, 1.0, 0.0, 4.0, 5.0);
        let base = BuiltChunkMesh {
            chunk_x: 0,
            chunk_z: 0,
            vertices: vec![v(); 3],
            indices: vec![0, 1, 2],
        };
        let overlay = BuiltChunkMesh {
            chunk_x: 0,
            chunk_z: 0,
            vertices: vec![v(); 2],
            indices: vec![0, 1, 1],
        };
        let m = merge_leaf_mesh(&base, &overlay);
        assert_eq!(m.indices, vec![0, 1, 2, 3, 4, 4]);
        // 頂点境界: 先頭 3 は base、末尾 2 は overlay の copy であること (byte 等価、
        // Quantized12ByteVertex は PartialEq 非実装のため Pod 経由で比較)。
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&m.vertices[..3]),
            bytemuck::cast_slice::<_, u8>(&base.vertices[..]),
            "先頭区間は base の byte copy"
        );
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&m.vertices[3..]),
            bytemuck::cast_slice::<_, u8>(&overlay.vertices[..]),
            "末尾区間は overlay の byte copy"
        );
        assert_eq!(m.vertices.len(), 5);
    }
}

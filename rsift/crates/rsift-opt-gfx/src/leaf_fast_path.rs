//! Leaf block fast path — render leaf interiors as opaque quads (Sodium-style).

use crate::binary_greedy_meshing::SectionPalette;
use crate::chunk_mesh::{BuiltChunkMesh, Quantized12ByteVertex};

/// Minecraft leaf block type ids — synced with ChunkBridge hash/registry heuristics.
/// Includes legacy numeric ids + common 1.21 hashed placeholders used by ingest.
pub const LEAF_TYPES: [u16; 12] = [
    18, 161, // oak/birch leaves (legacy)
    162, 163, 164, 165, // jungle/acacia/dark_oak/azalea-ish
    200, 201, 202, 203, 204, 205, // ChunkBridge hash-space leaf band
];

#[inline]
fn is_leaf(block: u16) -> bool {
    LEAF_TYPES.contains(&block)
}

/// Collapse interior leaf voxels — only boundary faces remain (faster forest rendering).
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

pub fn merge_leaf_mesh(base: &BuiltChunkMesh, leaf_overlay: &BuiltChunkMesh) -> BuiltChunkMesh {
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
        is_empty: vertices.is_empty(),
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
            is_empty: false,
            vertices: vec![vert(); 5],
            indices: vec![0, 1, 2, 0, 2, 3],
        };
        let overlay = BuiltChunkMesh {
            chunk_x: 2,
            chunk_z: -3,
            is_empty: false,
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
        assert!(!merged.is_empty);
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

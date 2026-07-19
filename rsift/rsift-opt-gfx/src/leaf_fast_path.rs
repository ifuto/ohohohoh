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

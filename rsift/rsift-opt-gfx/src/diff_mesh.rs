//! Section-level differential mesh updates (Tier 2).
//! Block edits dirty 16³ sections only — never rebuild the whole column.

use std::collections::HashMap;

pub const SECTION_SIZE: i32 = 16;
pub const SECTIONS_Y: i32 = 24; // -64..320 → 24 sections

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
}

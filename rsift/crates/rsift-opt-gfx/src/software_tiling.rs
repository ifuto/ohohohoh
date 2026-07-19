//! Software tile binning — TBDR-inspired draw reorder (32×32 / 64×64 tiles).

use std::collections::HashMap;
use tracing::trace;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileId {
    pub tx: u32,
    pub ty: u32,
}

#[derive(Debug, Clone)]
pub struct TileDrawList {
    pub tile: TileId,
    pub chunk_indices: Vec<usize>,
}

/// Bin chunk AABBs into screen tiles for front-to-back / overdraw locality.
pub struct SoftwareTileBinner {
    pub tile_size: u32,
    pub screen_w: u32,
    pub screen_h: u32,
}

impl SoftwareTileBinner {
    pub fn new(screen_w: u32, screen_h: u32, tile_size: u32) -> Self {
        Self {
            tile_size: tile_size.max(16),
            screen_w,
            screen_h,
        }
    }

    pub fn tile_count(&self) -> (u32, u32) {
        let tw = (self.screen_w + self.tile_size - 1) / self.tile_size;
        let th = (self.screen_h + self.tile_size - 1) / self.tile_size;
        (tw.max(1), th.max(1))
    }

    /// Project chunk (cx, cz) to NDC-ish screen coords and assign tiles.
    pub fn bin_chunks(
        &self,
        chunks: &[(i32, i32)],
        camera_chunk_x: f32,
        camera_chunk_z: f32,
    ) -> Vec<TileDrawList> {
        let (tw, th) = self.tile_count();
        let mut map: HashMap<TileId, Vec<usize>> = HashMap::new();

        for (i, &(cx, cz)) in chunks.iter().enumerate() {
            let dx = cx as f32 - camera_chunk_x;
            let dz = cz as f32 - camera_chunk_z;
            let ndc_x = (dx * 0.05).clamp(-1.0, 1.0);
            let ndc_y = (dz * 0.05).clamp(-1.0, 1.0);
            let px = ((ndc_x * 0.5 + 0.5) * self.screen_w as f32) as u32;
            let py = ((ndc_y * 0.5 + 0.5) * self.screen_h as f32) as u32;
            let tx = (px / self.tile_size).min(tw - 1);
            let ty = (py / self.tile_size).min(th - 1);
            map.entry(TileId { tx, ty }).or_default().push(i);
        }

        let mut lists: Vec<TileDrawList> = map
            .into_iter()
            .map(|(tile, mut chunk_indices)| {
                chunk_indices.sort_by_key(|&i| {
                    let (cx, cz) = chunks[i];
                    let d = (cx as f32 - camera_chunk_x).hypot(cz as f32 - camera_chunk_z);
                    (d * 1000.0) as u32
                });
                TileDrawList { tile, chunk_indices }
            })
            .collect();
        lists.sort_by_key(|l| (l.tile.ty, l.tile.tx));
        trace!("[TileBin] {} tiles for {} chunks", lists.len(), chunks.len());
        lists
    }
}

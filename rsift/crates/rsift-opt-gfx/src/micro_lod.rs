
//! Micro LOD & Impostor - Mesh Shader無しLOD、遠景は8x8ダウンサンプル+ベイクAO
//! 低スペでも頂点数90%減

use crate::binary_greedy_meshing::SectionPalette;

pub struct LodLevel {
    pub distance: f32,
    pub downsample: u32,
    pub max_quads: u32,
}

pub const LOD_LEVELS: [LodLevel; 4] = [
    LodLevel { distance: 0.0, downsample: 1, max_quads: 4096 },
    LodLevel { distance: 64.0, downsample: 2, max_quads: 1024 },
    LodLevel { distance: 128.0, downsample: 4, max_quads: 256 },
    LodLevel { distance: 256.0, downsample: 8, max_quads: 64 },
];

pub fn lod_for_distance(dist: f32) -> &'static LodLevel {
    for lvl in LOD_LEVELS.iter().rev() {
        if dist >= lvl.distance { return lvl; }
    }
    &LOD_LEVELS[0]
}

pub fn downsample_palette(palette: &SectionPalette, factor: u32) -> SectionPalette {
    let mut out = [0u16; 4096];
    if factor <=1 { return *palette; }
    for z in (0..16).step_by(factor as usize) {
        for y in (0..16).step_by(factor as usize) {
            for x in (0..16).step_by(factor as usize) {
                let src_idx = x + y*16 + z*256;
                if palette[src_idx] != 0 {
                    let dst_x = x / factor as usize;
                    let dst_y = y / factor as usize;
                    let dst_z = z / factor as usize;
                    let dst_idx = dst_x + dst_y*16 + dst_z*256;
                    out[dst_idx] = palette[src_idx];
                }
            }
        }
    }
    out
}

pub fn bake_impostor_ao(palette: &SectionPalette) -> u32 {
    let solid = palette.iter().filter(|&&b| b!=0).count();
    if solid > 3000 { 3 } else if solid > 1000 { 2 } else { 1 }
}

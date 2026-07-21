
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lod_thresholds_and_negative_fallback() {
        assert_eq!(lod_for_distance(0.0).downsample, 1);
        assert_eq!(lod_for_distance(63.0).downsample, 1);
        assert_eq!(lod_for_distance(64.0).downsample, 2); // 境界は >=
        assert_eq!(lod_for_distance(128.0).downsample, 4);
        assert_eq!(lod_for_distance(256.0).downsample, 8);
        assert_eq!(lod_for_distance(1e6).downsample, 8);
        assert_eq!(lod_for_distance(256.0).max_quads, 64);
        // 負距離は levels を抜けて最寄り (level 0) フォールバック。
        assert_eq!(lod_for_distance(-5.0).downsample, 1);
    }

    #[test]
    fn downsample_palette_identity_and_mapping() {
        let mut p = [0u16; 4096];
        p[7] = 1; // 奇数座標 (7,0,0) は factor 2 のサンプル点外
        p[64] = 2; // (0,4,0) → dst (0,2,0) = idx 32
        p[546] = 3; // (2,2,2) → dst (1,1,1) = idx 273
        let id = downsample_palette(&p, 1);
        assert_eq!(&id[..], &p[..]); // factor<=1 はコピー
        let d = downsample_palette(&p, 2);
        assert_eq!(d[7], 0);
        assert_eq!(d[32], 2);
        assert_eq!(d[273], 3);
    }

    #[test]
    fn bake_impostor_ao_thresholds() {
        let count_ao = |solid: usize| {
            let mut p = [0u16; 4096];
            for v in p.iter_mut().take(solid) {
                *v = 1;
            }
            bake_impostor_ao(&p)
        };
        assert_eq!(count_ao(0), 1);
        assert_eq!(count_ao(1000), 1); // 境界: >1000 で 2
        assert_eq!(count_ao(1001), 2);
        assert_eq!(count_ao(3000), 2); // 境界: >3000 で 3
        assert_eq!(count_ao(3001), 3);
        assert_eq!(count_ao(4096), 3);
    }
}

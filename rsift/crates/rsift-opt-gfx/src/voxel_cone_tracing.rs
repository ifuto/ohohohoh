//! # 38. Voxel Cone Tracing (`VoxelConeTracing` - GI Global Illumination)
//!
//! SVO (Sparse Voxel Octree) をコーントレーシングし、環境遮蔽や間接拡散照明 (Diffuse GI)
//! をスクリーンスペースに依存せず計算する。コーンの広がりに応じて SVO のミップ階層 (`LOD`)
//! を自動選択し、低コストでリアルタイムグローバルイルミネーション近似を実現する。

use crate::svo::SparseVoxelOctree;

#[derive(Debug, Clone, Copy)]
pub struct ConeRay {
    pub origin: [f32; 3],
    pub dir: [f32; 3],
    pub aperture: f32, // half-angle tangent e.g. 0.577 for 60 degrees
    pub max_dist: f32,
}

/// コーン直径 → SVO LOD レベル (floor log2、0..=10 clamp)。
///
/// **ビット抽出による厳密 floor(log2(d))** (2^k ≤ d < 2^(k+1) の bucket)。
/// 以前の `diameter.log2().clamp(0.0, 10.0) as u32` は、2 冪の 1ulp 下の値で
/// log2f の最近接丸めが丁度整数になる (= レベルが 1 跳ぶ) 丸めアーティファクト
/// を持ち、GPU (実装定義精度の log2) との一致が原理的に保てなかった。
/// この厳密版は `voxel_cone_tracing.wgsl::lod_from_diameter` と同一規則で、
/// GPU/CPU の LOD 選択が **構造的に bitwise 一致** する (Phase D で導入)。
/// 挙動差は境界 1ulp のみで、旧値は bucket 意味論の誤差側だった。
pub fn lod_from_diameter(d: f32) -> u32 {
    // partial_cmp 版: NaN も「1.0 以下扱い」で lod 0 (WGSL の !(d > 1.0) と完全一致)
    if !matches!(d.partial_cmp(&1.0), Some(std::cmp::Ordering::Greater)) {
        return 0;
    }
    let e = ((d.to_bits() >> 23) as i32) - 127;
    if e <= 0 {
        return 0;
    }
    (e as u32).min(10)
}

pub struct VoxelConeTracing;

impl VoxelConeTracing {
    /// Trace a diffuse cone through the SVO to accumulate indirect light and occlusion.
    pub fn trace_diffuse_cone(svo: &SparseVoxelOctree, cone: &ConeRay) -> ([f32; 3], f32) {
        let mut accum_color = [0.0f32; 3];
        let mut accum_alpha = 0.0f32;
        let mut dist = 0.5f32; // initial offset step beyond wall

        while dist < cone.max_dist && accum_alpha < 0.99 {
            let pos = [
                cone.origin[0] + cone.dir[0] * dist,
                cone.origin[1] + cone.dir[1] * dist,
                cone.origin[2] + cone.dir[2] * dist,
            ];

            let diameter = (2.0 * cone.aperture * dist).max(1.0);
            let lod_level = lod_from_diameter(diameter);

            if let Some((color_rgb, alpha)) = svo.sample_lod(pos[0], pos[1], pos[2], lod_level) {
                if alpha > 0.001 {
                    let weight = alpha * (1.0 - accum_alpha);
                    accum_color[0] += color_rgb[0] * weight;
                    accum_color[1] += color_rgb[1] * weight;
                    accum_color[2] += color_rgb[2] * weight;
                    accum_alpha += weight;
                }
            }

            dist += diameter * 0.75; // step forward proportionally to cone width
        }

        (accum_color, accum_alpha.clamp(0.0, 1.0))
    }
}

/// 空の SVO ではコーンは何にも当たらず、alpha は 0 のまま規格内を保つ。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_voxel_cone_tracing() {
        let svo = SparseVoxelOctree::empty();
        let cone = ConeRay {
            origin: [0.0, 64.0, 0.0],
            dir: [0.0, 1.0, 0.0],
            aperture: 0.5,
            max_dist: 50.0,
        };
        let (color, alpha) = VoxelConeTracing::trace_diffuse_cone(&svo, &cone);
        assert!(alpha >= 0.0 && alpha <= 1.0);
        let _ = color;
    }
}

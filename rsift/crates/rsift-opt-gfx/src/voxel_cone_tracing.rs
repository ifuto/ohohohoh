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

impl ConeRay {
    /// **契約 (wave 60 BJ で fail-loud 化)**: 全成分が有限、aperture ≥ 0、
    /// max_dist ≥ 0。旧実装は `max_dist = +∞` で **while ループが発散**
    /// (dist は毎 step ≥ +0.75 伸びるが ∞ に到達しない。空 SVO では
    /// CPU 永遠ループ、WGSL 側なら GPU ハング = device loss) し得た。
    /// NaN origin/dir/aperture も sample 拒否で静寂に黒出力となるのみで
    /// 発見不能だったため、入口契約として拒否する。
    pub fn validate_contract(&self) {
        assert!(
            self.origin.iter().all(|v| v.is_finite()),
            "ConeRay 契約違反: origin に非有限 {:?}",
            self.origin
        );
        assert!(
            self.dir.iter().all(|v| v.is_finite()),
            "ConeRay 契約違反: dir に非有限 {:?}",
            self.dir
        );
        assert!(
            self.aperture.is_finite() && self.aperture >= 0.0,
            "ConeRay 契約違反: aperture={} (half-angle tangent は有限かつ非負)",
            self.aperture
        );
        assert!(
            self.max_dist.is_finite() && self.max_dist >= 0.0,
            "ConeRay 契約違反: max_dist={} は有限かつ非負 (+∞ はループ発散のため拒否)",
            self.max_dist
        );
    }
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
    ///
    /// Front-to-back premultiplied under-blend:
    /// `C += (1−α)·aᵢ·Cᵢ`, `α += (1−α)·aᵢ`。
    /// WGSL (`voxel_cone_tracing.wgsl::main`) とはループ条件・f32 演算順も
    /// 完全一致 (ミラー)。`alpha > 0.001` ガードは CPU では Option 一致後の
    /// 冗長条件 (実 alpha ∈ {1/8 ..= 1} のため常に真) だが、WGSL では
    /// NO_HIT (w=−1) と実サンプルを分ける実条件 — ミラー対称のため保持。
    pub fn trace_diffuse_cone(svo: &SparseVoxelOctree, cone: &ConeRay) -> ([f32; 3], f32) {
        cone.validate_contract();
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

/// 空の SVO ではコーンは何にも当たらない (alpha/color は厳密に 0)。
#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary_greedy_meshing::{idx, SECTION_SIZE};

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
        // wave 60 BJ: 範囲検査を厳密値へ強化 (nodes 空 → sample 常に None
        // → 蓄積は厳密に 0)。
        assert_eq!(alpha, 0.0);
        assert_eq!(color, [0.0, 0.0, 0.0]);
    }

    /// wave 60 BJ: floor(log2) のビット抽出厳密性ピン。
    /// 2 冪境界の ±1ulp で正しい bucket に落ちること
    /// (旧 log2().clamp の丸めアーティファクト領域を直接検査)。
    #[test]
    fn lod_from_diameter_pow2_boundaries_exact() {
        // 非正規/境界: ≤1 / NaN / 0 / 負 → 0 (WGSL !(d>1.0) と完全一致)
        assert_eq!(lod_from_diameter(0.5), 0);
        assert_eq!(lod_from_diameter(1.0), 0);
        assert_eq!(lod_from_diameter(0.0), 0);
        assert_eq!(lod_from_diameter(-3.5), 0);
        assert_eq!(lod_from_diameter(f32::NAN), 0);
        // 2^k の 1ulp 下 → k−1 bucket、ちょうど 2^k → k bucket (k=1..=9 で網羅)
        for k in 1i32..=9 {
            let pow = 2f32.powi(k);
            let below = f32::from_bits(pow.to_bits() - 1);
            assert!(below < pow, "1ulp 下の構成");
            assert_eq!(
                lod_from_diameter(below),
                (k - 1) as u32,
                "2^{k}-ulp → {}",
                k - 1
            );
            assert_eq!(lod_from_diameter(pow), k as u32, "2^{k} → {k}");
        }
        // clamp 10: 2^10=1024 → 10、2^11 以上・+∞ も 10
        assert_eq!(lod_from_diameter(1024.0), 10);
        assert_eq!(lod_from_diameter(1e30), 10);
        assert_eq!(lod_from_diameter(f32::INFINITY), 10);
    }

    /// 全面 solid セクション: root が Uniform(1) 1 ノードに折畳まれ、
    /// 最初のサンプル (alpha=1) で weight=1 により即座に飽和する。
    /// 全演算が dyadic 厳密なので f32 == でピン。
    #[test]
    fn cone_accumulates_exact_full_saturation() {
        let p = [1u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        let svo = SparseVoxelOctree::from_section(&p);
        let cone = ConeRay {
            origin: [8.0, 1.0, 8.0],
            dir: [0.0, 1.0, 0.0],
            aperture: 0.577,
            max_dist: 32.0,
        };
        let (color, alpha) = VoxelConeTracing::trace_diffuse_cone(&svo, &cone);
        assert_eq!(
            color,
            [0.5, 0.5, 0.5],
            "NEUTRAL_ALBEDO=0.5 に weight 1 で正確着地"
        );
        assert_eq!(alpha, 1.0);
    }

    /// 下半分 solid のセクション: root は Branch で 4/8 占有。
    /// aperture=16 → 全サンプルが粗 LOD (budget=0) で alpha=4/8=0.5 を返し、
    /// front-to-back で weight 0.5 → 0.25 の厳密 dyadic 系列になることを
    /// 独立手導出でピン (dist: 0.5 → +12 → 12.5 → diameter 400 → +300 で終了)。
    #[test]
    fn cone_accumulates_exact_half_occupancy_series() {
        let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        for z in 0..SECTION_SIZE {
            for y in 0..8 {
                for x in 0..SECTION_SIZE {
                    p[idx(x, y, z)] = 1;
                }
            }
        }
        let svo = SparseVoxelOctree::from_section(&p);
        let cone = ConeRay {
            origin: [8.0, 1.0, 8.0],
            dir: [0.0, 1.0, 0.0],
            aperture: 16.0,
            max_dist: 50.0,
        };
        let (color, alpha) = VoxelConeTracing::trace_diffuse_cone(&svo, &cone);
        // 独立導出 (全て 2 冪系列で f32 厳密):
        //   w1 = 0.5·(1−0)   = 0.5   → aa=0.5,  C=0.5·0.5=0.25
        //   w2 = 0.5·(1−0.5) = 0.25  → aa=0.75, C=0.25+0.125=0.375
        //   3 ステップ目は dist=312.5 ≥ max_dist=50 で終了
        assert_eq!(color, [0.375, 0.375, 0.375], "w 系列 0.5,0.25 の厳密蓄積");
        assert_eq!(alpha, 0.75);
    }

    /// wave 60 BJ-1: 非有限 origin は拒否。
    #[test]
    #[should_panic(expected = "ConeRay 契約違反: origin に非有限")]
    fn cone_rejects_nan_origin() {
        let svo = SparseVoxelOctree::empty();
        let cone = ConeRay {
            origin: [f32::NAN, 0.0, 0.0],
            dir: [0.0, 1.0, 0.0],
            aperture: 0.5,
            max_dist: 10.0,
        };
        let _ = VoxelConeTracing::trace_diffuse_cone(&svo, &cone);
    }

    /// wave 60 BJ-1: +∞ max_dist は拒否 (旧実装では while 発散)。
    #[test]
    #[should_panic(expected = "ConeRay 契約違反: max_dist")]
    fn cone_rejects_infinite_max_dist() {
        let svo = SparseVoxelOctree::empty();
        let cone = ConeRay {
            origin: [8.0, 1.0, 8.0],
            dir: [0.0, 1.0, 0.0],
            aperture: 0.5,
            max_dist: f32::INFINITY,
        };
        let _ = VoxelConeTracing::trace_diffuse_cone(&svo, &cone);
    }

    /// wave 60 BJ-1: 負 aperture (cone が負に広がる = 意味論なし) は拒否。
    #[test]
    #[should_panic(expected = "ConeRay 契約違反: aperture")]
    fn cone_rejects_negative_aperture() {
        let svo = SparseVoxelOctree::empty();
        let cone = ConeRay {
            origin: [8.0, 1.0, 8.0],
            dir: [0.0, 1.0, 0.0],
            aperture: -0.1,
            max_dist: 10.0,
        };
        let _ = VoxelConeTracing::trace_diffuse_cone(&svo, &cone);
    }

    /// wave 60 BJ-2: WGSL コーンループが CPU ミラー語彙と一致する表記ピン。
    #[test]
    fn wgsl_cone_loop_mirrors_cpu_vocabulary() {
        let wgsl = crate::frame_vct::VCT_WGSL;
        for needle in [
            "// CPU: VoxelConeTracing::trace_diffuse_cone と同一ループ・同一 f32 演算順。",
            "var dist = 0.5f;",
            "if (!(dist < cone.max_dist && aa < 0.99)) {",
            "let diameter = max(2.0 * cone.aperture * dist, 1.0);",
            "let weight = s.w * (1.0 - aa);",
            "dist = dist + diameter * 0.75;",
        ] {
            assert!(
                wgsl.contains(needle),
                "WGSL コーンループ語彙ピン乖離: {needle}"
            );
        }
    }
}

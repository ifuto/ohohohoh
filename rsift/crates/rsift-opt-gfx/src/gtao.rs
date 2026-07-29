//! GTAO — Ground Truth Ambient Occlusion (horizon-based).
//!
//! Approximates ray-traced AO by integrating the horizon angle of the depth
//! profile around each pixel. Much higher quality than SSAO (matches offline
//! reference better, stable in motion) and at ~1ms on iGPUs is affordable when
//! run at half resolution. The CPU side is a faithful single-slice reference
//! used by the tests; the WGSL does the full multi-direction integral.

/// GTAO CPU 参照。【wave 176 FV 捕捉 116】旧 directions/steps/radius
/// フィールドは Default 定数保持のみで読み取り消費者ゼロ (CPU 参照の
/// occlusion()/slice_occlusion() は呼出し側の samples/slices 駆動で
/// パラメータ非参照) → unit-struct 化 (EL-1 tbdr_hints 波 138 判例)。
/// WGSL 側の radius/steps/power は uniform truth (gtao.wgsl) であり、
/// 本 module は CPU 参照 (power=1.0 特殊形) と WGSL ソース供給に限定される。
#[derive(Clone, Copy, Debug, Default)]
pub struct Gtao;
impl Gtao {
    pub fn new() -> Self {
        Gtao
    }

    /// Single-slice (1D) occlusion factor in [0,1] (1 = unoccluded).
    /// `samples` are `(horizontal_offset, height)` pairs with `offset > 0`,
    /// `offset` increasing. `height` is the sample's "height" above the surface;
    /// a sample taller than `center_height` raises the horizon and occludes.
    pub fn slice_occlusion(&self, center_height: f32, samples: &[(f32, f32)]) -> f32 {
        let mut max_horizon = -std::f32::consts::FRAC_PI_2; // -90°
        for &(off, h) in samples {
            if off <= 0.0 {
                continue;
            }
            let angle = ((h - center_height) / off).atan();
            if angle > max_horizon {
                max_horizon = angle;
            }
        }
        // Ow = occlusion grows with the horizon angle above the tangent plane.
        let occlusion = (max_horizon / std::f32::consts::FRAC_PI_2).clamp(0.0, 1.0);
        1.0 - occlusion
    }

    /// Multi-slice occlusion, averaging `directions` rotated 1D slices. Each
    /// slice's `samples` are pre-rotated by the caller. Returns [0,1].
    pub fn occlusion(&self, center_height: f32, slices: &[Vec<(f32, f32)>]) -> f32 {
        if slices.is_empty() {
            return 1.0;
        }
        let mut sum = 0.0f32;
        for s in slices {
            sum += self.slice_occlusion(center_height, s);
        }
        sum / slices.len() as f32
    }
}

pub const GTAO_WGSL: &str = include_str!("../shaders/gtao.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flat_surface_is_unoccluded() {
        let g = Gtao::new();
        // All samples at the same height as the center => no horizon rise.
        let samples = vec![(1.0, 0.0), (2.0, 0.0), (3.0, 0.0)];
        let o = g.slice_occlusion(0.0, &samples);
        assert!((o - 1.0).abs() < 1e-6);
    }
    #[test]
    fn nearby_occluder_darkens() {
        let g = Gtao::new();
        // A tall sample just to the side raises the horizon -> some occlusion.
        let samples = vec![(1.0, 2.0), (2.0, 1.0), (3.0, 0.0)];
        let o = g.slice_occlusion(0.0, &samples);
        assert!(o < 1.0);
        assert!(o > 0.0);
    }
    #[test]
    fn closer_taller_occludes_more() {
        let g = Gtao::new();
        let near = vec![(0.5, 3.0), (1.0, 1.0)];
        let far = vec![(2.0, 3.0), (4.0, 1.0)];
        let o_near = g.slice_occlusion(0.0, &near);
        let o_far = g.slice_occlusion(0.0, &far);
        assert!(o_near < o_far, "nearer occluder should occlude more");
    }
    /// 【wave 176 FV】slice_occlusion 厳密 bit golden (f32 probe 機械値、
    /// bit→10進は python 照合): near(0.5,3.0)=0x3dd75208・far(2.0,3.0)=
    /// 0x3ebfa8b8・flat=1.0=0x3f800000・avg=0x3e757d3a。green-today pin。
    #[test]
    fn fv_slice_golden_bits() {
        let g = Gtao::new();
        let near = vec![(0.5f32, 3.0f32), (1.0, 1.0)];
        assert_eq!(g.slice_occlusion(0.0, &near).to_bits(), 0x3dd75208);
        let far = vec![(2.0f32, 3.0f32), (4.0, 1.0)];
        assert_eq!(g.slice_occlusion(0.0, &far).to_bits(), 0x3ebfa8b8);
        let flat = vec![(1.0f32, 0.0f32), (2.0, 0.0)];
        assert_eq!(g.slice_occlusion(0.0, &flat).to_bits(), 0x3f800000);
        let avg = g.occlusion(0.0, &[near.clone(), far.clone()]);
        assert_eq!(avg.to_bits(), 0x3e757d3a, "multi-slice 平均 (probe)");
    }

    /// 【wave 176 FV】CPU 参照 (1.0 - occlusion) は WGSL
    /// `pow(1.0 - occlusion, u.power)` の power=1.0 特殊形と語彙一致する
    /// truth pin (FE 判例): gtao.wgsl テキストに atan2(h - center /
    /// clamp(maxHorizon / ・pow(1.0 - occlusion, u.power) が存在することを照合。
    #[test]
    fn fv_wgsl_power_special_form_vocabulary() {
        let w = GTAO_WGSL;
        assert!(w.contains("atan2(h - center"), "horizon angle 語彙");
        assert!(
            w.contains("clamp(maxHorizon / 1.5707963"),
            "occlusion clamp 語彙"
        );
        assert!(
            w.contains("pow(1.0 - occlusion, u.power)"),
            "power 一般形語彙"
        );
    }

    /// 【wave 176 FV】clamp(0.0, 1.0) 下端の truth pin (adversarial 変異 A で
    /// 非検出捕捉 = FV-3): 全サンプルが center 以下 (ホライズンが tangent
    /// より下) のケースで WGSL `clamp(maxHorizon / ...)` 同様に CPU 参照も
    /// occlusion=0 (=1.0 返却、unoccluded) に潰れること。空/負のみの 2 ケース
    /// で bit 0x3f800000 = 1.0 を厳密 pin (f32 probe /tmp/fv_probe2 機械値、
    /// clamp 除去変異で 2.0/1.5 になり検出可能 → 非検出 25 例を回収済)。
    #[test]
    fn fv_clamp_downward_returns_unoccluded() {
        let g = Gtao::new();
        let empty: Vec<(f32, f32)> = vec![];
        assert_eq!(g.slice_occlusion(0.0, &empty).to_bits(), 0x3f800000);
        let down = vec![(1.0f32, -1.0f32), (2.0, -2.0)];
        assert_eq!(g.slice_occlusion(0.0, &down).to_bits(), 0x3f800000);
    }
    #[test]
    fn multi_slice_average_in_range() {
        let g = Gtao::new();
        let flat = vec![vec![(1.0, 0.0), (2.0, 0.0)]; 4];
        let o = g.occlusion(0.0, &flat);
        assert!((o - 1.0).abs() < 1e-6);
    }
    /// 【wave 184 GD】削除済み API `wgsl_source` の再出現 lexeme pin (adversarial
    /// 176-c・25 例目 非検出回収、wave 26 例未採から 1 回収): 削除 truth 証跡として
    /// 宣言形が再起しないことを機械 pin。自己言及 vacuous 回避のため検出
    /// 語彙は分割記述 (doc 証跡条文には `fn ` 接頭で択定範囲外)。
    #[test]
    fn gd_removed_wgsl_source_lexeme() {
        let src = include_str!("gtao.rs");
        let lex = concat!("fn wgsl", "_source");
        assert!(
            !src.contains(lex),
            "削除 API `wgsl_source` の宣言再来を検出 → 死救出は lint/テスト限界で"
        );
    }
}

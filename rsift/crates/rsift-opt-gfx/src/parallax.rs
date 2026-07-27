//! Parallax occlusion mapping (POM) for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): march a view ray through a height field in tangent
//! space and, on the first layer below the surface, interpolate to find the
//! offset UV. Gives surfaces real depth from a height map with *no* geometry
//! cost — a quality improvement that integrated GPUs handle fine (just fewer
//! `layers`).
//!
//! 誠実注記 (wave 148 EU 監査):
//! 1. view_dir.z は 1e-3 底上げ (`max(1e-3)`)。z 極小では p_step が無制約に
//!    巨大化し final_uv が [0,1] 外へ発散し得る (rq: z=1e-4 → uv.x=-2.83)。
//!    NaN z は f32::max 規律で他方 1e-3 が選ばれ同様に巨大化。接線空間の
//!    正規化された view_dir 前提 (z>0) は呼出側契約。
//! 2. 捕捉 61: 補間 before は直前層 (cur_layer - layer_depth) が正しい
//!    (LearnOpenGL POM 一次情報照合)。旧 (cur_layer + layer_depth) は
//!    2*layer_depth ずれで w を系統誤差させ、旧分母 .max(1e-4) は負の
//!    正しい分母を破壊 (w=-300 発散) していた → abs ガードへ根治。
//! 3. 高さが layer_depth 格子 (k/16 等) に載る退化時は after=0 で
//!    final=cur_uv (w=0)。wiring palette 高さ (y/16) は 1/16=layer_depth と
//!    同格子のため常にこの退化 (補間式不問) — module pin で補間を検証。
//! 4. NaN height_scale/height は静寂伝播 (fail-loud しない設計: G-buffer
//!    側契約)。NaN hs → p_step NaN → 16 層尽くし ld=1.0・final NaN。
//!    layers=0 は max(1) 底上げ。
//! 5. Vec4 (型+Add/Sub/Mul) は本体・テスト・wiring 全消費者ゼロ (census
//!    grep 機械確定) のため完全削除 (EN-2/EP-2 同型)。Vec3 は cur_uv - p_step
//!    等で使用のため型+Add/Sub/Mul 維持。

use std::ops::{Add, Mul, Sub};

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
}
impl Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f32) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}

pub struct ParallaxParams {
    pub layers: u32,
    pub height_scale: f32,
}
impl Default for ParallaxParams {
    fn default() -> Self {
        Self {
            layers: 16,
            height_scale: 0.1,
        }
    }
}

/// Returns `(offset_uv, layer_depth)` for the first surface hit. `view_dir`
/// points from the surface toward the eye (tangent space, `z` = out of map).
pub fn parallax_occlusion(
    uv: Vec3,
    view_dir: Vec3,
    height: &dyn Fn(Vec3) -> f32,
    p: &ParallaxParams,
) -> (Vec3, f32) {
    let layers = p.layers.max(1);
    let layer_depth = 1.0 / layers as f32;
    let p_step = Vec3::new(view_dir.x, view_dir.y, 0.0)
        * (p.height_scale * layer_depth / view_dir.z.max(1e-3));
    let mut cur_uv = uv;
    let mut cur_layer = 0.0f32;
    let mut cur_depth = height(cur_uv);
    for _ in 0..layers {
        if cur_layer >= cur_depth {
            break;
        }
        cur_uv = cur_uv - p_step;
        cur_depth = height(cur_uv);
        cur_layer += layer_depth;
    }
    let prev_uv = cur_uv + p_step;
    let prev_depth = height(prev_uv);
    let after = cur_depth - cur_layer;
    // 捕捉 61: before は直前層 (cur_layer - layer_depth) に対する深度差。
    // 一次情報 LearnOpenGL POM: beforeDepth = prevDepth - currentLayerDepth
    // + layerDepth。旧来は (cur_layer + layer_depth) 参照で 2*layer_depth ずれ。
    let before = prev_depth - (cur_layer - layer_depth);
    // 捕捉 61(2): denom <= 0 保証 (before は未衝突 prev 層 >= 0、after は衝突
    // 層 <= 0)。旧 .max(1e-4) は負の正しい分母を 1e-4 に置換して w を発散
    // (sloped で w=-300/final=-0.655) → 退化 (after=before=0、層に正確に
    // 載る) のみ w=0 (=cur_uv) にする abs ガードへ。
    let denom = after - before;
    let w = if denom.abs() < 1e-6 {
        0.0
    } else {
        after / denom
    };
    let final_uv = prev_uv * w + cur_uv * (1.0 - w);
    (final_uv, cur_layer)
}

pub fn wgsl_source() -> &'static str {
    PARALLAX_WGSL
}

pub const PARALLAX_WGSL: &str = include_str!("../shaders/parallax.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flat_surface_has_no_offset() {
        let (uv, _) = parallax_occlusion(
            Vec3::new(0.5, 0.5, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            &|_u: Vec3| 0.0,
            &ParallaxParams::default(),
        );
        assert!((uv.x - 0.5).abs() < 1e-6 && (uv.y - 0.5).abs() < 1e-6);
    }
    #[test]
    fn sloped_surface_offsets() {
        let (uv, _) = parallax_occlusion(
            Vec3::new(0.5, 0.5, 0.0),
            Vec3::new(0.3, 0.0, 1.0),
            &|u: Vec3| u.x.clamp(0.0, 1.0),
            &ParallaxParams {
                layers: 16,
                height_scale: 0.2,
            },
        );
        assert!((uv.x - 0.5).abs() > 1e-4, "offset x = {}", uv.x - 0.5);
    }
    #[test]
    fn layer_depth_in_unit_interval() {
        let (_, ld) = parallax_occlusion(
            Vec3::new(0.5, 0.5, 0.0),
            Vec3::new(0.0, 0.3, 1.0),
            &|_u: Vec3| 0.5,
            &ParallaxParams::default(),
        );
        assert!(ld >= 0.0 && ld <= 1.0);
    }

    // wave 148 EU strict 群 (全 golden rq eu_pom 事前導出)

    // 捕捉 61 golden: 補間 LearnOpenGL 一次情報照合 (解析真値 0.47169811)
    #[test]
    fn pom_interpolation_exact_golden() {
        let (uv, ld) = parallax_occlusion(
            Vec3::new(0.5, 0.5, 0.0),
            Vec3::new(0.3, 0.0, 1.0),
            &|u: Vec3| u.x.clamp(0.0, 1.0),
            &ParallaxParams {
                layers: 16,
                height_scale: 0.2,
            },
        );
        assert_eq!(
            uv.x.to_bits(),
            0x3EF1826B,
            "final_uv.x = 解析真値 1ulp (rq)"
        );
        assert_eq!(uv.y.to_bits(), 0x3F000000, "uv.y = 0.5 exact (rq)");
        assert_eq!(ld.to_bits(), 0x3F000000, "hit layer 0.5 (8 層)");
    }

    #[test]
    fn pom_layers_zero_clamps_to_one_exact() {
        let (uv, ld) = parallax_occlusion(
            Vec3::new(0.5, 0.5, 0.0),
            Vec3::new(0.3, 0.0, 1.0),
            &|u: Vec3| u.x.clamp(0.0, 1.0),
            &ParallaxParams {
                layers: 0,
                height_scale: 0.2,
            },
        );
        assert_eq!(ld, 1.0, "layers=0 → 1 底上げ・1 march");
        assert_eq!(
            uv.x.to_bits(),
            0x3EF1826A,
            "1 層補間 = 16 層と 1ulp 差 (rq)"
        );
    }

    #[test]
    fn pom_z_floor_extreme_diverges() {
        let (uv, _) = parallax_occlusion(
            Vec3::new(0.5, 0.5, 0.0),
            Vec3::new(0.3, 0.0, 1e-4),
            &|u: Vec3| u.x.clamp(0.0, 1.0),
            &ParallaxParams {
                layers: 16,
                height_scale: 0.2,
            },
        );
        assert_eq!(
            uv.x.to_bits(),
            0xC0355556,
            "z 底上げ巨大 step → uv 負発散 (rq) 注記 1"
        );
    }

    #[test]
    fn pom_flat_lattice_exact() {
        let (uv, ld) = parallax_occlusion(
            Vec3::new(0.5, 0.5, 0.0),
            Vec3::new(0.3, 0.0, 1.0),
            &|_u: Vec3| 0.5,
            &ParallaxParams::default(),
        );
        assert_eq!(uv.x.to_bits(), 0x3EF851E8, "8 層減算 lattice golden (rq)");
        assert_eq!(ld.to_bits(), 0x3F000000, "hit layer 0.5 (格子退化)");
    }

    #[test]
    fn pom_height_call_count() {
        use std::cell::Cell;
        let calls = Cell::new(0u32);
        let probe = |u: Vec3| {
            calls.set(calls.get() + 1);
            u.x.clamp(0.0, 1.0)
        };
        let _ = parallax_occlusion(
            Vec3::new(0.5, 0.5, 0.0),
            Vec3::new(0.3, 0.0, 1.0),
            &probe,
            &ParallaxParams {
                layers: 16,
                height_scale: 0.2,
            },
        );
        assert_eq!(calls.get(), 10, "初期 1 + march 8 + prev 1");
    }

    #[test]
    fn pom_height_scale_nan_propagates() {
        let (uv, ld) = parallax_occlusion(
            Vec3::new(0.5, 0.5, 0.0),
            Vec3::new(0.3, 0.0, 1.0),
            &|u: Vec3| u.x.clamp(0.0, 1.0),
            &ParallaxParams {
                layers: 16,
                height_scale: f32::NAN,
            },
        );
        assert!(uv.x.is_nan(), "NaN hs → p_step NaN → final NaN 伝播");
        assert_eq!(ld, 1.0, "layer>=NaN false で 16 層尽くし → ld=1.0");
    }

    #[test]
    fn pom_contract_pins() {
        assert!(1.0f32 / 16.0 == 0.0625, "ld 2 冪 exact");
        assert_eq!(wgsl_source(), PARALLAX_WGSL, "wgsl identity (&str 内容)");
        let d = ParallaxParams::default();
        assert!(
            d.layers == 16 && d.height_scale.to_bits() == 0x3DCCCCCD,
            "default (16, 0.1)"
        );
        // EU-3 contract pin: Vec3 型+Add/Sub/Mul 維持 (Vec4 は完全削除)
        let a = Vec3::new(1.0, 2.0, 3.0) - Vec3::new(0.5, 1.0, 1.5);
        assert!(
            a.x == 0.5 && a.y == 1.0 && a.z == 1.5,
            "Vec3 Sub (cur_uv - p_step 本体使用)"
        );
        let b = a + Vec3::new(1.0, 1.0, 1.0);
        let m = b * 2.0;
        assert!(
            b.x == 1.5 && m.x == 3.0 && m.z == 5.0,
            "Vec3 Add/Mul (m.z=5 私の初 pin 誤りを RED 捕捉→訂正)"
        );
    }
}

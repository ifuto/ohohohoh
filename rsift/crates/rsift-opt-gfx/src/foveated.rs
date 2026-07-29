//! Foveated shading-rate (VRS) mask for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): a radial shading-rate field centred on the gaze
//! point. Full rate (1.0) at the fovea, falling to `min_rate` at the edge of
//! `radius`. Variable-rate shading spends GPU pixels where the eye actually
//! looks — on integrated GPUs this is nearly free performance with *no*
//! resolution downscale of the final image (it only changes shading rate).
//!
//! 誠実注記 (wave 142 EP-3):
//! 1. `min_rate > 1.0` は最終 `.clamp(min_rate, 1.0)` の **min > max で
//!    panic** (f32::clamp の契約違反は debug/release 共通) — 呼出側事前
//!    条件は min_rate ∈ [0,1]。wiring は定数 0.5 供給で範囲内。
//!    本 panic 挙動は should_panic pin で挙動として固定。
//! 2. NaN 伝播 (捕捉 57 訂正版 — f32::clamp は **引数 min/max が NaN
//!    でも panic** (std doc 一次情報)、セルフ値 NaN のみ透過):
//!    `radius.max(1e-4)` は f32::max の NaN 落としで NaN radius を
//!    1e-4 に切替 (0 除算回避) → rate = min_rate。**NaN min_rate は
//!    最終 clamp で min=NaN → panic** (should_panic pin)。
//!    NaN uv は d=NaN → t=NaN → セルフ NaN 透過で最終出力 NaN
//!    (min 引数は有限値のため panic しない) — pin 済。
//! 3. `uv.z`・`gaze.z` は**未使用** (radial は 2D 評価、z 成分は契約に
//!    寄与しない)。Vec3 型採用は座標型一貫性のため。
//! 4. 旧 `Vec4`+両型 Add/Sub/Mul trait 実装は crate+workspace 消費者
//!    ゼロ (Vec3 型自体は shading_rate 引数型で消費、演算子は不使用)
//!    の完全装飾 → 削除 (EN-2 同型、wave 142 EP-2)。

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

/// Shading rate in [min_rate, 1.0]: 1.0 at the gaze, lower toward the edge.
pub fn shading_rate(uv: Vec3, gaze: Vec3, radius: f32, min_rate: f32) -> f32 {
    let dx = uv.x - gaze.x;
    let dy = uv.y - gaze.y;
    let d = (dx * dx + dy * dy).sqrt();
    let t = (d / radius.max(1e-4)).clamp(0.0, 1.0);
    (1.0 - t * (1.0 - min_rate)).clamp(min_rate, 1.0)
}

pub fn wgsl_source() -> &'static str {
    FOVEATED_WGSL
}

pub const FOVEATED_WGSL: &str = include_str!("../shaders/foveated.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn at_gaze_is_full_rate() {
        let r = shading_rate(
            Vec3::new(0.5, 0.5, 0.0),
            Vec3::new(0.5, 0.5, 0.0),
            0.4,
            0.25,
        );
        assert!((r - 1.0).abs() < 1e-6);
    }
    #[test]
    fn far_from_gaze_is_min_rate() {
        let r = shading_rate(
            Vec3::new(0.99, 0.99, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            0.4,
            0.25,
        );
        assert!((r - 0.25).abs() < 1e-6, "rate = {}", r);
    }
    #[test]
    fn rate_is_monotonic_with_distance() {
        let near = shading_rate(Vec3::new(0.55, 0.5, 0.0), Vec3::new(0.5, 0.5, 0.0), 0.4, 0.25);
        let far = shading_rate(Vec3::new(0.9, 0.5, 0.0), Vec3::new(0.5, 0.5, 0.0), 0.4, 0.25);
        assert!(far < near, "near={} far={}", near, far);
    }
}

#[cfg(test)]
mod strict_tests {
    use super::*;

    /// EP-4: wiring 実引数 golden (rq ep_foveated 導出、f32 逐次丸め追従):
    /// uv=(0.5,0.5), gaze=(dir0*0.5+0.5, dir2*0.5+0.5), radius=0.2,
    /// min_rate=0.5。camera_dir=[0,0,1] (empty/chunked 共通) →
    /// dy=-0.5 → t=2.5→clamp 1 → 0.5=0x3F000000。dir=0 → 1.0。
    /// dir_z=0.2 → d=0.099999994 で rate=0x3F3FFFFF
    /// (0.75 ではなく **1 ulp 下**、rq strict 導出)。
    #[test]
    fn wiring_shape_golden_bits() {
        let mk = |x: f32, y: f32| Vec3::new(x, y, 0.0);
        let uv = mk(0.5, 0.5);
        let rate = |dz: f32| shading_rate(uv, mk(0.0 * 0.5 + 0.5, dz * 0.5 + 0.5), 0.2, 0.5);
        assert_eq!(rate(1.0).to_bits(), 0x3F000000, "dir=[0,0,1] → 0.5");
        assert_eq!(rate(0.0).to_bits(), 0x3F800000, "gaze=uv → 1.0");
        assert_eq!(
            rate(0.2).to_bits(),
            0x3F3FFFFF,
            "dir_z=0.2 は 0.75 ではなく 1 ulp 下 (rq 導出)"
        );
        assert_eq!(rate(0.4).to_bits(), 0x3F000000, "t=1 境界");
        assert_eq!(rate(-0.4).to_bits(), 0x3F000000, "対称");
    }

    /// EP-3-1 pin: min_rate > 1.0 は clamp(min,max) で min>max panic
    /// (f32::clamp 契約違反、debug/release 共通)。
    #[test]
    #[should_panic(expected = "min > max")]
    fn min_rate_above_one_panics() {
        let _ = shading_rate(Vec3::new(0.5, 0.5, 0.0), Vec3::new(0.5, 0.5, 0.0), 0.2, 1.5);
    }

    /// EP-3-2 pin (捕捉 57 訂正後): NaN 系伝播 — radius NaN → 1e-4
    /// 底上げ (f32::max の NaN 落とし)、radius=0 → min_rate 収束。
    /// NaN uv → セルフ NaN 透過で NaN 出力 (doc 例証:
    /// `(f32::NAN).clamp(-2.0, 1.0).is_nan()`)。
    #[test]
    fn nan_semantics_pin() {
        let uv = Vec3::new(0.5, 0.5, 0.0);
        let g = Vec3::new(0.2, 0.3, 0.0);
        let r_nan_radius = shading_rate(uv, g, f32::NAN, 0.5);
        assert_eq!(r_nan_radius.to_bits(), 0x3F000000, "NaN radius → min_rate");
        let r_zero_radius = shading_rate(uv, g, 0.0, 0.5);
        assert_eq!(r_zero_radius.to_bits(), 0x3F000000, "radius 0 → 1e-4 底");
        assert!(
            shading_rate(Vec3::new(f32::NAN, 0.5, 0.0), g, 0.2, 0.5).is_nan(),
            "NaN uv → NaN 出力 (セルフ NaN 透過)"
        );
    }

    /// 捕捉 57 pin: NaN min_rate は最終 `.clamp(min_rate, 1.0)` で
    /// **min=NaN 引数 → panic** (f32::clamp は引数 min/max NaN で
    /// panic、std doc 一次情報)。
    #[test]
    #[should_panic(expected = "min > max, or either was NaN")]
    fn min_rate_nan_panics() {
        let _ = shading_rate(
            Vec3::new(0.5, 0.5, 0.0),
            Vec3::new(0.2, 0.3, 0.0),
            0.2,
            f32::NAN,
        );
    }

    /// EP-2 等価 pin: free fn ≡ const 同一内容 + Vec3 構築一致
    /// (crate 内唯一の Vec3 消費経路である shading_rate 引数契約)。
    #[test]
    fn wgsl_identity_and_vec3_contract() {
        assert_eq!(wgsl_source(), FOVEATED_WGSL);
        let v = Vec3::new(1.0, 2.0, 3.0);
        assert_eq!((v.x, v.y, v.z), (1.0, 2.0, 3.0));
        let d = Vec3::default();
        assert_eq!((d.x, d.y, d.z), (0.0, 0.0, 0.0));
    }

    /// 【wave 185 GE フェーズ2 回収】dead code 系採番枠外の先行同型 (wave 142 EP-2(d)
    /// adversarial Vec4+全 trait 復活 revert 非検出、EP-2 で census 消費者ゼロ不可能
    /// 証明削除済) の lexeme pin 化。dead code 系採番は wave 148 EU-3(c) 起点のため
    /// 本件は枠外だが、同一限界構造の回収として本 wave で一括 pin 化。同宣言形の
    /// 将来復活を静寂に通さない。
    #[test]
    fn ge_removed_vec4_lexeme() {
        let src = include_str!("foveated.rs");
        for lex in [
            concat!("struct ", "Vec4"),
            concat!("impl ", "Vec4"),
            concat!("Add for ", "Vec4"),
            concat!("Sub for ", "Vec4"),
            concat!("Mul<f32> for ", "Vec4"),
        ] {
            assert!(
                !src.contains(lex),
                "dead code 系削除語彙の宣言形復活を検出 (wave 185 GE lexeme pin)"
            );
        }
    }
}

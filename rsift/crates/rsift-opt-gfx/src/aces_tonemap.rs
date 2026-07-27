//! ACES Filmic tone mapping (Narkowicz 2015 approximation).
//!
//! Maps HDR linear color into displayable [0,1] with a filmic "shoulder" that
//! preserves saturation and avoids the harsh clip of `min(c,1)`. Cheap (a few
//! multiplies), so it is safe on low-spec GPUs, and pairs well with FSR1/2.
//!
//! ## 一次情報 (2026-07-23 wave 66 BP-2 で照合)
//! Narkowicz 原著 (knarkowicz.wordpress.com/2016/01/06
//! /aces-filmic-tone-mapping-curve) の HLSL:
//! `saturate((x*(a*x+b))/(x*(c*x+d)+e))`, a=2.51, b=0.03, c=2.43, d=0.59,
//! e=0.14 — 本実装の係数・式構造と完全一致。著者自身の用法注記:
//! 「exposure をトーンマップ**前**に乗算、gamma 補正は**後**」
//! (本モジュールの構造と同順)。また著者明示の限界: 「luminance only の
//! 単純 fit でブライトは過飽和気味」— 詳細 fit 必要時は Stephen Hill
//! (BakingLab ACES.hlsl) 系を検討すること。「入力 1 で出力 ~0.8」
//! (実値 0.8038) も原文通り。
//!
//! ## 3連鎖の分担
//! WGSL 側語彙ピンは `frame_reference::tests::wgsl_aces_mirror_constants
//! _and_fullscreen_triangle` (wave 63 BM-2) が担任。本モジュールは
//! exposure 乗算位置 (WGSL `hdr * u.exposure`) と `aces_channel` の
//! 厳密 bit 値を担任する。sRGB エンコードは WGSL が `pow(max(x,0),1/2.2)`
//! (上限クランプ無し) なのに対し Rust 側 `linear_to_srgb` は
//! `x >= 1.0 → 1.0` の飽和を持つ — 実パスでは aces_channel 出力 (≤1.0 に
//! clamp 済) しか到達しないため両者一致、差が出るのは契約外 (>1.0) 直接
//! 呼出のみ (LDR [0,1] 入力契約、doc 明記)。
//! なお `powf` は libm 依存で ±1 ulp 程度の実装間差があり得るため
//! sRGB 側は bit ではなく許容差ピン (1e-6 ≫ 1 ulp) とする。

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec3 {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}
impl Vec3 {
    pub fn new(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b }
    }
    pub fn clamp(self, lo: f32, hi: f32) -> Vec3 {
        Vec3::new(
            self.r.clamp(lo, hi),
            self.g.clamp(lo, hi),
            self.b.clamp(lo, hi),
        )
    }
}
impl std::ops::Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f32) -> Vec3 {
        Vec3::new(self.r * s, self.g * s, self.b * s)
    }
}
impl std::ops::Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.r + o.r, self.g + o.g, self.b + o.b)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AcesTonemap {
    /// Exposure multiplier applied before the filmic curve.
    pub exposure: f32,
}
impl Default for AcesTonemap {
    fn default() -> Self {
        Self { exposure: 1.0 }
    }
}
impl AcesTonemap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply exposure then the ACES filmic approximation to a single channel.
    pub fn aces_channel(x: f32) -> f32 {
        let a = 2.51f32;
        let b = 0.03f32;
        let c = 2.43f32;
        let d = 0.59f32;
        let e = 0.14f32;
        let num = x * (a * x + b);
        let den = x * (c * x + d) + e;
        (num / den).clamp(0.0, 1.0)
    }

    /// Tone map an HDR color (linear) to LDR (still linear; gamma-encode after).
    pub fn tonemap(&self, c: Vec3) -> Vec3 {
        let e = c * self.exposure;
        Vec3::new(
            Self::aces_channel(e.r),
            Self::aces_channel(e.g),
            Self::aces_channel(e.b),
        )
    }

    /// Full display pipeline: exposure -> ACES -> gamma (sRGB-ish) encode.
    pub fn tonemap_display(&self, c: Vec3) -> Vec3 {
        let t = self.tonemap(c);
        Vec3::new(
            linear_to_srgb(t.r),
            linear_to_srgb(t.g),
            linear_to_srgb(t.b),
        )
    }

    pub fn wgsl_source(&self) -> &'static str {
        ACES_WGSL
    }
}

/// Approximate sRGB encode (gamma 2.2) for LDR linear input.
pub fn linear_to_srgb(x: f32) -> f32 {
    if x <= 0.0 {
        0.0
    } else if x >= 1.0 {
        1.0
    } else {
        x.powf(1.0 / 2.2)
    }
}

pub const ACES_WGSL: &str = include_str!("../shaders/aces_tonemap.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zero_stays_zero() {
        let a = AcesTonemap::new();
        let o = a.tonemap(Vec3::new(0.0, 0.0, 0.0));
        assert!((o.r).abs() < 1e-6);
    }
    #[test]
    fn monotonic_increasing() {
        let mut prev = -1.0f32;
        for i in 0..20 {
            let x = i as f32 * 0.5;
            let y = AcesTonemap::aces_channel(x);
            assert!(y >= prev - 1e-6, "not monotonic at {}", x);
            prev = y;
        }
    }
    #[test]
    fn clamps_to_one() {
        let a = AcesTonemap::new();
        let o = a.tonemap(Vec3::new(100.0, 100.0, 100.0));
        assert!(o.r <= 1.0 + 1e-6);
        assert!(o.r >= 0.0);
    }
    #[test]
    fn midtone_preserved_and_saturating() {
        // a value near 1.0 after exposure should stay well below clip.
        // Narkowicz 近似では f(1.0) = 2.54/3.16 ≈ 0.8038 が数学的に正しい。
        let a = AcesTonemap::new();
        let o = a.tonemap(Vec3::new(1.0, 1.0, 1.0));
        assert!(o.r > 0.7 && o.r < 0.9, "midtone shoulder: {}", o.r);
    }
    #[test]
    fn srgb_encode_endpoints() {
        assert!((linear_to_srgb(0.0)).abs() < 1e-6);
        assert!((linear_to_srgb(1.0) - 1.0).abs() < 1e-6);
    }

    /// wave 66 BP-1: `aces_channel` の公式式 `saturate(x(a x+b)/(x(c x+d)+e))`
    /// を f32 エミュレーション独立導出の厳密ビットで固定。
    /// 係数・式構造は Narkowicz 原著 HLSL と一次情報照合済 (module doc)。
    /// 乗除加算のみで構成 (超越関数なし) されるため全プラットフォーム決定的。
    /// aces(1.0) ≈ 0.8038 は原著「1 on input maps to ~0.8」の実測値に一致。
    #[test]
    fn aces_channel_exact_bits_canonical() {
        let cases: [(f32, u32); 5] = [
            (0.0, 0x00000000), // num=0 → +0.0 厳密
            (0.5, 0x3f1dc64a), // 0.6163069...
            (1.0, 0x3f4dc5ab), // 0.8037974... (原著 ~0.8)
            (2.0, 0x3f6a33f0), // 0.9148550...
            (5.0, 0x3f7c3b06), // 0.9852756... (HDR ショルダー、saturate 未満)
        ];
        for (x, want) in cases {
            assert_eq!(
                AcesTonemap::aces_channel(x).to_bits(),
                want,
                "aces_channel({x}) bit drift"
            );
        }
        // 5.0 でも 1.0 未満 (ショルダーが副作用で飽和しないことの定量化)
        assert!(AcesTonemap::aces_channel(5.0) < 1.0);
    }

    /// wave 66 BP-3: 負入力の挙動を Narkowicz 式の数学的性格通りに厳密固定。
    /// 分母は判別式 0.59²-4·2.43·0.14 < 0 より実数全域で厳に正 (浮動小数でも
    /// 常用範囲で 0 除算 NaN は不可達)。挙動は 2 領域に分岐:
    /// - 0 ≥ x > -b/a (≈-0.01195): 分子 < 0 → saturate で厳密 +0.0
    /// - x < -b/a: 分子・分母とも正 → 正リターンに **wrap** (負 HDR が白へ
    ///   巻き戻る)。契約は HDR リニア ≥ 0 — 負値を生成しないのが producer
    ///   責務 (NaN 哲学と同型) だが、到達時の挙動は決定的に固定する。
    #[test]
    fn aces_channel_negative_input_behavior_is_deterministic() {
        // 小さな負値: 厳密 +0.0
        for x in [-0.01, -1e-6, -0.0119] {
            assert_eq!(
                AcesTonemap::aces_channel(x).to_bits(),
                0,
                "aces_channel({x}) must saturate to +0.0 (numerator < 0)"
            );
        }
        // wrap 領域: 正リターン (この 2 点では >1 → saturate 上限)
        for x in [-1.0, -30.0] {
            assert_eq!(
                AcesTonemap::aces_channel(x).to_bits(),
                0x3f800000,
                "aces_channel({x}) wraps positive (clamped to 1.0)"
            );
        }
    }

    /// wave 66 BP-1: exposure はトーンマップ前乗算 (著者用法注記順)。
    /// exposure=2.0 で 0.5 → aces(1.0) とビット一致 (位置検証の厳密形)。
    #[test]
    fn exposure_multiplies_before_curve() {
        let a = AcesTonemap { exposure: 2.0 };
        let pre = a.tonemap(Vec3::new(0.5, 0.5, 0.5));
        let direct = AcesTonemap::aces_channel(1.0);
        assert_eq!(
            pre.r.to_bits(),
            direct.to_bits(),
            "exposure must multiply input before the curve"
        );
    }

    /// wave 66 BP-1: sRGB エンコードの中間点は許容差ピン (1e-6 ≫ 1 ulp)。
    /// powf は libm 依存で ±1 ulp 実装間差があり得るため bit 固定はしない
    /// (module doc の方針記載参照)。理論値 0.5^(1/2.2) = 0.72974005...。
    /// 契約外 (>1.0) は Rust 側のみ飽和 (WGSL との差分は実パス非到達)。
    #[test]
    fn srgb_encode_midpoint_within_1ulp_class_tolerance() {
        let v = linear_to_srgb(0.5);
        let expect = 0.5f64.powf(1.0f64 / 2.2f64) as f32;
        assert!(
            (v as f64 - expect as f64).abs() < 1e-6,
            "linear_to_srgb(0.5) = {v} vs {expect}"
        );
        // 契約外飽和の Rust 側挙動を決定的に固定
        assert_eq!(linear_to_srgb(1.5).to_bits(), 1.0f32.to_bits());
        assert_eq!(linear_to_srgb(-0.25).to_bits(), 0.0f32.to_bits());
    }
}

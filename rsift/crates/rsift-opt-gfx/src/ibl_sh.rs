//! Spherical-harmonics (2nd-order, 9 coeffs) image-based ambient for
//! `rsift-opt-gfx`.
//!
//! Real logic (no stubs): evaluate a baked 3-band SH ambient probe for an
//! arbitrary direction. This is the cheap, GPU-friendly ambient model used by
//! modern engines for indirect light. Quality only — integrated GPUs evaluate
//! 9 dot products per pixel, essentially free.
//!
//! 【wave 164 FJ (2026-07-28)】消費者: `FullGraphWiring` の IBL 抽出
//! (Vec3::new/sh_basis/evaluate_sh/Add/Mul) → report.ambient_light、
//! `wgsl_source` = gpu_runtime 登録。捕捉 96 [小]: (a) 消費者完全ゼロの
//! `Vec4` (型+演算 3 実装) と `Vec3::Sub` impl を機械 grep 証明で不可能
//! 証明削除 (async_compute EJ-2 / ddgi FF 判例)。(b) WGSL `evaluate_sh`
//! は `sh_basis(normalize(dir))` 二重正規化で CPU 参照と bit 語彙不一致
//! の装飾 — sh_basis 内部の 1 回へ統一 (5 定数は CPU/WGSL 完全一致を
//! 機械照合済、値精度 6dp trunc は両側同一の設計)。Vec3 の残愛器:
//! new/dot/length/normalize/Add/Mul<f32> は sh_basis/evaluate_sh/wiring
//! で全て実消費。

use std::ops::{Add, Mul};

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
    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }
    pub fn normalize(self) -> Vec3 {
        let l = self.length();
        if l > 1e-8 {
            self * (1.0 / l)
        } else {
            self
        }
    }
}
impl Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f32) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}

/// 3-band (9 coefficient) SH basis evaluated at a normalized direction.
pub fn sh_basis(dir: Vec3) -> [f32; 9] {
    let d = dir.normalize();
    let x = d.x;
    let y = d.y;
    let z = d.z;
    [
        0.282095,                 // Y0,0
        0.488603 * y,             // Y1,-1
        0.488603 * z,             // Y1,0
        0.488603 * x,             // Y1,1
        1.092548 * x * y,         // Y2,-2
        1.092548 * y * z,         // Y2,-1
        0.315392 * (3.0 * z * z - 1.0), // Y2,0
        1.092548 * x * z,         // Y2,1
        0.546274 * (x * x - y * y), // Y2,2
    ]
}

/// Evaluate the ambient radiance from a 9-coefficient (per-channel) probe.
pub fn evaluate_sh(coeffs: &[Vec3; 9], dir: Vec3) -> Vec3 {
    let b = sh_basis(dir);
    let mut c = Vec3::new(0.0, 0.0, 0.0);
    for i in 0..9 {
        c = c + coeffs[i] * b[i];
    }
    c
}

pub fn wgsl_source() -> &'static str {
    IBL_SH_WGSL
}

pub const IBL_SH_WGSL: &str = include_str!("../shaders/ibl_sh.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn basis_has_nine_terms_and_constant_term() {
        let b = sh_basis(Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(b.len(), 9);
        assert!((b[0] - 0.282095).abs() < 1e-6);
    }
    #[test]
    fn constant_probe_is_direction_independent() {
        // A constant ambient value of 1.0 is represented by only the DC term,
        // scaled by sqrt(4*pi) so that Y00 (0.282095) reconstructs to 1.0.
        let mut coeffs = [Vec3::new(0.0, 0.0, 0.0); 9];
        let dc = 3.5449077; // sqrt(4*pi)
        coeffs[0] = Vec3::new(dc, dc, dc);
        let a = evaluate_sh(&coeffs, Vec3::new(1.0, 0.0, 0.0));
        let b = evaluate_sh(&coeffs, Vec3::new(0.0, 1.0, 0.0));
        let c = evaluate_sh(&coeffs, Vec3::new(0.0, 0.0, 1.0));
        assert!((a.x - 1.0).abs() < 1e-4, "a.x = {}", a.x);
        assert!((b.x - 1.0).abs() < 1e-4, "b.x = {}", b.x);
        assert!((c.x - 1.0).abs() < 1e-4, "c.x = {}", c.x);
    }
    // ==================== wave 164 (FJ) strict ====================

    /// 捕捉 96 [小]: WGSL 側 sh_basis は CPU と 5 定数の逐語同一語彙であり、
    /// evaluate_sh の正規化は sh_basis 内部の 1 回のみ (旧 WGSL は
    /// `sh_basis(normalize(dir))` の二重正規化で CPU 参照と語彙不一致 —
    /// 値は不動点だが bit 語彙は一意でない装飾)。テキスト pin (FE 先例)。
    #[test]
    fn fj_wgsl_matches_cpu_reference_vocabulary() {
        for c in ["0.282095", "0.488603", "1.092548", "0.315392", "0.546274"] {
            assert!(IBL_SH_WGSL.contains(c), "WGSL 定数 {c} は CPU 参照と同一");
        }
        assert!(
            IBL_SH_WGSL.contains("let b = sh_basis(dir);"),
            "正規化は sh_basis 内部の 1 回 (二重正規化撤去済)"
        );
        assert!(
            !IBL_SH_WGSL.contains("sh_basis(normalize(dir))"),
            "旧二重正規化形は存在しない"
        );
    }

    /// sh_basis の端点 bit 厳密 pin (実機 probe: z+ / x+ / y+ 全 9 項、
    /// (0,0,2) 非単位入力は正規化不動点で z+ と bit 同一)。
    #[test]
    fn fj_sh_basis_endpoint_bit_golden() {
        let z = sh_basis(Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(
            z.map(f32::to_bits),
            [
                0x3e906ec1, 0x00000000, 0x3efa2a2c, 0x00000000, 0x00000000, 0x00000000, 0x3f217b0f,
                0x00000000, 0x00000000
            ],
            "z+ golden (probe)"
        );
        let x = sh_basis(Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(
            x.map(f32::to_bits),
            [
                0x3e906ec1, 0x00000000, 0x00000000, 0x3efa2a2c, 0x00000000, 0x00000000, 0xbea17b0f,
                0x00000000, 0x3f0bd89d
            ],
            "x+ golden (probe): Y2,2 = +0.546274"
        );
        let y = sh_basis(Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(
            y.map(f32::to_bits),
            [
                0x3e906ec1, 0x3efa2a2c, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0xbea17b0f,
                0x00000000, 0xbf0bd89d
            ],
            "y+ golden (probe): Y2,2 = -0.546274 (符号反転)"
        );
        let z2 = sh_basis(Vec3::new(0.0, 0.0, 2.0));
        assert_eq!(
            z2.map(f32::to_bits),
            z.map(f32::to_bits),
            "非単位入力は正規化不動点で bit 同一 (probe)"
        );
    }

    #[test]
    fn z_probe_flips_sign_across_hemisphere() {
        // Only the Y1,0 (z) coefficient is non-zero.
        let mut coeffs = [Vec3::new(0.0, 0.0, 0.0); 9];
        coeffs[2] = Vec3::new(1.0, 0.0, 0.0);
        let up = evaluate_sh(&coeffs, Vec3::new(0.0, 0.0, 1.0));
        let down = evaluate_sh(&coeffs, Vec3::new(0.0, 0.0, -1.0));
        assert!(up.x > 0.0 && down.x < 0.0);
    }
}

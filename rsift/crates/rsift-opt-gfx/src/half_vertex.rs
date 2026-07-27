//! Half-precision vertex quantization.
//!
//! Packs `f32` vertex attributes into IEEE-754 `f16`. On tile-based integrated
//! GPUs (Intel/Apple/ARM) this halves vertex-fetch bandwidth and, where the GPU
//! packs two `f16` into one register, doubles ALU throughput for attribute math.
//! Positions keep full `f32` (precision matters); normals/UVs/colors go to `f16`.
//!
//! **エンコーダ規約 (2026-07-23 wave 52 厳密化)**: `f32_to_f16` は
//! **真の round-to-nearest-even** (旧実装は `mant >> 13` の切捨てで、doc が
//! 謳う RNE と乖離し誤差最大 1 ulp / 65520 overflow 境界も不正だった)。
//! subnormal 結果は **flush-to-zero** (頂点属性では不要かつ iGPU では
//! subnormal 演算が遅い): |x| < 2^-14-2^-25 → ±0、2^-14-2^-25 ≤ |x| < 2^-14
//! の薄帯は最近接 normal (2^-14) に RNE 丸め上げ (subnormal グリッド込みで
//! 丸めてから flush する IEEE FTZ 動作と一致)。overflow は RNE 自然境界
//! |x| ≥ 65520 → ±Inf (65520 は max finite 65504 と 2^16 のちょうど中点で、
//! ties-to-even は mantissa LSB 0 側 = Inf を選ぶ)。NaN は payload を潰した
//! 正準 qNaN (sign 保存)。decoder `f16_to_f32` は全 65536 入力で厳密
//! (shaders/half_vertex.wgsl のビット配置デコードと bit 完全対応)。

/// Encode an `f32` to an IEEE-754 binary16 (`u16`). Round-to-nearest-even,
/// Inf/NaN 処理済み、subnormal 結果は flush-to-zero (上記モジュール規約)。
pub fn f32_to_f16(value: f32) -> u16 {
    let x = value.to_bits();
    let sign = ((x >> 31) & 0x1) as u32;
    let exp = ((x >> 23) & 0xff) as i32;
    let mant = x & 0x7f_ffff;

    let (half_exp, half_mant): (u32, u32) = if exp == 255 {
        if mant != 0 {
            (0x1f, 0x200) // NaN → 正準 qNaN (payload 切捨て、sign は最終合成で保存)
        } else {
            (0x1f, 0) // Inf
        }
    } else {
        let e = exp - 127; // unbiased exponent (f32 subnormal は e=-127 で下の FTZ に落ちる)
        if e < -15 {
            // |x| < 2^-15: 最近接は subnormal 以下 → FTZ で ±0。
            (0, 0)
        } else if e == -15 {
            // x ∈ [2^-15, 2^-14): subnormal グリッド込み RNE ののち flush。
            // min normal (2^-14) へ丸め上がるのは x ≥ 2^-14-2^-25 (tie は even
            // mantissa 0 側 = min normal) ⇔ 上位 10 bit が 1023 のときのみ。
            if (mant >> 13) == 0x3ff {
                (1, 0)
            } else {
                (0, 0)
            }
        } else if e > 15 {
            // |x| ≥ 2^16 (e≥16) は RNE でも ±Inf。
            (0x1f, 0)
        } else {
            // 正常レンジ: 真の RNE。drop した 13 bit が半分超、またはちょうど
            // 半分で keep が奇数 (ties-to-even) なら繰上げ。mantissa 溢れは
            // 指数へ自然に桁上げされ、e+1=16 なら Inf (65520 境界) となる。
            let keep = mant >> 13;
            let dropped = mant & 0x1fff;
            let round_up = dropped > 0x1000 || (dropped == 0x1000 && (keep & 1) == 1);
            let (keep, e) = if round_up {
                let k2 = keep + 1;
                if k2 == 0x400 {
                    (0, e + 1)
                } else {
                    (k2, e)
                }
            } else {
                (keep, e)
            };
            if e > 15 {
                (0x1f, 0)
            } else {
                ((e + 15) as u32, keep)
            }
        }
    };
    ((sign << 15) | (half_exp << 10) | half_mant) as u16
}

/// Decode an IEEE-754 binary16 (`u16`) back to `f32` (全入力で厳密 — 各項は
/// dyadic 有理数で f32 に正確に表現可能)。
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = (h >> 15) & 1;
    let exp = (h >> 10) & 0x1f;
    let mant = h & 0x3ff;
    let f = if exp == 0 {
        if mant == 0 {
            0.0f32
        } else {
            (mant as f32) / 1024.0 * (2.0f32).powi(-14)
        }
    } else if exp == 0x1f {
        if mant == 0 {
            f32::INFINITY
        } else {
            f32::NAN
        }
    } else {
        (1.0 + (mant as f32) / 1024.0) * (2.0f32).powi((exp as i32) - 15)
    };
    if sign == 1 {
        -f
    } else {
        f
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// Quantize a 3D attribute (e.g. normal/UV) to `f16`, returning the packed bytes.
pub fn quantize_vec3(v: Vec3) -> [u16; 3] {
    [f32_to_f16(v.x), f32_to_f16(v.y), f32_to_f16(v.z)]
}
/// Quantize a 2D attribute (e.g. UV) to `f16`.
pub fn quantize_vec2(v: Vec2) -> [u16; 2] {
    [f32_to_f16(v.x), f32_to_f16(v.y)]
}

/// Bandwidth saved (in bytes) when converting a vertex of `attrs` f32 components
/// to f16. e.g. 8 floats -> 8*2 = 16 bytes (was 32): saves 16.
pub fn bandwidth_saved(components: usize) -> usize {
    components * (4 - 2)
}

/// Stride (bytes) of a packed vertex: `pos` stays f32 (3*4) plus `half_comps` f16 (2 each).
pub fn packed_stride(half_components: usize) -> usize {
    12 + half_components * 2
}

pub const HALF_VERTEX_WGSL: &str = include_str!("../shaders/half_vertex.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    /// wave 52: 2^-n は f32 で厳密 (powi は 2 の冪の square/multiply のみで
    /// 丸めを挟まない)。2 項の和/差も結果が 24 bit 以内に収まり厳密。
    fn p2(n: i32) -> f32 {
        (2.0f32).powi(n)
    }

    #[test]
    fn known_encodings_exact() {
        assert_eq!(f32_to_f16(1.0), 0x3C00);
        assert_eq!(f32_to_f16(0.5), 0x3800);
        assert_eq!(f32_to_f16(2.0), 0x4000);
        assert_eq!(f32_to_f16(0.0), 0x0000);
        assert_eq!(f32_to_f16(-1.0), 0xBC00);
        // π (f32 厳密値 3.14159274101257…): keep=584, dropped=4059<4096 → 切捨て
        assert_eq!(f32_to_f16(std::f32::consts::PI), 0x4248);
    }

    /// wave 52 BB-1: RNE の tie/繰上げ規則を厳密 codeword でピン
    /// (期待値は Python Fraction による全演算厳密 RNE オラクルで導出)。
    #[test]
    fn rne_ties_and_roundups_exact() {
        // 1+2^-11: 0x3C00/0x3C01 のちょうど中点 → even mantissa 側 (下)
        assert_eq!(f32_to_f16(1.0 + p2(-11)), 0x3C00);
        // (1+2^-10)+2^-11: 0x3C01/0x3C02 の中点、keep 奇数 → 繰上げ
        assert_eq!(f32_to_f16(1.0 + p2(-10) + p2(-11)), 0x3C02);
        // 1+3·2^-12: 0.75 ulp → 繰上げ (旧 truncation 実装は 0x3C00 を返した)
        assert_eq!(f32_to_f16(1.0 + 3.0 * p2(-12)), 0x3C01);
        // 負側でも対称 (sign は独立処理)
        assert_eq!(f32_to_f16(-(1.0 + 3.0 * p2(-12))), 0xBC01);
    }

    /// wave 52 BB-1: overflow は RNE 自然境界 65520 (mantissa 桁上げで実現)。
    #[test]
    fn rne_overflow_boundary_exact() {
        assert_eq!(f32_to_f16(65504.0), 0x7BFF); // max finite
        assert_eq!(f32_to_f16(65519.0), 0x7BFF); // < 65520 → 65504 (dropped=3840<half)
        assert_eq!(f32_to_f16(65520.0), 0x7C00); // tie → even = Inf
        assert_eq!(f32_to_f16(65521.0), 0x7C00);
        assert_eq!(f32_to_f16(70000.0), 0x7C00);
        assert_eq!(f32_to_f16(-65520.0), 0xFC00);
        assert_eq!(f32_to_f16(f32::INFINITY), 0x7C00);
        assert_eq!(f32_to_f16(f32::NEG_INFINITY), 0xFC00);
    }

    /// wave 52 BB-1: FTZ は「subnormal グリッド込み RNE ののち flush」。
    #[test]
    fn ftz_subnormal_boundary_exact() {
        // flush 域: 最近接が subnormal (flush 対象) または 0
        assert_eq!(f32_to_f16(p2(-25)), 0x0000); // 0/最小sub の中点 tie → even 0
        assert_eq!(f32_to_f16(p2(-15)), 0x0000);
        assert_eq!(f32_to_f16(p2(-14) - p2(-24)), 0x0000); // 最頂 subnormal そのもの → flush
                                                           // 薄帯の丸め上げ: 2^-14-2^-25 は 0x03FF/0x0400 の中点 → even 側 0x0400
        assert_eq!(f32_to_f16(p2(-14) - p2(-25)), 0x0400);
        // 4095·2^-26 = 2^-14-2^-26 (中点より上) → min normal
        assert_eq!(f32_to_f16(4095.0 * p2(-26)), 0x0400);
        assert_eq!(f32_to_f16(p2(-14)), 0x0400); // min normal そのもの
                                                 // ゼロ符号の保存 (IEEE 上 ±0 は区別される)
        assert_eq!(f32_to_f16(-0.0), 0x8000);
        assert_eq!(f32_to_f16(-p2(-40)), 0x8000);
    }

    /// NaN は payload 非保持の正準 qNaN (sign 保存)。
    #[test]
    fn nan_canonicalized() {
        assert_eq!(f32_to_f16(f32::NAN), 0x7E00);
        assert_eq!(f32_to_f16(f32::from_bits(0xFFC0_0000)), 0xFE00); // 負 NaN
        assert_eq!(f32_to_f16(f32::from_bits(0x7F80_0001)), 0x7E00); // signaling → quiet
        assert!(f16_to_f32(0x7E00).is_nan());
    }

    /// decoder は全 65536 入力で厳密: 代表値の bit 完全一致ピン。
    #[test]
    fn decode_bit_exact_pins() {
        assert_eq!(f16_to_f32(0x3C00).to_bits(), 1.0f32.to_bits());
        assert_eq!(f16_to_f32(0x3800).to_bits(), 0.5f32.to_bits());
        assert_eq!(f16_to_f32(0x0001).to_bits(), 0x3380_0000); // 2^-24
        assert_eq!(f16_to_f32(0x0400).to_bits(), 0x3880_0000); // 2^-14
        assert_eq!(f16_to_f32(0x7BFF).to_bits(), 0x477F_E000); // 65504
        assert_eq!(f16_to_f32(0x7C00), f32::INFINITY);
        assert_eq!(f16_to_f32(0xFC00), f32::NEG_INFINITY);
        assert_eq!(f16_to_f32(0x7E00).to_bits(), 0x7FC0_0000);
        assert_eq!(f16_to_f32(0xFE00).to_bits(), 0xFFC0_0000);
        assert_eq!(f16_to_f32(0x0000).to_bits(), 0x0000_0000);
        assert_eq!(f16_to_f32(0x8000).to_bits(), 0x8000_0000); // -0.0
    }

    /// 全テーブル機械検証: decode の分類・単調性・再エンコード則を
    /// 65536 全 codeword で確認 (モジュール規約との整合を網羅的に)。
    #[test]
    fn full_table_semantics() {
        let mut prev = f64::NEG_INFINITY;
        for h in 0u32..=0xFFFF {
            let h = h as u16;
            let d = f16_to_f32(h);
            let exp = (h >> 10) & 0x1f; // 符号ビットを除いた exponent フィールド
            let mant = h & 0x3ff;
            let sign = u16::from(h >> 15 == 1);
            let re = f32_to_f16(d);
            match (exp, mant) {
                (0x1f, 0) => {
                    assert!(d.is_infinite(), "h={h:#06x}");
                    assert_eq!(re, h, "Inf は再エンコード恒等 h={h:#06x}");
                }
                (0x1f, _) => {
                    assert!(d.is_nan(), "h={h:#06x}");
                    assert_eq!(re, (sign << 15) | 0x7E00, "NaN は正準形へ h={h:#06x}");
                }
                (0, 0) => {
                    assert_eq!(d.to_bits(), (sign as u32) << 31, "±0 h={h:#06x}");
                    assert_eq!(re, h, "±0 は再エンコード恒等");
                }
                (0, _) => {
                    // subnormal decode は 0 < |d| < 2^-14 → 再エンコードは FTZ で ±0
                    assert!(d.abs() < p2(-14), "h={h:#06x}");
                    assert_eq!(re, sign << 15, "subnormal は ±0 へ h={h:#06x}");
                }
                _ => {
                    // normal: f32 に厳密表現可能 → 再エンコード恒等
                    assert_eq!(re, h, "normal 再エンコード恒等 h={h:#06x}");
                }
            }
            // decode は codeword 順 (0x0000..=0x7C00、有限+Inf で NaN 無し) で
            // Codeword 単調非減少 (負側は符号対称なので正側の検証で十分)。
            if h <= 0x7C00 {
                let cur = d as f64;
                assert!(cur >= prev, "decode 単調性 h={h:#06x}");
                prev = cur;
            }
        }
    }

    /// encode RNE 全範囲検証: 決定的 LCG で 2^18 個の f32 ビット列を生成し、
    /// テスト内独立オラクル (f64 厳密比較 + 全有限 f16 グリッド bisect) と
    /// codeword 一致を確認。f32/f16 値は f64 で厳密表現可能。tie 判定は
    /// 厳密等距離の丸めが同値に潰れるため安全、非 tie は f32 粒度 (≫ f64
    /// 丸め) の差があり判定を誤らない (W-3 規律)。
    #[test]
    fn encode_matches_exact_oracle_lcg_sweep() {
        // 全有限 f16 の厳密値テーブル (f64: 11bit significand で厳密)
        let grid: Vec<f64> = (0u32..0x7C00)
            .map(|cw| f16_to_f32(cw as u16) as f64)
            .collect();
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        for _ in 0..(1usize << 18) {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let bits = (state >> 32) as u32;
            let x = f32::from_bits(bits);
            if x.is_nan() {
                continue; // NaN は payload 非保持 (nan_canonicalized で規約ピン)
            }
            let enc = f32_to_f16(x);
            if x.is_infinite() {
                assert_eq!(enc, if x.signum() > 0.0 { 0x7C00 } else { 0xFC00 });
                continue;
            }
            let ax = (x as f64).abs();
            // オラクル: |x| ≥ 65520 → Inf (65520 は max finite/2^16 の tie で
            // even=Inf 側)。それ以下は有限グリッド両側の (距離, LSB odd) 辞書最小。
            let best_cw = if ax >= 65520.0 {
                0x7C00
            } else {
                let hi = grid.partition_point(|&g| g < ax);
                let mut best: (f64, u32) = (f64::INFINITY, 0);
                let upd = |best: &mut (f64, u32), gv: f64, cw: u32| {
                    let d = (gv - ax).abs();
                    if d < best.0 || (d == best.0 && (cw & 1) < (best.1 & 1)) {
                        *best = (d, cw);
                    }
                };
                if hi > 0 {
                    upd(&mut best, grid[hi - 1], (hi - 1) as u32);
                }
                if hi < grid.len() {
                    upd(&mut best, grid[hi], hi as u32);
                }
                best.1
            };
            // FTZ は符号合成前の magnitude codeword に適用 (subnormal 結果は
            // 符号付きゼロ)。合成後に判定すると符号ビットが exp 判定を潰す。
            let mut mag = best_cw;
            if (mag >> 10) == 0 && (mag & 0x3ff) != 0 {
                mag = 0;
            }
            let expected = (u32::from(x.is_sign_negative()) << 15) | mag;
            assert_eq!(
                u32::from(enc),
                expected,
                "x={x:e} bits={bits:#010x}: got {enc:#06x} want {expected:#06x}"
            );
        }
    }

    #[test]
    fn quantize_and_layout_exact() {
        let v = quantize_vec3(Vec3 {
            x: 1.0,
            y: -1.0,
            z: 0.5,
        });
        assert_eq!(v, [0x3C00, 0xBC00, 0x3800]);
        let u = quantize_vec2(Vec2 { x: 2.0, y: 0.0 });
        assert_eq!(u, [0x4000, 0x0000]);
        assert_eq!(bandwidth_saved(8), 16);
        assert_eq!(bandwidth_saved(0), 0);
        assert_eq!(packed_stride(5), 22);
        assert_eq!(packed_stride(0), 12);
    }
}

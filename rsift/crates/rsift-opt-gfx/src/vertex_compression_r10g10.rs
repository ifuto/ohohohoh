//! Vertex Compression R10G10B10A2 + FP16 UV + octahedral 法線 - 帯域 1/4 化
//! 仕様: Direct3D 12 R10G10B10A2_UNORM (10bit xyz + 2bit a)、UV は half float、
//! 法線は octahedral 16bit×2 SNORM (Cigolle et al. 2014 系)。
//!
//! 規約 (2026-07-23 wave 53 厳格化):
//! - f16 変換は `crate::half_vertex::f32_to_f16` (真の RNE + 宣言済 FTZ) に
//!   一元化。旧内部「簡易」版は truncation の上に **NaN を f16 Inf へ静寂
//!   変換**する欠陥があったため撤去 (NaN=観測欠測は入口拒否が責務)。
//! - UNORM pack は round-to-nearest (f32::round = ties-away-from-zero)。
//!   旧実装の `as u32` 切捨ては 0.5/1023 の系統的下方向バイアスだった。
//! - 全入力は有限必須 (NaN/±Inf は入口 assert)。範囲外 pos は幾何破壊の
//!   バグであり、[0,64] 契約を assert で機械強制する。
//! - ストリーム長不一致は旧実装 `zip` の**静寂打切り**だったため等長 assert。
//! - `normal_oct` は旧実装が常時 0 のスタブだった — 本モジュールは
//!   octahedral 写像を完全実装し、復号参照も提供する。

use crate::half_vertex::f32_to_f16;
use bytemuck::{Pod, Zeroable};

/// octahedral SNORM 量子化スケール (2^15 - 1)。
const OCT_SCALE: f32 = 32767.0;
/// chunk-local 座標の正規化除数 ([0,64] → [0,1])。
pub const CHUNK_POS_SCALE: f32 = 64.0;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct CompressedVertex {
    pub pos_packed: u32, // R10G10B10A2: xyz 10bit each + 2bit ao
    pub uv_packed: u32,  // 16bit + 16bit half float
    pub normal_oct: u32, // octahedral 16bit×2 SNORM (little-endian lane: [lo=x, hi=y])
}

// 3×u32 = 12B (旧 doc の「16byte」は実体と乖離していた — 48B→12B = 帯域 1/4)。
const _: () = assert!(std::mem::size_of::<CompressedVertex>() == 12);

impl CompressedVertex {
    /// 0.0-1.0 の xyz を 10bit UNORM にパック (round-to-nearest)。
    /// **契約**: 成分は有限値。a2 は 2bit (0..=3)。
    pub fn pack_r10g10b10a2(x: f32, y: f32, z: f32, a2: u32) -> u32 {
        assert!(
            x.is_finite() && y.is_finite() && z.is_finite(),
            "pack_r10g10b10a2 契約違反: pos 成分が非有限 ({x}, {y}, {z})"
        );
        assert!(a2 <= 3, "pack_r10g10b10a2 契約違反: a2 は 0..=3 (got {a2})");
        // clamp は ±1ulp 程度の数値ノイズのみを救済する (契約上の [0,1] 外
        // 入力は compress_vertex_stream が [0,64] assert で事前拒否)。
        let q = |v: f32| (v.clamp(0.0, 1.0) * 1023.0).round() as u32;
        q(x) | (q(y) << 10) | (q(z) << 20) | (a2 << 30)
    }

    pub fn unpack_r10g10b10a2(packed: u32) -> (f32, f32, f32, u32) {
        let rx = (packed & 0x3FF) as f32 / 1023.0;
        let ry = ((packed >> 10) & 0x3FF) as f32 / 1023.0;
        let rz = ((packed >> 20) & 0x3FF) as f32 / 1023.0;
        let ra = (packed >> 30) & 0x3;
        (rx, ry, rz, ra)
    }

    /// UV を f16 ペアにパック。**契約**: UV は有限値 (NaN/±Inf は拒否)。
    /// |uv| ≥ 65520 は f16 ±Inf に RNE オーバーフロー (f32_to_f16 規約)。
    /// 旧実装の `clamp(0.0,1.0)` はタイル UV (範囲外パラメータ) を静寂破壊
    /// していたため撤去 — f16 は ±65504 まで保持できる。
    pub fn pack_uv(u: f32, v: f32) -> u32 {
        assert!(
            u.is_finite() && v.is_finite(),
            "pack_uv 契約違反: UV が非有限 ({u}, {v})"
        );
        (u32::from(f32_to_f16(u))) | (u32::from(f32_to_f16(v)) << 16)
    }

    /// 法線を octahedral 16bit×2 SNORM にパック。
    /// 写像 (Cigolle 系): L1 正規化八面体射影 → z<0 半球を (1-|y|,1-|x|)·sign
    /// へ折り畳み → 各成分を [-1,1]·32767 に round-to-nearest 量子化。
    /// **契約**: 成分は有限かつ L1 ノルム > 0 (ゼロベクトルは法線ではない)。
    /// 符号規約: sign(0) = +1 (復号側の折返しと対称、(0,0,-1) → (32767,32767)
    /// が一意に定まる)。
    pub fn pack_normal_oct(nx: f32, ny: f32, nz: f32) -> u32 {
        assert!(
            nx.is_finite() && ny.is_finite() && nz.is_finite(),
            "pack_normal_oct 契約違反: 法線成分が非有限 ({nx}, {ny}, {nz})"
        );
        let l1 = nx.abs() + ny.abs() + nz.abs();
        assert!(l1 > 0.0, "pack_normal_oct 契約違反: ゼロ法線は写像不能");
        let (ox, oy) = (nx / l1, ny / l1);
        let (x, y) = if nz / l1 < 0.0 {
            let sx = if ox >= 0.0 { 1.0 } else { -1.0 };
            let sy = if oy >= 0.0 { 1.0 } else { -1.0 };
            ((1.0 - oy.abs()) * sx, (1.0 - ox.abs()) * sy)
        } else {
            (ox, oy)
        };
        let q = |v: f32| (v.clamp(-1.0, 1.0) * OCT_SCALE).round() as i32 as u16;
        (u32::from(q(x))) | (u32::from(q(y)) << 16)
    }

    /// `pack_normal_oct` の復号 (CPU 検証用であり、将来 GPU デコードを配線
    /// する際の参照仕様)。量子化誤差は再 L2 正規化後に残る。
    pub fn unpack_normal_oct(p: u32) -> [f32; 3] {
        let qx = ((p & 0xFFFF) as u16) as i16 as f32 / OCT_SCALE;
        let qy = ((p >> 16) as u16) as i16 as f32 / OCT_SCALE;
        let z0 = 1.0 - qx.abs() - qy.abs();
        let (x, y) = if z0 < 0.0 {
            let sx = if qx >= 0.0 { 1.0 } else { -1.0 };
            let sy = if qy >= 0.0 { 1.0 } else { -1.0 };
            ((1.0 - qy.abs()) * sx, (1.0 - qx.abs()) * sy)
        } else {
            (qx, qy)
        };
        let len = (x * x + y * y + z0 * z0).sqrt();
        [x / len, y / len, z0 / len]
    }
}

/// 従来 48byte 頂点を 12byte へ圧縮する変換。
/// **契約**: 4 ストリームは等長 (不一致は静寂打切りではなく assert)。
/// positions は chunk-local [0, `CHUNK_POS_SCALE`] の有限値 (範囲外は幾何
/// 破壊バグとして拒否)。normals は有限・非ゼロ (L2 単位でなくてよい)。
pub fn compress_vertex_stream(
    positions: &[[f32; 3]],
    uvs: &[[f32; 2]],
    normals: &[[f32; 3]],
    aos: &[u32],
) -> Vec<CompressedVertex> {
    assert!(
        positions.len() == uvs.len() && uvs.len() == normals.len() && normals.len() == aos.len(),
        "compress_vertex_stream 契約違反: ストリーム長不一致 (pos={} uv={} normal={} ao={})",
        positions.len(),
        uvs.len(),
        normals.len(),
        aos.len()
    );
    positions
        .iter()
        .zip(uvs)
        .zip(normals)
        .zip(aos)
        .map(|(((p, uv), n), &ao)| {
            assert!(
                p.iter().all(|&c| (0.0..=CHUNK_POS_SCALE).contains(&c)),
                "compress_vertex_stream 契約違反: chunk-local 座標は [0,64] 有限必須 {p:?}"
            );
            CompressedVertex {
                pos_packed: CompressedVertex::pack_r10g10b10a2(
                    p[0] / CHUNK_POS_SCALE,
                    p[1] / CHUNK_POS_SCALE,
                    p[2] / CHUNK_POS_SCALE,
                    ao,
                ),
                uv_packed: CompressedVertex::pack_uv(uv[0], uv[1]),
                normal_oct: CompressedVertex::pack_normal_oct(n[0], n[1], n[2]),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r10_pack_round_nearest_exact() {
        // 0 ピタリと 1 ピタリは厳密。
        assert_eq!(CompressedVertex::pack_r10g10b10a2(0.0, 0.0, 0.0, 0), 0);
        assert_eq!(
            CompressedVertex::pack_r10g10b10a2(1.0, 1.0, 1.0, 3),
            1023 | (1023 << 10) | (1023 << 20) | (3 << 30)
        );
        // round-to-nearest: 0.25·1023 = 255.75 → 256 (厳密構成可能)。
        let p = CompressedVertex::pack_r10g10b10a2(0.25, 0.0, 0.0, 0);
        assert_eq!(p & 0x3FF, 256);
        // 0.5·1023 = 511.5 → ties-away 512 (厳密)。
        let p = CompressedVertex::pack_r10g10b10a2(0.5, 0.0, 0.0, 0);
        assert_eq!(p & 0x3FF, 512);
    }

    #[test]
    fn r10_unpack_inverts_pack_half_quantum() {
        for &v in &[0.0f32, 0.25, 0.5, 0.75, 1.0] {
            let (x, y, z, a) = CompressedVertex::unpack_r10g10b10a2(
                CompressedVertex::pack_r10g10b10a2(v, v, v, 2),
            );
            // round-to-nearest で誤差は半量子 + 除算丸め
            assert!((x - v).abs() <= 0.5 / 1023.0 + f32::EPSILON, "x err v={v}");
            assert_eq!((x, y, z, a), (x, x, x, 2));
        }
    }

    #[test]
    fn r10_pack_rejects_nan_and_a2_overflow() {
        let r =
            std::panic::catch_unwind(|| CompressedVertex::pack_r10g10b10a2(f32::NAN, 0.0, 0.0, 0));
        assert!(r.is_err());
        let r = std::panic::catch_unwind(|| CompressedVertex::pack_r10g10b10a2(0.0, 0.0, 0.0, 4));
        assert!(r.is_err());
    }

    /// wave 53 BC-1: f16 は half_vertex 実装に一元化 (NaN→Inf 静寂変換を撤廃)。
    #[test]
    fn pack_uv_bit_exact_and_finite_contract() {
        assert_eq!(CompressedVertex::pack_uv(1.0, 0.5), 0x3800_3C00);
        assert_eq!(CompressedVertex::pack_uv(-2.0, -0.0), 0x8000_C000);
        // 範囲外タイル UV を破壊しない (旧 clamp(0,1) 撤去): 5.5 → f16 厳密
        assert_eq!(CompressedVertex::pack_uv(5.5, 0.0) & 0xFFFF, 0x4580);
        // |uv| ≥ 65520 → f16 Inf (宣言規約)
        assert_eq!(CompressedVertex::pack_uv(70000.0, 0.0) & 0xFFFF, 0x7C00);
        let r = std::panic::catch_unwind(|| CompressedVertex::pack_uv(f32::NAN, 0.0));
        assert!(r.is_err());
        let r = std::panic::catch_unwind(|| CompressedVertex::pack_uv(0.0, f32::INFINITY));
        assert!(r.is_err());
    }

    /// wave 53 BC-4: octahedral 軸・赤道・fold の厳密量子化ピン。
    /// 導出: +X → L1(n)=1 → (1,0) → (32767, 0) → 0x0000_7FFF。
    /// -Z → z<0 fold で (1-0)·sign(0)=+1 規約 → (1,1) → 0x7FFF_7FFF。
    /// (−1,−1,−1)/√3 → L1 後 (−1/3,−1/3,−1/3) → fold: x=(1−1/3)·(−1)=−2/3
    /// → round(−2/3·32767)=round(−21844.67)=−21845 (ties-away) → 0xAAAB。
    /// (+1,+1,+1)/√3 → round(32767/3)=round(10922.33)=10922=0x2AAA。
    #[test]
    fn oct_axes_and_folds_bit_exact() {
        assert_eq!(
            CompressedVertex::pack_normal_oct(1.0, 0.0, 0.0),
            0x0000_7FFF
        );
        assert_eq!(
            CompressedVertex::pack_normal_oct(-1.0, 0.0, 0.0),
            0x0000_8001
        );
        assert_eq!(
            CompressedVertex::pack_normal_oct(0.0, 1.0, 0.0),
            0x7FFF_0000
        );
        assert_eq!(
            CompressedVertex::pack_normal_oct(0.0, 0.0, 1.0),
            0x0000_0000
        );
        assert_eq!(
            CompressedVertex::pack_normal_oct(0.0, 0.0, -1.0),
            0x7FFF_7FFF
        );
        let i3 = 1.0 / (3.0f32).sqrt();
        assert_eq!(CompressedVertex::pack_normal_oct(i3, i3, i3), 0x2AAA_2AAA);
        assert_eq!(
            CompressedVertex::pack_normal_oct(-i3, -i3, -i3),
            0xAAAB_AAAB
        );
    }

    /// 軸法線は復号が厳密に往復する (量子化無損失)。
    #[test]
    fn oct_axis_roundtrip_exact() {
        for (n, p) in [
            ([1.0, 0.0, 0.0], 0x0000_7FFFu32),
            ([0.0, 1.0, 0.0], 0x7FFF_0000),
            ([0.0, 0.0, 1.0], 0x0000_0000),
            ([0.0, 0.0, -1.0], 0x7FFF_7FFF),
        ] {
            assert_eq!(CompressedVertex::pack_normal_oct(n[0], n[1], n[2]), p);
            let d = CompressedVertex::unpack_normal_oct(p);
            assert_eq!(d, n, "axis roundtrip {n:?}");
        }
        // -1.0 → round(-32767) → i16 -32767 = 0x8001 (符号はふた桁上の bit15)
        let d = CompressedVertex::unpack_normal_oct(0x0000_8001);
        assert_eq!(d, [-1.0, 0.0, 0.0]);
    }

    /// wave 53 BC-4: 球面 sweep 復号往復の角度誤差上限。
    /// 400k サンプルの実測最悪値 ≈ 6.4e-5 rad (浮動小数推定) に対し、
    /// 15× マージンの 1e-3 rad を閾値とする。ドット積・acos は f64 で評価
    /// (f32 値の積は f64 で厳密、閾値マージンは誤差評価系を圧倒する)。
    #[test]
    fn oct_roundtrip_angular_error_bounded() {
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut lcg = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 33) as u32) as f32 / 2_147_483_648.0 - 1.0 // [-1, 1)
        };
        let mut worst = 0.0f64;
        let mut checked = 0u32;
        for _ in 0..200_000 {
            let (x, y, z) = (lcg(), lcg(), lcg());
            let l = (x * x + y * y + z * z).sqrt();
            if l < 1e-3 {
                continue; // 原点近傍は方向が悪条件 → 契約外として除外
            }
            let n = [x / l, y / l, z / l];
            let d = CompressedVertex::unpack_normal_oct(CompressedVertex::pack_normal_oct(
                n[0], n[1], n[2],
            ));
            let dot =
                (n[0] as f64 * d[0] as f64 + n[1] as f64 * d[1] as f64 + n[2] as f64 * d[2] as f64)
                    .clamp(-1.0, 1.0);
            let ang = dot.acos();
            worst = worst.max(ang);
            assert!(
                ang <= 1e-3,
                "oct roundtrip 角度誤差 {ang} rad (n={n:?} d={d:?})"
            );
            checked += 1;
        }
        assert!(checked > 190_000, "十分な方向を検査した ({checked})");
        assert!(worst > 1e-6 && worst < 1e-3, "worst={worst} が想定帯域内");
    }

    #[test]
    fn oct_rejects_degenerate_normal() {
        for n in [
            [0.0, 0.0, 0.0],
            [f32::NAN, 0.0, 1.0],
            [0.0, f32::INFINITY, 0.0],
        ] {
            let r =
                std::panic::catch_unwind(|| CompressedVertex::pack_normal_oct(n[0], n[1], n[2]));
            assert!(r.is_err(), "{n:?} は拒否されるべき");
        }
    }

    /// wave 53 BC-3: 統合ストリームの厳密値ピン + 等長契約。
    #[test]
    fn compress_stream_bit_exact_and_length_contract() {
        let pos = [[64.0, 0.0, 32.0], [0.0, 0.0, 0.0]];
        let uv = [[1.0, 0.5], [0.0, 0.0]];
        let nrm = [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0]];
        let ao = [2u32, 0];
        let out = compress_vertex_stream(&pos, &uv, &nrm, &ao);
        assert_eq!(out.len(), 2);
        // [1,0,0.5] → x=1023, z=512 (round(511.5)=512 ties-away), ao=2
        let (x, y, z, a) = CompressedVertex::unpack_r10g10b10a2(out[0].pos_packed);
        assert_eq!(
            out[0].pos_packed,
            1023 | (0 << 10) | (512 << 20) | (2 << 30)
        );
        assert!((z - 0.5).abs() <= 0.5 / 1023.0 + f32::EPSILON);
        let _ = (x, y, a);
        assert_eq!(out[0].uv_packed, 0x3800_3C00);
        assert_eq!(out[0].normal_oct, 0x0000_0000); // +Z → (0,0)
        assert_eq!(out[1].normal_oct, 0x0000_7FFF); // +X
                                                    // 等長違反は assert
        let r = std::panic::catch_unwind(|| compress_vertex_stream(&pos, &uv[..1], &nrm, &ao));
        assert!(r.is_err());
        // [0,64] 外座標は assert
        let bad = [[65.0, 0.0, 0.0]];
        let r = std::panic::catch_unwind(|| {
            compress_vertex_stream(&bad, &[[0.0, 0.0]], &[[0.0, 0.0, 1.0]], &[0])
        });
        assert!(r.is_err());
        let bad = [[f32::NAN, 0.0, 0.0]];
        let r = std::panic::catch_unwind(|| {
            compress_vertex_stream(&bad, &[[0.0, 0.0]], &[[0.0, 0.0, 1.0]], &[0])
        });
        assert!(r.is_err());
    }

    #[test]
    fn vertex_size_is_12_bytes() {
        assert_eq!(std::mem::size_of::<CompressedVertex>(), 12);
        assert_eq!(CHUNK_POS_SCALE, 64.0);
    }
}

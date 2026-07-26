//! HDR bloom for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): a soft-knee brightness prefilter, a separable
//! Gaussian blur step, and a final additive composite. Bloom only *adds*
//! light where it already exists, so it is a quality improvement with no
//! resolution change — safe for the low-spec path (the blur can also run at a
//! fraction of resolution to save bandwidth).
//!
//! ## 主要消費者の実効挙動 (監査 2026-07-26 DM-2)
//! full_graph_wiring:1671 は `prefilter(mapped, threshold=1.0, knee=0.5)` を
//! **tonemap_display 後の値** (linear_to_srgb が `x >= 1.0 → 1.0` に clamp
//! するため mapped ∈ [0,1]³) に適用する。luma ≤ 0.2126+0.7152+0.0722 = 1.0
//! (= threshold、等号は全チャンネル 1.0 のみ) なのでゲート `l <= threshold`
//! は**常に真 → bloom ≡ 0**、composite は (m + 0).clamp(0,64) = m の
//! **bit 厳密な恒等写像**となる (strict pin 済)。閾値の再調整 (例: 0.7/
//! knee 0.3 への変更で bloom を実効化) はレンダ結果が変わる美的判断のため
//! ユーザー設計領域として registry 引継ぎとする。

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
    pub fn clamp(self, lo: f32, hi: f32) -> Vec3 {
        Vec3::new(
            self.x.clamp(lo, hi),
            self.y.clamp(lo, hi),
            self.z.clamp(lo, hi),
        )
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

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}
impl Vec4 {
    pub fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }
}
impl Add for Vec4 {
    type Output = Vec4;
    fn add(self, o: Vec4) -> Vec4 {
        Vec4::new(self.x + o.x, self.y + o.y, self.z + o.z, self.w + o.w)
    }
}
impl Sub for Vec4 {
    type Output = Vec4;
    fn sub(self, o: Vec4) -> Vec4 {
        Vec4::new(self.x - o.x, self.y - o.y, self.z - o.z, self.w - o.w)
    }
}
impl Mul<f32> for Vec4 {
    type Output = Vec4;
    fn mul(self, s: f32) -> Vec4 {
        Vec4::new(self.x * s, self.y * s, self.z * s, self.w * s)
    }
}

/// Luminance of a colour (Rec.709: 0.2126/0.7152/0.0722 の乗加順固定)。
/// frame_postfx::luma_run_cpu・exposure の内蔵式と同一次情報・同一演算順で
/// **bit 一致** (監査 2026-07-26 DM-4 で 3 系統相互 pin)。
pub fn luma(c: Vec3) -> f32 {
    0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z
}

/// Soft-knee prefilter: keep energy above `threshold`, rolling off over
/// `knee` to avoid a hard cutoff.
///
/// ## 契約 (監査 2026-07-26 DM-1)
/// * ゲート: `l <= threshold` → 0 (knee>0 でも境界連続、l=threshold は 0)。
/// * ゲイン f = `(l - threshold) / threshold.max(1e-4)` は**相対ゲイン**で、
///   出力輝度は l·f。l > 2·threshold で**入力超過の増幅**となる
///   (例: T=1, l=4 → f=3 → 出力 12、3 倍)。これは意図的な強め bloom 規約で、
///   luma 保存形 (c·(l−T)/l、増幅しない標準形) とは異なる。
/// * `threshold.max(1e-4)` の床により、threshold ≤ 1e-4 では発散級
///   (T=0.001, l=1 → f≈999)。小閾値は composite の 64 clamp で潰れるため、
///   呼出側契約: threshold ≫ 1e-4 の健全値のみ渡すこと。
/// * NaN/inf 入力は**伝播** (ゲートの NaN 比較は false → 後段で NaN 演算)。
pub fn prefilter(c: Vec3, threshold: f32, knee: f32) -> Vec3 {
    let l = luma(c);
    if l <= threshold {
        return Vec3::new(0.0, 0.0, 0.0);
    }
    let mut f = (l - threshold) / threshold.max(1e-4);
    // soft knee: smoothstep over [0, knee]
    if knee > 0.0 {
        let t = (l - threshold) / knee;
        let soft = t.clamp(0.0, 1.0);
        f = f * (soft * soft * (3.0 - 2.0 * soft));
    }
    c * f
}

/// 1D Gaussian blur of a row using 5 taps (weights normalized).
///
/// ## 契約 (監査 2026-07-26 DM-3)
/// * 重みは二項核 [1,4,6,4,1]/16 = [0.0625, 0.25, 0.375, 0.25, 0.0625] で
///   **全て二進厳密値**、和は f32 で厳密に 1.0 → 定数入力の保存は
///   1 ulp 誤差もない **bit 厳密** (strict pin 済)。
/// * 端は edge-clamp (j を 0..=n-1 に clamp)。radius=0 は恒等写像 (bit 厳密)。
/// * `dst.len() < src.len()` は **panic** (fail-loud、should_panic pin 済)。
pub fn blur_row(src: &[Vec3], dst: &mut [Vec3], radius: usize) {
    let w = [0.0625f32, 0.25, 0.375, 0.25, 0.0625];
    let offs = [-2, -1, 0, 1, 2];
    let n = src.len();
    for i in 0..n {
        let mut acc = Vec3::new(0.0, 0.0, 0.0);
        for k in 0..5 {
            let j = (i as isize + offs[k] * radius as isize).clamp(0, n as isize - 1) as usize;
            acc = acc + src[j] * w[k];
        }
        dst[i] = acc;
    }
}

/// Additively composite bloom over the scene.
///
/// ## 契約 (監査 2026-07-26 DM-5)
/// * HDR 上限 64.0 に clamp、負値は 0 側。NaN は**伝播** (f32 比較が false で
///   self 返却、0 側への静寂崩落ではない = fail-visible)。
pub fn composite(scene: Vec3, bloom: Vec3, intensity: f32) -> Vec3 {
    (scene + bloom * intensity).clamp(0.0, 64.0)
}

pub fn wgsl_source() -> &'static str {
    BLOOM_WGSL
}

pub const BLOOM_WGSL: &str = include_str!("../shaders/bloom.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prefilter_keeps_bright() {
        let c = Vec3::new(2.0, 2.0, 2.0); // luma 2
        let p = prefilter(c, 1.0, 0.0);
        assert!(luma(p) > 0.0, "bright pixels pass prefilter");
    }
    #[test]
    fn prefilter_drops_dark() {
        let c = Vec3::new(0.2, 0.2, 0.2);
        let p = prefilter(c, 1.0, 0.0);
        assert_eq!(luma(p), 0.0);
    }
    #[test]
    fn blur_of_constant_is_constant() {
        let src: Vec<Vec3> = (0..16).map(|_| Vec3::new(1.0, 2.0, 3.0)).collect();
        let mut dst = vec![Vec3::default(); 16];
        blur_row(&src, &mut dst, 1);
        for d in &dst {
            assert!((d.x - 1.0).abs() < 1e-6 && (d.y - 2.0).abs() < 1e-6);
        }
    }
    #[test]
    fn composite_adds_energy() {
        let s = Vec3::new(0.5, 0.5, 0.5);
        let b = Vec3::new(0.2, 0.0, 0.0);
        let c = composite(s, b, 1.0);
        assert!((c.x - 0.7).abs() < 1e-6);
    }

    // ------------------------------------------------------------ 監査 2026-07-26 DM追加分

    /// DM-1: prefilter の相対ゲイン意味論 pin。
    /// l = 2·threshold では f=1 で**bit 厳密に恒等**、l = 4·T では 3 倍増幅、
    /// 小閾値 (T=0.001) では f≈999 の発散級 (全て Python 機械検算と一致)。
    #[test]
    fn prefilter_relative_gain_semantics_are_exact() {
        // l = 2T: (2-1)/1 = 1 → 厳密恒等。
        let c = Vec3::new(2.0, 2.0, 2.0);
        let p = prefilter(c, 1.0, 0.0);
        assert_eq!(p.x.to_bits(), c.x.to_bits(), "l=2T で bit 恒等");
        // l = 4T: f=3 → 出力 12 = 入力の 3 倍 (増幅規約の証拠)。
        let c = Vec3::new(4.0, 4.0, 4.0);
        let p = prefilter(c, 1.0, 0.0);
        assert_eq!(p.x, 12.0, "l=4T で f=3 の増幅");
        // 小閾値: T=0.001, l=1 → f = 0.999/0.001 ≈ 999 (床 1e-4 は不発)。
        let c = Vec3::new(1.0, 1.0, 1.0);
        let p = prefilter(c, 0.001, 0.0);
        assert!(
            (luma(p) - 999.0).abs() < 1.0,
            "小閾値は発散級増幅: {}",
            luma(p)
        );
        // l = threshold 境界は 0 (ゲート・knee 無関係に連続)。
        assert_eq!(luma(prefilter(Vec3::new(1.0, 1.0, 1.0), 1.0, 0.0)), 0.0);
        assert_eq!(luma(prefilter(Vec3::new(1.0, 1.0, 1.0), 1.0, 0.5)), 0.0);
        // knee 帯中点の厳密値: l=1.5, T=1, knee=1 → f=0.5、t=0.5、
        // smoothstep(0.5)=0.5 → 最終 f=0.25 → 出力 0.375 (adversarial (c) で
        // 本 pin が無いと knee 乗算除去を検出できないことを確認し追加)。
        let mid = prefilter(Vec3::new(1.5, 1.5, 1.5), 1.0, 1.0);
        assert_eq!(mid.x, 0.375, "knee 帯中点 smoothstep 厳密値");
    }

    /// DM-2: wiring の bloom 段は数学的に恒等 — tonemap_display 後
    /// (mapped ∈ [0,1]³、luma ≤ 1.0 = threshold) では prefilter ≡ 0、
    /// composite = (m+0).clamp(0,64) ≡ m。729 点グリッド+境界で bit 厳密 pin。
    #[test]
    fn wiring_bloom_stage_is_provably_identity_for_01_inputs() {
        let mut checked = 0u32;
        for ix in 0..=8 {
            for iy in 0..=8 {
                for iz in 0..=8 {
                    let m =
                        crate::bloom::Vec3::new(ix as f32 / 8.0, iy as f32 / 8.0, iz as f32 / 8.0);
                    let out = composite(m, prefilter(m, 1.0, 0.5), 0.08);
                    assert_eq!(out.x.to_bits(), m.x.to_bits(), "x 非恒等");
                    assert_eq!(out.y.to_bits(), m.y.to_bits(), "y 非恒等");
                    assert_eq!(out.z.to_bits(), m.z.to_bits(), "z 非恒等");
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 729);
        // luma = 1.0 丁度 (全 1) もゲート closed 側 (l <= threshold) で 0。
        let w = Vec3::new(1.0, 1.0, 1.0);
        assert_eq!(luma(prefilter(w, 1.0, 0.5)), 0.0);
    }

    /// DM-3: blur の重みは全て二進厳密で和は厳密 1.0、定数保存は bit 厳密、
    /// radius=0 は bit 厳密な恒等。
    #[test]
    fn blur_weights_are_dyadic_and_preservation_is_bit_exact() {
        let wsum = 0.0625f32 + 0.25 + 0.375 + 0.25 + 0.0625;
        assert_eq!(wsum.to_bits(), 1.0f32.to_bits(), "重み和は二進厳密に 1.0");
        let v = Vec3::new(1.0, 2.0, 3.0);
        let src: Vec<Vec3> = (0..16).map(|_| v).collect();
        let mut dst = vec![Vec3::default(); 16];
        blur_row(&src, &mut dst, 1);
        for d in &dst {
            assert_eq!(d.x.to_bits(), v.x.to_bits(), "定数保存 (x) は bit 厳密");
            assert_eq!(d.z.to_bits(), v.z.to_bits(), "定数保存 (z) は bit 厳密");
        }
        // radius=0 は恒等 (offs 全 0 → acc = src[i]·Σw = src[i] bit 厳密)。
        let mut dst2 = vec![Vec3::default(); 16];
        blur_row(&src, &mut dst2, 0);
        for (a, b) in src.iter().zip(dst2.iter()) {
            assert_eq!(a.y.to_bits(), b.y.to_bits());
        }
        // 空スライスは no-op (panic なし)。
        let mut none: Vec<Vec3> = Vec::new();
        blur_row(&[], &mut none, 1);
        assert!(none.is_empty());
    }

    /// DM-3: dst<src は panic (fail-loud 契約)。
    #[test]
    #[should_panic]
    fn blur_row_panics_when_dst_shorter_than_src() {
        let src: Vec<Vec3> = (0..4).map(|_| Vec3::new(1.0, 1.0, 1.0)).collect();
        let mut dst = vec![Vec3::default(); 2];
        blur_row(&src, &mut dst, 1);
    }

    /// DM-4: luma 3 系統 (bloom::luma / frame_postfx::luma_run_cpu / 内蔵式)
    /// の bit 一致 pin (同一乗加順のため IEEE 厳密等価)。
    #[test]
    fn luma_three_systems_are_bit_identical() {
        let mut s = 0x243F_6A88u64;
        let mut rnd = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        for _ in 0..256 {
            let mut f = |sh: u32| ((rnd() >> sh) & 0xFFFF) as f32 / 65536.0;
            let (x, y, z) = (f(0), f(21), f(43));
            let a = luma(Vec3::new(x, y, z));
            let b = crate::frame_postfx::luma_run_cpu(&[[x, y, z, 1.0]])[0];
            let c_ = 0.2126 * x + 0.7152 * y + 0.0722 * z;
            assert_eq!(a.to_bits(), b.to_bits(), "frame_postfx との不一致");
            assert_eq!(a.to_bits(), c_.to_bits(), "内蔵式との不一致");
        }
    }

    /// DM-5: composite/clamp 契約 (64 上限・負 0 側・NaN 伝播) と
    /// prefilter の NaN 伝播 pin。
    #[test]
    fn composite_and_nan_contract() {
        let over = composite(Vec3::new(70.0, -1.0, 0.5), Vec3::new(0.0, 0.0, 0.0), 1.0);
        assert_eq!(over.x, 64.0, "HDR 上限 64 clamp");
        assert_eq!(over.y, 0.0, "負値は 0 側");
        let nan_out = composite(Vec3::new(f32::NAN, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.0), 1.0);
        assert!(nan_out.x.is_nan(), "NaN は伝播 (崩落しない)");
        let nan_pre = prefilter(Vec3::new(f32::NAN, 0.0, 0.0), 1.0, 0.5);
        assert!(nan_pre.x.is_nan(), "prefilter の NaN も伝播");
    }
}

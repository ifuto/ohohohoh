//! Checkerboard rendering — render only half the pixels (a 2x2 checkerboard
//! mask) at full shading rate, then reconstruct the missing pixels from the
//! four already-rendered diagonal neighbours.
//!
//! This halves the number of shaded pixels, which is a direct, large win on
//! integrated GPUs. Reconstruction is a cheap bilinear gather.

#[derive(Debug, Clone, Copy)]
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
impl std::ops::Add for Vec3 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl std::ops::Sub for Vec3 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl std::ops::Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s)
    }
}

pub struct Checkerboard;
impl Checkerboard {
    /// True if this pixel is shaded this frame; the complementary half is
    /// reconstructed afterwards.
    ///
    /// **wave 67 BQ-1**: `wrapping_add` を採用。パリティは mod 2 の性質なので
    /// u32 wrap 周回 (`(x+y) mod 2^32` と `x+y` は mod 2 で同値) を経ても
    /// 結果は不変であり、WGSL (`(gid.x + gid.y) & 1u` — u32 加算は wrap
    /// セマンティクス) と debug/release 双方で厳密一致する (旧実装の素朴な
    /// `x + y` は debug ビルドで u32::MAX 級の座標を渡すと overflow panic
    /// し得た — ピクセル座標契約外だが fail-loud ではなく単純一致が正しい)。
    /// また `(x + y) & 1 == (x ^ y) & 1` は常に真 (bit0 の和は XOR に一致、
    /// 桁上がりは bit1 以上にしか寄与しない)。
    #[inline]
    pub fn is_rendered(x: u32, y: u32) -> bool {
        x.wrapping_add(y) & 1 == 0
    }

    /// Bilinear reconstruction of a missing pixel from its four rendered
    /// diagonal neighbours (NW, NE, SW, SE).
    pub fn reconstruct(nw: Vec3, ne: Vec3, sw: Vec3, se: Vec3) -> Vec3 {
        (nw + ne + sw + se) * 0.25
    }

    pub fn wgsl_source(&self) -> &'static str {
        CHECKERBOARD_WGSL
    }
}

pub const CHECKERBOARD_WGSL: &str = include_str!("../shaders/checkerboard.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_alternates() {
        assert!(Checkerboard::is_rendered(0, 0));
        assert!(!Checkerboard::is_rendered(1, 0));
        assert!(!Checkerboard::is_rendered(0, 1));
        assert!(Checkerboard::is_rendered(1, 1));
    }

    #[test]
    fn reconstruct_averages_neighbors() {
        let nw = Vec3::new(1.0, 0.0, 0.0);
        let ne = Vec3::new(0.0, 1.0, 0.0);
        let sw = Vec3::new(0.0, 0.0, 1.0);
        let se = Vec3::new(1.0, 1.0, 1.0);
        let r = Checkerboard::reconstruct(nw, ne, sw, se);
        assert!((r.x - 0.5).abs() < 1e-6);
        assert!((r.y - 0.5).abs() < 1e-6);
        assert!((r.z - 0.5).abs() < 1e-6);
    }

    #[test]
    fn half_pixels_rendered_in_2x2() {
        let mut rendered = 0u32;
        for y in 0..2u32 {
            for x in 0..2u32 {
                if Checkerboard::is_rendered(x, y) {
                    rendered += 1;
                }
            }
        }
        assert_eq!(rendered, 2);
    }

    /// wave 67 BQ-1: パリティ規則の厳密性質を全域で機械固定。
    /// (x+y)&1 == (x^y)&1 は数学的恒等 (桁上がりは bit1 以上にしか影響
    /// しない)。wrapping 経路でも u32::MAX 級で debug panic せず WGSL の
    /// u32 wrap と厳密一致する (wrap は mod 2 で不変)。
    #[test]
    fn mask_parity_is_exact_and_wrap_safe() {
        // 全ビット幅スイープ: 2 冪・2 冪-1・境界近傍
        let interesting: Vec<u32> = (0..32u32)
            .flat_map(|k| [(1u32 << k).saturating_sub(1), 1u32 << k])
            .chain([u32::MAX, u32::MAX - 1, 0, 1])
            .collect();
        for &x in &interesting {
            for &y in &interesting {
                let expect = (x ^ y) & 1 == 0;
                assert_eq!(
                    Checkerboard::is_rendered(x, y),
                    expect,
                    "parity mismatch at ({x}, {y})"
                );
            }
        }
        // wrap 周回の実例: MAX+MAX = 2^32-2 (偶数) → true、MAX+1 = 2^32 (wrap で
        // 0, 偶数) → true。debug でも panic しないこと自体が契約。
        assert!(Checkerboard::is_rendered(u32::MAX, u32::MAX));
        assert!(Checkerboard::is_rendered(u32::MAX, 1));
        assert!(!Checkerboard::is_rendered(u32::MAX, 0));
    }

    /// wave 67 BQ-2: reconstruct の左結合和 ×0.25 の厳密ビット固定
    /// (f32 エミュレーション独立導出。非対称値でクロスチャンネル混入を
    /// 検出可能)。
    #[test]
    fn reconstruct_exact_bits_canonical() {
        let r = Checkerboard::reconstruct(
            Vec3::new(0.1, 0.2, 0.3),
            Vec3::new(0.4, 0.5, 0.6),
            Vec3::new(0.7, 0.8, 0.9),
            Vec3::new(1.0, 0.15, 0.55),
        );
        assert_eq!(
            [r.x.to_bits(), r.y.to_bits(), r.z.to_bits()],
            [0x3f0ccccd, 0x3ed33333, 0x3f166666],
            "reconstruct bit drift: {r:?}"
        );
    }

    /// wave 67 BQ-3: checkerboard.wgsl が 3連鎖 (本モジュール ↔
    /// frame_postfx::checker_run_cpu ↔ WGSL) と同一語彙であることの表記ピン。
    #[test]
    fn wgsl_mirror_lexical_tokens() {
        const WGSL: &str = include_str!("../shaders/checkerboard.wgsl");
        for tok in [
            // マスク規則 (u32 加算 wrap セマンティクス — BQ-1 と同値)
            "(gid.x + gid.y) & 1u) == 0u",
            // 描画済み画素はコピー
            "dst[idx] = src[idx];",
            // 斜め 4 近傍の方向 (NW/NE/SW/SE)
            "px(x - 1, y - 1)",
            "px(x + 1, y - 1)",
            "px(x - 1, y + 1)",
            "px(x + 1, y + 1)",
            // 左結合平均
            "(nw + ne + sw + se) * 0.25",
        ] {
            assert!(
                WGSL.contains(tok),
                "checkerboard.wgsl drift from module rules: {tok}"
            );
        }
    }
}

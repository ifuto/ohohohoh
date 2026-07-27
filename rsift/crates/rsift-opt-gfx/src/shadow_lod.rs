//! Shadow Level-of-Detail — scales shadow-map resolution to what the light
//! actually needs on screen, and drops tiny shadow casters entirely.
//!
//! On low-spec GPUs, shadow maps are a huge hidden cost (a 4K cascade atlas can
//! eat hundreds of MB of bandwidth/frame). Tying resolution to on-screen light
//! coverage and culling sub-pixel casters recovers most of that for free.
//!
//! 誠実注記 (wave 141 EO-3):
//! 1. wiring の coverage 供給は `draw_command_count + 16.0 ≥ 16` で常に
//!    clamp(1.0) → 現行供給では shadow_res ≡ max_resolution=2048 の
//!    **定数退化** (wave 136 EJ-2 注記の再確認)。LOD 判定 3 閾値
//!    (16/64/256) への到達性はカバレッジ入力依存。
//! 2. NaN 入力の一貫除外: `casts_shadow(NaN)`=false (比較演算の NaN
//!    偽値) → `caster_lod(NaN)`=3 (排除側へ統一)。一方
//!    `shadow_map_resolution(NaN)` は clamp 経由でも NaN が透過し
//!    `as u32` 飽和で **0** となる (契約域 [256,2048] 外の静寂退化、
//!    pin 済)。
//! 3. `caster_lod` 境界は全て `>=` 閉区間 — `min_caster_px(=4.0) ≤ px <
//!    16.0` は casts_shadow=true かつ lod=3 の共存形 (落とすが最粗)。
//! 4. 旧 `wgsl_source(&self)` は self 不使用の装飾レシーバ (返却は常に
//!    pub const と同一) で消費者がテストのみの中間構造だったため
//!    free fn へ根治 (EL-4 同型、wave 141 EO-2)。

#[derive(Clone, Copy, Debug)]
pub struct ShadowLod {
    pub max_resolution: u32,
    pub min_resolution: u32,
    /// Casters smaller than this (in screen pixels) do not cast shadows.
    pub min_caster_px: f32,
}
impl Default for ShadowLod {
    fn default() -> Self {
        Self {
            max_resolution: 2048,
            min_resolution: 256,
            min_caster_px: 4.0,
        }
    }
}
impl ShadowLod {
    pub fn new() -> Self {
        Self::default()
    }

    /// Shadow-map resolution for a light, scaled by how much of the screen it
    /// covers (`coverage` in [0,1]). Bigger on-screen light => more resolution.
    pub fn shadow_map_resolution(&self, coverage: f32) -> u32 {
        let t = coverage.clamp(0.0, 1.0);
        let r = self.min_resolution as f32
            + (self.max_resolution - self.min_resolution) as f32 * t;
        r.round().clamp(self.min_resolution as f32, self.max_resolution as f32) as u32
    }

    /// Whether a caster of `screen_px` height should cast a shadow at all.
    pub fn casts_shadow(&self, screen_px: f32) -> bool {
        screen_px >= self.min_caster_px
    }

    /// LOD bias for a caster: 0 = full-res shadow, 3 = coarsest. Smaller on
    /// screen => lower LOD (cheaper shadow geometry).
    pub fn caster_lod(&self, screen_px: f32) -> u8 {
        if !self.casts_shadow(screen_px) {
            return 3;
        }
        if screen_px >= 256.0 {
            0
        } else if screen_px >= 64.0 {
            1
        } else if screen_px >= 16.0 {
            2
        } else {
            3
        }
    }
}

/// WGSL 取得の公式アクセスポイント (wave 141 EO-2: self 不使用装飾
/// メソッドを free fn へ根治、gpu_runtime 登録の単一公式経路)。
pub fn wgsl_source() -> &'static str {
    SHADOW_LOD_WGSL
}

pub const SHADOW_LOD_WGSL: &str = include_str!("../shaders/shadow_lod.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolution_scales_with_coverage() {
        let s = ShadowLod::new();
        let small = s.shadow_map_resolution(0.0);
        let big = s.shadow_map_resolution(1.0);
        assert_eq!(small, 256);
        assert_eq!(big, 2048);
        assert!(big > small);
    }
    #[test]
    fn resolution_clamped() {
        let s = ShadowLod::new();
        assert_eq!(s.shadow_map_resolution(2.0), 2048);
        assert_eq!(s.shadow_map_resolution(-1.0), 256);
    }
    #[test]
    fn tiny_casters_skipped() {
        let s = ShadowLod::new();
        assert!(!s.casts_shadow(2.0));
        assert!(s.casts_shadow(10.0));
    }
    #[test]
    fn lod_buckets() {
        let s = ShadowLod::new();
        assert_eq!(s.caster_lod(500.0), 0);
        assert_eq!(s.caster_lod(100.0), 1);
        assert_eq!(s.caster_lod(30.0), 2);
        assert_eq!(s.caster_lod(2.0), 3);
    }
}

#[cfg(test)]
mod strict_tests {
    use super::*;

    /// EO-4: shadow_map_resolution golden (rq 導出:
    /// min256 + 1792*t の round clamp、t=0.25/0.5/0.75 は全整数 exact)。
    #[test]
    fn resolution_golden_exact() {
        let s = ShadowLod::new();
        assert_eq!(s.shadow_map_resolution(0.25), 704, "256+1792*0.25 (rq)");
        assert_eq!(s.shadow_map_resolution(0.5), 1152, "256+896 (rq)");
        assert_eq!(s.shadow_map_resolution(0.75), 1600, "256+1344 (rq)");
        assert_eq!(s.shadow_map_resolution(0.0), 256);
        assert_eq!(s.shadow_map_resolution(1.0), 2048);
    }

    /// EO-3-2 pin: NaN coverage は clamp 透過 → round → clamp →
    /// `as u32` 飽和で **0** (契約域 [256,2048] 外の静寂退化)。
    #[test]
    fn resolution_nan_saturates_to_zero() {
        let s = ShadowLod::new();
        assert!(f32::NAN.clamp(0.0, 1.0).is_nan(), "clamp は NaN 透過");
        assert_eq!(s.shadow_map_resolution(f32::NAN), 0, "NaN as u32 = 0 (pin)");
        assert_eq!(s.shadow_map_resolution(f32::INFINITY), 2048);
        assert_eq!(s.shadow_map_resolution(f32::NEG_INFINITY), 256);
    }

    /// EO-3-2/-3 pin: NaN px は一貫除外 (casts=false → lod=3)、
    /// 境界は全て `>=` 閉区間。
    #[test]
    fn caster_nan_and_boundary_closed_intervals() {
        let s = ShadowLod::new();
        assert!(!s.casts_shadow(f32::NAN), "NaN >= 4.0 は false");
        assert_eq!(s.caster_lod(f32::NAN), 3, "NaN は排除側へ統一");
        // min_caster_px = 4.0 境界
        assert!(!s.casts_shadow(3.999_999));
        assert!(s.casts_shadow(4.0));
        assert_eq!(s.caster_lod(4.0), 3, "4≤px<16 は casts=true かつ lod=3");
        // 16 / 64 / 256 境界 (全 `>=` 閉区間)
        assert_eq!(s.caster_lod(15.999_999), 3);
        assert_eq!(s.caster_lod(16.0), 2);
        assert_eq!(s.caster_lod(63.999_996), 2);
        assert_eq!(s.caster_lod(64.0), 1);
        assert_eq!(s.caster_lod(255.999_98), 1);
        assert_eq!(s.caster_lod(256.0), 0);
    }

    /// EO-4: new() ≡ Default::default() 契約 + WGSL identity。
    #[test]
    fn new_matches_default_and_wgsl_identity() {
        let a = ShadowLod::new();
        let b = ShadowLod::default();
        assert_eq!(a.max_resolution, b.max_resolution);
        assert_eq!(a.min_resolution, b.min_resolution);
        assert_eq!(a.min_caster_px.to_bits(), b.min_caster_px.to_bits());
        assert_eq!(a.max_resolution, 2048);
        assert_eq!(a.min_resolution, 256);
        assert_eq!(
            a.min_caster_px.to_bits(),
            0x40800000,
            "4.0 (rq 導出不要・規格値)"
        );
        assert_eq!(wgsl_source(), SHADOW_LOD_WGSL);
        assert!(wgsl_source().contains("@fragment"));
    }
}

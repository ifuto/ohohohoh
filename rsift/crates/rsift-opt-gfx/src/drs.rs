//! Dynamic Resolution Scaling + CAS settings (Tier 4).
//! AMD FidelityFX CAS companion — scale internal res to hold target FPS.

#[derive(Debug, Clone)]
pub struct CasSettings {
    pub enabled: bool,
    /// 0 = soft, 1 = max sharpen (AMD CAS sharpness).
    pub sharpness: f32,
    pub upscale: bool,
}

impl Default for CasSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            sharpness: 0.4,
            upscale: true,
        }
    }
}

#[derive(Debug)]
pub struct DynamicResolutionScaler {
    pub target_fps: f32,
    pub min_scale: f32,
    pub max_scale: f32,
    scale: f32,
    frame_ms_ema: f32,
    frames_below: u32,
    frames_above: u32,
    pub cas: CasSettings,
}

impl DynamicResolutionScaler {
    pub fn new(target_fps: f32) -> Self {
        Self {
            target_fps: target_fps.max(20.0),
            min_scale: 0.5,
            max_scale: 1.0,
            scale: 1.0,
            frame_ms_ema: 1000.0 / target_fps.max(20.0),
            frames_below: 0,
            frames_above: 0,
            cas: CasSettings::default(),
        }
    }

    pub fn for_tier_fps(cap: Option<u32>) -> Self {
        Self::new(cap.unwrap_or(60) as f32)
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn push_frame_ms(&mut self, frame_ms: f32) {
        // 観測契約 (wave 23 監査で追加): 非有限 ms (NaN/±inf) は観測欠測として
        // 捨てる。NaN は f32::clamp を素通りし (NaN.clamp = NaN)、EMA を一度
        // 汚染すると比較が全て偽になって scale が静かに永久凍結する旧動作の根治。
        // frame_pacing::record_frame (wave 17, S-3) と同一の契約。
        if !frame_ms.is_finite() {
            return;
        }
        let ms = frame_ms.clamp(1.0, 100.0);
        self.frame_ms_ema = self.frame_ms_ema * 0.9 + ms * 0.1;
        let target_ms = 1000.0 / self.target_fps;
        if self.frame_ms_ema > target_ms * 1.08 {
            self.frames_below += 1;
            self.frames_above = 0;
        } else if self.frame_ms_ema < target_ms * 0.92 {
            self.frames_above += 1;
            self.frames_below = 0;
        } else {
            self.frames_below = 0;
            self.frames_above = 0;
        }
        // Hysteresis: 8 frames before adjust
        if self.frames_below >= 8 {
            self.scale = (self.scale - 0.05).max(self.min_scale);
            self.frames_below = 0;
        } else if self.frames_above >= 12 {
            self.scale = (self.scale + 0.05).min(self.max_scale);
            self.frames_above = 0;
        }
    }

    pub fn internal_size(&self, display_w: u32, display_h: u32) -> (u32, u32) {
        let w = ((display_w as f32) * self.scale).round().max(64.0) as u32;
        let h = ((display_h as f32) * self.scale).round().max(64.0) as u32;
        // even dims for YUV/post
        (w & !1, h & !1)
    }

    pub fn fps_ema(&self) -> f32 {
        1000.0 / self.frame_ms_ema.max(0.1)
    }
}

pub const CAS_WGSL: &str = include_str!("../shaders/cas.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_down_under_load() {
        let mut d = DynamicResolutionScaler::new(60.0);
        for _ in 0..40 {
            d.push_frame_ms(25.0); // 40 FPS
        }
        assert!(d.scale() < 1.0);
        let (w, h) = d.internal_size(1920, 1080);
        assert!(w < 1920);
        assert!(h < 1080);
    }

    /// wave 23-4: 非有限 frame_ms は観測欠測として捨てられ EMA/scale を
    /// 汚染しないこと (NaN は clamp 素通りで永久凍結する旧動作の回帰防止)。
    #[test]
    fn push_frame_ms_rejects_non_finite() {
        let mut d = DynamicResolutionScaler::new(60.0);
        for _ in 0..16 {
            d.push_frame_ms(16.0);
        }
        let ema_before = d.fps_ema().to_bits();
        let scale_before = d.scale().to_bits();
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            d.push_frame_ms(bad);
        }
        assert_eq!(
            d.fps_ema().to_bits(),
            ema_before,
            "non-finite must be dropped"
        );
        assert_eq!(d.scale().to_bits(), scale_before, "scale must stay");
        // 以後の正常観測で統計が壊れず動き続けること
        for _ in 0..64 {
            d.push_frame_ms(25.0);
        }
        assert!(d.scale() < 1.0, "still functional after bad samples");
    }

    /// wave 23-5: EMA + 帯域 + ヒステリシス (8/12) のタイムラインは完全決定的 —
    /// f32 単一回丸め規則から exact rational で厳密導出 (float64 近似禁止、W-3
    /// 教訓): 60fps 目標・25ms 連続観測で初回調整は 9 push 目、30 push で
    /// 3 回調整 (scale 0.85)、fps_ema は厳密これ。
    #[test]
    fn hysteresis_timeline_is_deterministic() {
        let mut d = DynamicResolutionScaler::new(60.0);
        let mut first_adjust = None;
        let mut n_adjust = 0u32;
        let mut prev = 1.0f32;
        for i in 1..=30u32 {
            d.push_frame_ms(25.0);
            if d.scale() < prev {
                n_adjust += 1;
                if first_adjust.is_none() {
                    first_adjust = Some(i);
                }
                prev = d.scale();
            }
        }
        assert_eq!(first_adjust, Some(9), "8-frames hysteresis after ema>18.0");
        assert_eq!(n_adjust, 3, "adjustments in 30 pushes");
        assert_eq!(d.scale().to_bits(), 0x3f599999, "scale 0.85 exact bits");
        assert_eq!(d.fps_ema().to_bits(), 0x42224b15, "fps_ema exact bits");
    }
}

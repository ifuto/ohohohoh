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
}

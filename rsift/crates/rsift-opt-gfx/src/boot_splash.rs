//! Boot splash progress logger — honest: no HWND/GPU override of Mojang screen.

use tracing::{info, debug, warn};

#[derive(Debug, Clone)]
pub struct BootStage {
    pub step_name: String,
    pub current: u32,
    pub total: u32,
}

pub struct RsiftModernBootSplash {
    pub is_active: bool,
    pub current_stage: BootStage,
}

impl Default for RsiftModernBootSplash {
    fn default() -> Self {
        Self::new()
    }
}

impl RsiftModernBootSplash {
    pub fn new() -> Self {
        Self {
            is_active: false,
            current_stage: BootStage {
                step_name: "Initializing Rsift Native Engine...".into(),
                current: 0,
                total: 100,
            },
        }
    }

    /// Progress tracking only — does not replace Mojang splash (no HWND path yet).
    pub fn activate_override(&mut self) {
        warn!(
            "[BootSplash] activate_override: logging progress only — Mojang screen is NOT hijacked \
             (no DXGI/HWND splash surface). Claims of frosted-glass GPU splash are disabled."
        );
        self.is_active = true;
    }

    pub fn update_progress(&mut self, step: &str, current: u32, total: u32) {
        self.current_stage = BootStage {
            step_name: step.to_string(),
            current,
            total,
        };
        debug!("[BootSplash] {}: {}/{}", step, current, total);
    }

    pub fn render_splash_frame(&self) {
        if !self.is_active {
            return;
        }
        let pct = if self.current_stage.total > 0 {
            (self.current_stage.current as f32 / self.current_stage.total as f32) * 100.0
        } else {
            0.0
        };
        info!(
            "[BootSplash] stage='{}' {:.1}% (console progress — not a GPU framebuffer)",
            self.current_stage.step_name, pct
        );
    }

    pub fn finish_and_fade_out(&mut self) {
        info!("[BootSplash] progress complete");
        self.is_active = false;
    }
}

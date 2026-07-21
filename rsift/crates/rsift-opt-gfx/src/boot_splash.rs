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

#[cfg(test)]
mod strict_tests {
    use super::*;

    #[test]
    fn default_state_is_inactive_with_initial_stage() {
        let s = RsiftModernBootSplash::new();
        assert!(!s.is_active, "起動時は非アクティブ (Mojang 画面は差し替えない)");
        assert_eq!(s.current_stage.step_name, "Initializing Rsift Native Engine...");
        assert_eq!((s.current_stage.current, s.current_stage.total), (0, 100));
        let d = RsiftModernBootSplash::default();
        assert_eq!(d.current_stage.step_name, s.current_stage.step_name, "Default == new()");
        assert!(!d.is_active);
    }

    #[test]
    fn activate_update_finish_lifecycle() {
        let mut s = RsiftModernBootSplash::new();
        s.render_splash_frame(); // 非アクティブでも早期 return でパニックしない
        s.activate_override();
        assert!(s.is_active);
        s.update_progress("Loading native libraries", 42, 100);
        assert_eq!(s.current_stage.step_name, "Loading native libraries");
        assert_eq!((s.current_stage.current, s.current_stage.total), (42, 100));
        s.render_splash_frame(); // アクティブ時: ログ出力のみ (HWND 差し替えはしない、がクラッシュもしない)
        s.finish_and_fade_out();
        assert!(!s.is_active, "finish で非アクティブへ");
        s.render_splash_frame(); // 非アクティブ復帰後も安全
    }

    #[test]
    fn zero_total_progress_never_panics() {
        let mut s = RsiftModernBootSplash::new();
        s.activate_override();
        s.update_progress("indeterminate", 5, 0);
        assert_eq!((s.current_stage.current, s.current_stage.total), (5, 0), "値は検証せず透過保管");
        s.render_splash_frame(); // total==0 の除算ガード経路
    }
}

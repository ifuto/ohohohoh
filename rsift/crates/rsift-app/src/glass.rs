//! OS-level glass: Mica (Win11) / Acrylic (Win10) + macOS vibrancy.

pub fn apply_to_context(cc: &eframe::CreationContext<'_>) {
    #[cfg(target_os = "windows")]
    {
        use window_vibrancy::{apply_acrylic, apply_mica};
        // Mica = modern frosted glass on Win11; Acrylic = blur+tint on Win10+
        if apply_mica(cc, None).is_err() {
            let _ = apply_acrylic(cc, Some((186, 230, 253, 160)));
        }
    }
    #[cfg(target_os = "macos")]
    {
        use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial};
        let _ = apply_vibrancy(cc, NSVisualEffectMaterial::HudWindow, None, None);
    }
}

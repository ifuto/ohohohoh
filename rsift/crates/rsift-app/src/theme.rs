//! Aqua glassmorphism — frosted panels over OS Mica/Acrylic.

use eframe::egui::{self, Color32, FontFamily, FontId, Margin, Rounding, Stroke, Visuals};

pub const AQUA: Color32 = Color32::from_rgb(34, 211, 238);
pub const AQUA_GLOW: Color32 = Color32::from_rgb(103, 232, 249);
pub const AQUA_DEEP: Color32 = Color32::from_rgb(8, 145, 178);
pub const SKY: Color32 = Color32::from_rgb(186, 230, 253);
pub const INK: Color32 = Color32::from_rgb(240, 249, 255);

// --- Soft pastel accents (ふわっとしたモダン) ---
pub const LAVENDER: Color32 = Color32::from_rgb(196, 181, 253);
pub const PEACH: Color32 = Color32::from_rgb(252, 190, 205);
pub const MINT: Color32 = Color32::from_rgb(134, 239, 172);

pub const GLASS_FILL: Color32 = Color32::from_rgba_premultiplied(255, 255, 255, 32);
pub const GLASS_FILL_HOVER: Color32 = Color32::from_rgba_premultiplied(255, 255, 255, 48);
pub const GLASS_STROKE: Color32 = Color32::from_rgba_premultiplied(255, 255, 255, 88);
pub const GLASS_RIM: Color32 = Color32::from_rgba_premultiplied(255, 255, 255, 140);

pub fn apply_aqua_theme(ctx: &egui::Context) {
    let mut visuals = Visuals::dark();
    visuals.window_fill = Color32::TRANSPARENT;
    visuals.panel_fill = Color32::TRANSPARENT;
    visuals.extreme_bg_color = Color32::TRANSPARENT;
    visuals.faint_bg_color = GLASS_FILL;
    visuals.widgets.noninteractive.bg_fill = Color32::TRANSPARENT;
    visuals.widgets.inactive.bg_fill = Color32::from_rgba_premultiplied(255, 255, 255, 28);
    visuals.widgets.hovered.bg_fill = Color32::from_rgba_premultiplied(125, 211, 252, 50);
    visuals.widgets.active.bg_fill = Color32::from_rgba_premultiplied(34, 211, 238, 100);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, INK);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, SKY);
    visuals.selection.bg_fill = Color32::from_rgba_premultiplied(34, 211, 238, 80);
    visuals.hyperlink_color = AQUA;
    visuals.window_rounding = Rounding::same(20.0);
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(12.0, 10.0);
    style.spacing.button_padding = egui::vec2(16.0, 10.0);
    style.spacing.indent = 20.0;
    style.text_styles.insert(
        egui::TextStyle::Heading,
        FontId::new(28.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Body,
        FontId::new(15.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Button,
        FontId::new(15.0, FontFamily::Proportional),
    );
    ctx.set_style(style);
}

pub fn glass_card() -> egui::Frame {
    egui::Frame::none()
        .fill(GLASS_FILL)
        .stroke(Stroke::new(1.0, GLASS_STROKE))
        .rounding(Rounding::same(20.0))
        .inner_margin(Margin::same(24.0))
        .shadow(egui::epaint::Shadow {
            offset: egui::vec2(0.0, 16.0),
            blur: 40.0,
            spread: 0.0,
            color: Color32::from_rgba_premultiplied(6, 78, 99, 55),
        })
}

/// Hero panel — lighter, softer, dreamier than `glass_card`.
pub fn hero_card() -> egui::Frame {
    egui::Frame::none()
        .fill(Color32::from_rgba_premultiplied(255, 255, 255, 26))
        .stroke(Stroke::new(
            1.0,
            Color32::from_rgba_premultiplied(255, 255, 255, 110),
        ))
        .rounding(Rounding::same(26.0))
        .inner_margin(Margin::symmetric(30.0, 28.0))
        .shadow(egui::epaint::Shadow {
            offset: egui::vec2(0.0, 20.0),
            blur: 48.0,
            spread: 0.0,
            color: Color32::from_rgba_premultiplied(76, 29, 149, 45),
        })
}

/// Small info chip (version / memory / resolution summaries).
pub fn chip_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(Color32::from_rgba_premultiplied(255, 255, 255, 22))
        .stroke(Stroke::new(
            1.0,
            Color32::from_rgba_premultiplied(255, 255, 255, 60),
        ))
        .rounding(Rounding::same(14.0))
        .inner_margin(Margin::symmetric(14.0, 10.0))
}

/// Log / console panel.
pub fn console_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(Color32::from_rgba_premultiplied(6, 26, 38, 165))
        .stroke(Stroke::new(
            1.0,
            Color32::from_rgba_premultiplied(255, 255, 255, 42),
        ))
        .rounding(Rounding::same(14.0))
        .inner_margin(Margin::same(12.0))
}

pub fn title_bar_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(Color32::from_rgba_premultiplied(255, 255, 255, 38))
        .stroke(Stroke::new(1.0, GLASS_RIM))
        .inner_margin(Margin::symmetric(14.0, 0.0))
}

pub fn sidebar_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(Color32::from_rgba_premultiplied(255, 255, 255, 22))
        .stroke(Stroke::new(1.0, Color32::from_rgba_premultiplied(255, 255, 255, 50)))
        .inner_margin(Margin::symmetric(10.0, 16.0))
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

pub fn ease_out_expo(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t >= 1.0 {
        1.0
    } else {
        1.0 - 2f32.powf(-10.0 * t)
    }
}

// Back-compat alias
pub fn glass_frame() -> egui::Frame {
    glass_card()
}

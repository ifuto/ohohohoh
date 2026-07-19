//! Rsift Launcher — glass aqua UI, offline play, local install.

mod app;
mod fonts;
mod glass;
mod profiles;
mod theme;

use eframe::egui;

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    let mut native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(egui::vec2(1180.0, 760.0))
            .with_min_inner_size(egui::vec2(960.0, 640.0))
            .with_transparent(true)
            .with_decorations(false)
            .with_active(true),
        multisampling: 1,
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    native.centered = true;

    eframe::run_native(
        "Rsift Launcher",
        native,
        Box::new(|cc| {
            fonts::install(&cc.egui_ctx);
            theme::apply_aqua_theme(&cc.egui_ctx);
            glass::apply_to_context(cc);
            Ok(Box::new(app::RsiftApp::new()))
        }),
    )
}

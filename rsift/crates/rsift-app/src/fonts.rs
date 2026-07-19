//! System fonts — Segoe UI (Latin) + Yu Gothic (CJK).

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use std::path::PathBuf;

fn font_dir() -> Option<PathBuf> {
    std::env::var("SystemRoot")
        .ok()
        .map(|r| PathBuf::from(r).join("Fonts"))
}

pub fn install(ctx: &egui::Context) {
    let Some(dir) = font_dir() else {
        return;
    };

    let mut fonts = FontDefinitions::default();
    let mut inserted = false;

    if let Ok(bytes) = std::fs::read(dir.join("segoeui.ttf")) {
        fonts
            .font_data
            .insert("segoe".to_owned(), FontData::from_owned(bytes));
        if let Some(fam) = fonts.families.get_mut(&FontFamily::Proportional) {
            fam.insert(0, "segoe".to_owned());
        }
        inserted = true;
    }

    for name in ["YuGothR.ttc", "YuGothM.ttc", "meiryo.ttc", "msgothic.ttc"] {
        let path = dir.join(name);
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        fonts
            .font_data
            .insert("cjk".to_owned(), FontData::from_owned(bytes));
        if let Some(fam) = fonts.families.get_mut(&FontFamily::Proportional) {
            let pos = if fam.first().map(|s| s.as_str()) == Some("segoe") {
                1
            } else {
                0
            };
            fam.insert(pos, "cjk".to_owned());
        }
        if let Some(fam) = fonts.families.get_mut(&FontFamily::Monospace) {
            fam.insert(0, "cjk".to_owned());
        }
        tracing::info!("rsift-app: UI font {}", path.display());
        inserted = true;
        break;
    }

    if inserted {
        ctx.set_fonts(fonts);
    }
}

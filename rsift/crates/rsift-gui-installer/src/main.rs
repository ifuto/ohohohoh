//! # Rsift Single-File GUI Downloader & Auto-Installer Main
//!
//! GitHub経由は後回し — ローカルバンドルから起動構成へ自動追加。

use rsift_gui_installer::GuiDownloaderEngine;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

fn main() {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_target(false)
        .with_thread_ids(false)
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);

    info!("==========================================================================");
    info!(" 🖥️ [Rsift GUI Installer] Local Bundle Auto-Setup");
    info!("    GitHub download: deferred | Local mods: ON");
    info!("==========================================================================");

    let bundle_root = GuiDownloaderEngine::detect_bundle_root();
    info!("📂 Bundle root: {:?}", bundle_root);

    match GuiDownloaderEngine::execute_local_bundle_install(&bundle_root) {
        Ok(profile) => {
            info!("✨ 起動構成に自動追加完了: [ {} ]", profile);
            info!("   同梱Mod: RsGraphics + RsCalc + RsReplay");
        }
        Err(e) => {
            tracing::error!("Local install failed: {}", e);
            std::process::exit(1);
        }
    }
}

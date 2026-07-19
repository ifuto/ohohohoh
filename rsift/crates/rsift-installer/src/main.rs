//! # Rsift Official Launcher Auto-Installer Executable

use clap::Parser;
use rsift_api::engine_caps::ShaderModelTier;
use rsift_installer::{InstallOptions, LauncherInstaller};
use std::env;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

#[derive(Parser, Debug)]
#[command(name = "rsift-installer", about = "Rsift Minecraft launcher integration")]
struct Cli {
    /// Shader Model tier: auto (default), 6.6 (legacy GPUs), or 6.9 (mid-high+).
    /// SM 6.9 is rejected if the GPU fails the minimum spec check.
    #[arg(long, value_name = "TIER", default_value = "auto")]
    shader_model: String,
}

fn parse_shader_model(s: &str) -> Result<Option<ShaderModelTier>, String> {
    match s.trim().to_lowercase().as_str() {
        "auto" => Ok(None),
        "6.6" | "sm66" => Ok(Some(ShaderModelTier::Sm66)),
        "6.9" | "sm69" => Ok(Some(ShaderModelTier::Sm69)),
        other => Err(format!(
            "unknown shader model {:?} — use auto, 6.6, or 6.9",
            other
        )),
    }
}

fn main() {
    let cli = Cli::parse();
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_target(false)
        .with_thread_ids(false)
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();

    let shader_model = match parse_shader_model(&cli.shader_model) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("{}", e);
            std::process::exit(1);
        }
    };

    let installer = match LauncherInstaller::new() {
        Ok(i) => i,
        Err(e) => {
            tracing::error!("Initialization failed: {}", e);
            std::process::exit(1);
        }
    };

    let current_dir = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    match installer.install_with_options(
        &current_dir,
        InstallOptions { shader_model },
    ) {
        Ok(result) => {
            if let Ok(n) = LauncherInstaller::repair_profiles(&installer.minecraft_dir) {
                if n > 0 {
                    tracing::info!("Repaired {} stale Rsift profile(s)", n);
                }
            }
            if let Ok(n) = LauncherInstaller::repair_version_jsons(&installer.minecraft_dir) {
                if n > 0 {
                    tracing::info!("Stripped duplicate agents from {} version JSON(s)", n);
                }
            }
            tracing::info!(
                "Installed: {} | agent={} | SM={}",
                result.version_id,
                result.agent_loaded,
                result.engine_caps.shader_model.as_str()
            );
        }
        Err(e) => {
            tracing::error!("Installation failed: {}", e);
            std::process::exit(1);
        }
    }
}

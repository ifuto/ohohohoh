//! Phase 0 — DirectX 12 Agility SDK bootstrap.
//!
//! Loads `D3D12SDKVersion` / `D3D12GetInterface` from the Agility redistributable
//! instead of the OS-bundled D3D12.

use crate::error::{Dx12Error, Dx12Result};
use tracing::info;

/// Agility SDK version bundled with Rsift (match `D3D12SDKVersion` export).
pub const AGILITY_SDK_VERSION: u32 = 613;

#[derive(Debug, Clone)]
pub struct AgilityConfig {
    pub sdk_path: std::path::PathBuf,
    pub sdk_version: u32,
}

impl Default for AgilityConfig {
    fn default() -> Self {
        let sdk_path = std::env::var("RSIFT_D3D12_SDK_PATH")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|d| d.join("D3D12")))
                    .unwrap_or_else(|| std::path::PathBuf::from("./D3D12"))
            });
        Self {
            sdk_path,
            sdk_version: AGILITY_SDK_VERSION,
        }
    }
}

/// Export Agility symbols so the loader resolves the redist DLL.
#[cfg(windows)]
pub fn export_agility_symbols() {
    #[link_section = ".d3d12exports"]
    #[used]
    static D3D12SDKVERSION: u32 = AGILITY_SDK_VERSION;

    // MSVC / link.exe: /EXPORT:D3D12SDKVersion
    info!(
        "[Agility] SDK version export = {} path={:?}",
        AGILITY_SDK_VERSION,
        AgilityConfig::default().sdk_path
    );
}

#[cfg(windows)]
pub fn ensure_agility_path(config: &AgilityConfig) -> Dx12Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::System::LibraryLoader::{AddDllDirectory, SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_USER_DIRS};

    unsafe {
        let _ = SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_USER_DIRS);
        let wide: Vec<u16> = config
            .sdk_path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let handle = AddDllDirectory(PCWSTR(wide.as_ptr()));
        if handle.is_null() {
            return Err(Dx12Error::Msg("AddDllDirectory failed".into()));
        }
    }
    info!(
        "[Agility] DLL search path registered: {:?} (v{})",
        config.sdk_path, config.sdk_version
    );
    Ok(())
}

#[cfg(windows)]
pub fn bootstrap(config: &AgilityConfig) -> Dx12Result<()> {
    export_agility_symbols();
    if config.sdk_path.exists() {
        ensure_agility_path(config)?;
    } else {
        info!(
            "[Agility] SDK folder not found at {:?} — using system D3D12",
            config.sdk_path
        );
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn bootstrap(_config: &AgilityConfig) -> Dx12Result<()> {
    Err(Dx12Error::Msg("Windows only".into()))
}

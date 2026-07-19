//! Shared DXGI / D3D12 helpers and portable stub compatibility for `windows` 0.58 bindings.

#[cfg(windows)]
pub use windows::Win32::Graphics::Dxgi::Common::*;
#[cfg(windows)]
pub use windows::Win32::Graphics::Dxgi::*;
#[cfg(windows)]
pub use windows::Win32::Graphics::Direct3D12::*;

#[cfg(windows)]
use crate::error::{Dx12Error, Dx12Result};

/// Preview flag — not yet in `windows` 0.58 `D3D12_RESOURCE_FLAGS`.
#[cfg(windows)]
pub const D3D12_RESOURCE_FLAG_ALLOW_SAMPLER_FEEDBACK: D3D12_RESOURCE_FLAGS =
    D3D12_RESOURCE_FLAGS(0x8000);

#[cfg(windows)]
pub fn win_err<T>(result: windows::core::Result<T>) -> Dx12Result<T> {
    result.map_err(|e| Dx12Error::Msg(e.to_string()))
}

#[cfg(windows)]
pub fn check_present(hr: windows::core::HRESULT) -> Dx12Result<()> {
    if hr.is_ok() {
        Ok(())
    } else {
        Err(Dx12Error::Hresult(hr.0, "Present"))
    }
}

#[cfg(windows)]
pub fn shader_bytecode(bytecode: &[u8]) -> D3D12_SHADER_BYTECODE {
    D3D12_SHADER_BYTECODE {
        pShaderBytecode: bytecode.as_ptr() as *const std::ffi::c_void,
        BytecodeLength: bytecode.len(),
    }
}

// ============================================================================
// Non-Windows Portable Compatibility Stubs (`cargo check --workspace` on Linux/macOS)
// ============================================================================

#[cfg(not(windows))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(transparent)]
pub struct DXGI_FORMAT(pub i32);

#[cfg(not(windows))]
pub const DXGI_FORMAT_R8G8B8A8_UNORM: DXGI_FORMAT = DXGI_FORMAT(28);
#[cfg(not(windows))]
pub const DXGI_FORMAT_UNKNOWN: DXGI_FORMAT = DXGI_FORMAT(0);
#[cfg(not(windows))]
pub const DXGI_FORMAT_D32_FLOAT: DXGI_FORMAT = DXGI_FORMAT(40);
#[cfg(not(windows))]
pub const DXGI_FORMAT_BC7_UNORM: DXGI_FORMAT = DXGI_FORMAT(98);

#[cfg(not(windows))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DXGI_SAMPLE_DESC {
    pub Count: u32,
    pub Quality: u32,
}

#[cfg(not(windows))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(transparent)]
pub struct DXGI_ALPHA_MODE(pub i32);

#[cfg(not(windows))]
pub const DXGI_ALPHA_MODE_UNSPECIFIED: DXGI_ALPHA_MODE = DXGI_ALPHA_MODE(0);

#[cfg(not(windows))]
pub fn check_present(hr: i32) -> crate::error::Dx12Result<()> {
    if hr >= 0 {
        Ok(())
    } else {
        Err(crate::error::Dx12Error::Hresult(hr, "Present"))
    }
}

#[cfg(not(windows))]
pub fn win_err<T>(result: Result<T, String>) -> crate::error::Dx12Result<T> {
    result.map_err(|e| crate::error::Dx12Error::Msg(e))
}

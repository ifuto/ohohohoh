//! Shared DXGI / D3D12 helpers for `windows` 0.58 bindings.

#[cfg(windows)]
pub use windows::Win32::Graphics::Dxgi::Common::*;

#[cfg(windows)]
use crate::error::{Dx12Error, Dx12Result};
#[cfg(windows)]
use windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_FLAGS;

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
pub fn shader_bytecode(bytecode: &[u8]) -> windows::Win32::Graphics::Direct3D12::D3D12_SHADER_BYTECODE {
    windows::Win32::Graphics::Direct3D12::D3D12_SHADER_BYTECODE {
        pShaderBytecode: bytecode.as_ptr() as *const std::ffi::c_void,
        BytecodeLength: bytecode.len(),
    }
}

//! # DirectX 12 Agility Engine Error Types (`Dx12Error` / `Dx12Result`)

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Dx12Error {
    #[error("HRESULT failed: {0:#010x} at {1}")]
    Hresult(i32, &'static str),
    #[error("DX12: {0}")]
    Msg(String),
    #[error("feature unavailable: {0}")]
    FeatureUnavailable(String),
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("Dynamic Library Loading Error: {0}")]
    LibLoading(#[from] libloading::Error),
}

pub type Dx12Result<T> = Result<T, Dx12Error>;

impl From<String> for Dx12Error {
    fn from(s: String) -> Self {
        Dx12Error::Msg(s)
    }
}

impl From<&str> for Dx12Error {
    fn from(s: &str) -> Self {
        Dx12Error::Msg(s.to_string())
    }
}

impl From<&String> for Dx12Error {
    fn from(s: &String) -> Self {
        Dx12Error::Msg(s.clone())
    }
}

#[cfg(windows)]
impl From<windows::core::Error> for Dx12Error {
    fn from(e: windows::core::Error) -> Self {
        Dx12Error::Hresult(e.code().0, "Windows Core Error")
    }
}

#[cfg(windows)]
pub fn check_hresult(hr: windows::core::HRESULT, ctx: &'static str) -> Dx12Result<()> {
    if hr.is_ok() {
        Ok(())
    } else {
        Err(Dx12Error::Hresult(hr.0, ctx))
    }
}

#[cfg(not(windows))]
pub fn check_hresult(hr: i32, ctx: &'static str) -> Dx12Result<()> {
    if hr >= 0 {
        Ok(())
    } else {
        Err(Dx12Error::Hresult(hr, ctx))
    }
}

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
}

pub type Dx12Result<T> = Result<T, Dx12Error>;

#[cfg(windows)]
pub fn check_hresult(hr: windows::core::HRESULT, ctx: &'static str) -> Dx12Result<()> {
    if hr.is_ok() {
        Ok(())
    } else {
        Err(Dx12Error::Hresult(hr.0, ctx))
    }
}

#[cfg(windows)]
impl From<windows::core::Error> for Dx12Error {
    fn from(e: windows::core::Error) -> Self {
        Dx12Error::Msg(e.to_string())
    }
}

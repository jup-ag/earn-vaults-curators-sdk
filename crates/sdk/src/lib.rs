pub use earn_vaults_interface::*;
use {earn_vaults_interface::encoding::ZeroPodError, thiserror::Error};

pub mod instructions;
pub mod nav;
pub mod offerbook;
pub mod pda;

#[derive(Error, Debug)]
pub enum SdkErrors {
    #[error("Wincode write error: {0}")]
    WriteError(#[from] wincode::WriteError),
    #[error("ZeroPod error")]
    ZeroPodError,
    #[error("invalid arguments: {0}")]
    InvalidArgs(&'static str),
}

impl From<ZeroPodError> for SdkErrors {
    fn from(_value: ZeroPodError) -> Self {
        Self::ZeroPodError
    }
}

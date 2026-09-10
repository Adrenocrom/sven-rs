use thiserror::Error;

#[derive(Debug, Error)]
pub enum SecurityError {
    #[error("The request was not authorized")]
    Unauthorized,
    #[error("The given Path was an empty String")]
    EmptyPath,
    #[error("{0}")]
    NormalizationError(#[from] std::path::NormalizeError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

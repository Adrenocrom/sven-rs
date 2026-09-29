use thiserror::Error;

#[derive(Debug, Error)]
pub enum SecurityError {
    #[error("path {0} is outside the current directory")]
    Unauthorized(String),
    #[error("the given path was an empty string")]
    EmptyPath,
    #[error("{0}")]
    Io(#[from] std::io::Error),
}
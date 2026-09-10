use thiserror::Error;

use crate::sven::security_error::SecurityError;

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("{0}")]
    SecurityError(#[from] SecurityError),
    #[error("Invalid parameter: {0}")]
    InvalidParameters(String),
    #[error("Missing parameter: {0}")]
    MissingParameter(String),
    #[error("{0}")]
    FromUtf8Error(#[from] std::string::FromUtf8Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

use thiserror::Error;

#[derive(Error, Debug)]
#[allow(dead_code)]
pub enum AppError {
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("HTTP error {status}: {message}")]
    Http {
        status: u16,
        message: String,
    },

    #[error("Filesystem I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Operation cancelled by user")]
    Cancelled,

    #[error("Invalid filesystem path: {0}")]
    InvalidPath(String),

    #[error("Stream error: {0}")]
    StreamError(String),

    #[error("Application error: {0}")]
    Generic(String),
}

pub type Result<T> = std::result::Result<T, AppError>;

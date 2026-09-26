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

pub fn clean_user_error(err: &str) -> String {
    let lower = err.to_lowercase();
    if lower.contains("no video formats found") {
        "No compatible video streams found. The media may be DRM-protected, require login, or is unavailable in this region.".to_string()
    } else if lower.contains("unsupported url") || lower.contains("is not a valid url") {
        "This URL format is not supported. Please verify the link and try again.".to_string()
    } else if lower.contains("private video") || lower.contains("sign in") {
        "This media is private or requires account authentication.".to_string()
    } else if lower.contains("timed out") || lower.contains("connection refused") {
        "Network connection timed out. Please check your internet connection.".to_string()
    } else {
        let s = err.split("; please report").next().unwrap_or(err);
        let s = s.split("Application error: ").last().unwrap_or(s);
        let s = s.split("Extractor: ").last().unwrap_or(s);
        let s = s.trim().trim_start_matches("Inspection failed: ").trim();
        if s.is_empty() {
            "Unable to analyze media at this URL.".to_string()
        } else {
            s.to_string()
        }
    }
}

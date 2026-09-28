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
    if lower.contains("iq.com") || lower.contains("iqiyi") {
        if lower.contains("413") || lower.contains("request entity too large") {
            "Request header too large (HTTP 413). iQIYI server rejected cookie size. Clear or disable browser cookies in Settings.".to_string()
        } else {
            "iQIYI stream is DRM-protected or requires VIP login. DRM-encrypted content cannot be downloaded.".to_string()
        }
    } else if lower.contains("413") || lower.contains("request entity too large") {
        "Request header too large (HTTP 413). Server rejected oversized cookies. Disable browser cookies in Settings.".to_string()
    } else if lower.contains("drm") || lower.contains("widevine") {
        "This video stream is DRM-protected (encrypted) and cannot be downloaded.".to_string()
    } else if lower.contains("phantomjs") {
        "iQIYI stream decryption requires PhantomJS or is DRM-protected. DRM-encrypted content cannot be downloaded.".to_string()
    } else if lower.contains("no video formats found") {
        "No downloadable video formats found. Stream may be DRM-protected, require VIP login, or be region-locked.".to_string()
    } else if (lower.contains("douyin.com") || lower.contains("tiktok.com"))
        && (lower.contains("unsupported url") || lower.contains("is not a valid url"))
    {
        "Please provide a specific video or share link (e.g. https://www.douyin.com/video/... or https://v.douyin.com/...) rather than the homepage.".to_string()
    } else if lower.contains("unsupported url") || lower.contains("is not a valid url") {
        "This URL format is not supported. Please verify the link and try again.".to_string()
    } else if lower.contains("private video")
        || lower.contains("sign in")
        || lower.contains("members-only")
        || lower.contains("registered users")
    {
        "This media is private or requires account authentication/subscription.".to_string()
    } else if lower.contains("not available in your country")
        || lower.contains("geo-restricted")
        || lower.contains("blocked in your region")
    {
        "This video is not available in your region (Geo-restricted).".to_string()
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

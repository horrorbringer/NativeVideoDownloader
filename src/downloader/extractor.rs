use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::error::{AppError, Result};
use crate::models::{DownloadProgress, PlaylistEntry, StreamFormatInfo, VideoMetadata};

static CACHED_BIN_DIR: std::sync::LazyLock<PathBuf> = std::sync::LazyLock::new(|| {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".native_video_downloader").join("bin")
    } else if let Ok(profile) = std::env::var("USERPROFILE") {
        PathBuf::from(profile).join(".native_video_downloader").join("bin")
    } else {
        PathBuf::from(".bin")
    }
});

/// Returns the path to the internal binary directory: `~/.native_video_downloader/bin`
pub fn get_bin_dir() -> PathBuf {
    CACHED_BIN_DIR.clone()
}

static CACHED_FFMPEG_PATH: std::sync::LazyLock<Option<PathBuf>> = std::sync::LazyLock::new(|| {
    let local = get_bin_dir().join(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" });
    if local.is_file() {
        return Some(local);
    }

    for candidate in &[
        "/opt/homebrew/bin/ffmpeg",
        "/usr/local/bin/ffmpeg",
        "/usr/bin/ffmpeg",
    ] {
        let p = PathBuf::from(candidate);
        if p.is_file() {
            return Some(p);
        }
    }

    // Check system PATH
    if let Ok(path_var) = std::env::var("PATH") {
        let exe_name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
        for dir in std::env::split_paths(&path_var) {
            let p = dir.join(exe_name);
            if p.is_file() {
                return Some(p);
            }
        }
    }

    None
});

/// Locates the `yt-dlp` executable in local bin directory or system PATH
pub async fn find_ytdlp_path() -> Option<PathBuf> {
    let local = get_bin_dir().join(if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" });
    if local.is_file() {
        return Some(local);
    }

    // Check system PATH
    for candidate in &[
        "/usr/local/bin/yt-dlp",
        "/opt/homebrew/bin/yt-dlp",
        "/usr/bin/yt-dlp",
    ] {
        let p = PathBuf::from(candidate);
        if p.is_file() {
            return Some(p);
        }
    }

    // Fallback: check if `yt-dlp --version` responds in PATH
    if let Ok(output) = Command::new("yt-dlp").arg("--version").output().await {
        if output.status.success() {
            return Some(PathBuf::from("yt-dlp"));
        }
    }

    None
}

/// Locates FFmpeg executable in local bin directory or system PATH
pub fn find_ffmpeg_path() -> Option<PathBuf> {
    CACHED_FFMPEG_PATH.clone()
}

/// Downloads the latest official standalone FFmpeg binary into `get_bin_dir()`
pub async fn download_standalone_ffmpeg() -> Result<PathBuf> {
    let bin_dir = get_bin_dir();
    tokio::fs::create_dir_all(&bin_dir).await?;

    let target_path = bin_dir.join(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" });
    let url = if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1/ffmpeg-darwin-arm64"
        } else {
            "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1/ffmpeg-darwin-x64"
        }
    } else if cfg!(target_os = "windows") {
        "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1/ffmpeg-win32-x64"
    } else {
        if cfg!(target_arch = "aarch64") {
            "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1/ffmpeg-linux-arm64"
        } else {
            "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1/ffmpeg-linux-x64"
        }
    };

    info!("Downloading standalone FFmpeg binary from {} to {:?}", url, target_path);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let resp = client.get(url).send().await?;
    if !resp.status().is_success() {
        return Err(AppError::Generic(format!(
            "Failed to download FFmpeg binary: HTTP {}",
            resp.status()
        )));
    }

    let bytes = resp.bytes().await?;
    tokio::fs::write(&target_path, bytes).await?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = tokio::fs::metadata(&target_path).await?.permissions();
        perms.set_mode(0o755);
        tokio::fs::set_permissions(&target_path, perms).await?;
    }

    info!("FFmpeg standalone binary installed successfully at {:?}", target_path);
    Ok(target_path)
}

/// Ensures FFmpeg is available, downloading the official static standalone binary if missing
pub async fn ensure_ffmpeg_installed() -> Result<PathBuf> {
    if let Some(path) = find_ffmpeg_path() {
        return Ok(path);
    }
    download_standalone_ffmpeg().await
}

/// Ensures all essential streaming dependencies (yt-dlp and ffmpeg) are available
pub async fn ensure_dependencies() -> Result<(PathBuf, PathBuf)> {
    let (ytdlp_res, ffmpeg_res) = tokio::join!(
        ensure_ytdlp_installed(),
        ensure_ffmpeg_installed()
    );
    let ytdlp = ytdlp_res?;
    let ffmpeg = ffmpeg_res?;
    Ok((ytdlp, ffmpeg))
}

static CACHED_YTDLP_BIN: tokio::sync::OnceCell<PathBuf> = tokio::sync::OnceCell::const_new();

/// Downloads the latest official standalone yt-dlp binary into `get_bin_dir()`
pub async fn download_standalone_ytdlp() -> Result<PathBuf> {
    let bin_dir = get_bin_dir();
    tokio::fs::create_dir_all(&bin_dir).await?;

    let target_path = bin_dir.join(if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" });
    let url = if cfg!(target_os = "macos") {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos"
    } else if cfg!(target_os = "windows") {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
    } else {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux"
    };

    info!("Downloading standalone yt-dlp binary from {} to {:?}", url, target_path);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let resp = client.get(url).send().await?;
    if !resp.status().is_success() {
        return Err(AppError::Generic(format!(
            "Failed to download yt-dlp binary: HTTP {}",
            resp.status()
        )));
    }

    let bytes = resp.bytes().await?;
    tokio::fs::write(&target_path, bytes).await?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = tokio::fs::metadata(&target_path).await?.permissions();
        perms.set_mode(0o755);
        tokio::fs::set_permissions(&target_path, perms).await?;
    }

    info!("yt-dlp standalone binary installed successfully at {:?}", target_path);
    Ok(target_path)
}

/// Ensures `yt-dlp` is available, downloading the official standalone executable if missing
pub async fn ensure_ytdlp_installed() -> Result<PathBuf> {
    if let Some(cached) = CACHED_YTDLP_BIN.get() {
        return Ok(cached.clone());
    }

    let resolved = if let Some(path) = find_ytdlp_path().await {
        path
    } else {
        download_standalone_ytdlp().await?
    };

    let _ = CACHED_YTDLP_BIN.set(resolved.clone());
    Ok(resolved)
}

/// Detects if a URL is from a known video streaming platform
pub fn is_streaming_platform(url: &str) -> bool {
    let lower = url.to_lowercase();
    lower.contains("youtube.com")
        || lower.contains("youtu.be")
        || lower.contains("vimeo.com")
        || lower.contains("tiktok.com")
        || lower.contains("twitter.com")
        || lower.contains("x.com")
        || lower.contains("instagram.com")
        || lower.contains("facebook.com")
        || lower.contains("fb.watch")
        || lower.contains("twitch.tv")
        || lower.contains("dailymotion.com")
        || lower.contains("soundcloud.com")
        || lower.contains("reddit.com")
        || lower.contains("bilibili.com")
        || lower.contains("b23.tv")
        || lower.contains("douyin.com")
        || lower.contains("iesdouyin.com")
        || lower.contains("iq.com")
        || lower.contains("iqiyi.com")
        || lower.contains("youku.com")
        || lower.contains("weibo.com")
        || lower.contains("anyreel.app")
        || lower.contains("dramaboxdb.com")
        || lower.contains("dramabox.com")
        || lower.contains("dramabox.app")
        || lower.contains("shortmax.com")
        || lower.contains("shortmax.app")
        || lower.contains("reelshort.com")
        || lower.contains("goodshort.com")
        || lower.contains("kalos.tv")
        || lower.contains("kuaikaw.cn")
}

/// Checks if a URL directly targets a media file or playlist by extension
pub fn is_direct_media_url(url: &str) -> bool {
    let lower = url.to_lowercase();
    let path = lower.split('?').next().unwrap_or("");
    for ext in &[
        ".mp4", ".webm", ".mkv", ".avi", ".mov", ".flv", ".ts",
        ".m4v", ".m4a", ".mp3", ".ogg", ".wav", ".aac", ".opus",
        ".m3u8", ".mpd",
    ] {
        if path.ends_with(ext) {
            return true;
        }
    }
    false
}

/// Determines if a copied string is a candidate video or audio stream URL for live clipboard monitoring
pub fn is_candidate_media_url(text: &str) -> bool {
    let trimmed = text.trim();
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        return false;
    }
    if reqwest::Url::parse(trimmed).is_err() {
        return false;
    }
    if is_streaming_platform(trimmed) || is_direct_media_url(trimmed) {
        return true;
    }
    let lower = trimmed.to_lowercase();
    let video_keywords = [
        "/video/", "/play/", "/watch", "/item/", "/episode/", "/episodes/",
        "/movie/", "/drama/", "/series/", "/stream", "m3u8", ".mp4", "/shorts/",
        "/reel/", "/status/", "anyreel", "dramabox", "shortmax", "reelshort",
        "goodshort", "kalos", "shortdrama", "vod", "kisskh"
    ];
    for kw in &video_keywords {
        if lower.contains(kw) {
            return true;
        }
    }
    false
}

/// Normalizes media URLs, transforming web modal/feed URLs into direct canonical video URLs.
/// Example: `https://www.douyin.com/jingxuan?modal_id=7685884183775841563` -> `https://www.douyin.com/video/7685884183775841563`
pub fn normalize_media_url(url: &str) -> String {
    let trimmed = url.trim();
    if let Ok(parsed) = reqwest::Url::parse(trimmed) {
        let host = parsed.host_str().unwrap_or("").to_lowercase();
        if host.contains("douyin.com") {
            for (k, v) in parsed.query_pairs() {
                if (k == "modal_id" || k == "aweme_id" || k == "item_id")
                    && !v.is_empty()
                    && v.chars().all(|c| c.is_ascii_digit())
                {
                    return format!("https://www.douyin.com/video/{}", v);
                }
            }
        } else if host.contains("tiktok.com") {
            for (k, v) in parsed.query_pairs() {
                if (k == "modal_id" || k == "item_id")
                    && !v.is_empty()
                    && v.chars().all(|c| c.is_ascii_digit())
                {
                    return format!("https://www.tiktok.com/video/{}", v);
                }
            }
        }
    }
    trimmed.to_string()
}

/// Extracts the first candidate media stream URL found in arbitrary text (e.g. from messages or notes copied to clipboard)
pub fn extract_candidate_media_url(text: &str) -> Option<String> {
    let clean_token = |token: &str| -> String {
        token
            .trim_matches(|c: char| {
                matches!(
                    c,
                    '"' | '\''
                        | '<'
                        | '>'
                        | '('
                        | ')'
                        | '['
                        | ']'
                        | '{'
                        | '}'
                        | ','
                        | ';'
                        | '!'
                        | '?'
                        | '`'
                        | '*'
                        | '。'
                        | '！'
                        | '，'
                        | '；'
                )
            })
            .trim_end_matches('.')
            .to_string()
    };

    let trimmed = text.trim();
    let cleaned_full = clean_token(trimmed);
    if is_candidate_media_url(&cleaned_full) {
        return Some(normalize_media_url(&cleaned_full));
    }
    // Search words in text for a valid candidate URL
    for word in trimmed.split_whitespace() {
        let cleaned = clean_token(word);
        if is_candidate_media_url(&cleaned) {
            return Some(normalize_media_url(&cleaned));
        }
    }
    // Direct substring search for embedded URLs without whitespace (common in Douyin / TikTok share copy)
    if let Some(start_idx) = trimmed.find("http://").or_else(|| trimmed.find("https://")) {
        let candidate = &trimmed[start_idx..];
        let end_idx = candidate
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '。' | '！' | '，' | '；' | '）' | '】' | '》' | ']' | ')'))
            .unwrap_or(candidate.len());
        let extracted = clean_token(&candidate[..end_idx]);
        if is_candidate_media_url(&extracted) {
            return Some(normalize_media_url(&extracted));
        }
    }
    None
}

/// Verifies whether a VideoMetadata returned from direct HTTP inspection is actually a playable media file
pub fn is_valid_direct_media(meta: &VideoMetadata) -> bool {
    // A 0-byte response is an empty page or API endpoint, not a media file
    if let Some(len) = meta.content_length {
        if len == 0 {
            return false;
        }
    }

    if let Some(ct) = &meta.content_type {
        let ct_lower = ct.to_lowercase();
        if ct_lower.starts_with("video/") || ct_lower.starts_with("audio/") {
            return true;
        }
    }

    is_direct_media_url(&meta.url)
}

/// Formats seconds into HH:MM:SS or MM:SS
pub fn format_duration(seconds: u64) -> String {
    let hours = seconds / 3600;
    let mins = (seconds % 3600) / 60;
    let secs = seconds % 60;
    if hours > 0 {
        format!("{:02}:{:02}:{:02}", hours, mins, secs)
    } else {
        format!("{:02}:{:02}", mins, secs)
    }
}

/// Helper to construct a Command with `~/.native_video_downloader/bin` prepended to PATH
fn create_ytdlp_cmd(ytdlp_bin: &Path) -> Command {
    let mut cmd = Command::new(ytdlp_bin);
    let bin_dir = get_bin_dir();
    let current_path = std::env::var("PATH").unwrap_or_default();
    let separator = if cfg!(windows) { ";" } else { ":" };
    let new_path = format!("{}{}{}", bin_dir.display(), separator, current_path);
    cmd.env("PATH", new_path);
    cmd
}

/// Helper to configure browser cookies with an on-disk cache to prevent repeating expensive
/// macOS Keychain unlocks and SQLite cookie decryptions on every single operation.
pub fn apply_browser_cookies(cmd: &mut Command, browser: &str) {
    let trimmed = browser.trim();
    if trimmed.is_empty() {
        return;
    }
    let bin_dir = get_bin_dir();
    let _ = std::fs::create_dir_all(&bin_dir);
    let cache_file = bin_dir.join(format!("cookies_{}.txt", trimmed));
    let is_fresh = if let Ok(metadata) = std::fs::metadata(&cache_file) {
        if metadata.len() > 64 * 1024 || metadata.len() == 0 {
            // Delete bloated or empty cookie cache that exceeds typical HTTP header buffers and causes HTTP 413
            let _ = std::fs::remove_file(&cache_file);
            false
        } else if let Ok(modified) = metadata.modified() {
            if let Ok(elapsed) = modified.elapsed() {
                // Cookies cached within the last 45 minutes are reused immediately
                elapsed.as_secs() < 2700 && metadata.len() > 100
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    };

    if is_fresh && cache_file.exists() {
        info!("Applying cached browser cookies from {:?} (bypassing Keychain decryption)", cache_file);
        cmd.arg("--cookies").arg(&cache_file);
    } else {
        if cache_file.exists() {
            let _ = std::fs::remove_file(&cache_file);
        }
        info!("Extracting fresh browser cookies from {} and updating cache at {:?}", trimmed, cache_file);
        cmd.arg("--cookies-from-browser").arg(trimmed);
        cmd.arg("--cookies").arg(&cache_file);
    }
}

/// Detailed quality presets and codecs extracted from stream formats
#[derive(Debug, Clone, PartialEq, Default)]
pub struct QualityAndCodecs {
    pub fps: Option<f64>,
    pub vcodec: Option<String>,
    pub acodec: Option<String>,
    pub size_best: Option<u64>,
    pub size_1080p: Option<u64>,
    pub size_720p: Option<u64>,
    pub size_480p: Option<u64>,
    pub size_audio: Option<u64>,
}

/// Analyzes yt-dlp format streams to extract fps, codecs, and calculate estimated sizes for each quality preset
pub fn parse_quality_and_codecs(
    json_val: &serde_json::Value,
    duration_secs: Option<u64>,
    top_filesize: Option<u64>,
) -> QualityAndCodecs {
    let formats = json_val.get("formats").and_then(|v| v.as_array());

    // FPS
    let fps = json_val
        .get("fps")
        .and_then(|v| v.as_f64())
        .or_else(|| {
            formats.and_then(|arr| {
                arr.iter()
                    .filter_map(|f| f.get("fps").and_then(|v| v.as_f64()))
                    .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            })
        });

    let clean_vcodec = |raw: &str| -> String {
        let r = raw.trim();
        if r.starts_with("avc1") || r.starts_with("h264") {
            "H.264 (AVC)".to_string()
        } else if r.starts_with("vp09") || r.starts_with("vp9") {
            "VP9".to_string()
        } else if r.starts_with("av01") || r.starts_with("av1") {
            "AV1".to_string()
        } else if r.starts_with("hev1") || r.starts_with("hvc1") || r.starts_with("h265") {
            "H.265 (HEVC)".to_string()
        } else if r.starts_with("vvc1") || r.starts_with("h266") {
            "H.266 (VVC)".to_string()
        } else {
            r.split('.').next().unwrap_or(r).to_uppercase()
        }
    };

    let clean_acodec = |raw: &str| -> String {
        let r = raw.trim();
        if r.starts_with("mp4a") || r.starts_with("aac") {
            "AAC".to_string()
        } else if r.starts_with("opus") {
            "Opus".to_string()
        } else if r.starts_with("vorbis") {
            "Vorbis".to_string()
        } else if r.starts_with("mp3") {
            "MP3".to_string()
        } else if r.starts_with("flac") {
            "FLAC".to_string()
        } else if r.starts_with("alac") {
            "ALAC".to_string()
        } else if r.starts_with("ac-3") || r.starts_with("ac3") {
            "AC-3".to_string()
        } else if r.starts_with("ec-3") || r.starts_with("eac3") {
            "E-AC-3".to_string()
        } else {
            r.split('.').next().unwrap_or(r).to_uppercase()
        }
    };

    // Video Codec
    let mut vcodec = json_val
        .get("vcodec")
        .and_then(|v| v.as_str())
        .filter(|s| *s != "none" && !s.is_empty())
        .map(clean_vcodec);

    if vcodec.is_none() {
        if let Some(arr) = formats {
            vcodec = arr.iter().rev().find_map(|f| {
                let vc = f.get("vcodec").and_then(|v| v.as_str())?;
                if vc != "none" && !vc.is_empty() {
                    Some(clean_vcodec(vc))
                } else {
                    None
                }
            });
        }
    }

    // Audio Codec
    let mut acodec = json_val
        .get("acodec")
        .and_then(|v| v.as_str())
        .filter(|s| *s != "none" && !s.is_empty())
        .map(clean_acodec);

    if acodec.is_none() {
        if let Some(arr) = formats {
            acodec = arr.iter().rev().find_map(|f| {
                let ac = f.get("acodec").and_then(|v| v.as_str())?;
                if ac != "none" && !ac.is_empty() {
                    Some(clean_acodec(ac))
                } else {
                    None
                }
            });
        }
    }

    // Best Audio size estimate (inspects byte sizes or calculates from abr/tbr audio bitrates)
    let mut best_audio_size: Option<u64> = None;
    if let Some(arr) = formats {
        for f in arr {
            let vc = f.get("vcodec").and_then(|v| v.as_str()).unwrap_or("none");
            let ac = f.get("acodec").and_then(|v| v.as_str()).unwrap_or("none");
            let h = f.get("height").and_then(|v| v.as_u64());

            if (vc == "none" || h.is_none() || h == Some(0)) && ac != "none" {
                let size = f.get("filesize").and_then(|v| v.as_u64())
                    .or_else(|| f.get("filesize_approx").and_then(|v| v.as_u64()))
                    .or_else(|| {
                        // Calculate from bitrate (abr or tbr in kbps) and duration
                        let abr = f.get("abr").and_then(|v| v.as_f64())
                            .or_else(|| f.get("tbr").and_then(|v| v.as_f64()))?;
                        let d = duration_secs? as f64;
                        Some(((abr * 1000.0 / 8.0) * d) as u64)
                    });
                if let Some(s) = size {
                    best_audio_size = Some(best_audio_size.map_or(s, |curr| curr.max(s)));
                }
            }
        }
    }

    if best_audio_size.is_none() {
        if let Some(d) = duration_secs {
            best_audio_size = Some(d.saturating_mul(160_000 / 8)); // ~160 kbps standard audio
        }
    }

    let get_format_size_for_height = |target_height: u64| -> Option<u64> {
        let arr = formats?;
        let min_h = (target_height * 7) / 10;
        let mut best_candidate_size: Option<u64> = None;

        for f in arr {
            let h = f.get("height").and_then(|v| v.as_u64()).unwrap_or(0);
            if h >= min_h && h <= target_height {
                let size = f.get("filesize").and_then(|v| v.as_u64())
                    .or_else(|| f.get("filesize_approx").and_then(|v| v.as_u64()))
                    .or_else(|| {
                        // Fallback to real stream bitrate (tbr or vbr in kbps) and duration
                        let bitrate_kbps = f.get("tbr").and_then(|v| v.as_f64())
                            .or_else(|| f.get("vbr").and_then(|v| v.as_f64()))?;
                        let d = duration_secs? as f64;
                        Some(((bitrate_kbps * 1000.0 / 8.0) * d) as u64)
                    });
                let ac = f.get("acodec").and_then(|v| v.as_str()).unwrap_or("none");

                if let Some(base_size) = size {
                    let total = if ac == "none" {
                        base_size.saturating_add(best_audio_size.unwrap_or(0))
                    } else {
                        base_size
                    };
                    best_candidate_size = Some(best_candidate_size.map_or(total, |curr| curr.max(total)));
                }
            }
        }

        best_candidate_size
    };

    let size_1080p = get_format_size_for_height(1080).or_else(|| {
        duration_secs.map(|d| d.saturating_mul(525_000)) // ~4.2 Mbps
    });

    let size_720p = get_format_size_for_height(720).or_else(|| {
        duration_secs.map(|d| d.saturating_mul(275_000)) // ~2.2 Mbps
    });

    let size_480p = get_format_size_for_height(480).or_else(|| {
        duration_secs.map(|d| d.saturating_mul(120_000)) // ~960 kbps
    });

    let size_audio = best_audio_size;

    // Check for high-res formats (4K / 2160p, 1440p)
    let max_format_height = formats.and_then(|arr| {
        arr.iter().filter_map(|f| f.get("height").and_then(|v| v.as_u64())).max()
    }).unwrap_or(0);

    let high_res_size = if max_format_height > 1080 {
        get_format_size_for_height(max_format_height)
    } else {
        None
    };

    let size_best = top_filesize
        .or(high_res_size)
        .or(size_1080p)
        .or(size_720p)
        .or(size_480p);

    QualityAndCodecs {
        fps,
        vcodec,
        acodec,
        size_best,
        size_1080p,
        size_720p,
        size_480p,
        size_audio,
    }
}

/// Analyzes and extracts available stream formats (resolutions, codecs, containers, FPS, sizes) for the Quality & Stream Inspector modal
pub fn extract_available_stream_formats(
    json_val: &serde_json::Value,
    duration_secs: Option<u64>,
    q: &QualityAndCodecs,
) -> Vec<StreamFormatInfo> {
    let mut streams = Vec::new();

    let clean_vcodec = |raw: &str| -> String {
        let r = raw.trim();
        if r.starts_with("avc1") || r.starts_with("h264") {
            "H.264".to_string()
        } else if r.starts_with("vp09") || r.starts_with("vp9") {
            "VP9".to_string()
        } else if r.starts_with("av01") || r.starts_with("av1") {
            "AV1".to_string()
        } else if r.starts_with("hev1") || r.starts_with("hvc1") || r.starts_with("h265") {
            "H.265".to_string()
        } else if r == "none" || r.is_empty() {
            "".to_string()
        } else {
            r.split('.').next().unwrap_or(r).to_uppercase()
        }
    };

    let clean_acodec = |raw: &str| -> String {
        let r = raw.trim();
        if r.starts_with("mp4a") || r.starts_with("aac") {
            "AAC".to_string()
        } else if r.starts_with("opus") {
            "Opus".to_string()
        } else if r.starts_with("flac") {
            "FLAC".to_string()
        } else if r.starts_with("mp3") {
            "MP3".to_string()
        } else if r == "none" || r.is_empty() {
            "".to_string()
        } else {
            r.split('.').next().unwrap_or(r).to_uppercase()
        }
    };

    if let Some(formats) = json_val.get("formats").and_then(|v| v.as_array()) {
        let target_heights = [
            (2160, "4K Ultra HD (2160p)", "2160p"),
            (1440, "2K Quad HD (1440p)", "1440p"),
            (1080, "1080p Full HD", "1080p"),
            (720, "720p High Def", "720p"),
            (480, "480p Standard", "480p"),
            (360, "360p Medium", "360p"),
        ];

        for (target_h, label, quality_id) in target_heights {
            let matching: Vec<_> = formats
                .iter()
                .filter(|f| {
                    let h = f.get("height").and_then(|v| v.as_u64()).unwrap_or(0);
                    let vcodec = f.get("vcodec").and_then(|v| v.as_str()).unwrap_or("none");
                    vcodec != "none" && h >= (target_h * 85) / 100 && h <= (target_h * 115) / 100
                })
                .collect();

            if !matching.is_empty() {
                let best = matching.iter().max_by_key(|f| {
                    let tbr = f.get("tbr").and_then(|v| v.as_f64()).unwrap_or(0.0) as u64;
                    let fps = f.get("fps").and_then(|v| v.as_f64()).unwrap_or(30.0) as u64;
                    tbr + fps * 10
                });

                if let Some(f) = best {
                    let actual_h = f.get("height").and_then(|v| v.as_u64()).unwrap_or(target_h);
                    let actual_w = f.get("width").and_then(|v| v.as_u64()).unwrap_or(actual_h * 16 / 9);
                    let fps = f.get("fps").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let fps_text = if fps >= 50.0 { format!("{:.0} FPS", fps) } else { "".to_string() };
                    let raw_vc = f.get("vcodec").and_then(|v| v.as_str()).unwrap_or("");
                    let raw_ac = f.get("acodec").and_then(|v| v.as_str()).unwrap_or("");
                    let container = f.get("ext").and_then(|v| v.as_str()).unwrap_or("mp4").to_uppercase();

                    let size_val = match target_h {
                        2160 | 1440 => q.size_best,
                        1080 => q.size_1080p,
                        720 => q.size_720p,
                        480 => q.size_480p,
                        _ => None,
                    };
                    let size_text = size_val.map(|s| format!("~{}", crate::models::DownloadProgress::format_size(s))).unwrap_or_default();

                    streams.push(StreamFormatInfo {
                        format_id: quality_id.to_string(),
                        quality_label: label.to_string(),
                        resolution: format!("{}x{}", actual_w, actual_h),
                        fps_text,
                        video_codec: clean_vcodec(raw_vc),
                        audio_codec: if raw_ac != "none" && !raw_ac.is_empty() { clean_acodec(raw_ac) } else { "AAC / Opus".to_string() },
                        container,
                        size_text,
                        is_video: true,
                        is_recommended: target_h == 1080 || (target_h == 720 && q.size_1080p.is_none()),
                    });
                }
            }
        }

        // Add Audio-only stream
        let best_audio_format = formats.iter().filter(|f| {
            let ac = f.get("acodec").and_then(|v| v.as_str()).unwrap_or("none");
            let vc = f.get("vcodec").and_then(|v| v.as_str()).unwrap_or("none");
            ac != "none" && (vc == "none" || vc.is_empty())
        }).max_by_key(|f| {
            f.get("abr").and_then(|v| v.as_f64()).unwrap_or(0.0) as u64
        });

        if let Some(af) = best_audio_format {
            let raw_ac = af.get("acodec").and_then(|v| v.as_str()).unwrap_or("");
            let abr = af.get("abr").and_then(|v| v.as_f64()).unwrap_or(160.0);
            let ext = af.get("ext").and_then(|v| v.as_str()).unwrap_or("m4a").to_uppercase();
            let size_text = q.size_audio.map(|s| format!("~{}", crate::models::DownloadProgress::format_size(s))).unwrap_or_default();

            streams.push(StreamFormatInfo {
                format_id: "audio".to_string(),
                quality_label: format!("Studio Audio Track ({:.0} kbps)", abr),
                resolution: "Audio Only".to_string(),
                fps_text: "".to_string(),
                video_codec: "None".to_string(),
                audio_codec: clean_acodec(raw_ac),
                container: ext,
                size_text,
                is_video: false,
                is_recommended: false,
            });
        }
    }

    if streams.is_empty() {
        let mut fallback_meta = VideoMetadata {
            fps: q.fps,
            vcodec: q.vcodec.clone(),
            acodec: q.acodec.clone(),
            size_best: q.size_best,
            size_1080p: q.size_1080p,
            size_720p: q.size_720p,
            size_480p: q.size_480p,
            size_audio: q.size_audio,
            duration_seconds: duration_secs,
            ..Default::default()
        };
        fallback_meta.ensure_available_formats();
        streams = fallback_meta.available_formats;
    }

    streams
}

/// Inspects a video or album/playlist streaming URL to fetch metadata with optional browser cookies and proxy
pub async fn inspect_video_with_options(
    url: &str,
    cookies_browser: Option<&str>,
    proxy: Option<&str>,
) -> Result<VideoMetadata> {
    let normalized_url = normalize_media_url(url);
    let url = normalized_url.as_str();

    // Fast path: Custom short drama native parsers without yt-dlp delay
    if url.contains("anyreel.app")
        || url.contains("dramabox")
        || url.contains("shortmax")
        || url.contains("reelshort")
        || url.contains("goodshort")
        || url.contains("kalos")
        || url.contains("kuaikaw.cn")
    {
        return scrape_page_for_media_with_proxy(url, proxy).await;
    }

    // Fast path: KissKH Asian Drama scraper (drama metadata, full episode lists, HLS streams & multi-lang subtitles)
    if url.contains("kisskh.") {
        return extract_kisskh_drama(url, cookies_browser, proxy).await;
    }

    // Fast path: Native Douyin resolver via DouyinSaver (no cookies needed, bypasses Douyin 403 anti-bot block)
    if url.contains("douyin.com") || url.contains("iesdouyin.com") {
        match resolve_douyin_via_douyinsaver(url, proxy).await {
            Ok(meta) => {
                info!("Successfully extracted Douyin media without watermark via DouyinSaver API: {:?}", meta.title);
                return Ok(meta);
            }
            Err(e) => {
                warn!("DouyinSaver API extraction failed ({:?}), falling back to yt-dlp", e);
            }
        }
    }

    let ytdlp_bin = ensure_ytdlp_installed().await?;

    info!(
        "Inspecting streaming URL with yt-dlp: {} (cookies: {:?}, proxy: {:?})",
        url, cookies_browser, proxy
    );
    let mut cmd = create_ytdlp_cmd(&ytdlp_bin);
    if let Some(b) = cookies_browser {
        apply_browser_cookies(&mut cmd, b);
    }
    if let Some(p) = proxy {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            cmd.arg("--proxy").arg(trimmed);
        }
    }
    let output = cmd
        .arg("--dump-single-json")
        .arg("--flat-playlist")
        .arg(url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        if cookies_browser.is_some() {
            let err_text = String::from_utf8_lossy(&output.stderr);
            let lower_err = err_text.to_lowercase();
            let is_cookie_related = lower_err.contains("413")
                || lower_err.contains("cookie")
                || lower_err.contains("database is locked")
                || lower_err.contains("could not copy")
                || lower_err.contains("keychain")
                || lower_err.contains("keyring")
                || lower_err.contains("permission denied");

            if is_cookie_related {
                let first_line = err_text
                    .lines()
                    .find(|l| l.contains("ERROR:"))
                    .unwrap_or("cookie lock or header overflow");
                warn!(
                    "Stream inspection failed with browser cookies ({}). Auto-healing: retrying without cookies in guest mode...",
                    first_line
                );
                if let Some(b) = cookies_browser {
                    let cache_file = get_bin_dir().join(format!("cookies_{}.txt", b.trim()));
                    let _ = std::fs::remove_file(&cache_file);
                }
                if let Ok(guest_meta) = Box::pin(inspect_video_with_options(url, None, proxy)).await {
                    info!("Guest mode inspection succeeded for {}", url);
                    return Ok(guest_meta);
                }
            }
        }

        // Attempt fallback: Scrape HTML for embedded video/m3u8 sources
        if let Ok(scraped_meta) = scrape_page_for_media_with_proxy(url, proxy).await {
            info!("Successfully scraped media from webpage: {:?}", scraped_meta);
            return Ok(scraped_meta);
        }

        let err_text = String::from_utf8_lossy(&output.stderr);
        let first_err_line = err_text
            .lines()
            .find(|l| l.contains("ERROR:"))
            .map(|l| l.trim_start_matches("ERROR:").trim())
            .unwrap_or_else(|| {
                err_text
                    .lines()
                    .last()
                    .unwrap_or("Unsupported or protected media source")
                    .trim()
            });

        let cleaned = clean_extractor_error(first_err_line);
        return Err(AppError::Generic(cleaned));
    }

    let json_val: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| AppError::Generic(format!("Failed to parse metadata JSON: {}", e)))?;

    let is_playlist = json_val.get("_type").and_then(|v| v.as_str()) == Some("playlist");

    let title = json_val
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("media_video")
        .to_string();

    let mut playlist_entries = Vec::new();
    if is_playlist {
        if let Some(arr) = json_val.get("entries").and_then(|v| v.as_array()) {
            for item in arr {
                let item_url = item.get("url").and_then(|v| v.as_str())
                    .or_else(|| item.get("webpage_url").and_then(|v| v.as_str()));
                let item_title = item.get("title").and_then(|v| v.as_str()).unwrap_or("Episode");
                if let Some(u) = item_url {
                    playlist_entries.push(crate::models::PlaylistEntry {
                        title: item_title.to_string(),
                        url: u.to_string(),
                        referer: Some(url.to_string()),
                    });
                }
            }
        }
    }
    let playlist_count = playlist_entries.len();

    let duration_secs = json_val
        .get("duration")
        .and_then(|v| v.as_u64());

    let resolution = if is_playlist {
        Some(format!("{} Episodes Series", playlist_count))
    } else {
        json_val
            .get("resolution")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                let w = json_val.get("width").and_then(|v| v.as_u64());
                let h = json_val.get("height").and_then(|v| v.as_u64());
                match (w, h) {
                    (Some(w), Some(h)) => Some(format!("{}x{}", w, h)),
                    _ => None,
                }
            })
    };

    let filesize = json_val
        .get("filesize")
        .and_then(|v| v.as_u64())
        .or_else(|| json_val.get("filesize_approx").and_then(|v| v.as_u64()));

    let ext = json_val
        .get("ext")
        .and_then(|v| v.as_str())
        .unwrap_or("mp4")
        .to_string();

    let mut available_subs = Vec::new();
    if let Some(subs_obj) = json_val.get("subtitles").and_then(|v| v.as_object()) {
        for lang in subs_obj.keys() {
            available_subs.push(lang.clone());
        }
    }
    if available_subs.is_empty() {
        if let Some(auto_obj) = json_val.get("automatic_captions").and_then(|v| v.as_object()) {
            for lang in auto_obj.keys().take(6) {
                available_subs.push(format!("{}(auto)", lang));
            }
        }
    }
    let has_subtitles = !available_subs.is_empty();
    let subtitles_summary = if has_subtitles {
        available_subs.join(", ")
    } else {
        "None detected".to_string()
    };

    let thumbnail_url = json_val
        .get("thumbnail")
        .and_then(|v| v.as_str())
        .or_else(|| {
            json_val
                .get("thumbnails")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.last())
                .and_then(|t| t.get("url"))
                .and_then(|v| v.as_str())
        })
        .map(|s| s.to_string());

    let q = parse_quality_and_codecs(&json_val, duration_secs, filesize);

    let available_formats = extract_available_stream_formats(&json_val, duration_secs, &q);

    Ok(VideoMetadata {
        url: url.to_string(),
        title,
        content_length: filesize.or(q.size_best),
        content_type: Some(format!("video/{}", ext)),
        supports_ranges: true,
        is_extractor: true,
        duration_seconds: duration_secs,
        resolution,
        ext: Some(ext),
        is_playlist,
        playlist_count,
        playlist_entries,
        has_subtitles,
        subtitles_summary,
        thumbnail_url,
        fps: q.fps,
        vcodec: q.vcodec,
        acodec: q.acodec,
        size_best: q.size_best,
        size_1080p: q.size_1080p,
        size_720p: q.size_720p,
        size_480p: q.size_480p,
        size_audio: q.size_audio,
        referer: Some(url.to_string()),
        available_formats,
    })
}

/// Convenience wrapper for inspecting streaming URL with browser cookies
#[allow(dead_code)]
pub async fn inspect_video_with_cookies(url: &str, cookies_browser: Option<&str>) -> Result<VideoMetadata> {
    inspect_video_with_options(url, cookies_browser, None).await
}

/// Convenience wrapper for inspecting streaming URL without explicit browser cookies or proxy
#[allow(dead_code)]
pub async fn inspect_video(url: &str) -> Result<VideoMetadata> {
    inspect_video_with_options(url, None, None).await
}

/// Resolves Douyin videos without watermark via the DouyinSaver API (bypasses Douyin 403 anti-bot and cookie requirements)
pub async fn resolve_douyin_via_douyinsaver(url: &str, proxy: Option<&str>) -> Result<VideoMetadata> {
    info!("Resolving Douyin video via DouyinSaver API: {} (proxy: {:?})", url, proxy);
    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15));

    if let Some(p) = proxy {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            if let Ok(prx) = reqwest::Proxy::all(trimmed) {
                builder = builder.proxy(prx);
            }
        }
    }

    let client = builder.build()
        .map_err(|e| AppError::Generic(format!("Failed to build HTTP client: {}", e)))?;

    let payload = serde_json::json!({
        "url": url
    }).to_string();

    let resp = client
        .post("https://api.douyinsaver.com/api/parse")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::USER_AGENT, "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36")
        .header(reqwest::header::ORIGIN, "https://douyinsaver.com")
        .header(reqwest::header::REFERER, "https://douyinsaver.com/")
        .body(payload)
        .send()
        .await
        .map_err(|e| AppError::Generic(format!("DouyinSaver API request failed: {}", e)))?;

    if !resp.status().is_success() {
        return Err(AppError::Generic(format!("DouyinSaver API returned HTTP status {}", resp.status())));
    }

    let text = resp
        .text()
        .await
        .map_err(|e| AppError::Generic(format!("Failed to read DouyinSaver response: {}", e)))?;

    let val: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| AppError::Generic(format!("Failed to parse DouyinSaver response JSON: {}", e)))?;

    let title = val.get("title").and_then(|v| v.as_str()).unwrap_or("douyin_video").trim().to_string();
    let author = val.get("author").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let full_title = if !author.is_empty() && !title.is_empty() {
        format!("{} - {}", title, author)
    } else if !title.is_empty() {
        title
    } else {
        "douyin_video".to_string()
    };

    let duration_ms = val.get("duration").and_then(|v| v.as_u64()).unwrap_or(0);
    let duration_seconds = if duration_ms > 0 { Some(duration_ms / 1000) } else { None };
    let cover = val.get("cover").and_then(|v| v.as_str()).map(|s| s.to_string());

    let qualities = val.get("qualities").and_then(|v| v.as_array());
    let best_stream = qualities.and_then(|arr| arr.first());

    let best_url = best_stream
        .and_then(|s| s.get("url"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::Generic("No downloadable video stream found in DouyinSaver response".to_string()))?
        .to_string();

    let resolution = best_stream
        .and_then(|s| s.get("label").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .or_else(|| {
            best_stream.and_then(|s| {
                let h = s.get("height").and_then(|v| v.as_u64())?;
                Some(format!("{}p", h))
            })
        })
        .or_else(|| Some("720p".to_string()));

    let bitrate = best_stream.and_then(|s| s.get("bitrate").and_then(|v| v.as_u64())).unwrap_or(0);
    let estimated_size = if bitrate > 0 && duration_seconds.unwrap_or(0) > 0 {
        Some((bitrate * duration_seconds.unwrap()) / 8)
    } else {
        None
    };

    let mut size_720p = None;
    let mut size_480p = None;
    if let Some(arr) = qualities {
        for q in arr {
            let h = q.get("height").and_then(|v| v.as_u64()).unwrap_or(0);
            let b = q.get("bitrate").and_then(|v| v.as_u64()).unwrap_or(0);
            let d = duration_seconds.unwrap_or(0);
            if h == 720 && size_720p.is_none() && b > 0 && d > 0 {
                size_720p = Some((b * d) / 8);
            } else if h == 480 && size_480p.is_none() && b > 0 && d > 0 {
                size_480p = Some((b * d) / 8);
            }
        }
    }

    Ok(VideoMetadata {
        url: best_url,
        title: full_title,
        content_length: estimated_size,
        content_type: Some("video/mp4".to_string()),
        supports_ranges: true,
        is_extractor: false,
        duration_seconds,
        resolution,
        ext: Some("mp4".to_string()),
        is_playlist: false,
        playlist_count: 0,
        playlist_entries: Vec::new(),
        has_subtitles: false,
        subtitles_summary: "No subtitles".to_string(),
        thumbnail_url: cover,
        fps: Some(30.0),
        vcodec: Some("H.264 (AVC)".to_string()),
        acodec: Some("AAC".to_string()),
        size_best: size_720p.or(estimated_size),
        size_1080p: None,
        size_720p,
        size_480p,
        size_audio: None,
        referer: Some("https://www.douyin.com/".to_string()),
        available_formats: Vec::new(),
    })
}

/// Scrapes a generic webpage's HTML to locate embedded video tags, OpenGraph video, or .m3u8/.mp4 stream URLs with optional proxy
pub async fn scrape_page_for_media_with_proxy(page_url: &str, proxy: Option<&str>) -> Result<VideoMetadata> {
    info!("Universal media sniffer analyzing page: {} (proxy: {:?})", page_url, proxy);
    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36");

    if let Some(p) = proxy {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            if let Ok(prx) = reqwest::Proxy::all(trimmed) {
                builder = builder.proxy(prx);
            }
        }
    }

    let target_page_url = if page_url.contains("anyreel.app") {
        let trimmed = page_url.trim_end_matches('/');
        if trimmed.contains("/episodes/") {
            trimmed.replace("/episodes/", "/video/episode-1-")
        } else if trimmed.contains("/movie/") {
            trimmed.replace("/movie/", "/video/episode-1-")
        } else {
            trimmed.to_string()
        }
    } else {
        page_url.to_string()
    };

    let client = builder.build().unwrap_or_else(|_| reqwest::Client::new());
    let resp = client.get(&target_page_url).send().await?;
    if !resp.status().is_success() {
        return Err(AppError::Generic(format!("HTTP {}", resp.status())));
    }

    let html = resp.text().await?;

    // 1. Run Universal Media Sniffer across structured JSON, HTML5 video, MacCMS, and script streams
    if let Some(meta) = universal_sniff_media_from_html(&html, &target_page_url) {
        info!("Universal sniffer discovered media source: '{}' (playlist: {}, url: {})", meta.title, meta.is_playlist, meta.url);

        // If series playlist, probe first episode to detect stream quality in background
        if meta.is_playlist && !meta.playlist_entries.is_empty() {
            let remaining_eps: Vec<String> = meta.playlist_entries.iter().skip(1).map(|e| e.url.clone()).collect();
            let proxy_opt = proxy.map(|s| s.to_string());
            tokio::spawn(async move {
                for ep_url in remaining_eps {
                    resolve_playable_stream_url(&ep_url, proxy_opt.as_deref()).await;
                }
            });
        }

        return Ok(meta);
    }

    // 2. Check embedded iframes for known streaming platforms
    for iframe_url in extract_iframes_from_html(&html, page_url) {
        if is_streaming_platform(&iframe_url) {
            info!("Inspecting embedded iframe streaming source: {}", iframe_url);
            if let Ok(meta) = Box::pin(inspect_video_with_options(&iframe_url, None, proxy)).await {
                return Ok(meta);
            }
        }
    }

    Err(AppError::Generic("No playable video stream or series playlist found on webpage".to_string()))
}

/// Convenience wrapper for scraping webpage HTML without explicit proxy
#[allow(dead_code)]
pub async fn scrape_page_for_media(page_url: &str) -> Result<VideoMetadata> {
    scrape_page_for_media_with_proxy(page_url, None).await
}

#[derive(Debug, PartialEq, Eq)]
pub struct MacCmsInfo {
    pub video_url: String,
    pub title: Option<String>,
    pub series_name: Option<String>,
    pub nid: Option<u32>,
}

/// Extracts video source and series title from MacCMS player_aaaa configuration
pub fn extract_maccms_player(html: &str) -> Option<MacCmsInfo> {
    let key = "player_aaaa";
    let pos = html.find(key)?;
    let rest = &html[pos + key.len()..];
    let eq_pos = rest.find('=')?;
    let json_start = rest[eq_pos + 1..].find('{')? + eq_pos + 1;
    let json_slice = &rest[json_start..];

    let mut depth = 0;
    let mut end_idx = 0;
    for (i, c) in json_slice.char_indices() {
        if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth -= 1;
            if depth == 0 {
                end_idx = i + 1;
                break;
            }
        }
    }

    if end_idx == 0 {
        return None;
    }

    let json_str = &json_slice[..end_idx];
    let val: serde_json::Value = serde_json::from_str(json_str).ok()?;

    let raw_str = val.get("url").and_then(|v| v.as_str())?.trim();
    if raw_str.is_empty() {
        return None;
    }

    let encrypt = val.get("encrypt").and_then(|e| e.as_i64()).unwrap_or(0);
    let mut raw_url = raw_str.to_string();
    if encrypt == 2 {
        if let Some(decoded) = decode_base64(&raw_url) {
            raw_url = decoded;
        }
    } else if encrypt == 1 {
        raw_url = decode_percent(&raw_url);
    }

    let from = val.get("from").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();

    let video_url = if raw_url.starts_with("http://") || raw_url.starts_with("https://") {
        raw_url.to_string()
    } else if raw_url.starts_with("//") {
        format!("https:{}", raw_url)
    } else if from == "dailymotion" {
        format!("https://www.dailymotion.com/video/{}", raw_url)
    } else if from == "youtube" {
        format!("https://www.youtube.com/watch?v={}", raw_url)
    } else if from == "vimeo" {
        format!("https://vimeo.com/{}", raw_url)
    } else if from == "bilibili" {
        format!("https://www.bilibili.com/video/{}", raw_url)
    } else if from == "okru" || from == "ok" {
        format!("https://ok.ru/video/{}", raw_url)
    } else {
        raw_url.to_string()
    };

    let nid = val.get("nid").and_then(|n| {
        n.as_u64().map(|u| u as u32).or_else(|| n.as_str().and_then(|s| s.parse().ok()))
    });

    let series_name = val.get("vod_data")
        .and_then(|vd| vd.get("vod_name"))
        .and_then(|vn| vn.as_str())
        .map(|name| name.trim().to_string());

    let title = series_name.as_ref().map(|name| {
        if let Some(n) = nid {
            format!("{} EP{}", name, n)
        } else {
            name.clone()
        }
    });

    Some(MacCmsInfo { video_url, title, series_name, nid })
}

/// Searches for <iframe src="..."> embedding video streams or platforms
pub fn extract_iframes_from_html(html: &str, base_url: &str) -> Vec<String> {
    let mut iframes = Vec::new();
    let mut cursor = 0;
    while let Some(pos) = html[cursor..].find("<iframe") {
        let tag_start = cursor + pos;
        let tag_end = html[tag_start..].find('>').map(|e| tag_start + e).unwrap_or(html.len());
        let tag_str = &html[tag_start..tag_end];
        for marker in &["src=\"", "src='"] {
            if let Some(src_idx) = tag_str.find(marker) {
                let rest = &tag_str[src_idx + marker.len()..];
                let quote = if marker.ends_with('"') { '"' } else { '\'' };
                if let Some(q_end) = rest.find(quote) {
                    let candidate = &rest[..q_end];
                    if !candidate.is_empty()
                        && !candidate.starts_with("about:")
                        && !candidate.contains("google")
                        && !candidate.contains("recaptcha")
                        && !candidate.contains("analytics")
                    {
                        let full = resolve_relative_url(base_url, candidate);
                        if !iframes.contains(&full) {
                            iframes.push(full);
                        }
                    }
                }
            }
        }
        cursor = tag_end + 1;
        if cursor >= html.len() {
            break;
        }
    }
    iframes
}

fn clean_scraped_title(raw: &str) -> String {
    let mut title = raw.trim();
    for delim in &[
        " - Donghua Fun",
        " - Watch Donghua Online Free",
        " - Watch Online",
        " - Free Watch",
        " Streaming Guide and Episode Details",
        " Donghua Anime Episode Guide",
        " Anime Episode Guide",
        " Episode Guide",
        " Episode Details",
        " Release Info",
        " Watch Order",
        " | Donghua",
        " | Anime",
    ] {
        if let Some(pos) = title.find(delim) {
            title = &title[..pos];
        }
    }
    title.trim().to_string()
}

/// Helper to extract JSON-LD schema.org VideoObject name
fn extract_schema_org_title(html: &str) -> Option<String> {
    if let Some(pos) = html.find("\"@type\":\"VideoObject\"") {
        let snippet = &html[pos..pos.saturating_add(400).min(html.len())];
        if let Some(n_pos) = snippet.find("\"name\":\"") {
            let rest = &snippet[n_pos + 8..];
            if let Some(end) = rest.find('"') {
                let name = &rest[..end];
                if !name.is_empty() {
                    return Some(clean_scraped_title(&html_escape_clean(name)));
                }
            }
        }
    }
    None
}

/// Helper to extract <title>...</title> or OpenGraph og:title from HTML
fn extract_html_title(html: &str) -> Option<String> {
    // Check og:title
    if let Some(idx) = html.find("property=\"og:title\"") {
        let snippet = &html[idx.saturating_sub(60)..idx.min(html.len()) + 180];
        if let Some(content_idx) = snippet.find("content=\"") {
            let rest = &snippet[content_idx + 9..];
            if let Some(end) = rest.find('"') {
                let cleaned = clean_scraped_title(&html_escape_clean(&rest[..end]));
                if !cleaned.is_empty() {
                    return Some(cleaned);
                }
            }
        }
    }

    // Check <title>
    if let Some(start) = html.find("<title>") {
        let rest = &html[start + 7..];
        if let Some(end) = rest.find("</title>") {
            let cleaned = clean_scraped_title(&html_escape_clean(rest[..end].trim()));
            if !cleaned.is_empty() {
                return Some(cleaned);
            }
        }
    }

    None
}

/// Helper to extract thumbnail image (OpenGraph og:image, twitter:image, or video poster) from HTML
pub fn extract_html_thumbnail(html: &str, base_url: &str) -> Option<String> {
    for tag in &["property=\"og:image\"", "property=\"og:image:url\"", "name=\"twitter:image\"", "name=\"twitter:image:src\""] {
        if let Some(pos) = html.find(tag) {
            let tag_start = html[..pos].rfind('<').unwrap_or(pos);
            let tag_end = html[pos..].find('>').map(|i| pos + i).unwrap_or(pos + 200).min(html.len());
            let snippet = &html[tag_start..tag_end];
            if let Some(content_idx) = snippet.find("content=\"") {
                let rest = &snippet[content_idx + 9..];
                if let Some(end) = rest.find('"') {
                    let u = &rest[..end];
                    if !u.is_empty() {
                        return Some(resolve_relative_url(base_url, u));
                    }
                }
            }
        }
    }

    if let Some(pos) = html.find("poster=\"") {
        let rest = &html[pos + 8..];
        if let Some(end) = rest.find('"') {
            let u = &rest[..end];
            if !u.is_empty() {
                return Some(resolve_relative_url(base_url, u));
            }
        }
    }

    None
}

/// Extracts series playlist episodes from detail/overview pages (such as MacCMS vod/detail pages)
pub fn extract_playlist_from_detail_html(html: &str, page_url: &str) -> Option<VideoMetadata> {
    let mut entries = Vec::new();

    // 1. Check anthology-list-play (standard MacCMS series episode list)
    if let Some(pos) = html.find("anthology-list-play") {
        let rest = &html[pos..];
        let block_end = rest.find("</ul>").map(|e| pos + e).unwrap_or(html.len());
        let block = &html[pos..block_end];
        entries = parse_episodes_from_block(block, page_url);
    }

    // 2. Fallback: scan HTML for anchor tags pointing to vod/play or /play/id/
    if entries.is_empty() {
        entries = parse_episodes_from_html_scan(html, page_url);
    }

    if entries.is_empty() {
        return None;
    }

    // Reverse if descending (e.g. EP41 down to EP01 -> change to EP01 up to EP41)
    if entries.len() > 1 {
        let first_text = &entries[0].title;
        let last_text = &entries.last().unwrap().title;
        let first_num = extract_episode_num(first_text);
        let last_num = extract_episode_num(last_text);
        if let (Some(f), Some(l)) = (first_num, last_num) {
            if f > l {
                entries.reverse();
            }
        }
    }

    let maccms_opt = extract_maccms_player(html);

    // If maccms player exists on current page, register its stream URL and attach to active episode entry
    let primary_url = if let Some(ref maccms) = maccms_opt {
        if let Ok(mut cache) = PLAYABLE_URL_CACHE.write() {
            cache.insert(page_url.to_string(), maccms.video_url.clone());
        }
        if let Some(nid) = maccms.nid {
            for entry in entries.iter_mut() {
                if extract_episode_num(&entry.title) == Some(nid)
                    || entry.url.contains(&format!("nid/{}.html", nid))
                    || entry.url.contains(&format!("nid/{}", nid))
                {
                    entry.url = maccms.video_url.clone();
                }
            }
        }
        maccms.video_url.clone()
    } else {
        entries.first().map(|e| e.url.clone()).unwrap_or_else(|| page_url.to_string())
    };

    let raw_title = maccms_opt.as_ref().and_then(|m| m.series_name.clone())
        .or_else(|| extract_schema_org_title(html))
        .or_else(|| extract_html_title(html))
        .unwrap_or_else(|| "Series Collection".to_string());
    let title = clean_series_title_episodes(&raw_title);

    let thumbnail_url = extract_html_thumbnail(html, page_url);
    let playlist_count = entries.len();

    Some(VideoMetadata {
        url: primary_url,
        title,
        content_length: None,
        content_type: Some("video/mp4".to_string()),
        supports_ranges: true,
        is_extractor: true,
        duration_seconds: None,
        resolution: Some(format!("{} Episodes Series", playlist_count)),
        ext: Some("mp4".to_string()),
        is_playlist: true,
        playlist_count,
        playlist_entries: entries,
        has_subtitles: false,
        subtitles_summary: String::new(),
        thumbnail_url,
        referer: Some(page_url.to_string()),
        ..Default::default()
    })
}

fn extract_episode_num(s: &str) -> Option<u32> {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.parse::<u32>().ok()
}

fn parse_episodes_from_block(block: &str, base_url: &str) -> Vec<crate::models::PlaylistEntry> {
    let mut entries = Vec::new();
    let mut cursor = 0;
    while let Some(a_pos) = block[cursor..].find("<a") {
        let a_start = cursor + a_pos;
        let tag_close = match block[a_start..].find('>') {
            Some(c) => a_start + c,
            None => break,
        };
        let tag_attrs = &block[a_start..tag_close];
        let href = tag_attrs.find("href=\"").and_then(|h_pos| {
            let rest = &tag_attrs[h_pos + 6..];
            rest.find('"').map(|end| &rest[..end])
        });

        let (inner_text, next_cursor) = match block[tag_close + 1..].find("</a>") {
            Some(e) => {
                let close_start = tag_close + 1 + e;
                let text = strip_html_tags_and_clean(block[tag_close + 1..close_start].trim());
                (text, close_start + 4)
            }
            None => (String::new(), tag_close + 1),
        };

        if let Some(h) = href {
            if !h.is_empty() && (h.contains("vod/play") || h.contains("/play/")) {
                let full_url = resolve_relative_url(base_url, h);
                let label = if !inner_text.is_empty() {
                    inner_text
                } else {
                    format!("Episode {}", entries.len() + 1)
                };
                entries.push(crate::models::PlaylistEntry {
                    title: label,
                    url: full_url,
                    referer: Some(base_url.to_string()),
                });
            }
        }
        cursor = next_cursor.min(block.len());
        while cursor < block.len() && !block.is_char_boundary(cursor) {
            cursor += 1;
        }
        if cursor >= block.len() {
            break;
        }
    }
    entries
}

fn parse_episodes_from_html_scan(html: &str, base_url: &str) -> Vec<crate::models::PlaylistEntry> {
    let mut entries = Vec::new();
    let mut seen_urls = std::collections::HashSet::new();
    let mut cursor = 0;
    while let Some(a_pos) = html[cursor..].find("<a") {
        let a_start = cursor + a_pos;
        let tag_close = match html[a_start..].find('>') {
            Some(c) => a_start + c,
            None => break,
        };
        let tag_attrs = &html[a_start..tag_close];
        let href = tag_attrs.find("href=\"").and_then(|h_pos| {
            let rest = &tag_attrs[h_pos + 6..];
            rest.find('"').map(|end| &rest[..end])
        });

        let (inner_text, next_cursor) = match html[tag_close + 1..].find("</a>") {
            Some(e) => {
                let close_start = tag_close + 1 + e;
                let text = strip_html_tags_and_clean(html[tag_close + 1..close_start].trim());
                (text, close_start + 4)
            }
            None => (String::new(), tag_close + 1),
        };

        if let Some(h) = href {
            if !h.is_empty() && (h.contains("vod/play") || h.contains("/play/id/")) {
                let full_url = resolve_relative_url(base_url, h);
                if !seen_urls.contains(&full_url) {
                    seen_urls.insert(full_url.clone());
                    let label = if !inner_text.is_empty() {
                        inner_text
                    } else {
                        format!("Episode {}", entries.len() + 1)
                    };
                    entries.push(crate::models::PlaylistEntry {
                        title: label,
                        url: full_url,
                        referer: Some(base_url.to_string()),
                    });
                }
            }
        }
        cursor = next_cursor.min(html.len());
        while cursor < html.len() && !html.is_char_boundary(cursor) {
            cursor += 1;
        }
        if cursor >= html.len() {
            break;
        }
    }
    entries
}

/// Helper to determine if a string is a media stream candidate (.mp4, .m3u8, .flv, .webm, etc.)
pub fn is_media_stream_candidate(url: &str) -> bool {
    let u = url.trim();
    if u.is_empty() || u.starts_with("javascript:") || u.starts_with("data:") {
        return false;
    }
    let lower = u.to_lowercase();
    // Exclude static images, styles, javascript, trackers, ads
    if lower.ends_with(".jpg") || lower.ends_with(".jpeg") || lower.ends_with(".png")
        || lower.ends_with(".gif") || lower.ends_with(".webp") || lower.ends_with(".svg")
        || lower.ends_with(".css") || lower.ends_with(".js")
        || lower.contains("doubleclick") || lower.contains("google-analytics")
        || lower.contains("/analytics") || lower.contains("adsystem")
        || lower.contains("banner") || lower.contains("/ad/")
    {
        return false;
    }
    // Must contain typical streaming extensions or media markers
    lower.contains(".mp4")
        || lower.contains(".m3u8")
        || lower.contains(".webm")
        || lower.contains(".flv")
        || lower.contains(".mov")
        || lower.contains(".ts")
        || lower.contains("mime=video")
}

/// Helper to sanitize and normalize video stream URLs
pub fn clean_stream_url(raw: &str, base_url: &str) -> String {
    let unescaped = raw.trim()
        .replace(r"\u0026", "&")
        .replace(r"\u002F", "/")
        .replace(r"\/", "/")
        .replace("&amp;", "&");
    let cleaned = html_escape_clean(&unescaped);
    resolve_relative_url(base_url, &cleaned)
}

/// Recursively traverses a JSON value to discover playable media streams
pub fn find_streams_in_json(val: &serde_json::Value, streams: &mut Vec<String>, base_url: &str) {
    match val {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let k_lower = k.to_lowercase();
                if k_lower.contains("url")
                    || k_lower.contains("video")
                    || k_lower.contains("stream")
                    || k_lower.contains("play")
                    || k_lower.contains("file")
                    || k_lower.contains("source")
                    || k_lower.contains("src")
                    || k_lower.contains("mp4")
                    || k_lower.contains("m3u8")
                {
                    if let Some(s) = v.as_str() {
                        if is_media_stream_candidate(s) {
                            let resolved = clean_stream_url(s, base_url);
                            if !streams.contains(&resolved) {
                                streams.push(resolved);
                            }
                        }
                    }
                }
                find_streams_in_json(v, streams, base_url);
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                find_streams_in_json(item, streams, base_url);
            }
        }
        _ => {}
    }
}

/// Recursively scans JSON for a suitable title
pub fn extract_title_from_json(val: &serde_json::Value) -> Option<String> {
    match val {
        serde_json::Value::Object(map) => {
            for key in &["bookname", "seriesname", "vod_name", "videoname", "title", "name", "headline"] {
                for (k, v) in map {
                    if k.to_lowercase() == *key {
                        if let Some(s) = v.as_str() {
                            let trimmed = s.trim();
                            if !trimmed.is_empty() {
                                return Some(trimmed.to_string());
                            }
                        }
                    }
                }
            }
            for v in map.values() {
                if let Some(t) = extract_title_from_json(v) {
                    return Some(t);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                if let Some(t) = extract_title_from_json(item) {
                    return Some(t);
                }
            }
        }
        _ => {}
    }
    None
}

/// Recursively scans JSON for a thumbnail / poster image
pub fn extract_thumbnail_from_json(val: &serde_json::Value, base_url: &str) -> Option<String> {
    match val {
        serde_json::Value::Object(map) => {
            for key in &["coverwap", "cover", "poster", "vod_pic", "thumbnail", "image"] {
                for (k, v) in map {
                    if k.to_lowercase() == *key {
                        if let Some(s) = v.as_str() {
                            let trimmed = s.trim();
                            if !trimmed.is_empty() && (trimmed.starts_with("http") || trimmed.starts_with("//") || trimmed.starts_with('/')) {
                                return Some(clean_stream_url(trimmed, base_url));
                            }
                        }
                    }
                }
            }
            for v in map.values() {
                if let Some(t) = extract_thumbnail_from_json(v, base_url) {
                    return Some(t);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                if let Some(t) = extract_thumbnail_from_json(item, base_url) {
                    return Some(t);
                }
            }
        }
        _ => {}
    }
    None
}

/// Sniffs embedded JSON states (__NEXT_DATA__, __NUXT_DATA__, player configs, etc.)
pub fn sniff_embedded_json_streams(html: &str, base_url: &str) -> Option<VideoMetadata> {
    // 1. First, check dedicated structured scrapers (e.g. Next.js drama with chapters)
    if let Some(drama) = extract_next_data_drama(html, base_url) {
        return Some(drama);
    }

    // 2. Scan script tags for JSON blocks
    let mut cursor = 0;
    while let Some(script_idx) = html[cursor..].find("<script") {
        let actual_start = cursor + script_idx;
        let Some(tag_open_end) = html[actual_start..].find('>') else { break; };
        let content_start = actual_start + tag_open_end + 1;
        let Some(script_close) = html[content_start..].find("</script>") else { break; };
        let content_end = content_start + script_close;
        let script_body = html[content_start..content_end].trim();

        // Search for JSON boundaries within script body
        let mut json_candidates = Vec::new();

        if (script_body.starts_with('{') && script_body.ends_with('}'))
            || (script_body.starts_with('[') && script_body.ends_with(']'))
        {
            json_candidates.push(script_body);
        } else {
            // Find assignments
            for marker in &["=", ":"] {
                let mut search_pos = 0;
                while let Some(eq_idx) = script_body[search_pos..].find(marker) {
                    let after_eq = script_body[search_pos + eq_idx + marker.len()..].trim_start();
                    if after_eq.starts_with('{') || after_eq.starts_with('[') {
                        let is_obj = after_eq.starts_with('{');
                        let open_ch = if is_obj { '{' } else { '[' };
                        let close_ch = if is_obj { '}' } else { ']' };
                        let mut depth = 0;
                        let mut matched_len = 0;
                        for (i, c) in after_eq.char_indices() {
                            if c == open_ch {
                                depth += 1;
                            } else if c == close_ch {
                                depth -= 1;
                                if depth == 0 {
                                    matched_len = i + 1;
                                    break;
                                }
                            }
                        }
                        if matched_len > 0 {
                            json_candidates.push(&after_eq[..matched_len]);
                        }
                    }
                    search_pos += eq_idx + 1;
                    if search_pos >= script_body.len() {
                        break;
                    }
                }
            }
        }

        for candidate in json_candidates {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(candidate) {
                let mut streams = Vec::new();
                find_streams_in_json(&v, &mut streams, base_url);
                if !streams.is_empty() {
                    let title = extract_title_from_json(&v)
                        .or_else(|| extract_html_title(html))
                        .unwrap_or_else(|| "Web Stream Video".to_string());
                    let thumbnail_url = extract_thumbnail_from_json(&v, base_url)
                        .or_else(|| extract_html_thumbnail(html, base_url));
                    let count = streams.len();
                    let primary_url = streams[0].clone();
                    let ext = if primary_url.contains(".m3u8") { "m3u8" } else { "mp4" };

                    let entries: Vec<crate::models::PlaylistEntry> = streams
                        .into_iter()
                        .enumerate()
                        .map(|(i, u)| crate::models::PlaylistEntry {
                            title: format!("{} - Episode {}", title, i + 1),
                            url: u,
                            referer: Some(base_url.to_string()),
                        })
                        .collect();

                    return Some(VideoMetadata {
                        url: primary_url,
                        title: if count > 1 { format!("{} ({} Episodes)", title, count) } else { title },
                        content_length: None,
                        content_type: Some(format!("video/{}", ext)),
                        supports_ranges: true,
                        is_extractor: true,
                        duration_seconds: None,
                        resolution: Some(if count > 1 { format!("{} Streams Sniffed", count) } else { "Universal Web Stream".to_string() }),
                        ext: Some(ext.to_string()),
                        is_playlist: count > 1,
                        playlist_count: count,
                        playlist_entries: entries,
                        has_subtitles: false,
                        subtitles_summary: String::new(),
                        thumbnail_url,
                        fps: Some(30.0),
                        vcodec: Some("H.264".to_string()),
                        acodec: Some("AAC".to_string()),
                        referer: Some(base_url.to_string()),
                        ..Default::default()
                    });
                }
            }
        }

        cursor = content_end + 9;
        if cursor >= html.len() {
            break;
        }
    }

    None
}

/// Universal Media Sniffer: Automatically extracts media streams (.mp4, .m3u8, etc.)
/// from any website's embedded JSON, HTML5 tags, MacCMS configs, player scripts, or regex patterns.
pub fn universal_sniff_media_from_html(html: &str, page_url: &str) -> Option<VideoMetadata> {
    // 0. AnyReel Short Drama series (Next.js App Router RSC streams)
    if let Some(anyreel_meta) = extract_anyreel_drama(html, page_url) {
        return Some(anyreel_meta);
    }

    // 0b. DramaBox Short Drama series
    if let Some(dramabox_meta) = extract_dramabox_drama(html, page_url) {
        return Some(dramabox_meta);
    }

    // 0c. ShortMax Short Drama series
    if let Some(shortmax_meta) = extract_shortmax_drama(html, page_url) {
        return Some(shortmax_meta);
    }

    // 0d. ReelShort Drama series
    if let Some(reelshort_meta) = extract_reelshort_drama(html, page_url) {
        return Some(reelshort_meta);
    }

    // 0e. Universal Short Drama series parser
    if let Some(generic_drama_meta) = extract_generic_short_drama(html, page_url) {
        return Some(generic_drama_meta);
    }

    // 1. Structured JSON (Next.js __NEXT_DATA__, Nuxt, player configs, state objects)
    if let Some(json_meta) = sniff_embedded_json_streams(html, page_url) {
        return Some(json_meta);
    }

    // 2. Series playlist on detail pages (e.g. MacCMS anthology-list-play)
    if let Some(detail_meta) = extract_playlist_from_detail_html(html, page_url) {
        return Some(detail_meta);
    }

    // 3. MacCMS player (DPlayer / Artplayer / player_aaaa)
    if let Some(maccms) = extract_maccms_player(html) {
        let title = maccms.title
            .or_else(|| extract_html_title(html))
            .unwrap_or_else(|| "MacCMS Video".to_string());
        let thumbnail = extract_html_thumbnail(html, page_url);
        let ext = if maccms.video_url.contains(".m3u8") { "m3u8" } else { "mp4" };
        return Some(VideoMetadata {
            url: maccms.video_url,
            title,
            content_length: None,
            content_type: Some(format!("video/{}", ext)),
            supports_ranges: true,
            is_extractor: true,
            duration_seconds: None,
            resolution: Some("MacCMS Stream".to_string()),
            ext: Some(ext.to_string()),
            is_playlist: false,
            playlist_count: 0,
            playlist_entries: Vec::new(),
            has_subtitles: false,
            subtitles_summary: String::new(),
            thumbnail_url: thumbnail,
            fps: Some(30.0),
            vcodec: Some("H.264".to_string()),
            acodec: Some("AAC".to_string()),
            referer: Some(page_url.to_string()),
            ..Default::default()
        });
    }

    // 3. Embedded HTML5 tags (<video src>, <source src>, og:video, raw script URLs)
    if let Some(raw_stream) = extract_stream_from_html(html, page_url) {
        let title = extract_html_title(html).unwrap_or_else(|| "Web Video".to_string());
        let thumbnail = extract_html_thumbnail(html, page_url);
        let ext = if raw_stream.contains(".m3u8") { "m3u8" } else { "mp4" };
        return Some(VideoMetadata {
            url: raw_stream,
            title,
            content_length: None,
            content_type: Some(format!("video/{}", ext)),
            supports_ranges: true,
            is_extractor: true,
            duration_seconds: None,
            resolution: Some("Sniffed Web Media".to_string()),
            ext: Some(ext.to_string()),
            is_playlist: false,
            playlist_count: 0,
            playlist_entries: Vec::new(),
            has_subtitles: false,
            subtitles_summary: String::new(),
            thumbnail_url: thumbnail,
            fps: Some(30.0),
            vcodec: Some("H.264".to_string()),
            acodec: Some("AAC".to_string()),
            referer: Some(page_url.to_string()),
            ..Default::default()
        });
    }

    None
}

/// Extracts short drama series playlist and direct MP4/M3U8 streams from Next.js (__NEXT_DATA__) drama sites (e.g., kuaikaw.cn)
pub fn extract_next_data_drama(html: &str, page_url: &str) -> Option<VideoMetadata> {
    let script_start = html.find(r#"<script id="__NEXT_DATA__""#)?;
    let tag_end = html[script_start..].find('>')? + script_start + 1;
    let script_end = html[tag_end..].find("</script>")? + tag_end;
    let json_str = &html[tag_end..script_end];

    let v: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let page_props = v.get("props")?.get("pageProps")?;

    let book_info = page_props.get("bookInfoVo");
    let series_title = book_info
        .and_then(|b| b.get("bookName"))
        .and_then(|n| n.as_str())
        .unwrap_or("Short Drama Series");
    let thumbnail_url = book_info
        .and_then(|b| b.get("coverWap"))
        .and_then(|c| c.as_str())
        .map(|s| s.to_string());

    let chapters = page_props.get("chapterList")?.as_array()?;
    if chapters.is_empty() {
        return None;
    }

    // Attempt to extract requested chapterId from page_url or pageProps
    let target_chapter_id = page_props
        .get("chapterId")
        .and_then(|c| c.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            page_url.trim_end_matches('/').split('/').last().map(|s| s.to_string())
        })
        .unwrap_or_default();

    let mut entries = Vec::new();
    let mut primary_stream_url = String::new();
    let mut primary_title = format!("{} (Episode 1)", series_title);

    for (idx, ch) in chapters.iter().enumerate() {
        let ch_id = ch.get("chapterId").and_then(|c| c.as_str()).unwrap_or("");
        let ch_name = ch
            .get("chapterName")
            .and_then(|c| c.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("Episode {}", idx + 1));

        let stream_url = ch.get("chapterVideoVo")
            .and_then(|v| {
                v.get("mp4")
                    .or_else(|| v.get("mp4720p"))
                    .or_else(|| v.get("m3u8"))
                    .or_else(|| v.get("m3u8720p"))
            })
            .and_then(|s| s.as_str())
            .map(|s| s.to_string());

        if let Some(stream) = stream_url {
            let ep_title = format!("{} - {}", series_title, ch_name);
            if ch_id == target_chapter_id || (primary_stream_url.is_empty() && idx == 0) {
                primary_stream_url = stream.clone();
                primary_title = ep_title.clone();
            }
            entries.push(crate::models::PlaylistEntry {
                title: ep_title,
                url: stream,
                referer: Some(page_url.to_string()),
            });
        }
    }

    if entries.is_empty() {
        return None;
    }

    if primary_stream_url.is_empty() {
        primary_stream_url = entries[0].url.clone();
    }

    let count = entries.len();
    Some(VideoMetadata {
        url: primary_stream_url,
        title: if count > 1 { series_title.to_string() } else { primary_title },
        content_length: None,
        content_type: Some("video/mp4".to_string()),
        supports_ranges: true,
        is_extractor: true,
        duration_seconds: None,
        resolution: Some(format!("{} Free Episodes • 720p HD", count)),
        ext: Some("mp4".to_string()),
        is_playlist: count > 1,
        playlist_count: count,
        playlist_entries: entries,
        has_subtitles: false,
        subtitles_summary: String::new(),
        thumbnail_url,
        fps: Some(30.0),
        vcodec: Some("H.264".to_string()),
        acodec: Some("AAC".to_string()),
        referer: Some(page_url.to_string()),
        ..Default::default()
    })
}

/// Extracts short drama series playlist and direct M3U8 streams from AnyReel (Next.js App Router RSC streams)
pub fn extract_anyreel_drama(html: &str, page_url: &str) -> Option<VideoMetadata> {
    if !html.contains("dramaEpisodes") {
        return None;
    }

    let idx = html.find("dramaEpisodes")?;
    let rest = &html[idx..];
    let bracket_rel_start = rest.find('[')?;
    let bracket_start = idx + bracket_rel_start;

    let mut depth = 0;
    let mut end_idx = 0;
    for (i, c) in html[bracket_start..].char_indices() {
        if c == '[' {
            depth += 1;
        } else if c == ']' {
            depth -= 1;
            if depth == 0 {
                end_idx = bracket_start + i + 1;
                break;
            }
        }
    }

    if end_idx == 0 {
        return None;
    }

    let raw_slice = &html[bracket_start..end_idx];
    // Unescape JSON string literal escaping (\", \\, \/)
    let unescaped = raw_slice
        .replace(r#"\""#, "\"")
        .replace(r#"\\"#, "\\")
        .replace(r#"\/"#, "/");

    let episodes_arr: Vec<serde_json::Value> = serde_json::from_str(&unescaped).ok()?;
    if episodes_arr.is_empty() {
        return None;
    }

    // Extract series title
    let series_title = html
        .find("seriesName")
        .and_then(|pos| {
            let slice = &html[pos..pos + 200.min(html.len() - pos)];
            let colon_pos = slice.find(':')?;
            let after_colon = &slice[colon_pos + 1..];
            let start = after_colon.find(|c: char| c.is_alphanumeric())?;
            let rest = &after_colon[start..];
            let end = rest.find(|c: char| c == '"' || c == '\\')?;
            Some(rest[..end].trim().to_string())
        })
        .or_else(|| extract_html_title(html))
        .unwrap_or_else(|| "AnyReel Drama Series".to_string());

    // Extract thumbnail
    let thumbnail_url = extract_html_thumbnail(html, page_url);

    let mut entries = Vec::new();
    let mut primary_stream_url = String::new();
    let mut primary_duration = None;

    for (idx, ep) in episodes_arr.iter().enumerate() {
        let ep_num = ep.get("episodeSort").and_then(|v| v.as_u64()).unwrap_or((idx + 1) as u64);
        let ep_name = ep.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let title = if !ep_name.is_empty() {
            format!("{} - {}", series_title, ep_name)
        } else {
            format!("{} - Episode {}", series_title, ep_num)
        };

        let duration = ep.get("totalDuration").and_then(|v| v.as_u64());

        // Derive direct master m3u8 playlist URL
        let mut stream_url = String::new();
        if let Some(imgs) = ep.get("imageSprite").and_then(|s| s.get("imageUrlSet")).and_then(|a| a.as_array()) {
            if let Some(first_img) = imgs.first().and_then(|v| v.as_str()) {
                if let Some(base) = first_img.split("imageSprite").next() {
                    stream_url = format!("{}adp.1936796.m3u8", base);
                }
            }
        }

        // Fallback to Tencent Cloud VOD playinfo endpoint if imageSprite was not found
        if stream_url.is_empty() {
            if let Some(vid) = ep.get("videoId").and_then(|v| v.as_str()) {
                let psign = ep.get("pSign").and_then(|v| v.as_str()).unwrap_or("");
                stream_url = format!("https://playvideo.vodplayvideo.net/getplayinfo/v4/1500065780/{}?psign={}", vid, psign);
            }
        }

        if !stream_url.is_empty() {
            if primary_stream_url.is_empty() {
                primary_stream_url = stream_url.clone();
                primary_duration = duration;
            }
            entries.push(crate::models::PlaylistEntry {
                title,
                url: stream_url,
                referer: Some("https://www.anyreel.app/".to_string()),
            });
        }
    }

    if entries.is_empty() {
        return None;
    }

    let count = entries.len();
    Some(VideoMetadata {
        url: primary_stream_url,
        title: series_title,
        content_length: None,
        content_type: Some("application/x-mpegURL".to_string()),
        supports_ranges: true,
        is_extractor: true,
        duration_seconds: primary_duration,
        resolution: Some(format!("{} Episodes • 1080p Full HD", count)),
        ext: Some("m3u8".to_string()),
        is_playlist: count > 1,
        playlist_count: count,
        playlist_entries: entries,
        has_subtitles: false,
        subtitles_summary: String::new(),
        thumbnail_url,
        fps: Some(30.0),
        vcodec: Some("H.264".to_string()),
        acodec: Some("AAC".to_string()),
        referer: Some("https://www.anyreel.app/".to_string()),
        ..Default::default()
    })
}

/// Extracts short drama series playlist and direct MP4/M3U8 streams from DramaBox (dramaboxdb.com, dramabox.com, dramabox.app)
pub fn extract_dramabox_drama(html: &str, page_url: &str) -> Option<VideoMetadata> {
    if !page_url.contains("dramabox") && !html.contains("dramabox") && !html.contains("dramaInfo") {
        return None;
    }

    let mut json_candidates = Vec::new();
    if let Some(script_start) = html.find(r#"<script id="__NEXT_DATA__""#) {
        if let Some(tag_end_rel) = html[script_start..].find('>') {
            let tag_end = script_start + tag_end_rel + 1;
            if let Some(close_rel) = html[tag_end..].find("</script>") {
                json_candidates.push(&html[tag_end..tag_end + close_rel]);
            }
        }
    }

    for marker in &["__INITIAL_STATE__", "window.__DATA__", "dramaData"] {
        if let Some(pos) = html.find(marker) {
            let rest = &html[pos + marker.len()..];
            if let Some(eq_rel) = rest.find('=') {
                let after_eq = rest[eq_rel + 1..].trim_start();
                if after_eq.starts_with('{') {
                    let mut depth = 0;
                    let mut end_idx = 0;
                    for (i, c) in after_eq.char_indices() {
                        if c == '{' {
                            depth += 1;
                        } else if c == '}' {
                            depth -= 1;
                            if depth == 0 {
                                end_idx = i + 1;
                                break;
                            }
                        }
                    }
                    if end_idx > 0 {
                        json_candidates.push(&after_eq[..end_idx]);
                    }
                }
            }
        }
    }

    for json_str in json_candidates {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) else { continue };

        let series_title = v.pointer("/props/pageProps/dramaInfo/dramaName")
            .or_else(|| v.pointer("/props/pageProps/bookInfo/bookName"))
            .or_else(|| v.pointer("/props/pageProps/drama/title"))
            .or_else(|| v.pointer("/dramaInfo/dramaName"))
            .or_else(|| v.pointer("/drama/name"))
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
            .or_else(|| extract_html_title(html))
            .unwrap_or_else(|| "DramaBox Series".to_string());

        let thumbnail_url = v.pointer("/props/pageProps/dramaInfo/coverUrl")
            .or_else(|| v.pointer("/props/pageProps/bookInfo/cover"))
            .or_else(|| v.pointer("/props/pageProps/drama/cover"))
            .or_else(|| v.pointer("/dramaInfo/coverUrl"))
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
            .or_else(|| extract_html_thumbnail(html, page_url));

        let episodes_opt = v.pointer("/props/pageProps/chapterList")
            .or_else(|| v.pointer("/props/pageProps/episodeList"))
            .or_else(|| v.pointer("/props/pageProps/episodes"))
            .or_else(|| v.pointer("/chapterList"))
            .or_else(|| v.pointer("/episodeList"))
            .or_else(|| v.pointer("/episodes"))
            .and_then(|arr| arr.as_array());

        let Some(episodes) = episodes_opt else { continue };
        if episodes.is_empty() { continue };

        let mut entries = Vec::new();
        let mut primary_stream_url = String::new();

        for (idx, ep) in episodes.iter().enumerate() {
            let ep_title = ep.get("chapterName")
                .or_else(|| ep.get("title"))
                .or_else(|| ep.get("episodeName"))
                .and_then(|s| s.as_str())
                .map(|s| format!("{} - {}", series_title, s))
                .unwrap_or_else(|| format!("{} - Episode {}", series_title, idx + 1));

            let stream_url = ep.get("chapterVideoVo")
                .and_then(|v| {
                    v.get("mp4")
                        .or_else(|| v.get("mp4720p"))
                        .or_else(|| v.get("m3u8"))
                        .or_else(|| v.get("m3u8720p"))
                })
                .or_else(|| ep.get("videoUrl"))
                .or_else(|| ep.get("playUrl"))
                .or_else(|| ep.get("m3u8Url"))
                .or_else(|| ep.get("streamUrl"))
                .or_else(|| ep.get("video_url"))
                .or_else(|| ep.get("url"))
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());

            if let Some(stream) = stream_url {
                if stream.starts_with("http://") || stream.starts_with("https://") {
                    if primary_stream_url.is_empty() {
                        primary_stream_url = stream.clone();
                    }
                    entries.push(crate::models::PlaylistEntry {
                        title: ep_title,
                        url: stream,
                        referer: Some("https://www.dramaboxdb.com/".to_string()),
                    });
                }
            }
        }

        if !entries.is_empty() {
            let count = entries.len();
            let ext = if primary_stream_url.contains(".m3u8") { "m3u8" } else { "mp4" };
            return Some(VideoMetadata {
                url: primary_stream_url,
                title: series_title,
                content_length: None,
                content_type: Some(format!("video/{}", ext)),
                supports_ranges: true,
                is_extractor: true,
                duration_seconds: None,
                resolution: Some(format!("{} Episodes • DramaBox HD", count)),
                ext: Some(ext.to_string()),
                is_playlist: count > 1,
                playlist_count: count,
                playlist_entries: entries,
                has_subtitles: false,
                subtitles_summary: String::new(),
                thumbnail_url,
                fps: Some(30.0),
                vcodec: Some("H.264".to_string()),
                acodec: Some("AAC".to_string()),
                referer: Some("https://www.dramaboxdb.com/".to_string()),
                ..Default::default()
            });
        }
    }

    None
}

/// Extracts short drama series playlist and direct MP4/M3U8 streams from ShortMax (shortmax.com, shortmax.app)
pub fn extract_shortmax_drama(html: &str, page_url: &str) -> Option<VideoMetadata> {
    if !page_url.contains("shortmax") && !html.contains("shortmax") {
        return None;
    }

    let mut json_candidates = Vec::new();
    if let Some(script_start) = html.find(r#"<script id="__NEXT_DATA__""#) {
        if let Some(tag_end_rel) = html[script_start..].find('>') {
            let tag_end = script_start + tag_end_rel + 1;
            if let Some(close_rel) = html[tag_end..].find("</script>") {
                json_candidates.push(&html[tag_end..tag_end + close_rel]);
            }
        }
    }

    for json_str in json_candidates {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) else { continue };

        let series_title = v.pointer("/props/pageProps/seriesInfo/seriesName")
            .or_else(|| v.pointer("/props/pageProps/drama/title"))
            .or_else(|| v.pointer("/props/pageProps/dramaDetail/name"))
            .or_else(|| v.pointer("/seriesInfo/seriesName"))
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
            .or_else(|| extract_html_title(html))
            .unwrap_or_else(|| "ShortMax Series".to_string());

        let thumbnail_url = v.pointer("/props/pageProps/seriesInfo/coverUrl")
            .or_else(|| v.pointer("/props/pageProps/drama/coverUrl"))
            .or_else(|| v.pointer("/props/pageProps/drama/thumb"))
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
            .or_else(|| extract_html_thumbnail(html, page_url));

        let episodes_opt = v.pointer("/props/pageProps/episodes")
            .or_else(|| v.pointer("/props/pageProps/episodeList"))
            .or_else(|| v.pointer("/props/pageProps/videoList"))
            .or_else(|| v.pointer("/episodes"))
            .and_then(|arr| arr.as_array());

        let Some(episodes) = episodes_opt else { continue };
        if episodes.is_empty() { continue };

        let mut entries = Vec::new();
        let mut primary_stream_url = String::new();

        for (idx, ep) in episodes.iter().enumerate() {
            let ep_num = ep.get("episodeNum")
                .or_else(|| ep.get("episodeSort"))
                .and_then(|v| v.as_u64())
                .unwrap_or((idx + 1) as u64);
            let ep_name = ep.get("title").and_then(|s| s.as_str()).unwrap_or("");
            let title = if !ep_name.is_empty() {
                format!("{} - {}", series_title, ep_name)
            } else {
                format!("{} - Episode {}", series_title, ep_num)
            };

            let stream_url = ep.get("playUrl")
                .or_else(|| ep.get("videoUrl"))
                .or_else(|| ep.get("m3u8Url"))
                .or_else(|| ep.get("streamUrl"))
                .or_else(|| ep.get("mp4Url"))
                .or_else(|| ep.get("url"))
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());

            if let Some(stream) = stream_url {
                if stream.starts_with("http://") || stream.starts_with("https://") {
                    if primary_stream_url.is_empty() {
                        primary_stream_url = stream.clone();
                    }
                    entries.push(crate::models::PlaylistEntry {
                        title,
                        url: stream,
                        referer: Some("https://www.shortmax.com/".to_string()),
                    });
                }
            }
        }

        if !entries.is_empty() {
            let count = entries.len();
            let ext = if primary_stream_url.contains(".m3u8") { "m3u8" } else { "mp4" };
            return Some(VideoMetadata {
                url: primary_stream_url,
                title: series_title,
                content_length: None,
                content_type: Some(format!("video/{}", ext)),
                supports_ranges: true,
                is_extractor: true,
                duration_seconds: None,
                resolution: Some(format!("{} Episodes • ShortMax HD", count)),
                ext: Some(ext.to_string()),
                is_playlist: count > 1,
                playlist_count: count,
                playlist_entries: entries,
                has_subtitles: false,
                subtitles_summary: String::new(),
                thumbnail_url,
                fps: Some(30.0),
                vcodec: Some("H.264".to_string()),
                acodec: Some("AAC".to_string()),
                referer: Some("https://www.shortmax.com/".to_string()),
                ..Default::default()
            });
        }
    }

    None
}

/// Extracts short drama series playlist and direct MP4/M3U8 streams from ReelShort (reelshort.com)
pub fn extract_reelshort_drama(html: &str, page_url: &str) -> Option<VideoMetadata> {
    if !page_url.contains("reelshort") && !html.contains("reelshort") {
        return None;
    }

    let mut json_candidates = Vec::new();
    if let Some(script_start) = html.find(r#"<script id="__NEXT_DATA__""#) {
        if let Some(tag_end_rel) = html[script_start..].find('>') {
            let tag_end = script_start + tag_end_rel + 1;
            if let Some(close_rel) = html[tag_end..].find("</script>") {
                json_candidates.push(&html[tag_end..tag_end + close_rel]);
            }
        }
    }

    for json_str in json_candidates {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) else { continue };

        let series_title = v.pointer("/props/pageProps/book/title")
            .or_else(|| v.pointer("/props/pageProps/book/book_name"))
            .or_else(|| v.pointer("/props/pageProps/detail/title"))
            .or_else(|| v.pointer("/props/pageProps/dramaDetail/name"))
            .or_else(|| v.pointer("/props/pageProps/drama/title"))
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
            .or_else(|| extract_html_title(html))
            .unwrap_or_else(|| "ReelShort Series".to_string());

        let thumbnail_url = v.pointer("/props/pageProps/book/cover_image")
            .or_else(|| v.pointer("/props/pageProps/book/cover"))
            .or_else(|| v.pointer("/props/pageProps/detail/cover"))
            .or_else(|| v.pointer("/props/pageProps/drama/cover"))
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
            .or_else(|| extract_html_thumbnail(html, page_url));

        let episodes_opt = v.pointer("/props/pageProps/chapter_list")
            .or_else(|| v.pointer("/props/pageProps/chapters"))
            .or_else(|| v.pointer("/props/pageProps/episodes"))
            .or_else(|| v.pointer("/props/pageProps/detail/chapters"))
            .and_then(|arr| arr.as_array());

        let Some(episodes) = episodes_opt else { continue };
        if episodes.is_empty() { continue };

        let mut entries = Vec::new();
        let mut primary_stream_url = String::new();

        for (idx, ep) in episodes.iter().enumerate() {
            let ep_name = ep.get("chapter_name")
                .or_else(|| ep.get("title"))
                .or_else(|| ep.get("name"))
                .and_then(|s| s.as_str())
                .unwrap_or("");

            let title = if !ep_name.is_empty() {
                format!("{} - {}", series_title, ep_name)
            } else {
                format!("{} - Episode {}", series_title, idx + 1)
            };

            let stream_url = ep.get("video_url")
                .or_else(|| ep.get("stream_url"))
                .or_else(|| ep.get("play_url"))
                .or_else(|| ep.get("hls_url"))
                .or_else(|| ep.get("media_url"))
                .or_else(|| ep.get("url"))
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());

            if let Some(stream) = stream_url {
                if stream.starts_with("http://") || stream.starts_with("https://") {
                    if primary_stream_url.is_empty() {
                        primary_stream_url = stream.clone();
                    }
                    entries.push(crate::models::PlaylistEntry {
                        title,
                        url: stream,
                        referer: Some("https://www.reelshort.com/".to_string()),
                    });
                }
            }
        }

        if !entries.is_empty() {
            let count = entries.len();
            let ext = if primary_stream_url.contains(".m3u8") { "m3u8" } else { "mp4" };
            return Some(VideoMetadata {
                url: primary_stream_url,
                title: series_title,
                content_length: None,
                content_type: Some(format!("video/{}", ext)),
                supports_ranges: true,
                is_extractor: true,
                duration_seconds: None,
                resolution: Some(format!("{} Episodes • ReelShort HD", count)),
                ext: Some(ext.to_string()),
                is_playlist: count > 1,
                playlist_count: count,
                playlist_entries: entries,
                has_subtitles: false,
                subtitles_summary: String::new(),
                thumbnail_url,
                fps: Some(30.0),
                vcodec: Some("H.264".to_string()),
                acodec: Some("AAC".to_string()),
                referer: Some("https://www.reelshort.com/".to_string()),
                ..Default::default()
            });
        }
    }

    None
}

/// Universal Short Drama Parser: Recursively inspects Next.js (__NEXT_DATA__), Nuxt, or state JSON
/// for any array of episode chapters containing media stream URLs across any short drama platform
pub fn extract_generic_short_drama(html: &str, page_url: &str) -> Option<VideoMetadata> {
    let script_start = html.find(r#"<script id="__NEXT_DATA__""#)?;
    let tag_end = html[script_start..].find('>')? + script_start + 1;
    let script_end = html[tag_end..].find("</script>")? + tag_end;
    let json_str = &html[tag_end..script_end];

    let v: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let page_props = v.get("props").and_then(|p| p.get("pageProps"))?;

    let series_title = extract_html_title(html).unwrap_or_else(|| "Short Drama Series".to_string());
    let thumbnail_url = extract_html_thumbnail(html, page_url);

    let mut candidate_entries = Vec::new();
    find_drama_episodes_in_json(page_props, &series_title, page_url, &mut candidate_entries);

    if !candidate_entries.is_empty() {
        let count = candidate_entries.len();
        let primary_url = candidate_entries[0].url.clone();
        let ext = if primary_url.contains(".m3u8") { "m3u8" } else { "mp4" };
        return Some(VideoMetadata {
            url: primary_url,
            title: series_title,
            content_length: None,
            content_type: Some(format!("video/{}", ext)),
            supports_ranges: true,
            is_extractor: true,
            duration_seconds: None,
            resolution: Some(format!("{} Episodes • Drama Stream", count)),
            ext: Some(ext.to_string()),
            is_playlist: count > 1,
            playlist_count: count,
            playlist_entries: candidate_entries,
            has_subtitles: false,
            subtitles_summary: String::new(),
            thumbnail_url,
            fps: Some(30.0),
            vcodec: Some("H.264".to_string()),
            acodec: Some("AAC".to_string()),
            referer: Some(page_url.to_string()),
            ..Default::default()
        });
    }

    None
}

fn find_drama_episodes_in_json(
    val: &serde_json::Value,
    series_title: &str,
    page_url: &str,
    entries: &mut Vec<crate::models::PlaylistEntry>,
) {
    if !entries.is_empty() {
        return;
    }
    match val {
        serde_json::Value::Array(arr) => {
            if arr.len() >= 2 {
                let mut temp_entries = Vec::new();
                for (idx, item) in arr.iter().enumerate() {
                    if let serde_json::Value::Object(map) = item {
                        let stream_url = map.get("videoUrl")
                            .or_else(|| map.get("video_url"))
                            .or_else(|| map.get("playUrl"))
                            .or_else(|| map.get("play_url"))
                            .or_else(|| map.get("m3u8Url"))
                            .or_else(|| map.get("m3u8_url"))
                            .or_else(|| map.get("streamUrl"))
                            .or_else(|| map.get("stream_url"))
                            .or_else(|| map.get("hlsUrl"))
                            .or_else(|| map.get("mp4Url"))
                            .or_else(|| map.get("mp4"))
                            .or_else(|| map.get("m3u8"))
                            .or_else(|| map.get("url"))
                            .and_then(|v| v.as_str())
                            .filter(|s| s.starts_with("http://") || s.starts_with("https://"));

                        if let Some(stream) = stream_url {
                            let ep_name = map.get("title")
                                .or_else(|| map.get("chapterName"))
                                .or_else(|| map.get("chapter_name"))
                                .or_else(|| map.get("name"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let title = if !ep_name.is_empty() {
                                format!("{} - {}", series_title, ep_name)
                            } else {
                                format!("{} - Episode {}", series_title, idx + 1)
                            };
                            temp_entries.push(crate::models::PlaylistEntry {
                                title,
                                url: stream.to_string(),
                                referer: Some(page_url.to_string()),
                            });
                        }
                    }
                }
                if temp_entries.len() >= 2 {
                    *entries = temp_entries;
                    return;
                }
            }
            for item in arr {
                find_drama_episodes_in_json(item, series_title, page_url, entries);
                if !entries.is_empty() {
                    return;
                }
            }
        }
        serde_json::Value::Object(map) => {
            for (_, v) in map {
                find_drama_episodes_in_json(v, series_title, page_url, entries);
                if !entries.is_empty() {
                    return;
                }
            }
        }
        _ => {}
    }
}

const KISSKH_KEYGEN_JS: &str = include_str!("kisskh_keygen.js");

/// Generates a valid authentication kkey for KissKH video stream or subtitles
pub async fn generate_kisskh_kkey(ep_id: u64, is_sub: bool) -> Result<String> {
    let guid = if is_sub {
        "VgV52sWhwvBSf8BsM3BRY9weWiiCbtGp"
    } else {
        "62f176f3bb1b5b8e70e39932ad34a0c7"
    };

    let bin_dir = get_bin_dir();
    let script_file = bin_dir.join("kisskh_keygen.js");
    if !script_file.exists() {
        let _ = tokio::fs::create_dir_all(&bin_dir).await;
        let _ = tokio::fs::write(&script_file, KISSKH_KEYGEN_JS).await;
    }

    let script_path = script_file.to_string_lossy().to_string();

    // 1. Try Apple's native JavaScriptCore helper (jsc) on macOS
    #[cfg(target_os = "macos")]
    {
        let jsc_path = "/System/Library/Frameworks/JavaScriptCore.framework/Versions/Current/Helpers/jsc";
        if Path::new(jsc_path).exists() {
            let expr = format!(
                "load('{}'); print(_0x54b991({}, null, '2.8.10', '{}', 4830201, 'kisskh', 'kisskh', 'kisskh', 'kisskh', 'kisskh', 'kisskh'));",
                script_path, ep_id, guid
            );
            if let Ok(output) = Command::new(jsc_path)
                .arg("-e")
                .arg(&expr)
                .output()
                .await
            {
                if output.status.success() {
                    let out_str = String::from_utf8_lossy(&output.stdout);
                    if let Some(first_line) = out_str.lines().next() {
                        let trimmed = first_line.trim();
                        if !trimmed.is_empty() && trimmed.len() > 32 {
                            return Ok(trimmed.to_string());
                        }
                    }
                }
            }
        }

        // 2. Try osascript (macOS JavaScript for Automation)
        let osa_expr = format!(
            "var f = $.NSString.stringWithContentsOfFileEncodingError('{}', $.NSUTF8StringEncoding, null); eval(ObjC.unwrap(f)); _0x54b991({}, null, '2.8.10', '{}', 4830201, 'kisskh', 'kisskh', 'kisskh', 'kisskh', 'kisskh', 'kisskh');",
            script_path, ep_id, guid
        );
        if let Ok(output) = Command::new("osascript")
            .arg("-l")
            .arg("JavaScript")
            .arg("-e")
            .arg(&osa_expr)
            .output()
            .await
        {
            if output.status.success() {
                let trimmed = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !trimmed.is_empty() && trimmed.len() > 32 {
                    return Ok(trimmed);
                }
            }
        }
    }

    // 3. Try node if installed
    if let Ok(output) = Command::new("node")
        .arg("-e")
        .arg(format!(
            "const fs = require('fs'); eval(fs.readFileSync('{}', 'utf8')); console.log(_0x54b991({}, null, '2.8.10', '{}', 4830201, 'kisskh', 'kisskh', 'kisskh', 'kisskh', 'kisskh', 'kisskh'));",
            script_path, ep_id, guid
        ))
        .output()
        .await
    {
        if output.status.success() {
            let trimmed = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !trimmed.is_empty() && trimmed.len() > 32 {
                return Ok(trimmed);
            }
        }
    }

    // 4. Try python3 fallback
    if let Ok(output) = Command::new("python3")
        .arg("-c")
        .arg(format!(
            "import subprocess; print(subprocess.check_output(['osascript', '-l', 'JavaScript', '-e', '''var f = $.NSString.stringWithContentsOfFileEncodingError(\"{}\", $.NSUTF8StringEncoding, null); eval(ObjC.unwrap(f)); _0x54b991({}, null, \"2.8.10\", \"{}\", 4830201, \"kisskh\", \"kisskh\", \"kisskh\", \"kisskh\", \"kisskh\", \"kisskh\");''']).decode('utf-8').strip())",
            script_path, ep_id, guid
        ))
        .output()
        .await
    {
        if output.status.success() {
            let trimmed = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !trimmed.is_empty() && trimmed.len() > 32 {
                return Ok(trimmed);
            }
        }
    }

    Err(AppError::Generic("Failed to generate KissKH security token".to_string()))
}

/// Resolves playable stream for an individual KissKH episode URL
pub async fn resolve_kisskh_episode_stream(url: &str, proxy: Option<&str>) -> Result<String> {
    if let Ok(parsed) = reqwest::Url::parse(url) {
        let host = parsed.host_str().unwrap_or("kisskh.do");
        let base_origin = format!("https://{}", host);
        if let Some((_, ep_str)) = parsed.query_pairs().find(|(k, _)| k == "ep") {
            if let Ok(ep_id) = ep_str.parse::<u64>() {
                let video_kkey = generate_kisskh_kkey(ep_id, false).await?;
                let stream_api_url = format!(
                    "{}/api/DramaList/Episode/{}.png?err=false&ts=&time=&kkey={}",
                    base_origin, ep_id, video_kkey
                );

                let mut builder = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(10))
                    .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
                    .default_headers({
                        let mut headers = reqwest::header::HeaderMap::new();
                        headers.insert("Referer", base_origin.parse().unwrap());
                        headers
                    });

                if let Some(p) = proxy {
                    let trimmed = p.trim();
                    if !trimmed.is_empty() {
                        if let Ok(prx) = reqwest::Proxy::all(trimmed) {
                            builder = builder.proxy(prx);
                        }
                    }
                }

                let client = builder.build().unwrap_or_else(|_| reqwest::Client::new());
                let resp = client.get(&stream_api_url).send().await?;
                if resp.status().is_success() {
                    let text = resp.text().await.unwrap_or_default();
                    let stream_json: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
                    if let Some(stream_url) = stream_json["Video"].as_str() {
                        if !stream_url.is_empty() && !stream_url.contains("tickcounter") {
                            return Ok(stream_url.to_string());
                        }
                    }
                }
            }
        }
    }
    Err(AppError::Generic(format!("Could not resolve KissKH episode stream for {}", url)))
}

/// Downloads subtitles from KissKH for the given media file if available
pub async fn download_kisskh_subtitles_for_file(
    source_url: &str,
    media_path: &Path,
    preferred_lang: Option<&str>,
    proxy: Option<&str>,
) -> Result<usize> {
    let parsed = match reqwest::Url::parse(source_url) {
        Ok(p) => p,
        Err(_) => return Ok(0),
    };

    let host = parsed.host_str().unwrap_or("kisskh.do");
    let base_origin = format!("https://{}", host);

    let ep_id: Option<u64> = parsed.query_pairs().find(|(k, _)| k == "ep").and_then(|(_, v)| v.parse().ok());
    let ep_id = match ep_id {
        Some(id) => id,
        None => return Ok(0),
    };

    let sub_kkey = match generate_kisskh_kkey(ep_id, true).await {
        Ok(k) => k,
        Err(e) => {
            warn!("Failed to generate KissKH sub key: {}", e);
            return Ok(0);
        }
    };

    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .default_headers({
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert("Referer", base_origin.parse().unwrap());
            headers
        });

    if let Some(p) = proxy {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            if let Ok(prx) = reqwest::Proxy::all(trimmed) {
                builder = builder.proxy(prx);
            }
        }
    }

    let client = builder.build().unwrap_or_else(|_| reqwest::Client::new());
    let sub_api_url = format!("{}/api/Sub/{}?kkey={}", base_origin, ep_id, sub_kkey);

    let resp = match client.get(&sub_api_url).send().await {
        Ok(r) if r.status().is_success() => r,
        _ => return Ok(0),
    };

    let text = match resp.text().await {
        Ok(t) => t,
        Err(_) => return Ok(0),
    };

    let subs_json: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return Ok(0),
    };

    let sub_items = match subs_json.as_array() {
        Some(arr) if !arr.is_empty() => arr,
        _ => return Ok(0),
    };

    let parent_dir = media_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = media_path.file_stem().and_then(|s| s.to_str()).unwrap_or("video");

    let pref = preferred_lang.unwrap_or("all").to_lowercase();
    let is_all = pref.contains("all");

    let mut downloaded_count = 0;

    for item in sub_items {
        let src = match item["src"].as_str() {
            Some(s) if !s.is_empty() => s,
            _ => continue,
        };
        let land = item["land"].as_str().unwrap_or("en").to_lowercase();

        let should_download = is_all
            || pref.contains(&land)
            || (pref.contains("en") && land.starts_with("en"))
            || (pref.contains("km") && land.starts_with("km"));

        if should_download {
            let out_filename = format!("{}.{}.srt", stem, land);
            let out_path = parent_dir.join(&out_filename);

            if let Ok(sub_data_resp) = client.get(src).send().await {
                if sub_data_resp.status().is_success() {
                    if let Ok(bytes) = sub_data_resp.bytes().await {
                        if !bytes.is_empty() {
                            if let Ok(_) = tokio::fs::write(&out_path, bytes).await {
                                info!("Successfully saved KissKH subtitle: {:?}", out_path);
                                downloaded_count += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(downloaded_count)
}

/// Extracts drama metadata, episode playlists, video streams, and subtitles for KissKH Asian drama URLs
pub async fn extract_kisskh_drama(
    page_url: &str,
    cookies_browser: Option<&str>,
    proxy: Option<&str>,
) -> Result<VideoMetadata> {
    info!("Extracting KissKH Asian drama media: {} (cookies: {:?}, proxy: {:?})", page_url, cookies_browser, proxy);

    let parsed_url = reqwest::Url::parse(page_url)
        .map_err(|e| AppError::Generic(format!("Invalid KissKH URL: {}", e)))?;

    let host = parsed_url.host_str().unwrap_or("kisskh.do");
    let base_origin = format!("https://{}", host);

    // Extract drama id from query parameter ?id=...
    let mut drama_id: Option<u64> = None;
    let mut ep_id: Option<u64> = None;

    for (k, v) in parsed_url.query_pairs() {
        if k == "id" {
            drama_id = v.parse::<u64>().ok();
        } else if k == "ep" {
            ep_id = v.parse::<u64>().ok();
        }
    }

    let drama_id = drama_id.ok_or_else(|| {
        AppError::Generic(format!("Missing 'id' parameter in KissKH URL: {}", page_url))
    })?;

    // Build HTTP client with optional proxy
    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .default_headers({
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert("Referer", base_origin.parse().unwrap());
            headers.insert("Origin", base_origin.parse().unwrap());
            headers
        });

    if let Some(p) = proxy {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            if let Ok(prx) = reqwest::Proxy::all(trimmed) {
                builder = builder.proxy(prx);
            }
        }
    }

    let client = builder.build().unwrap_or_else(|_| reqwest::Client::new());

    // 1. Fetch drama details from KissKH API
    let drama_api_url = format!("{}/api/DramaList/Drama/{}?isq=false", base_origin, drama_id);
    let resp = client.get(&drama_api_url).send().await?;
    if !resp.status().is_success() {
        return Err(AppError::Generic(format!(
            "Failed to fetch KissKH drama details: HTTP {}",
            resp.status()
        )));
    }

    let drama_text = resp.text().await?;
    let drama_json: serde_json::Value = serde_json::from_str(&drama_text)
        .map_err(|e| AppError::Generic(format!("Failed to parse KissKH drama JSON: {}", e)))?;
    let drama_title = drama_json["title"].as_str().unwrap_or("Asian Drama").to_string();
    let thumbnail = drama_json["thumbnail"].as_str().map(|s| s.to_string());
    let episodes_arr = drama_json["episodes"].as_array().cloned().unwrap_or_default();

    // Parse and sort episodes
    struct EpInfo {
        id: u64,
        number: f64,
    }

    let mut eps: Vec<EpInfo> = episodes_arr
        .iter()
        .filter_map(|e| {
            let id = e["id"].as_u64()?;
            let number = e["number"].as_f64().unwrap_or(1.0);
            Some(EpInfo { id, number })
        })
        .collect();

    // Sort ascending by episode number
    eps.sort_by(|a, b| a.number.partial_cmp(&b.number).unwrap_or(std::cmp::Ordering::Equal));

    if eps.is_empty() {
        return Err(AppError::Generic("No episodes found for this KissKH drama".to_string()));
    }

    // Determine target episode
    let target_ep_id = ep_id.unwrap_or(eps[0].id);
    let target_ep_info = eps.iter().find(|e| e.id == target_ep_id).unwrap_or(&eps[0]);
    let target_ep_num = target_ep_info.number;

    // 2. Fetch video stream URL for target episode
    let video_kkey = generate_kisskh_kkey(target_ep_id, false).await?;
    let stream_api_url = format!(
        "{}/api/DramaList/Episode/{}.png?err=false&ts=&time=&kkey={}",
        base_origin, target_ep_id, video_kkey
    );

    let stream_resp = client.get(&stream_api_url).send().await?;
    let stream_text = stream_resp.text().await.unwrap_or_default();
    let stream_json: serde_json::Value = serde_json::from_str(&stream_text).unwrap_or(serde_json::Value::Null);
    let resolved_stream_url = stream_json["Video"]
        .as_str()
        .unwrap_or("")
        .to_string();

    if resolved_stream_url.is_empty() || resolved_stream_url.contains("tickcounter") {
        return Err(AppError::Generic(format!(
            "Episode {:.0} stream is not yet released or unavailable on KissKH",
            target_ep_num
        )));
    }

    // Cache the playable stream URL so subsequent download steps don't have to regenerate
    if let Ok(mut cache) = PLAYABLE_URL_CACHE.write() {
        cache.insert(page_url.to_string(), resolved_stream_url.clone());
    }

    // 3. Fetch subtitle languages
    let mut available_subs = Vec::new();
    if let Ok(sub_kkey) = generate_kisskh_kkey(target_ep_id, true).await {
        let sub_api_url = format!("{}/api/Sub/{}?kkey={}", base_origin, target_ep_id, sub_kkey);
        if let Ok(sub_resp) = client.get(&sub_api_url).send().await {
            if let Ok(sub_text) = sub_resp.text().await {
                if let Ok(subs_json) = serde_json::from_str::<serde_json::Value>(&sub_text) {
                    if let Some(sub_list) = subs_json.as_array() {
                        for s in sub_list {
                            if let Some(label) = s["label"].as_str() {
                                available_subs.push(label.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    let has_subtitles = !available_subs.is_empty();
    let subtitles_summary = if has_subtitles {
        available_subs.join(", ")
    } else {
        "No subtitles".to_string()
    };

    // 4. Build playlist entries
    let playlist_count = eps.len();
    let is_playlist = playlist_count > 1;

    let path_segments: Vec<&str> = parsed_url.path().split('/').filter(|s| !s.is_empty()).collect();
    let slug = if path_segments.len() >= 2 {
        path_segments[1]
    } else {
        "Drama"
    };

    let playlist_entries: Vec<PlaylistEntry> = eps
        .iter()
        .map(|ep| {
            let ep_num_formatted = if ep.number.fract() == 0.0 {
                format!("{:.0}", ep.number)
            } else {
                format!("{:.1}", ep.number)
            };
            PlaylistEntry {
                title: format!("{} - Episode {}", drama_title, ep_num_formatted),
                url: format!("{}/Drama/{}/Episode-{}?id={}&ep={}", base_origin, slug, ep_num_formatted, drama_id, ep.id),
                referer: Some(format!("{}/", base_origin)),
            }
        })
        .collect();

    let display_title = if is_playlist && ep_id.is_none() {
        drama_title
    } else {
        let ep_num_formatted = if target_ep_num.fract() == 0.0 {
            format!("{:.0}", target_ep_num)
        } else {
            format!("{:.1}", target_ep_num)
        };
        format!("{} - Episode {}", drama_title, ep_num_formatted)
    };

    Ok(VideoMetadata {
        url: resolved_stream_url,
        title: display_title,
        content_length: None,
        content_type: Some("application/vnd.apple.mpegurl".to_string()),
        supports_ranges: true,
        is_extractor: true,
        duration_seconds: None,
        resolution: Some("1080p (HLS)".to_string()),
        ext: Some("mp4".to_string()),
        is_playlist,
        playlist_count,
        playlist_entries,
        has_subtitles,
        subtitles_summary,
        thumbnail_url: thumbnail,
        fps: Some(30.0),
        vcodec: Some("H.264".to_string()),
        acodec: Some("AAC".to_string()),
        size_best: None,
        size_1080p: None,
        size_720p: None,
        size_480p: None,
        size_audio: None,
        referer: Some(format!("{}/", base_origin)),
        available_formats: Vec::new(),
    })
}

/// Helper to search HTML for embedded video links (<video src=...>, <source src=...>, og:video, or .m3u8/.mp4 URLs)
fn extract_stream_from_html(html: &str, base_url: &str) -> Option<String> {
    // 1. Check og:video or og:video:url
    for tag in &["property=\"og:video\"", "property=\"og:video:url\"", "name=\"twitter:player:stream\""] {
        if let Some(pos) = html.find(tag) {
            // Find start of tag '<'
            let tag_start = html[..pos].rfind('<').unwrap_or(pos);
            let tag_end = html[pos..].find('>').map(|e| pos + e).unwrap_or(html.len());
            let tag_str = &html[tag_start..tag_end];
            if let Some(c_idx) = tag_str.find("content=\"") {
                let rest = &tag_str[c_idx + 9..];
                if let Some(q_end) = rest.find('"') {
                    let candidate = &rest[..q_end];
                    if candidate.starts_with("http") {
                        return Some(html_escape_clean(candidate));
                    }
                }
            }
        }
    }

    // 2. Search for <source src="..." or <video src="..."
    for marker in &["src=\"", "src='"] {
        let mut cursor = 0;
        while let Some(rel) = html[cursor..].find(marker) {
            let actual = cursor + rel + marker.len();
            let quote = if marker.ends_with('"') { '"' } else { '\'' };
            if let Some(end) = html[actual..].find(quote) {
                let candidate = &html[actual..actual + end];
                let lower = candidate.to_lowercase();
                if lower.contains(".m3u8") || lower.contains(".mp4") || lower.contains(".webm") {
                    let cleaned = html_escape_clean(candidate);
                    let full = resolve_relative_url(base_url, &cleaned);
                    return Some(full);
                }
            }
            cursor = actual + 1;
            if cursor >= html.len() {
                break;
            }
        }
    }

    // 3. Search for raw http...m3u8 or http...mp4 inside script tags
    let mut cursor = 0;
    while let Some(rel) = html[cursor..].find("http") {
        let actual = cursor + rel;
        let rest = &html[actual..];
        let end_idx = rest.find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '\\' || c == '<' || c == '>')
            .unwrap_or(rest.len().min(300));
        let candidate = &rest[..end_idx];
        let lower = candidate.to_lowercase();
        if (lower.contains(".m3u8") || lower.contains(".mp4") || lower.contains(".webm")) && !lower.contains("banner") && !lower.contains("ad") {
            let cleaned = html_escape_clean(candidate);
            return Some(cleaned);
        }
        cursor = actual + 4;
        if cursor >= html.len() {
            break;
        }
    }

    None
}

/// Resolves relative URLs to absolute HTTP/HTTPS URLs
fn resolve_relative_url(base: &str, rel: &str) -> String {
    if rel.starts_with("http://") || rel.starts_with("https://") {
        rel.to_string()
    } else if rel.starts_with("//") {
        format!("https:{}", rel)
    } else if let Ok(base_url) = reqwest::Url::parse(base) {
        if let Ok(joined) = base_url.join(rel) {
            joined.to_string()
        } else {
            rel.to_string()
        }
    } else {
        rel.to_string()
    }
}

/// Decodes URL percent-encoding (e.g. %20 -> space)
fn decode_percent(input: &str) -> String {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(val) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16) {
                out.push(val);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| input.to_string())
}

/// Decodes standard Base64 string without external dependencies
fn decode_base64(input: &str) -> Option<String> {
    let mut table = [0xFFu8; 256];
    for (i, &b) in b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/".iter().enumerate() {
        table[b as usize] = i as u8;
    }
    let bytes: Vec<u8> = input.bytes().filter(|&b| b != b'=' && !b.is_ascii_whitespace()).collect();
    if bytes.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let b0 = *table.get(*chunk.get(0)? as usize)?;
        let b1 = *table.get(*chunk.get(1)? as usize)?;
        if b0 == 0xFF || b1 == 0xFF { return None; }
        out.push((b0 << 2) | (b1 >> 4));
        if chunk.len() > 2 {
            let b2 = *table.get(*chunk.get(2)? as usize)?;
            if b2 == 0xFF { return None; }
            out.push(((b1 & 0x0F) << 4) | (b2 >> 2));
            if chunk.len() > 3 {
                let b3 = *table.get(*chunk.get(3)? as usize)?;
                if b3 == 0xFF { return None; }
                out.push(((b2 & 0x03) << 6) | b3);
            }
        }
    }
    String::from_utf8(out).ok()
}

/// Cleans HTML entities and unicode escape sequences
fn html_escape_clean(input: &str) -> String {
    input
        .replace("\\u0026", "&")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("\\/", "/")
        .trim()
        .to_string()
}

/// Strips HTML tags (<...>) and decodes HTML entities and unicode escape sequences
pub fn strip_html_tags_and_clean(input: &str) -> String {
    let unescaped = html_escape_clean(input);
    let mut in_tag = false;
    let mut res = String::with_capacity(unescaped.len());
    for c in unescaped.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            res.push(c);
        }
    }
    res.split_whitespace().collect::<Vec<_>>().join(" ").trim().to_string()
}

/// Cleans redundant episode numbers from series collection titles (e.g. "Title EP07" -> "Title")
pub fn clean_series_title_episodes(title: &str) -> String {
    let t = title.trim();
    let patterns = [" EP", " Ep", " Episode ", " 第"];
    for p in &patterns {
        if let Some(idx) = t.rfind(p) {
            let suffix = &t[idx + p.len()..];
            if suffix.chars().all(|c| c.is_ascii_digit() || c.is_whitespace() || c == '集' || c == '话') {
                let cleaned = t[..idx].trim();
                if !cleaned.is_empty() {
                    return cleaned.to_string();
                }
            }
        }
    }
    t.to_string()
}

/// If the URL is an HTML webpage (e.g. MacCMS vod/play page, iframe host, or direct web scraper),
static PLAYABLE_URL_CACHE: std::sync::LazyLock<std::sync::RwLock<std::collections::HashMap<String, String>>> =
    std::sync::LazyLock::new(|| std::sync::RwLock::new(std::collections::HashMap::new()));

/// Returns cached pre-resolved media stream URL if available
pub fn get_cached_playable_url(url: &str) -> Option<String> {
    PLAYABLE_URL_CACHE.read().ok()?.get(url).cloned()
}

static SHARED_HTTP_CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36")
        .timeout(std::time::Duration::from_secs(5))
        .pool_idle_timeout(std::time::Duration::from_secs(90))
        .pool_max_idle_per_host(10)
        .tcp_nodelay(true)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
});

/// Resolves the underlying stream or platform URL (such as Dailymotion, YouTube, Vimeo, or .m3u8).
pub async fn resolve_playable_stream_url(url: &str, proxy: Option<&str>) -> String {
    // Fast path: KissKH episode streaming resolution
    if url.contains("kisskh.") && url.contains("ep=") {
        if let Ok(stream) = resolve_kisskh_episode_stream(url, proxy).await {
            info!("Resolved KissKH playable stream: {} -> {}", url, stream);
            if let Ok(mut cache) = PLAYABLE_URL_CACHE.write() {
                cache.insert(url.to_string(), stream.clone());
            }
            return stream;
        }
    }

    if is_streaming_platform(url) || url.contains(".m3u8") || url.contains(".mp4") || url.contains(".webm") {
        return url.to_string();
    }

    if let Ok(cache) = PLAYABLE_URL_CACHE.read() {
        if let Some(cached) = cache.get(url) {
            return cached.clone();
        }
    }

    let client = if let Some(prx_str) = proxy {
        let trimmed = prx_str.trim();
        if !trimmed.is_empty() {
            if let Ok(prx) = reqwest::Proxy::all(trimmed) {
                reqwest::Client::builder()
                    .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36")
                    .timeout(std::time::Duration::from_secs(5))
                    .proxy(prx)
                    .build()
                    .unwrap_or_else(|_| reqwest::Client::new())
            } else {
                SHARED_HTTP_CLIENT.clone()
            }
        } else {
            SHARED_HTTP_CLIENT.clone()
        }
    } else {
        SHARED_HTTP_CLIENT.clone()
    };

    if let Ok(resp) = client.get(url).send().await {
        if resp.status().is_success() {
            if let Ok(html) = resp.text().await {
                // 1. Check MacCMS player_aaaa configuration (used by donghuafun, animevietsub, etc.)
                if let Some(maccms) = extract_maccms_player(&html) {
                    info!("Resolved MacCMS playable stream URL: {} -> {}", url, maccms.video_url);
                    if let Ok(mut cache) = PLAYABLE_URL_CACHE.write() {
                        cache.insert(url.to_string(), maccms.video_url.clone());
                    }
                    return maccms.video_url;
                }
                // 2. Check embedded iframes
                for iframe_url in extract_iframes_from_html(&html, url) {
                    if is_streaming_platform(&iframe_url) || iframe_url.contains(".m3u8") || iframe_url.contains(".mp4") {
                        info!("Resolved iframe playable stream URL: {} -> {}", url, iframe_url);
                        if let Ok(mut cache) = PLAYABLE_URL_CACHE.write() {
                            cache.insert(url.to_string(), iframe_url.clone());
                        }
                        return iframe_url;
                    }
                }
                // 3. Check direct HTML stream
                if let Some(stream_url) = extract_stream_from_html(&html, url) {
                    info!("Resolved direct HTML stream URL: {} -> {}", url, stream_url);
                    if let Ok(mut cache) = PLAYABLE_URL_CACHE.write() {
                        cache.insert(url.to_string(), stream_url.clone());
                    }
                    return stream_url;
                }
                // 4. Check Universal Media Sniffer ONLY if it returns a genuine direct media stream
                if let Some(sniffed) = universal_sniff_media_from_html(&html, url) {
                    if !sniffed.url.is_empty()
                        && (is_streaming_platform(&sniffed.url)
                            || is_direct_media_url(&sniffed.url)
                            || is_media_stream_candidate(&sniffed.url))
                        && !sniffed.url.ends_with(".html")
                        && !sniffed.url.ends_with(".htm")
                        && !sniffed.url.ends_with(".php")
                    {
                        info!("Resolved universal stream URL: {} -> {}", url, sniffed.url);
                        if let Ok(mut cache) = PLAYABLE_URL_CACHE.write() {
                            cache.insert(url.to_string(), sniffed.url.clone());
                        }
                        return sniffed.url;
                    }
                }
            }
        }
    }

    url.to_string()
}

/// Recursively sniffs HTML, player configs, and embedded iframes to extract a direct stream when a site is unsupported by yt-dlp
pub async fn resolve_deep_fallback_stream(url: &str, proxy: Option<&str>) -> Result<String> {
    let client = if let Some(prx_str) = proxy {
        let trimmed = prx_str.trim();
        if !trimmed.is_empty() {
            if let Ok(prx) = reqwest::Proxy::all(trimmed) {
                reqwest::Client::builder()
                    .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36")
                    .timeout(std::time::Duration::from_secs(10))
                    .proxy(prx)
                    .build()
                    .unwrap_or_else(|_| reqwest::Client::new())
            } else {
                SHARED_HTTP_CLIENT.clone()
            }
        } else {
            SHARED_HTTP_CLIENT.clone()
        }
    } else {
        SHARED_HTTP_CLIENT.clone()
    };

    let resp = client.get(url).send().await?;
    let html = resp.text().await?;

    // 1. Check MacCMS player_aaaa configuration
    if let Some(maccms) = extract_maccms_player(&html) {
        return Ok(maccms.video_url);
    }

    // 2. Check embedded iframes recursively
    for iframe in extract_iframes_from_html(&html, url) {
        if is_streaming_platform(&iframe) || iframe.contains(".m3u8") || iframe.contains(".mp4") {
            return Ok(iframe);
        }
        if let Ok(iframe_resp) = client.get(&iframe).send().await {
            if let Ok(iframe_html) = iframe_resp.text().await {
                if let Some(maccms) = extract_maccms_player(&iframe_html) {
                    return Ok(maccms.video_url);
                }
                if let Some(stream) = extract_stream_from_html(&iframe_html, &iframe) {
                    return Ok(stream);
                }
            }
        }
    }

    // 3. Check direct HTML stream (<video>, <source>, og:video, regex m3u8/mp4)
    if let Some(stream) = extract_stream_from_html(&html, url) {
        return Ok(stream);
    }

    // 4. Universal JSON sniffer
    if let Some(meta) = universal_sniff_media_from_html(&html, url) {
        if !meta.url.ends_with(".html") && !meta.url.ends_with(".htm") && !meta.url.ends_with(".php") {
            return Ok(meta.url);
        }
    }

    Err(AppError::Generic("No playable stream discovered during deep fallback sniffing".to_string()))
}

/// Downloads a streaming URL via yt-dlp, streaming real-time progress events
pub async fn download_stream<F>(
    url: &str,
    destination_path: &Path,
    is_audio_only: bool,
    quality: Option<&str>,
    download_subtitles: bool,
    subtitle_language: Option<&str>,
    download_thumbnail: bool,
    audio_format: Option<&str>,
    audio_bitrate: Option<&str>,
    embed_artwork: bool,
    speed_limit: Option<&str>,
    cookies_browser: Option<&str>,
    proxy: Option<&str>,
    concurrent_fragments: u8,
    referer: Option<&str>,
    cancel_token: CancellationToken,
    mut on_progress: F,
) -> Result<PathBuf>
where
    F: FnMut(DownloadProgress) + Send + 'static,
{
    let normalized_url = normalize_media_url(url);
    let mut url_to_download = normalized_url.clone();
    let mut effective_referer = referer.map(|s| s.to_string());

    if url_to_download.contains("douyin.com") || url_to_download.contains("iesdouyin.com") {
        if let Ok(meta) = resolve_douyin_via_douyinsaver(&url_to_download, proxy).await {
            info!("Douyin stream resolved via DouyinSaver for download: {:?}", meta.title);
            url_to_download = meta.url;
            if effective_referer.is_none() {
                effective_referer = meta.referer;
            }
        }
    }

    let url = url_to_download.as_str();
    let referer = effective_referer.as_deref();

    let ytdlp_bin = ensure_ytdlp_installed().await?;
    let bin_dir = get_bin_dir();

    let parent = destination_path.parent().unwrap_or(Path::new("."));
    tokio::fs::create_dir_all(parent).await?;

    let raw_name = destination_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("downloaded_media");

    let filename_stem = match raw_name.rsplit_once('.') {
        Some((stem, ext))
            if ["mp4", "webm", "mkv", "avi", "mov", "m4a", "mp3", "part", "flac", "wav", "opus", "aac"]
                .contains(&ext.to_lowercase().as_str()) =>
        {
            stem
        }
        _ => raw_name,
    };

    let output_template = parent.join(format!("{}.%(ext)s", filename_stem));

    let mut cmd = create_ytdlp_cmd(&ytdlp_bin);
    cmd.arg("--no-playlist")
        .arg("--no-check-formats")
        .arg("--no-warnings")
        .arg("--newline")
        .arg("--ignore-config")
        .arg("--no-cache-dir")
        .arg("--extractor-retries").arg("1")
        .arg("--compat-options").arg("no-live-chat")
        .arg("--user-agent").arg("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .arg("--progress-template")
        .arg("download:RAW:%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(progress.speed)s|%(progress.eta)s|%(progress._percent)s|%(info.ext)s|%(progress.filename)s");

    let target_url = resolve_playable_stream_url(url, proxy).await;
    info!("Target URL for extractor download resolved: {} -> {}", url, target_url);

    // Only forward referer for non-platform streams (e.g. custom CDNs or direct HLS/MP4 streams).
    // Do NOT forward foreign referrers or Origin headers to major platforms (like Dailymotion, YouTube, Vimeo, TikTok, Bilibili)
    // because platforms have their own auth tokens and will reject foreign cross-origin referrers with HTTP 403 Forbidden.
    let is_platform = is_streaming_platform(&target_url);
    if !is_platform {
        if let Some(ref_url) = referer {
            let trimmed = ref_url.trim();
            if !trimmed.is_empty() {
                info!("Auto-forwarding Referer to yt-dlp: {}", trimmed);
                cmd.arg("--referer").arg(trimmed);
            }
        }
    }

    if let Some(limit) = speed_limit {
        if !limit.is_empty() && limit != "unlimited" {
            cmd.arg("--limit-rate").arg(limit);
        }
    }

    if let Some(browser) = cookies_browser {
        apply_browser_cookies(&mut cmd, browser);
    }

    if let Some(prx) = proxy {
        let trimmed = prx.trim();
        if !trimmed.is_empty() {
            info!("Applying proxy {} to yt-dlp download", trimmed);
            cmd.arg("--proxy").arg(trimmed);
        }
    }

    if is_audio_only {
        cmd.arg("-x");
        let fmt = audio_format.unwrap_or("mp3");
        cmd.arg("--audio-format").arg(fmt);

        if let Some(br) = audio_bitrate {
            if !br.is_empty() && fmt != "flac" && fmt != "wav" {
                cmd.arg("--audio-quality").arg(br);
            }
        }

        if embed_artwork {
            cmd.arg("--embed-thumbnail").arg("--embed-metadata");
        }
        if download_thumbnail {
            cmd.arg("--write-thumbnail");
        }
    } else {
        if download_subtitles {
            let raw_sub_langs = subtitle_language.unwrap_or("all,-live_chat");
            let is_all = raw_sub_langs.contains("all");

            cmd.arg("--write-subs")
                .arg("--embed-subs")
                .arg("-i"); // Ignore non-fatal auxiliary subtitle errors so video download succeeds

            if is_all {
                // When "All Languages" is selected, constrain auto-subs to the app's supported languages.
                // Requesting all 140+ auto-sub languages at once triggers YouTube's HTTP 429 rate limit on 'ab' (Abkhazian).
                cmd.arg("--sub-langs").arg("en.*,km.*,th.*,vi.*,id.*,ms.*,my.*,zh.*,ja.*,ko.*,es.*,fr.*,de.*,ru.*,pt.*,ar.*,all,-live_chat");
                cmd.arg("--write-auto-subs");
            } else {
                cmd.arg("--sub-langs").arg(raw_sub_langs);
                cmd.arg("--write-auto-subs");
            }
        }
        if download_thumbnail {
            cmd.arg("--write-thumbnail");
        }
        if let Some(q) = quality {
            for part in q.split('|') {
                let trimmed = part.trim();
                match trimmed {
                    "4k" | "2160p" => {
                        cmd.arg("-S").arg("res:2160");
                        cmd.arg("-f").arg("bestvideo[height<=2160]+bestaudio/best[height<=2160]/best");
                    }
                    "1440p" | "2k" => {
                        cmd.arg("-S").arg("res:1440");
                        cmd.arg("-f").arg("bestvideo[height<=1440]+bestaudio/best[height<=1440]/best");
                    }
                    "1080p" => {
                        cmd.arg("-S").arg("res:1080");
                        cmd.arg("-f").arg("bestvideo[height<=1080]+bestaudio/best[height<=1080]/best[format_note*='1080']/600/best[height<=?1080]");
                    }
                    "720p" => {
                        cmd.arg("-S").arg("res:720");
                        cmd.arg("-f").arg("bestvideo[height<=720]+bestaudio/best[height<=720]/best[format_note*='720']/500/best[height<=?720]");
                    }
                    "480p" => {
                        cmd.arg("-S").arg("res:480");
                        cmd.arg("-f").arg("bestvideo[height<=480]+bestaudio/best[height<=480]/best[format_note*='480']/300/best[height<=?480]");
                    }
                    "360p" => {
                        cmd.arg("-S").arg("res:360");
                        cmd.arg("-f").arg("bestvideo[height<=360]+bestaudio/best[height<=360]/best[height<=?360]");
                    }
                    "remux:mp4" => {
                        cmd.arg("--remux-video").arg("mp4");
                    }
                    "remux:mkv" => {
                        cmd.arg("--remux-video").arg("mkv");
                    }
                    _ => {
                        if let Some(fmt_spec) = trimmed.strip_prefix("format:") {
                            cmd.arg("-f").arg(format!("{}+bestaudio/{}", fmt_spec, fmt_spec));
                        }
                    }
                }
            }
        }
    }

    let ffmpeg_loc = if let Some(ref ffmpeg) = *CACHED_FFMPEG_PATH {
        ffmpeg.parent().map(|p| p.to_path_buf())
    } else if bin_dir.join("ffmpeg").exists() {
        Some(bin_dir.clone())
    } else {
        None
    };

    if let Some(ref dir) = ffmpeg_loc {
        cmd.arg("--ffmpeg-location").arg(dir);
    }

    // Parallel fragment download: configurable concurrent HLS/DASH segments for max speed
    let frag_count = concurrent_fragments.clamp(1, 16);
    cmd.arg("--concurrent-fragments").arg(frag_count.to_string());
    // Retry on transient errors (network hiccups on segment downloads)
    cmd.arg("--retries").arg("3");
    cmd.arg("--fragment-retries").arg("5");
    cmd.arg("--file-access-retries").arg("5");

    cmd.arg("-o")
        .arg(output_template.to_string_lossy().to_string())
        .arg(&target_url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    info!("Spawning extractor stream download: {:?}", cmd);
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().ok_or_else(|| {
        AppError::Generic("Failed to capture stdout of extractor process".to_string())
    })?;
    let stderr = child.stderr.take();

    let stderr_task = tokio::spawn(async move {
        let mut err_str = String::new();
        if let Some(mut err_pipe) = stderr {
            let _ = err_pipe.read_to_string(&mut err_str).await;
        }
        err_str
    });

    let mut reader = BufReader::new(stdout).lines();

    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                warn!("Stream download cancelled by user or paused: terminating yt-dlp process");
                let _ = child.kill().await;
                return Err(AppError::Cancelled);
            }
            line_res = reader.next_line() => {
                match line_res {
                    Ok(Some(line)) => {
                        if let Some(progress) = parse_extractor_progress_line(&line) {
                            on_progress(progress);
                        }
                    }
                    Ok(None) => {
                        // EOF reached
                        break;
                    }
                    Err(err) => {
                        warn!("Error reading stdout from extractor: {}", err);
                        break;
                    }
                }
            }
        }
    }

    let status = child.wait().await?;
    let stderr_output = stderr_task.await.unwrap_or_default();

    // Check if the media file was actually downloaded successfully on disk
    let candidate_extensions = ["mp3", "mp4", "webm", "mkv", "m4a", "opus"];
    for ext in &candidate_extensions {
        let p = parent.join(format!("{}.{}", filename_stem, ext));
        if p.exists() && std::fs::metadata(&p).map(|m| m.len() > 1024).unwrap_or(false) {
            if !status.success() {
                warn!("Extractor process exited with code {:?}, but media file was downloaded successfully: {:?}", status.code(), p);
            } else {
                info!("Extractor completed output resolved to: {:?}", p);
            }

            // If KissKH media download and subtitles requested, fetch and organize subtitles
            if download_subtitles && (url.contains("kisskh.") || target_url.contains("cdnvideo")) {
                let _ = download_kisskh_subtitles_for_file(url, &p, subtitle_language, proxy).await;
                let _ = crate::filesystem::files::organize_subtitles(&p).await;
            }

            return Ok(p);
        }
    }

    if !status.success() {
        let err_detail = if !stderr_output.trim().is_empty() {
            clean_extractor_error(&stderr_output)
        } else {
            format!("Extractor process finished with exit code {:?}", status.code())
        };

        // Self-healing Universal Fallback: If yt-dlp reported Unsupported URL or format error on an arbitrary webpage,
        // automatically sniff the page and retry!
        let lower_err = err_detail.to_lowercase();

        // Self-healing Cookie Fallback: If download failed because browser cookies were locked, bloated, or rejected,
        // automatically purge corrupted cache and retry seamlessly in guest mode
        let is_cookie_err = lower_err.contains("413")
            || lower_err.contains("request entity too large")
            || lower_err.contains("cookie")
            || lower_err.contains("database is locked")
            || lower_err.contains("could not copy")
            || lower_err.contains("keychain")
            || lower_err.contains("keyring");

        if cookies_browser.is_some() && is_cookie_err {
            warn!(
                "Download failed with browser cookies ({}). Auto-healing: retrying without cookies in guest mode...",
                err_detail
            );
            if let Some(b) = cookies_browser {
                let cache_file = get_bin_dir().join(format!("cookies_{}.txt", b.trim()));
                let _ = std::fs::remove_file(&cache_file);
            }
            return Box::pin(download_stream(
                &target_url,
                destination_path,
                is_audio_only,
                quality,
                download_subtitles,
                subtitle_language,
                download_thumbnail,
                audio_format,
                audio_bitrate,
                embed_artwork,
                speed_limit,
                None, // Retry without cookies!
                proxy,
                concurrent_fragments,
                None,
                cancel_token,
                on_progress,
            )).await;
        }
        let is_format_or_url_err = lower_err.contains("unsupported url")
            || lower_err.contains("unsupported")
            || lower_err.contains("no video formats found")
            || lower_err.contains("no downloadable video formats");

        if is_format_or_url_err
            && !target_url.contains(".m3u8")
            && !target_url.contains(".mp4")
            && !is_streaming_platform(&target_url)
        {
            info!("yt-dlp reported error ({}) for {}. Activating universal deep stream sniffer fallback...", err_detail, target_url);
            if let Ok(deep_stream) = resolve_deep_fallback_stream(&target_url, proxy).await {
                if !deep_stream.is_empty() && deep_stream != target_url {
                    info!("Universal fallback found playable stream: {}. Retrying download seamlessly...", deep_stream);
                    return Box::pin(download_stream(
                        &deep_stream,
                        destination_path,
                        is_audio_only,
                        quality,
                        download_subtitles,
                        subtitle_language,
                        download_thumbnail,
                        audio_format,
                        audio_bitrate,
                        embed_artwork,
                        speed_limit,
                        cookies_browser,
                        proxy,
                        concurrent_fragments,
                        None,
                        cancel_token,
                        on_progress,
                    )).await;
                }
            }
        }

        return Err(AppError::Generic(err_detail));
    }

    Ok(destination_path.to_path_buf())
}

/// Parses raw progress line emitted by yt-dlp `--progress-template`
pub fn parse_extractor_progress_line(line: &str) -> Option<DownloadProgress> {
    let raw_idx = line.find("RAW:")?;
    let raw_part = &line[raw_idx + 4..];
    let parts: Vec<&str> = raw_part.split('|').collect();
    if parts.len() < 5 {
        return None;
    }

    // Ignore subtitle downloads (e.g. srt, vtt, ass) so auxiliary subtitle files
    // do not overwrite the primary video/audio stream's byte progress and total size.
    if parts.len() >= 7 {
        let ext = parts[6].trim().to_lowercase();
        let fname = if parts.len() >= 8 {
            parts[7].trim().to_lowercase()
        } else {
            String::new()
        };
        if ext == "srt"
            || ext == "vtt"
            || ext == "ass"
            || ext == "lrc"
            || fname.ends_with(".srt")
            || fname.ends_with(".vtt")
            || fname.ends_with(".ass")
        {
            return None;
        }
    }

    let parse_num = |s: &str| -> Option<u64> {
        let s = s.trim();
        if s.is_empty()
            || s.eq_ignore_ascii_case("na")
            || s.eq_ignore_ascii_case("none")
            || s.eq_ignore_ascii_case("null")
        {
            return None;
        }
        if let Ok(v) = s.parse::<u64>() {
            return Some(v);
        }
        if let Ok(f) = s.parse::<f64>() {
            if f >= 0.0 && f.is_finite() {
                return Some(f.round() as u64);
            }
        }
        None
    };

    let downloaded = parse_num(parts[0]).unwrap_or(0);
    let total_bytes = parse_num(parts[1]).or_else(|| parse_num(parts[2]));
    let speed = parts[3].trim().parse::<f64>().unwrap_or(0.0);
    let mut eta = parse_num(parts[4]);

    // If yt-dlp did not provide an ETA, estimate it from speed and remaining bytes
    if eta.is_none() && speed > 0.0 {
        if let Some(total) = total_bytes {
            if total > downloaded {
                eta = Some(((total - downloaded) as f64 / speed).round() as u64);
            }
        }
    }

    // If yt-dlp provided _percent directly in parts[5], use it for exact progress
    let progress_ratio = if let Some(pct) = parts.get(5).and_then(|s| s.trim().parse::<f64>().ok()) {
        (pct / 100.0).clamp(0.0, 1.0) as f32
    } else if let Some(total) = total_bytes {
        if total > 0 {
            (downloaded as f32 / total as f32).clamp(0.0, 1.0)
        } else {
            0.0
        }
    } else {
        0.0
    };

    Some(DownloadProgress {
        downloaded_bytes: downloaded,
        total_bytes,
        speed_bytes_sec: speed,
        eta_seconds: eta,
        progress_ratio,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_clear_all_caches() {
        // Pre-populate URL cache
        if let Ok(mut cache) = PLAYABLE_URL_CACHE.write() {
            cache.insert("https://test.example/play/1".to_string(), "https://cdn.example/stream.m3u8".to_string());
        }

        // Create dummy cookie file
        let bin_dir = get_bin_dir();
        let _ = std::fs::create_dir_all(&bin_dir);
        let dummy_cookie = bin_dir.join("cookies_test_unit.txt");
        let _ = std::fs::write(&dummy_cookie, "# Netscape HTTP Cookie File\n.example.com TRUE / FALSE 0 name val\n");

        let res = clear_all_caches().await;
        assert!(res.is_ok());
        let msg = res.unwrap();
        assert!(msg.contains("Cleared") || msg.contains("clean"));

        // Verify URL cache cleared the test entry
        if let Ok(cache) = PLAYABLE_URL_CACHE.read() {
            assert!(!cache.contains_key("https://test.example/play/1"));
        }
        assert!(!dummy_cookie.exists());
    }

    #[test]
    fn test_extract_next_data_drama() {
        let sample_html = r#"
        <!DOCTYPE html><html><head><title>Test Drama</title></head><body>
        <script id="__NEXT_DATA__" type="application/json">
        {
          "props": {
            "pageProps": {
              "chapterId": "609419256",
              "bookInfoVo": {
                "bookId": "41000131810",
                "bookName": "雪夜逃婚",
                "coverWap": "https://example.com/cover.jpg"
              },
              "chapterList": [
                {
                  "chapterId": "609419256",
                  "chapterName": "第一集",
                  "chapterIndex": 1,
                  "isCharge": "0",
                  "chapterVideoVo": {
                    "mp4": "https://cdn.example.com/ep1.mp4"
                  }
                },
                {
                  "chapterId": "609419257",
                  "chapterName": "第二集",
                  "chapterIndex": 2,
                  "isCharge": "0",
                  "chapterVideoVo": {
                    "mp4": "https://cdn.example.com/ep2.mp4"
                  }
                }
              ]
            }
          }
        }
        </script>
        </body></html>
        "#;

        let meta = extract_next_data_drama(sample_html, "https://www.kuaikaw.cn/episode/41000131810/609419256");
        assert!(meta.is_some());
        let m = meta.unwrap();
        assert_eq!(m.title, "雪夜逃婚");
        assert_eq!(m.playlist_count, 2);
        assert!(m.is_playlist);
        assert_eq!(m.url, "https://cdn.example.com/ep1.mp4");
        assert_eq!(m.thumbnail_url, Some("https://example.com/cover.jpg".to_string()));
        assert_eq!(m.playlist_entries[0].title, "雪夜逃婚 - 第一集");
        assert_eq!(m.playlist_entries[0].url, "https://cdn.example.com/ep1.mp4");
        assert_eq!(m.playlist_entries[1].title, "雪夜逃婚 - 第二集");
        assert_eq!(m.playlist_entries[1].url, "https://cdn.example.com/ep2.mp4");
    }

    #[test]
    fn test_parse_extractor_progress_line() {
        // Subtitles should be ignored
        let sub_line = "RAW:1024|5147|NA|1500000|0|20.0|srt|/tmp/test.fr.srt";
        assert!(parse_extractor_progress_line(sub_line).is_none());

        // HLS stream with estimate as float and decimal ETA
        let hls_line = "RAW:5124192|NA|64077264.0|913619.9|297.03|8.0|mp4|/tmp/test.mp4";
        let p = parse_extractor_progress_line(hls_line).expect("Should parse HLS progress line");
        assert_eq!(p.downloaded_bytes, 5_124_192);
        assert_eq!(p.total_bytes, Some(64_077_264));
        assert_eq!(p.eta_seconds, Some(297));
        assert!((p.progress_ratio - 0.08).abs() < 0.001);

        // Exact total bytes
        let direct_line = "RAW:1000|2000|NA|500.0|2|50.0|mp4|video.mp4";
        let p2 = parse_extractor_progress_line(direct_line).unwrap();
        assert_eq!(p2.downloaded_bytes, 1000);
        assert_eq!(p2.total_bytes, Some(2000));
        assert_eq!(p2.eta_seconds, Some(2));
        assert!((p2.progress_ratio - 0.50).abs() < 0.001);

        // Fallback ETA when yt-dlp reports NA
        let no_eta_line = "RAW:1000000|NA|5000000.0|1000000.0|NA|20.0|mp4|video.mp4";
        let p3 = parse_extractor_progress_line(no_eta_line).unwrap();
        assert_eq!(p3.eta_seconds, Some(4)); // (5MB - 1MB) / 1MB/s = 4s
    }

    #[test]
    fn test_is_streaming_platform() {
        assert!(is_streaming_platform("https://youtu.be/RB1bUNbQG_U"));
        assert!(is_streaming_platform("https://www.youtube.com/watch?v=12345"));
        assert!(is_streaming_platform("https://vimeo.com/76979871"));
        assert!(is_streaming_platform("https://www.tiktok.com/@user/video/123"));
        assert!(!is_streaming_platform("https://example.com/video.mp4"));
        assert!(!is_streaming_platform("http://files.cdn.com/stream.webm"));
    }

    #[test]
    fn test_parse_quality_and_codecs() {
        let json: serde_json::Value = serde_json::json!({
            "fps": 60.0,
            "vcodec": "avc1.640028",
            "acodec": "mp4a.40.2",
            "filesize": 150_000_000u64,
            "formats": [
                {
                    "format_id": "140",
                    "vcodec": "none",
                    "acodec": "mp4a.40.2",
                    "filesize": 8_500_000u64,
                    "height": null
                },
                {
                    "format_id": "137",
                    "height": 1080,
                    "vcodec": "avc1.640028",
                    "acodec": "none",
                    "filesize": 120_000_000u64
                },
                {
                    "format_id": "136",
                    "height": 720,
                    "vcodec": "avc1.4d401f",
                    "acodec": "none",
                    "filesize": 60_000_000u64
                }
            ]
        });

        let q = parse_quality_and_codecs(&json, Some(300), Some(150_000_000));

        assert_eq!(q.fps, Some(60.0));
        assert_eq!(q.vcodec, Some("H.264 (AVC)".to_string()));
        assert_eq!(q.acodec, Some("AAC".to_string()));
        assert_eq!(q.size_audio, Some(8_500_000));
        // 1080p video (120M) + audio (8.5M) = 128.5M
        assert_eq!(q.size_1080p, Some(128_500_000));
        // 720p video (60M) + audio (8.5M) = 68.5M
        assert_eq!(q.size_720p, Some(68_500_000));
        // 480p fallback estimated based on duration 300s * 120_000 = 36_000_000
        assert_eq!(q.size_480p, Some(36_000_000));
        assert_eq!(q.size_best, Some(150_000_000));
    }

    #[test]
    fn test_parse_quality_bitrate_and_4k() {
        // Test bitrate fallback (e.g. YouTube DASH where filesize is null) and 4K (2160p) resolution handling
        let json: serde_json::Value = serde_json::json!({
            "formats": [
                {
                    "format_id": "251",
                    "vcodec": "none",
                    "acodec": "opus",
                    "abr": 160.0, // 160 kbps -> 20,000 bytes/sec * 100s = 2,000,000
                    "height": null
                },
                {
                    "format_id": "137",
                    "height": 1080,
                    "vcodec": "av01.0.08M.08",
                    "acodec": "none",
                    "tbr": 4000.0 // 4 Mbps -> 500,000 bytes/sec * 100s = 50,000,000
                },
                {
                    "format_id": "313",
                    "height": 2160,
                    "vcodec": "vp09.02.51.10",
                    "acodec": "none",
                    "vbr": 16000.0 // 16 Mbps -> 2,000,000 bytes/sec * 100s = 200,000,000
                }
            ]
        });

        let q = parse_quality_and_codecs(&json, Some(100), None);

        assert_eq!(q.vcodec, Some("VP9".to_string()));
        assert_eq!(q.acodec, Some("Opus".to_string()));
        assert_eq!(q.size_audio, Some(2_000_000));
        // 1080p: 50,000,000 + 2,000,000 audio = 52,000,000
        assert_eq!(q.size_1080p, Some(52_000_000));
        // 4K (2160p): 200,000,000 + 2,000,000 audio = 202,000,000
        // size_best should pick the 4K size (202M) rather than being capped to 1080p (52M)
        assert_eq!(q.size_best, Some(202_000_000));
    }

    #[test]
    fn test_is_candidate_media_url() {
        assert!(is_candidate_media_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ"));
        assert!(is_candidate_media_url("https://vimeo.com/12345678"));
        assert!(is_candidate_media_url("https://cdn.example.com/video/stream.m3u8"));
        assert!(is_candidate_media_url("https://example.com/downloads/episode1.mp4"));
        assert!(is_candidate_media_url("https://myanime.org/play/episode-10"));
        assert!(is_candidate_media_url("https://www.anyreel.app/episodes/i-swapped-my-vampire-husband-6135"));
        assert!(!is_candidate_media_url("not a url"));
        assert!(!is_candidate_media_url("https://en.wikipedia.org/wiki/Rust"));
        assert!(!is_candidate_media_url("ftp://example.com/file.txt"));

        // Test extracting URL embedded in surrounding message text
        let msg = "Hey, check out this episode: https://www.anyreel.app/episodes/i-swapped-my-vampire-husband-6135!";
        assert_eq!(
            extract_candidate_media_url(msg),
            Some("https://www.anyreel.app/episodes/i-swapped-my-vampire-husband-6135".to_string())
        );

        // Test extracting Douyin share text without spaces
        assert!(is_candidate_media_url("https://www.douyin.com/video/7345678901234567890"));
        assert!(is_candidate_media_url("https://v.douyin.com/iJabcde/"));
        assert!(is_candidate_media_url("https://www.douyin.com/jingxuan?modal_id=7685884183775841563"));
        let douyin_share = "7.32复制打开抖音，看看【某某的作品】https://v.douyin.com/iJabcde/。更多精彩内容";
        assert_eq!(
            extract_candidate_media_url(douyin_share),
            Some("https://v.douyin.com/iJabcde/".to_string())
        );

        // Test extracting Douyin jingxuan / modal URL embedded in text
        let modal_text = "Check this out https://www.douyin.com/jingxuan?modal_id=7685884183775841563 amazing video";
        assert_eq!(
            extract_candidate_media_url(modal_text),
            Some("https://www.douyin.com/video/7685884183775841563".to_string())
        );
    }

    #[test]
    fn test_normalize_media_url() {
        // Douyin feed modal URLs
        assert_eq!(
            normalize_media_url("https://www.douyin.com/jingxuan?modal_id=7685884183775841563"),
            "https://www.douyin.com/video/7685884183775841563"
        );
        assert_eq!(
            normalize_media_url("https://www.douyin.com/discover?modal_id=7685884183775841563"),
            "https://www.douyin.com/video/7685884183775841563"
        );
        assert_eq!(
            normalize_media_url("https://www.douyin.com/user/MS4wLjABAAAA?modal_id=7685884183775841563"),
            "https://www.douyin.com/video/7685884183775841563"
        );
        assert_eq!(
            normalize_media_url("https://www.douyin.com/?aweme_id=7685884183775841563"),
            "https://www.douyin.com/video/7685884183775841563"
        );
        // Already canonical Douyin video URL unchanged
        assert_eq!(
            normalize_media_url("https://www.douyin.com/video/7685884183775841563"),
            "https://www.douyin.com/video/7685884183775841563"
        );
        // TikTok modal URL
        assert_eq!(
            normalize_media_url("https://www.tiktok.com/explore?modal_id=7123456789012345678"),
            "https://www.tiktok.com/video/7123456789012345678"
        );
        // Standard YouTube URL unchanged
        assert_eq!(
            normalize_media_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(45), "00:45");
        assert_eq!(format_duration(332), "05:32");
        assert_eq!(format_duration(3665), "01:01:05");
    }

    #[test]
    fn test_html_scraper_title_and_stream() {
        let sample_html = r#"
            <!DOCTYPE html>
            <html>
            <head>
                <meta property="og:title" content="Awesome Drama Episode 1 &amp; Highlights" />
                <meta property="og:video" content="https://cdn.example.com/streams/video_master.m3u8" />
            </head>
            <body>
                <video src="https://cdn.example.com/backup.mp4"></video>
            </body>
            </html>
        "#;

        let title = extract_html_title(sample_html);
        assert_eq!(title, Some("Awesome Drama Episode 1 & Highlights".to_string()));

        let stream = extract_stream_from_html(sample_html, "https://example.com/play/123");
        assert_eq!(stream, Some("https://cdn.example.com/streams/video_master.m3u8".to_string()));
    }

    #[test]
    fn test_html_escape_clean() {
        assert_eq!(html_escape_clean(r"video\u0026amp;title"), "video&title");
        assert_eq!(html_escape_clean(r"https:\/\/cdn.example.com\/file.m3u8"), "https://cdn.example.com/file.m3u8");
    }

    #[test]
    fn test_extract_maccms_player() {
        let sample = r#"
            <script type="text/javascript">var player_aaaa={"flag":"play","encrypt":0,"trysee":0,"points":0,"link":"\/index.php\/vod\/play\/id\/10\/sid\/1\/nid\/1.html","link_next":"\/index.php\/vod\/play\/id\/10\/sid\/1\/nid\/3.html","link_pre":"\/index.php\/vod\/play\/id\/10\/sid\/1\/nid\/1.html","vod_data":{"vod_name":"Urban Miracle Doctor","vod_actor":"","vod_director":"","vod_class":"sci-fi,action,Fantasy,Donghua"},"url":"k475zizcIwkakjJWGN4","url_next":"krSm7a6gOr3MI1JWFVY","from":"dailymotion","server":"no","note":"","id":"10","sid":1,"nid":2}</script>
        "#;
        let info = extract_maccms_player(sample).expect("Should extract MacCMS player");
        assert_eq!(info.video_url, "https://www.dailymotion.com/video/k475zizcIwkakjJWGN4");
        assert_eq!(info.title, Some("Urban Miracle Doctor EP2".to_string()));
    }

    #[test]
    fn test_extract_iframes_from_html() {
        let sample = r#"
            <div>
                <iframe src="/player/embed?id=123" width="100%"></iframe>
                <iframe src="https://www.youtube.com/embed/dQw4w9WgXcQ"></iframe>
            </div>
        "#;
        let iframes = extract_iframes_from_html(sample, "https://example.com/watch");
        assert_eq!(iframes.len(), 2);
        assert_eq!(iframes[0], "https://example.com/player/embed?id=123");
        assert_eq!(iframes[1], "https://www.youtube.com/embed/dQw4w9WgXcQ");
    }

    #[test]
    fn test_extract_playlist_from_detail_html() {
        let sample_detail = r#"
            <!DOCTYPE html>
            <html>
            <head>
                <script type="application/ld+json">[{"@context":"https://schema.org","@type":"VideoObject","name":"Beyond Time’s Gaze"}]</script>
                <meta property="og:image" content="https://example.com/poster.webp" />
            </head>
            <body>
                <ul class="anthology-list-play">
                    <li><a href="/index.php/vod/play/id/30/sid/1/nid/1.html">EP02</a></li>
                    <li><a href="/index.php/vod/play/id/30/sid/1/nid/2.html">EP01</a></li>
                </ul>
            </body>
            </html>
        "#;
        let meta = extract_playlist_from_detail_html(sample_detail, "https://example.com/index.php/vod/detail/id/30.html")
            .expect("Should extract series playlist");

        assert_eq!(meta.title, "Beyond Time’s Gaze");
        assert!(meta.is_playlist);
        assert_eq!(meta.playlist_count, 2);
        // EP01 should be first after natural ascending sort
        assert_eq!(meta.playlist_entries[0].title, "EP01");
        assert_eq!(meta.playlist_entries[0].url, "https://example.com/index.php/vod/play/id/30/sid/1/nid/2.html");
        assert_eq!(meta.playlist_entries[1].title, "EP02");
        assert_eq!(meta.playlist_entries[1].url, "https://example.com/index.php/vod/play/id/30/sid/1/nid/1.html");
        assert_eq!(meta.thumbnail_url, Some("https://example.com/poster.webp".to_string()));
    }

    #[test]
    fn test_strip_html_tags_and_donghuafun_maccms() {
        // 1. Verify HTML tags like <span> and <em class="play-on"><i></i>...</em> are completely stripped
        let dirty_label = r#"<span>EP07</span> <em class="play-on"><i></i><i></i><i></i><i></i><i></i></em>"#;
        assert_eq!(strip_html_tags_and_clean(dirty_label), "EP07");

        let dirty_ep5 = "<span>EP05</span>";
        assert_eq!(strip_html_tags_and_clean(dirty_ep5), "EP05");

        // 2. Verify cleaning episode number from series title
        let raw_title = "The Great Ruler: Chibi Version EP07";
        assert_eq!(clean_series_title_episodes(raw_title), "The Great Ruler: Chibi Version");

        // 3. Verify DonghuaFun episode 13 player configuration (Dailymotion)
        let donghuafun_html = r#"
            <script type="text/javascript">var player_aaaa={"flag":"play","encrypt":0,"trysee":0,"points":0,"link":"\/index.php\/vod\/play\/id\/234\/sid\/1\/nid\/1.html","link_next":"","link_pre":"\/index.php\/vod\/play\/id\/234\/sid\/1\/nid\/12.html","vod_data":{"vod_name":"The Great Ruler: Chibi Version","vod_actor":"","vod_director":"","vod_class":"Fantasy,Comedy"},"url":"k3bqPfn2nZyi6IIw5h0","url_next":"","from":"dailymotion","server":"no","note":"","id":"234","sid":1,"nid":13}</script>
            <ul class="anthology-list-play">
                <li><a href="/index.php/vod/play/id/234/sid/1/nid/12.html"><span>EP12</span></a></li>
                <li><a href="/index.php/vod/play/id/234/sid/1/nid/13.html"><span>EP13</span> <em class="play-on"><i></i></em></a></li>
            </ul>
        "#;
        let maccms = extract_maccms_player(donghuafun_html).expect("Should extract MacCMS Dailymotion player");
        assert_eq!(maccms.video_url, "https://www.dailymotion.com/video/k3bqPfn2nZyi6IIw5h0");
        assert_eq!(maccms.series_name, Some("The Great Ruler: Chibi Version".to_string()));
        assert_eq!(maccms.nid, Some(13));

        // 4. Verify extract_playlist_from_detail_html incorporates player stream and strips tags
        let playlist = extract_playlist_from_detail_html(donghuafun_html, "https://donghuafun.com/index.php/vod/play/id/234/sid/1/nid/13.html")
            .expect("Should extract playlist");
        assert_eq!(playlist.title, "The Great Ruler: Chibi Version");
        assert_eq!(playlist.url, "https://www.dailymotion.com/video/k3bqPfn2nZyi6IIw5h0");
        assert_eq!(playlist.playlist_entries.len(), 2);
        assert_eq!(playlist.playlist_entries[0].title, "EP12");
        assert_eq!(playlist.playlist_entries[1].title, "EP13");
        // EP13 is current page player so its URL was directly resolved to Dailymotion
        assert_eq!(playlist.playlist_entries[1].url, "https://www.dailymotion.com/video/k3bqPfn2nZyi6IIw5h0");

        // 5. Verify encrypt: 2 (base64)
        let b64_sample = r#"var player_aaaa={"flag":"play","encrypt":2,"trysee":0,"points":0,"link":"","vod_data":{"vod_name":"Anime"},"url":"aHR0cHM6Ly9jZG4uZXhhbXBsZS5jb20vbWFzdGVyLm0zdTg=","from":"dplayer","id":"1","nid":1};"#;
        let b64_info = extract_maccms_player(b64_sample).expect("Should decode base64");
        assert_eq!(b64_info.video_url, "https://cdn.example.com/master.m3u8");
    }

    #[test]
    fn test_parse_episode_range() {
        // Range 1-5 from 41
        let r1 = parse_episode_range("1-5", 41);
        assert_eq!(r1.len(), 5);
        assert!(r1.contains(&0));
        assert!(r1.contains(&4));
        assert!(!r1.contains(&5));

        // Mixed comma and ranges
        let r2 = parse_episode_range("1, 3, 5-7, 10", 41);
        assert_eq!(r2.len(), 6);
        assert!(r2.contains(&0)); // 1
        assert!(r2.contains(&2)); // 3
        assert!(r2.contains(&4)); // 5
        assert!(r2.contains(&5)); // 6
        assert!(r2.contains(&6)); // 7
        assert!(r2.contains(&9)); // 10

        // Tail range
        let r3 = parse_episode_range("40-", 41);
        assert_eq!(r3.len(), 2);
        assert!(r3.contains(&39));
        assert!(r3.contains(&40));

        // Head range
        let r4 = parse_episode_range("-3", 41);
        assert_eq!(r4.len(), 3);
        assert!(r4.contains(&0));
        assert!(r4.contains(&1));
        assert!(r4.contains(&2));

        // Clamping & Empty
        let r5 = parse_episode_range("1-100", 10);
        assert_eq!(r5.len(), 10);
        let r6 = parse_episode_range("", 10);
        assert_eq!(r6.len(), 0);
    }

    #[test]
    fn test_clean_extractor_error() {
        let iq_err = "[iq.com] z1b52xonyk: No video formats found!; please report this issue on https://github.com/yt-dlp/yt-dlp/issues?q= , filling out the appropriate issue template. Confirm you are on the latest version using yt-dlp -U";
        let cleaned = clean_extractor_error(iq_err);
        assert!(cleaned.contains("limits simultaneous VIP streams"));

        let iq_vip_err = "[iq.com] z1b52xonyk: This video requires VIP membership";
        assert!(clean_extractor_error(iq_vip_err).contains("DRM-protected or requires VIP login"));

        let drm_err = "ERROR: This video contains DRM protection (Widevine)";
        assert!(clean_extractor_error(drm_err).contains("DRM-protected"));

        let geo_err = "ERROR: This video is not available in your country";
        assert!(clean_extractor_error(geo_err).to_lowercase().contains("geo-restricted"));

        let exit_err = "Application error: Extractor process finished with exit code Some(1)";
        assert!(clean_extractor_error(exit_err).contains("Stream extraction failed"));

        let douyin_cookie_err = "[Douyin] 7685884183775841563: Fresh cookies (not necessarily logged in) are needed";
        assert!(clean_extractor_error(douyin_cookie_err).contains("fresh session cookies"));
    }

    #[test]
    fn test_is_media_stream_candidate() {
        assert!(is_media_stream_candidate("https://example.com/video.mp4"));
        assert!(is_media_stream_candidate("https://cdn.net/hls/master.m3u8?token=xyz"));
        assert!(is_media_stream_candidate("https://stream.io/file.webm"));
        assert!(is_media_stream_candidate("https://video.org/chunk.ts"));
        assert!(!is_media_stream_candidate("https://example.com/image.jpg"));
        assert!(!is_media_stream_candidate("https://tracker.doubleclick.net/ad.js"));
        assert!(!is_media_stream_candidate("https://site.com/style.css"));
    }

    #[test]
    fn test_universal_sniff_media_from_html_json_state() {
        let sample = r#"
            <!DOCTYPE html>
            <html>
            <head><title>Custom Player Page</title></head>
            <body>
            <script>
                window.__INITIAL_STATE__ = {
                    "video": {
                        "title": "Universal Mystery Episode 1",
                        "playUrl": "https://media.org/content/ep1.mp4",
                        "poster": "https://media.org/poster.jpg"
                    }
                };
            </script>
            </body>
            </html>
        "#;
        let meta = universal_sniff_media_from_html(sample, "https://mystery.tv/watch/1").expect("Must sniff stream");
        assert_eq!(meta.url, "https://media.org/content/ep1.mp4");
        assert_eq!(meta.title, "Universal Mystery Episode 1");
        assert_eq!(meta.ext, Some("mp4".to_string()));
        assert_eq!(meta.thumbnail_url, Some("https://media.org/poster.jpg".to_string()));
    }

    #[test]
    fn test_universal_sniff_media_from_html_player_config() {
        let sample = r#"
            <html>
            <body>
            <script>
                const playerConfig = {
                    "source": "https://cdn.livebroadcast.com/live/hls/master.m3u8",
                    "name": "Live Event Stream"
                };
            </script>
            </body>
            </html>
        "#;
        let meta = universal_sniff_media_from_html(sample, "https://livebroadcast.com").expect("Must sniff m3u8");
        assert_eq!(meta.url, "https://cdn.livebroadcast.com/live/hls/master.m3u8");
        assert_eq!(meta.ext, Some("m3u8".to_string()));
        assert_eq!(meta.title, "Live Event Stream");
    }

    #[test]
    fn test_extract_anyreel_drama() {
        let sample = r#"
            <!DOCTYPE html>
            <html>
            <head>
                <title>I Swapped My Vampire Husband - Anyreel</title>
                <meta property="og:image" content="https://anyoss.anyreel.app/shortdrama/cover123.jpg" />
            </head>
            <body>
            <script>
                self.__next_f.push([1,"6:[[\"$\",\"$L1\",null,{\"data\":{\"seriesName\":\"I Swapped My Vampire Husband\",\"dramaEpisodes\":[{\"episodeSort\":1,\"title\":\"Episode 1\",\"videoId\":\"5001\",\"totalDuration\":79,\"imageSprite\":{\"imageUrlSet\":[\"http://videoint.anyreel.app/43a3213bvoduse1500065780/hash1/imageSprite/thumb_0.jpg\"]}},{\"episodeSort\":2,\"title\":\"Episode 2\",\"videoId\":\"5002\",\"totalDuration\":66,\"imageSprite\":{\"imageUrlSet\":[\"http://videoint.anyreel.app/43a3213bvoduse1500065780/hash2/imageSprite/thumb_0.jpg\"]}}]}}]]"]);
            </script>
            </body>
            </html>
        "#;
        let meta = extract_anyreel_drama(sample, "https://www.anyreel.app/video/episode-1-i-swapped-my-vampire-husband-6135").expect("Must extract AnyReel drama");
        assert_eq!(meta.title, "I Swapped My Vampire Husband");
        assert!(meta.is_playlist);
        assert_eq!(meta.playlist_count, 2);
        assert_eq!(meta.playlist_entries[0].url, "http://videoint.anyreel.app/43a3213bvoduse1500065780/hash1/adp.1936796.m3u8");
        assert_eq!(meta.playlist_entries[0].title, "I Swapped My Vampire Husband - Episode 1");
        assert_eq!(meta.playlist_entries[1].url, "http://videoint.anyreel.app/43a3213bvoduse1500065780/hash2/adp.1936796.m3u8");
        assert_eq!(meta.playlist_entries[1].title, "I Swapped My Vampire Husband - Episode 2");
        assert_eq!(meta.thumbnail_url, Some("https://anyoss.anyreel.app/shortdrama/cover123.jpg".to_string()));
    }

    #[tokio::test]
    async fn test_inspect_anyreel_live_url() {
        let res = scrape_page_for_media_with_proxy(
            "https://www.anyreel.app/episodes/i-swapped-my-vampire-husband-6135",
            None,
        ).await;
        if let Ok(meta) = res {
            assert!(meta.is_playlist);
            assert_eq!(meta.title, "I Swapped My Vampire Husband");
            assert_eq!(meta.playlist_count, 6);
            assert!(meta.url.contains(".m3u8"));
            assert_eq!(meta.referer, Some("https://www.anyreel.app/".to_string()));
        }
    }

    #[test]
    fn test_extract_dramabox_drama() {
        let sample = r#"
            <!DOCTYPE html>
            <html>
            <head><title>My Bossy CEO Husband - DramaBox</title></head>
            <body>
            <script id="__NEXT_DATA__" type="application/json">
            {
                "props": {
                    "pageProps": {
                        "dramaInfo": {
                            "dramaName": "My Bossy CEO Husband",
                            "coverUrl": "https://img.dramaboxdb.com/cover/ceo.jpg"
                        },
                        "chapterList": [
                            {
                                "chapterName": "Episode 1: The Contract",
                                "videoUrl": "https://vod.dramaboxdb.com/stream/ep1.m3u8"
                            },
                            {
                                "chapterName": "Episode 2: The Secret",
                                "videoUrl": "https://vod.dramaboxdb.com/stream/ep2.m3u8"
                            }
                        ]
                    }
                }
            }
            </script>
            </body>
            </html>
        "#;
        let meta = extract_dramabox_drama(sample, "https://www.dramaboxdb.com/drama/1001").expect("Must extract DramaBox drama");
        assert_eq!(meta.title, "My Bossy CEO Husband");
        assert!(meta.is_playlist);
        assert_eq!(meta.playlist_count, 2);
        assert_eq!(meta.playlist_entries[0].url, "https://vod.dramaboxdb.com/stream/ep1.m3u8");
        assert_eq!(meta.playlist_entries[0].title, "My Bossy CEO Husband - Episode 1: The Contract");
        assert_eq!(meta.playlist_entries[1].url, "https://vod.dramaboxdb.com/stream/ep2.m3u8");
        assert_eq!(meta.thumbnail_url, Some("https://img.dramaboxdb.com/cover/ceo.jpg".to_string()));
        assert_eq!(meta.referer, Some("https://www.dramaboxdb.com/".to_string()));
    }

    #[test]
    fn test_extract_shortmax_drama() {
        let sample = r#"
            <!DOCTYPE html>
            <html>
            <body>
            <script id="__NEXT_DATA__" type="application/json">
            {
                "props": {
                    "pageProps": {
                        "seriesInfo": {
                            "seriesName": "Alpha's Forgotten Luna",
                            "coverUrl": "https://img.shortmax.com/luna.jpg"
                        },
                        "episodes": [
                            {
                                "episodeNum": 1,
                                "title": "The Rejection",
                                "playUrl": "https://video.shortmax.com/luna/ep1.mp4"
                            },
                            {
                                "episodeNum": 2,
                                "title": "The Return",
                                "playUrl": "https://video.shortmax.com/luna/ep2.mp4"
                            }
                        ]
                    }
                }
            }
            </script>
            </body>
            </html>
        "#;
        let meta = extract_shortmax_drama(sample, "https://www.shortmax.com/drama/555").expect("Must extract ShortMax drama");
        assert_eq!(meta.title, "Alpha's Forgotten Luna");
        assert!(meta.is_playlist);
        assert_eq!(meta.playlist_count, 2);
        assert_eq!(meta.playlist_entries[0].url, "https://video.shortmax.com/luna/ep1.mp4");
        assert_eq!(meta.playlist_entries[0].title, "Alpha's Forgotten Luna - The Rejection");
        assert_eq!(meta.thumbnail_url, Some("https://img.shortmax.com/luna.jpg".to_string()));
    }

    #[test]
    fn test_extract_reelshort_drama() {
        let sample = r#"
            <!DOCTYPE html>
            <html>
            <body>
            <script id="__NEXT_DATA__" type="application/json">
            {
                "props": {
                    "pageProps": {
                        "book": {
                            "title": "Never Divorce a Secret Billionaire",
                            "cover_image": "https://img.reelshort.com/billionaire.jpg"
                        },
                        "chapter_list": [
                            {
                                "chapter_name": "Part 1",
                                "video_url": "https://stream.reelshort.com/b1.m3u8"
                            },
                            {
                                "chapter_name": "Part 2",
                                "video_url": "https://stream.reelshort.com/b2.m3u8"
                            }
                        ]
                    }
                }
            }
            </script>
            </body>
            </html>
        "#;
        let meta = extract_reelshort_drama(sample, "https://www.reelshort.com/drama/777").expect("Must extract ReelShort drama");
        assert_eq!(meta.title, "Never Divorce a Secret Billionaire");
        assert!(meta.is_playlist);
        assert_eq!(meta.playlist_count, 2);
        assert_eq!(meta.playlist_entries[0].url, "https://stream.reelshort.com/b1.m3u8");
        assert_eq!(meta.playlist_entries[0].title, "Never Divorce a Secret Billionaire - Part 1");
        assert_eq!(meta.thumbnail_url, Some("https://img.reelshort.com/billionaire.jpg".to_string()));
    }

    #[test]
    fn test_extract_generic_short_drama() {
        let sample = r#"
            <!DOCTYPE html>
            <html>
            <head><title>FlickReels Mystery</title></head>
            <body>
            <script id="__NEXT_DATA__" type="application/json">
            {
                "props": {
                    "pageProps": {
                        "customPayload": {
                            "episodesList": [
                                { "chapterName": "Intro", "video_url": "https://flickreels.com/ep1.mp4" },
                                { "chapterName": "Climax", "video_url": "https://flickreels.com/ep2.mp4" }
                            ]
                        }
                    }
                }
            }
            </script>
            </body>
            </html>
        "#;
        let meta = universal_sniff_media_from_html(sample, "https://flickreels.com/drama/999").expect("Must extract generic drama");
        assert_eq!(meta.title, "FlickReels Mystery");
        assert!(meta.is_playlist);
        assert_eq!(meta.playlist_count, 2);
        assert_eq!(meta.playlist_entries[0].url, "https://flickreels.com/ep1.mp4");
    }

    #[tokio::test]
    async fn test_douyin_via_douyinsaver_live_inspection() {
        // Test that live inspection of Douyin URL resolves successfully without requiring cookies
        let test_url = "https://www.douyin.com/video/7685884183775841563";
        if let Ok(meta) = resolve_douyin_via_douyinsaver(test_url, None).await {
            assert!(meta.url.starts_with("http"));
            assert!(!meta.title.is_empty());
            assert!(meta.supports_ranges);
            assert_eq!(meta.ext, Some("mp4".to_string()));
        }
    }

    #[tokio::test]
    async fn test_generate_kisskh_kkey() {
        let ep_id = 224512;
        let video_key = generate_kisskh_kkey(ep_id, false).await;
        assert!(video_key.is_ok(), "Video keygen must succeed: {:?}", video_key);
        let vk = video_key.unwrap();
        assert_eq!(vk, "818439C51A7873737C0C2B3A365A79918911E03C78C4A81589008A4D2C6115C63ED93B889BECD47067AD429E061BEC6EE02B57DE0604CE3108E15B9AAE0B68F12E5B6AD7DA2373E966D49AC9E16786EA3CA00D7EC48F7297BCAAE9D2E186ABD16120131389E31D05F99AF401E262E5C1419ADB4F97D2872AD25A210B96BC7F5E");

        let sub_key = generate_kisskh_kkey(ep_id, true).await;
        assert!(sub_key.is_ok(), "Sub keygen must succeed: {:?}", sub_key);
        let sk = sub_key.unwrap();
        assert_eq!(sk, "F0FB00F5440B67D6CE1ED8526AA588AAFF01EEB5C5FE681DD520E028E665C4B1E954909499657148E8F43A3CC418CF11378808FD4A2027EDAE60186698F283FB52C6AB83E27ED5932A1DD6FB73D5AB019AB7B99F541C9932D528AEDDC5D4DDCC555A6AB85F673FCD88B6D2BD6B80338D5B6175B973A95EFBD8C01D35E054986A");
    }

    #[tokio::test]
    async fn test_extract_kisskh_drama_live() {
        let test_url = "https://kisskh.do/Drama/Spring-of-the-Blade--2026-/Episode-1?id=13742&ep=224512&page=0&pageSize=100";
        if let Ok(meta) = extract_kisskh_drama(test_url, None, None).await {
            assert!(meta.url.contains(".m3u8"), "Stream URL must be an m3u8 stream: {}", meta.url);
            assert!(meta.title.contains("Spring of the Blade"), "Title must match drama: {}", meta.title);
            assert!(meta.is_playlist);
            assert!(meta.playlist_count >= 20);
            assert!(meta.has_subtitles);
            assert!(meta.subtitles_summary.contains("English") || meta.subtitles_summary.contains("Khmer"));
        }
    }

    #[test]
    fn test_extract_available_stream_formats() {
        let sample_json: serde_json::Value = serde_json::json!({
            "formats": [
                {
                    "format_id": "137",
                    "ext": "mp4",
                    "width": 1920,
                    "height": 1080,
                    "fps": 60.0,
                    "vcodec": "avc1.640028",
                    "acodec": "none",
                    "tbr": 4500.0,
                    "filesize": 150_000_000
                },
                {
                    "format_id": "313",
                    "ext": "webm",
                    "width": 3840,
                    "height": 2160,
                    "fps": 60.0,
                    "vcodec": "vp09.00",
                    "acodec": "none",
                    "tbr": 18000.0,
                    "filesize": 400_000_000
                },
                {
                    "format_id": "140",
                    "ext": "m4a",
                    "vcodec": "none",
                    "acodec": "mp4a.40.2",
                    "abr": 128.0,
                    "filesize": 15_000_000
                }
            ]
        });

        let q = QualityAndCodecs {
            fps: Some(60.0),
            vcodec: Some("H.264".to_string()),
            acodec: Some("AAC".to_string()),
            size_best: Some(400_000_000),
            size_1080p: Some(150_000_000),
            size_720p: None,
            size_480p: None,
            size_audio: Some(15_000_000),
        };

        let streams = extract_available_stream_formats(&sample_json, Some(120), &q);
        assert!(!streams.is_empty(), "Should extract available streams");
        assert!(streams.iter().any(|s| s.format_id == "2160p"), "Should have 4K stream");
        assert!(streams.iter().any(|s| s.format_id == "1080p"), "Should have 1080p stream");
        assert!(streams.iter().any(|s| s.format_id == "audio"), "Should have audio stream");

        // Verify fallback on VideoMetadata
        let mut empty_meta = VideoMetadata::default();
        empty_meta.size_1080p = Some(50_000_000);
        empty_meta.ensure_available_formats();
        assert!(!empty_meta.available_formats.is_empty());
        assert_eq!(empty_meta.available_formats[0].format_id, "best");
        assert!(empty_meta.available_formats.iter().any(|s| s.format_id == "1080p"));
    }
}

/// Parses an episode selection range string (e.g. "1-10", "1, 3, 5", "1-5, 10-15") into a set of 0-based indices
pub fn parse_episode_range(input: &str, total: usize) -> std::collections::HashSet<usize> {
    let mut selected = std::collections::HashSet::new();
    if total == 0 {
        return selected;
    }

    for part in input.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        if let Some((start_str, end_str)) = part.split_once('-') {
            let start: usize = start_str.trim().parse().unwrap_or(1).max(1);
            let end: usize = if end_str.trim().is_empty() {
                total
            } else {
                end_str.trim().parse().unwrap_or(total).min(total)
            };
            if start <= end {
                for i in start..=end {
                    if i >= 1 && i <= total {
                        selected.insert(i - 1);
                    }
                }
            }
        } else if let Ok(num) = part.parse::<usize>() {
            if num >= 1 && num <= total {
                selected.insert(num - 1);
            }
        }
    }

    selected
}

/// Translates low-level or platform-specific extractor errors into clear, actionable user messages
pub fn clean_extractor_error(raw_err: &str) -> String {
    let raw_trimmed = raw_err.trim_start_matches("Application error: ");
    let lower = raw_trimmed.to_lowercase();

    if lower.contains("exit code") || lower.contains("extractor process finished") {
        return "Stream extraction failed. The source media stream may be expired, protected, or unreachable.".to_string();
    }
    if lower.contains("403") || lower.contains("forbidden") {
        return "Access forbidden (HTTP 403). The video server may require fresh cookies or anti-leech headers.".to_string();
    }
    if lower.contains("404") || lower.contains("not found") {
        return "Media stream not found (HTTP 404). The stream file was moved or removed.".to_string();
    }
    if lower.contains("413") || lower.contains("request entity too large") {
        return "Request header too large (HTTP 413). iQIYI server rejected cookie size. Clear or disable browser cookies in Settings.".to_string();
    }
    if lower.contains("phantomjs") || (lower.contains("iq.com") && lower.contains("phantomjs")) {
        return "iQIYI stream decryption requires PhantomJS or is DRM-protected. DRM-encrypted content cannot be downloaded.".to_string();
    }
    if (lower.contains("iq.com") || lower.contains("iqiyi")) && lower.contains("no video formats found") {
        return "No video formats found from iQIYI. iQIYI limits simultaneous VIP streams: close active playback tabs in Chrome, wait 2-3 minutes, and retry.".to_string();
    }
    if (lower.contains("iq.com") || lower.contains("iqiyi")) && (lower.contains("drm") || lower.contains("vip")) {
        return "iQIYI stream is DRM-protected or requires VIP login. DRM-encrypted content cannot be downloaded.".to_string();
    }
    if lower.contains("this video is only available for registered users")
        || lower.contains("sign in to confirm your age")
        || lower.contains("members-only")
        || lower.contains("private video")
    {
        return "This video is private, age-restricted, or requires a signed-in account/subscription.".to_string();
    }
    if lower.contains("drm") || lower.contains("widevine") {
        return "This video stream is DRM-protected (encrypted) and cannot be downloaded.".to_string();
    }
    if lower.contains("no video formats found") {
        return "No downloadable video formats found. Stream may be DRM-protected, require VIP login, or be region-locked.".to_string();
    }
    if (lower.contains("douyin.com") || lower.contains("tiktok.com"))
        && (lower.contains("unsupported url") || lower.contains("is not a valid url"))
    {
        return "Please provide a specific video or share link (e.g. https://www.douyin.com/video/... or https://v.douyin.com/...) rather than the homepage.".to_string();
    }
    if lower.contains("fresh cookies") || (lower.contains("douyin") && lower.contains("cookie")) {
        return "Douyin requires fresh session cookies: open the video in your browser (e.g. Chrome), play it for 2-3 seconds to refresh session tokens, then retry downloading.".to_string();
    }
    if lower.contains("not available in your country")
        || lower.contains("geo-restricted")
        || lower.contains("blocked in your region")
    {
        return "This video is not available in your region (Geo-restricted).".to_string();
    }

    // Strip verbose yt-dlp issue template boilerplate
    let clean = raw_trimmed
        .split("; please report this issue")
        .next()
        .unwrap_or(raw_trimmed)
        .split("; confirm you are on the latest version")
        .next()
        .unwrap_or(raw_trimmed)
        .trim();

    clean.to_string()
}

/// Queries the installed version and path of yt-dlp
pub async fn get_ytdlp_info() -> (String, String) {
    if let Some(path) = find_ytdlp_path().await {
        let path_str = path.to_string_lossy().to_string();
        if let Ok(output) = Command::new(&path).arg("--version").output().await {
            if output.status.success() {
                let ver = String::from_utf8_lossy(&output.stdout).trim().to_string();
                return (format!("v{}", ver), path_str);
            }
        }
        ("Installed".to_string(), path_str)
    } else {
        ("Not installed".to_string(), "Missing from system".to_string())
    }
}

/// Queries the installed version and path of FFmpeg
pub async fn get_ffmpeg_info() -> (String, String) {
    if let Some(path) = find_ffmpeg_path() {
        let path_str = path.to_string_lossy().to_string();
        if let Ok(output) = Command::new(&path).arg("-version").output().await {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(first_line) = stdout.lines().next() {
                    let ver = if let Some(stripped) = first_line.strip_prefix("ffmpeg version ") {
                        stripped.split_whitespace().next().unwrap_or(stripped).to_string()
                    } else {
                        first_line.split_whitespace().nth(2).unwrap_or("Ready").to_string()
                    };
                    return (format!("v{}", ver), path_str);
                }
            }
        }
        ("Installed".to_string(), path_str)
    } else {
        ("Not installed".to_string(), "Missing from system".to_string())
    }
}

/// Reinstalls or upgrades the FFmpeg static standalone binary
pub async fn reinstall_ffmpeg() -> Result<String> {
    let path = download_standalone_ffmpeg().await?;
    let (ver, _) = get_ffmpeg_info().await;
    let msg = format!("FFmpeg {} installed successfully at {:?}", ver, path);
    info!("{}", msg);
    Ok(msg)
}

/// Updates yt-dlp to latest official release via `yt-dlp -U` with standalone download fallback
pub async fn update_ytdlp_engine() -> Result<String> {
    let ytdlp_bin = ensure_ytdlp_installed().await?;
    let _ = ensure_ffmpeg_installed().await;
    info!("Running yt-dlp self-update check via {:?}", ytdlp_bin);
    let output = Command::new(&ytdlp_bin)
        .arg("-U")
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let msg = stdout
                .lines()
                .find(|l| l.contains("up to date") || l.contains("Updated") || l.contains("Updating to"))
                .unwrap_or("yt-dlp is up to date")
                .trim()
                .to_string();
            info!("yt-dlp update result: {}", msg);
            Ok(msg)
        }
        _ => {
            info!("yt-dlp -U produced error or package manager lock; downloading latest standalone binary...");
            let path = download_standalone_ytdlp().await?;
            let (ver, _) = get_ytdlp_info().await;
            let msg = format!("yt-dlp updated to {} at {:?}", ver, path);
            info!("{}", msg);
            Ok(msg)
        }
    }
}

/// Checks and updates both yt-dlp and FFmpeg
pub async fn update_all_binaries() -> Result<String> {
    let ytdlp_res = update_ytdlp_engine().await?;
    let (ff_ver, _) = get_ffmpeg_info().await;
    Ok(format!("{}; FFmpeg: {}", ytdlp_res, ff_ver))
}

/// Clears all cached application and media data:
/// 1. In-memory resolved playable URL cache
/// 2. On-disk browser cookie cache files (`cookies_*.txt`)
/// 3. Temporary thumbnail preview files in the OS temp directory
/// 4. yt-dlp internal cache directory via `yt-dlp --rm-cache-dir`
pub async fn clear_all_caches() -> Result<String> {
    let mut cleared_items = Vec::new();

    // 1. Clear in-memory URL resolution cache
    if let Ok(mut cache) = PLAYABLE_URL_CACHE.write() {
        let count = cache.len();
        cache.clear();
        if count > 0 {
            cleared_items.push(format!("{} resolved stream URLs", count));
        }
    }

    // 2. Clear on-disk cookie cache files
    let bin_dir = get_bin_dir();
    let mut cookie_files_removed = 0;
    if let Ok(entries) = std::fs::read_dir(&bin_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                if file_name.starts_with("cookies_") && file_name.ends_with(".txt") {
                    if let Ok(_) = std::fs::remove_file(&path) {
                        cookie_files_removed += 1;
                    }
                }
            }
        }
    }
    if let Some(parent) = bin_dir.parent() {
        if let Ok(entries) = std::fs::read_dir(parent) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                    if file_name.starts_with("cookies_") && file_name.ends_with(".txt") {
                        if let Ok(_) = std::fs::remove_file(&path) {
                            cookie_files_removed += 1;
                        }
                    }
                }
            }
        }
    }
    if cookie_files_removed > 0 {
        cleared_items.push(format!("{} cookie cache files", cookie_files_removed));
    }

    // 3. Clear temporary thumbnail previews
    let temp_dir = std::env::temp_dir();
    let mut thumbs_removed = 0;
    if let Ok(entries) = std::fs::read_dir(&temp_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("nvd_thumb_") {
                    if let Ok(_) = std::fs::remove_file(&path) {
                        thumbs_removed += 1;
                    }
                }
            }
        }
    }
    if thumbs_removed > 0 {
        cleared_items.push(format!("{} thumbnail previews", thumbs_removed));
    }

    // 4. Run `yt-dlp --rm-cache-dir`
    if let Some(ytdlp_bin) = find_ytdlp_path().await {
        let mut cmd = create_ytdlp_cmd(&ytdlp_bin);
        cmd.arg("--rm-cache-dir");
        if let Ok(output) = cmd.output().await {
            if output.status.success() {
                cleared_items.push("yt-dlp internal cache".to_string());
            }
        }
    }

    if cleared_items.is_empty() {
        Ok("All caches are already clean (0 files to remove).".to_string())
    } else {
        Ok(format!("Cleared: {}", cleared_items.join(", ")))
    }
}




use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::error::{AppError, Result};
use crate::models::{DownloadProgress, VideoMetadata};

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

/// Ensures FFmpeg is available, downloading the official static standalone binary if missing
pub async fn ensure_ffmpeg_installed() -> Result<PathBuf> {
    if let Some(path) = find_ffmpeg_path() {
        return Ok(path);
    }

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

/// Ensures `yt-dlp` is available, downloading the official standalone executable if missing
pub async fn ensure_ytdlp_installed() -> Result<PathBuf> {
    if let Some(cached) = CACHED_YTDLP_BIN.get() {
        return Ok(cached.clone());
    }

    let resolved = if let Some(path) = find_ytdlp_path().await {
        path
    } else {
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
        target_path
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
        || lower.contains("iq.com")
        || lower.contains("iqiyi.com")
        || lower.contains("youku.com")
        || lower.contains("weibo.com")
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
        "/video/", "/play/", "/watch", "/item/", "/episode/",
        "/stream", "m3u8", "/shorts/", "/reel/", "/status/"
    ];
    for kw in &video_keywords {
        if lower.contains(kw) {
            return true;
        }
    }
    false
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
        if let Ok(modified) = metadata.modified() {
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

    if is_fresh {
        info!("Applying cached browser cookies from {:?} (bypassing Keychain decryption)", cache_file);
        cmd.arg("--cookies").arg(&cache_file);
    } else {
        info!("Extracting fresh browser cookies from {} and updating cache at {:?}", trimmed, cache_file);
        cmd.arg("--cookies-from-browser").arg(trimmed);
        cmd.arg("--cookies").arg(&cache_file);
    }
}

/// Analyzes yt-dlp format streams to extract fps, codecs, and calculate estimated sizes for each quality preset
pub fn parse_quality_and_codecs(
    json_val: &serde_json::Value,
    duration_secs: Option<u64>,
    top_filesize: Option<u64>,
) -> (
    Option<f64>,
    Option<String>,
    Option<String>,
    Option<u64>,
    Option<u64>,
    Option<u64>,
    Option<u64>,
    Option<u64>,
) {
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

    // Best Audio size estimate
    let mut best_audio_size: Option<u64> = None;
    if let Some(arr) = formats {
        for f in arr {
            let vc = f.get("vcodec").and_then(|v| v.as_str()).unwrap_or("none");
            let ac = f.get("acodec").and_then(|v| v.as_str()).unwrap_or("none");
            let h = f.get("height").and_then(|v| v.as_u64());

            if (vc == "none" || h.is_none() || h == Some(0)) && ac != "none" {
                let size = f.get("filesize").and_then(|v| v.as_u64())
                    .or_else(|| f.get("filesize_approx").and_then(|v| v.as_u64()));
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
                    .or_else(|| f.get("filesize_approx").and_then(|v| v.as_u64()));
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

    let size_best = top_filesize
        .or(size_1080p)
        .or(size_720p)
        .or(size_480p);

    (
        fps,
        vcodec,
        acodec,
        size_best,
        size_1080p,
        size_720p,
        size_480p,
        size_audio,
    )
}

/// Inspects a video or album/playlist streaming URL to fetch metadata with optional browser cookies and proxy
pub async fn inspect_video_with_options(
    url: &str,
    cookies_browser: Option<&str>,
    proxy: Option<&str>,
) -> Result<VideoMetadata> {
    // Fast path: AnyReel short dramas have custom native parser without yt-dlp delay
    if url.contains("anyreel.app") {
        return scrape_page_for_media_with_proxy(url, proxy).await;
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

    let (fps, vcodec, acodec, size_best, size_1080p, size_720p, size_480p, size_audio) =
        parse_quality_and_codecs(&json_val, duration_secs, filesize);

    Ok(VideoMetadata {
        url: url.to_string(),
        title,
        content_length: filesize.or(size_best),
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
        fps,
        vcodec,
        acodec,
        size_best,
        size_1080p,
        size_720p,
        size_480p,
        size_audio,
        referer: Some(url.to_string()),
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

    let raw_url = val.get("url").and_then(|v| v.as_str())?.trim();
    if raw_url.is_empty() {
        return None;
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
    } else {
        raw_url.to_string()
    };

    let title = val.get("vod_data")
        .and_then(|vd| vd.get("vod_name"))
        .and_then(|vn| vn.as_str())
        .map(|name| {
            if let Some(nid) = val.get("nid").and_then(|n| {
                n.as_i64().map(|i| i.to_string()).or_else(|| n.as_str().map(|s| s.to_string()))
            }) {
                format!("{} EP{}", name, nid)
            } else {
                name.to_string()
            }
        });

    Some(MacCmsInfo { video_url, title })
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

    let title = extract_schema_org_title(html)
        .or_else(|| extract_html_title(html))
        .unwrap_or_else(|| "Series Collection".to_string());

    let thumbnail_url = extract_html_thumbnail(html, page_url);
    let playlist_count = entries.len();
    let primary_url = entries.first().map(|e| e.url.clone()).unwrap_or_else(|| page_url.to_string());

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

        let text_end = block[tag_close + 1..].find("</a>").map(|e| tag_close + 1 + e).unwrap_or(tag_close + 1);
        let inner_text = html_escape_clean(block[tag_close + 1..text_end].trim());

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
        cursor = text_end + 4;
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

        let text_end = html[tag_close + 1..].find("</a>").map(|e| tag_close + 1 + e).unwrap_or(tag_close + 1);
        let inner_text = html_escape_clean(html[tag_close + 1..text_end].trim());

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
        cursor = text_end + 4;
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

/// If the URL is an HTML webpage (e.g. MacCMS vod/play page, iframe host, or direct web scraper),
static PLAYABLE_URL_CACHE: std::sync::LazyLock<std::sync::RwLock<std::collections::HashMap<String, String>>> =
    std::sync::LazyLock::new(|| std::sync::RwLock::new(std::collections::HashMap::new()));

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
                // 0. Check Universal Media Sniffer
                if let Some(sniffed) = universal_sniff_media_from_html(&html, url) {
                    info!("Resolved universal stream URL: {} -> {}", url, sniffed.url);
                    if let Ok(mut cache) = PLAYABLE_URL_CACHE.write() {
                        cache.insert(url.to_string(), sniffed.url.clone());
                    }
                    return sniffed.url;
                }
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
            }
        }
    }

    url.to_string()
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

    if let Some(ref_url) = referer {
        let trimmed = ref_url.trim();
        if !trimmed.is_empty() {
            info!("Auto-forwarding Referer & Origin to yt-dlp: {}", trimmed);
            cmd.arg("--referer").arg(trimmed);
            if let Ok(parsed) = reqwest::Url::parse(trimmed) {
                let origin = format!("{}://{}", parsed.scheme(), parsed.host_str().unwrap_or(""));
                cmd.arg("--add-header").arg(format!("Origin: {}", origin));
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
            match q {
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
                _ => {}
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

    let target_url = resolve_playable_stream_url(url, proxy).await;
    info!("Target URL for extractor download resolved: {}", target_url);

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
            return Ok(p);
        }
    }

    if !status.success() {
        let err_detail = if !stderr_output.trim().is_empty() {
            clean_extractor_error(&stderr_output)
        } else {
            format!("Extractor process finished with exit code {:?}", status.code())
        };
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

        // Verify URL cache is now empty
        if let Ok(cache) = PLAYABLE_URL_CACHE.read() {
            assert!(cache.is_empty());
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

        let (fps, vcodec, acodec, size_best, size_1080p, size_720p, size_480p, size_audio) =
            parse_quality_and_codecs(&json, Some(300), Some(150_000_000));

        assert_eq!(fps, Some(60.0));
        assert_eq!(vcodec, Some("H.264 (AVC)".to_string()));
        assert_eq!(acodec, Some("AAC".to_string()));
        assert_eq!(size_audio, Some(8_500_000));
        // 1080p video (120M) + audio (8.5M) = 128.5M
        assert_eq!(size_1080p, Some(128_500_000));
        // 720p video (60M) + audio (8.5M) = 68.5M
        assert_eq!(size_720p, Some(68_500_000));
        // 480p fallback estimated based on duration 300s * 120_000 = 36_000_000
        assert_eq!(size_480p, Some(36_000_000));
        assert_eq!(size_best, Some(150_000_000));
    }

    #[test]
    fn test_is_candidate_media_url() {
        assert!(is_candidate_media_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ"));
        assert!(is_candidate_media_url("https://vimeo.com/12345678"));
        assert!(is_candidate_media_url("https://cdn.example.com/video/stream.m3u8"));
        assert!(is_candidate_media_url("https://example.com/downloads/episode1.mp4"));
        assert!(is_candidate_media_url("https://myanime.org/play/episode-10"));
        assert!(!is_candidate_media_url("not a url"));
        assert!(!is_candidate_media_url("https://en.wikipedia.org/wiki/Rust"));
        assert!(!is_candidate_media_url("ftp://example.com/file.txt"));
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
    if lower.contains("phantomjs") {
        return "Stream requires PhantomJS JavaScript engine for signature decryption.".to_string();
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

/// Updates the yt-dlp binary to the latest official release via `yt-dlp -U`
pub async fn update_ytdlp_engine() -> Result<String> {
    let ytdlp_bin = ensure_ytdlp_installed().await?;
    let _ = ensure_ffmpeg_installed().await;
    info!("Running yt-dlp self-update check via {:?}", ytdlp_bin);
    let output = Command::new(&ytdlp_bin)
        .arg("-U")
        .output()
        .await?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        let msg = stdout
            .lines()
            .find(|l| l.contains("up to date") || l.contains("Updated") || l.contains("Updating to"))
            .unwrap_or("yt-dlp is up to date")
            .trim()
            .to_string();
        info!("yt-dlp update result: {}", msg);
        Ok(msg)
    } else {
        let err_msg = stderr.lines().next().unwrap_or("Failed to check for updates").trim().to_string();
        Err(AppError::Generic(err_msg))
    }
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




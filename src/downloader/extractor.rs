use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::error::{AppError, Result};
use crate::models::{DownloadProgress, VideoMetadata};

/// Returns the path to the internal binary directory: `~/.native_video_downloader/bin`
pub fn get_bin_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".native_video_downloader").join("bin")
    } else if let Ok(profile) = std::env::var("USERPROFILE") {
        PathBuf::from(profile).join(".native_video_downloader").join("bin")
    } else {
        PathBuf::from(".bin")
    }
}

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

    None
}

/// Ensures `yt-dlp` is available, downloading the official standalone executable if missing
pub async fn ensure_ytdlp_installed() -> Result<PathBuf> {
    if let Some(path) = find_ytdlp_path().await {
        return Ok(path);
    }

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

/// Inspects a video or album/playlist streaming URL to fetch metadata
pub async fn inspect_video(url: &str) -> Result<VideoMetadata> {
    let ytdlp_bin = ensure_ytdlp_installed().await?;

    info!("Inspecting streaming URL with yt-dlp: {}", url);
    let mut cmd = create_ytdlp_cmd(&ytdlp_bin);
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
        if let Ok(scraped_meta) = scrape_page_for_media(url).await {
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

        return Err(AppError::Generic(format!("Extractor: {}", first_err_line)));
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

    Ok(VideoMetadata {
        url: url.to_string(),
        title,
        content_length: filesize,
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
    })
}

/// Scrapes a generic webpage's HTML to locate embedded video tags, OpenGraph video, or .m3u8/.mp4 stream URLs
pub async fn scrape_page_for_media(page_url: &str) -> Result<VideoMetadata> {
    info!("Scraping webpage HTML for media sources: {}", page_url);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let resp = client.get(page_url).send().await?;
    if !resp.status().is_success() {
        return Err(AppError::Generic(format!("HTTP {}", resp.status())));
    }

    let html = resp.text().await?;

    // 1. Scrape title
    let title = extract_html_title(&html)
        .or_else(|| {
            page_url
                .split('/')
                .last()
                .and_then(|s| s.split('?').next())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "web_video".to_string());

    // 2. Scrape stream candidates
    let stream_url = extract_stream_from_html(&html, page_url)
        .ok_or_else(|| AppError::Generic("No direct video or m3u8 stream found in webpage HTML".to_string()))?;

    info!("Scraper found stream source: {}", stream_url);

    let ext = if stream_url.contains(".m3u8") {
        "mp4".to_string() // HLS will be merged to MP4
    } else if stream_url.contains(".webm") {
        "webm".to_string()
    } else {
        "mp4".to_string()
    };

    Ok(VideoMetadata {
        url: stream_url,
        title,
        content_length: None,
        content_type: Some(format!("video/{}", ext)),
        supports_ranges: true,
        is_extractor: true,
        duration_seconds: None,
        resolution: Some("Web Stream".to_string()),
        ext: Some(ext),
        is_playlist: false,
        playlist_count: 0,
        playlist_entries: Vec::new(),
        has_subtitles: false,
        subtitles_summary: String::new(),
    })
}

/// Helper to extract <title>...</title> or OpenGraph og:title from HTML
fn extract_html_title(html: &str) -> Option<String> {
    // Check og:title
    if let Some(idx) = html.find("property=\"og:title\"") {
        let snippet = &html[idx.saturating_sub(60)..idx.min(html.len()) + 120];
        if let Some(content_idx) = snippet.find("content=\"") {
            let rest = &snippet[content_idx + 9..];
            if let Some(end) = rest.find('"') {
                return Some(html_escape_clean(&rest[..end]));
            }
        }
    }

    // Check <title>
    if let Some(start) = html.find("<title>") {
        let rest = &html[start + 7..];
        if let Some(end) = rest.find("</title>") {
            return Some(html_escape_clean(rest[..end].trim()));
        }
    }

    None
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

/// Downloads a streaming URL via yt-dlp, streaming real-time progress events
pub async fn download_stream<F>(
    url: &str,
    destination_path: &Path,
    is_audio_only: bool,
    quality: Option<&str>,
    download_subtitles: bool,
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
            if ["mp4", "webm", "mkv", "avi", "mov", "m4a", "mp3", "part"]
                .contains(&ext.to_lowercase().as_str()) =>
        {
            stem
        }
        _ => raw_name,
    };

    let output_template = parent.join(format!("{}.%(ext)s", filename_stem));

    let mut cmd = create_ytdlp_cmd(&ytdlp_bin);
    cmd.arg("--newline")
        .arg("--progress-template")
        .arg("download:RAW:%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(progress.speed)s|%(progress.eta)s");

    if is_audio_only {
        cmd.arg("-x").arg("--audio-format").arg("mp3");
    } else {
        if download_subtitles {
            cmd.arg("--write-subs")
                .arg("--write-auto-subs")
                .arg("--sub-langs")
                .arg("all,-live_chat")
                .arg("--embed-subs");
        }
        if let Some(q) = quality {
            match q {
                "1080p" => {
                    cmd.arg("-f").arg("bestvideo[height<=1080]+bestaudio/best[height<=1080]/best");
                }
                "720p" => {
                    cmd.arg("-f").arg("bestvideo[height<=720]+bestaudio/best[height<=720]/best");
                }
                "480p" => {
                    cmd.arg("-f").arg("bestvideo[height<=480]+bestaudio/best[height<=480]/best");
                }
                _ => {}
            }
        }
    }

    if let Some(ffmpeg) = find_ffmpeg_path() {
        if let Some(ffmpeg_dir) = ffmpeg.parent() {
            cmd.arg("--ffmpeg-location").arg(ffmpeg_dir);
        }
    } else if bin_dir.join("ffmpeg").exists() {
        cmd.arg("--ffmpeg-location").arg(&bin_dir);
    }

    cmd.arg("-o")
        .arg(output_template.to_string_lossy().to_string())
        .arg(url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    info!("Spawning extractor stream download: {:?}", cmd);
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().ok_or_else(|| {
        AppError::Generic("Failed to capture stdout of extractor process".to_string())
    })?;

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
                        if let Some(raw_idx) = line.find("RAW:") {
                            let raw_part = &line[raw_idx + 4..];
                            let parts: Vec<&str> = raw_part.split('|').collect();
                            if parts.len() >= 5 {
                                let downloaded = parts[0].trim().parse::<u64>().unwrap_or(0);
                                let total_bytes = parts[1].trim().parse::<u64>().ok()
                                    .or_else(|| parts[2].trim().parse::<u64>().ok());
                                let speed = parts[3].trim().parse::<f64>().unwrap_or(0.0);
                                let eta = parts[4].trim().parse::<u64>().ok();

                                let progress_ratio = match total_bytes {
                                    Some(total) if total > 0 => {
                                        (downloaded as f32 / total as f32).clamp(0.0, 1.0)
                                    }
                                    _ => 0.0,
                                };

                                on_progress(DownloadProgress {
                                    downloaded_bytes: downloaded,
                                    total_bytes,
                                    speed_bytes_sec: speed,
                                    eta_seconds: eta,
                                    progress_ratio,
                                });
                            }
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
    if !status.success() {
        return Err(AppError::Generic(format!(
            "Extractor process finished with exit code {:?}",
            status.code()
        )));
    }

    // Determine the actual downloaded file on disk
    let candidate_extensions = ["mp3", "mp4", "webm", "mkv", "m4a", "opus"];
    for ext in &candidate_extensions {
        let p = parent.join(format!("{}.{}", filename_stem, ext));
        if p.exists() {
            info!("Extractor completed output resolved to: {:?}", p);
            return Ok(p);
        }
    }

    Ok(destination_path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

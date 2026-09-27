use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
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

/// Inspects a video or album/playlist streaming URL to fetch metadata with optional browser cookies and proxy
pub async fn inspect_video_with_options(
    url: &str,
    cookies_browser: Option<&str>,
    proxy: Option<&str>,
) -> Result<VideoMetadata> {
    let ytdlp_bin = ensure_ytdlp_installed().await?;

    info!(
        "Inspecting streaming URL with yt-dlp: {} (cookies: {:?}, proxy: {:?})",
        url, cookies_browser, proxy
    );
    let mut cmd = create_ytdlp_cmd(&ytdlp_bin);
    if let Some(b) = cookies_browser {
        if !b.is_empty() {
            cmd.arg("--cookies-from-browser").arg(b);
        }
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
        thumbnail_url,
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
    info!("Scraping webpage HTML for media sources: {} (proxy: {:?})", page_url, proxy);
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

    let client = builder.build().unwrap_or_else(|_| reqwest::Client::new());
    let resp = client.get(page_url).send().await?;
    if !resp.status().is_success() {
        return Err(AppError::Generic(format!("HTTP {}", resp.status())));
    }

    let html = resp.text().await?;

    let page_title = extract_html_title(&html);
    let page_thumbnail = extract_html_thumbnail(&html, page_url);

    // 1. Check MacCMS player_aaaa configuration (used by donghuafun, animevietsub, etc.)
    if let Some(maccms) = extract_maccms_player(&html) {
        info!("Found MacCMS embedded player: {:?}", maccms.video_url);
        if is_streaming_platform(&maccms.video_url) {
            info!("Inspecting extracted streaming platform source: {}", maccms.video_url);
            if let Ok(mut meta) = Box::pin(inspect_video(&maccms.video_url)).await {
                if let Some(t) = maccms.title.or(page_title.clone()) {
                    meta.title = t;
                }
                if meta.thumbnail_url.is_none() {
                    meta.thumbnail_url = page_thumbnail.clone();
                }
                return Ok(meta);
            }
        }
        if maccms.video_url.contains(".m3u8") || maccms.video_url.contains(".mp4") || maccms.video_url.contains(".webm") {
            let title = maccms.title.or(page_title.clone()).unwrap_or_else(|| "web_video".to_string());
            let ext = if maccms.video_url.contains(".m3u8") { "mp4".to_string() } else { "mp4".to_string() };
            return Ok(VideoMetadata {
                url: maccms.video_url,
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
                thumbnail_url: page_thumbnail.clone(),
            });
        }
    }

    // 2. Check embedded iframes for known streaming platforms
    for iframe_url in extract_iframes_from_html(&html, page_url) {
        if is_streaming_platform(&iframe_url) {
            info!("Inspecting embedded iframe streaming source: {}", iframe_url);
            if let Ok(mut meta) = Box::pin(inspect_video(&iframe_url)).await {
                if let Some(t) = page_title.clone() {
                    meta.title = t;
                }
                if meta.thumbnail_url.is_none() {
                    meta.thumbnail_url = page_thumbnail.clone();
                }
                return Ok(meta);
            }
        }
    }

    // 3. Check for series/playlist collection (e.g. MacCMS vod/detail pages or episode anthology lists)
    if let Some(playlist_meta) = extract_playlist_from_detail_html(&html, page_url) {
        info!("Successfully extracted series collection from webpage: '{}' ({} episodes)", playlist_meta.title, playlist_meta.playlist_count);
        return Ok(playlist_meta);
    }

    // 4. Check for direct <source src>, <video src>, OpenGraph video, or .m3u8/.mp4
    let title = page_title
        .or_else(|| {
            page_url
                .split('/')
                .last()
                .and_then(|s| s.split('?').next())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "web_video".to_string());

    let stream_url = extract_stream_from_html(&html, page_url)
        .ok_or_else(|| AppError::Generic("No direct video, series playlist, or m3u8 stream found in webpage HTML".to_string()))?;

    // If stream_url itself is a streaming platform link
    if is_streaming_platform(&stream_url) {
        if let Ok(mut meta) = Box::pin(inspect_video(&stream_url)).await {
            meta.title = title;
            if meta.thumbnail_url.is_none() {
                meta.thumbnail_url = page_thumbnail.clone();
            }
            return Ok(meta);
        }
    }

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
        thumbnail_url: page_thumbnail,
    })
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
/// resolves the underlying stream or platform URL (such as Dailymotion, YouTube, Vimeo, or .m3u8).
pub async fn resolve_playable_stream_url(url: &str, proxy: Option<&str>) -> String {
    if is_streaming_platform(url) || url.contains(".m3u8") || url.contains(".mp4") || url.contains(".webm") {
        return url.to_string();
    }

    let mut builder = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36")
        .timeout(std::time::Duration::from_secs(12));

    if let Some(prx_str) = proxy {
        let trimmed = prx_str.trim();
        if !trimmed.is_empty() {
            if let Ok(prx) = reqwest::Proxy::all(trimmed) {
                builder = builder.proxy(prx);
            }
        }
    }

    let client = builder.build().unwrap_or_else(|_| reqwest::Client::new());
    if let Ok(resp) = client.get(url).send().await {
        if resp.status().is_success() {
            if let Ok(html) = resp.text().await {
                // 1. Check MacCMS player_aaaa configuration (used by donghuafun, animevietsub, etc.)
                if let Some(maccms) = extract_maccms_player(&html) {
                    info!("Resolved MacCMS playable stream URL: {} -> {}", url, maccms.video_url);
                    return maccms.video_url;
                }
                // 2. Check embedded iframes
                for iframe_url in extract_iframes_from_html(&html, url) {
                    if is_streaming_platform(&iframe_url) || iframe_url.contains(".m3u8") || iframe_url.contains(".mp4") {
                        info!("Resolved iframe playable stream URL: {} -> {}", url, iframe_url);
                        return iframe_url;
                    }
                }
                // 3. Check direct HTML stream
                if let Some(stream_url) = extract_stream_from_html(&html, url) {
                    info!("Resolved direct HTML stream URL: {} -> {}", url, stream_url);
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
    audio_format: Option<&str>,
    audio_bitrate: Option<&str>,
    embed_artwork: bool,
    speed_limit: Option<&str>,
    cookies_browser: Option<&str>,
    proxy: Option<&str>,
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
    cmd.arg("--newline")
        .arg("--progress-template")
        .arg("download:RAW:%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(progress.speed)s|%(progress.eta)s|%(progress._percent)s|%(info.ext)s|%(progress.filename)s");

    if let Some(limit) = speed_limit {
        if !limit.is_empty() && limit != "unlimited" {
            cmd.arg("--limit-rate").arg(limit);
        }
    }

    if let Some(browser) = cookies_browser {
        if !browser.is_empty() {
            info!("Applying browser cookies from {} to yt-dlp download", browser);
            cmd.arg("--cookies-from-browser").arg(browser);
        }
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

    if !status.success() {
        let err_detail = if !stderr_output.trim().is_empty() {
            clean_extractor_error(&stderr_output)
        } else {
            format!("Extractor process finished with exit code {:?}", status.code())
        };
        return Err(AppError::Generic(err_detail));
    }

    // Determine the actual downloaded file on disk
    let candidate_extensions = ["mp3", "mp4", "webm", "mkv", "m4a", "opus"];
    for ext in &candidate_extensions {
        let p = parent.join(format!("{}.{}", filename_stem, ext));
        if p.exists() {
            info!("Extractor completed output resolved to: {:?}", p);
            let _ = crate::filesystem::organize_subtitles(&p).await;
            return Ok(p);
        }
    }

    let _ = crate::filesystem::organize_subtitles(destination_path).await;
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
        assert!(cleaned.contains("iQIYI stream is DRM-protected"));

        let drm_err = "ERROR: This video contains DRM protection (Widevine)";
        assert!(clean_extractor_error(drm_err).contains("DRM-protected"));

        let geo_err = "ERROR: This video is not available in your country";
        assert!(clean_extractor_error(geo_err).to_lowercase().contains("geo-restricted"));

        let exit_err = "Application error: Extractor process finished with exit code Some(1)";
        assert!(clean_extractor_error(exit_err).contains("Stream extraction failed"));
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
    if lower.contains("iq.com") || lower.contains("iqiyi") {
        return "iQIYI stream is DRM-protected or requires VIP login. DRM-encrypted content cannot be downloaded.".to_string();
    }
    if lower.contains("phantomjs") {
        return "Stream requires an external JavaScript execution engine or is DRM-encrypted.".to_string();
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



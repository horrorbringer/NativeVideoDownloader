mod database;
mod downloader;
mod error;
mod filesystem;
mod logger;
mod models;
mod network;
pub mod notifications;
pub mod theme;

use theme::{ThemeMode, is_system_dark_mode, resolve_is_dark};

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use slint::{ComponentHandle, LogicalSize, Model, ModelRc, VecModel, WindowSize};
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use uuid::Uuid;

use database::Database;
use downloader::DownloadManager;
use logger::UiLogLayer;
use models::{DownloadProgress, DownloadStatus, VideoMetadata};
use network::NetworkClient;

slint::include_modules!();

#[derive(Default, Clone)]
struct HistoryState {
    status_filter: i32,  // 0 = All, 1 = Completed, 2 = Failed
    domain_filter: i32,  // 0 = All, 1 = YouTube, 2 = KissKH, 3 = TikTok, 4 = Douyin, 5 = Facebook, 6 = Other
    search_query: String,
    selected_ids: HashSet<Uuid>,
}

fn extract_domain_from_url(url: &str) -> String {
    if let Ok(parsed) = reqwest::Url::parse(url) {
        if let Some(host) = parsed.host_str() {
            let clean = host.trim_start_matches("www.").to_lowercase();
            if clean == "youtu.be" || clean.ends_with(".youtube.com") || clean == "youtube.com" {
                return "youtube.com".to_string();
            }
            return clean;
        }
    }
    let s = url.trim().to_lowercase();
    if s.contains("youtube.com") || s.contains("youtu.be") {
        "youtube.com".to_string()
    } else if s.contains("kisskh") {
        "kisskh.do".to_string()
    } else if s.contains("tiktok.com") {
        "tiktok.com".to_string()
    } else if s.contains("douyin.com") {
        "douyin.com".to_string()
    } else if s.contains("facebook.com") || s.contains("fb.watch") {
        "facebook.com".to_string()
    } else {
        "web".to_string()
    }
}

fn resolve_batch_item_title_and_referer(url: &str) -> (String, Option<String>) {
    if url.contains("kisskh.do") || url.contains("kisskh.") {
        let parts: Vec<&str> = url.split('/').collect();
        let drama_name = parts
            .iter()
            .position(|&p| p == "Drama")
            .and_then(|idx| parts.get(idx + 1))
            .map(|s| s.replace("--", " ").replace('-', " "))
            .unwrap_or_else(|| "KissKH Drama".to_string());

        let ep_part = parts
            .iter()
            .find(|p| p.starts_with("Episode-"))
            .map(|s| s.split('?').next().unwrap_or(s).replace('-', " "))
            .unwrap_or_default();

        let title = if !ep_part.is_empty() {
            format!("{} - {}", drama_name.trim(), ep_part.trim())
        } else {
            drama_name.trim().to_string()
        };

        (title, Some("https://kisskh.do/".to_string()))
    } else if url.contains("douyin.com") || url.contains("iesdouyin.com") {
        let id = url.split('/').last().unwrap_or("video").split('?').next().unwrap_or("video");
        (format!("Douyin Video {}", id), Some("https://www.douyin.com/".to_string()))
    } else if url.contains("tiktok.com") {
        let id = url.split("/video/").nth(1).unwrap_or(url.split('/').last().unwrap_or("video")).split('?').next().unwrap_or("video");
        (format!("TikTok Video {}", id), Some("https://www.tiktok.com/".to_string()))
    } else if url.contains("youtube.com") || url.contains("youtu.be") {
        let id = if url.contains("youtu.be/") {
            url.split("youtu.be/").nth(1).unwrap_or("video").split('?').next().unwrap_or("video")
        } else if url.contains("v=") {
            url.split("v=").nth(1).unwrap_or("video").split('&').next().unwrap_or("video")
        } else {
            "video"
        };
        (format!("YouTube Video {}", id), None)
    } else if url.contains("bilibili.com") {
        let bvid = url.split("/video/").nth(1).unwrap_or("video").split('?').next().unwrap_or("video");
        (format!("Bilibili {}", bvid), Some("https://www.bilibili.com/".to_string()))
    } else if url.contains("facebook.com") || url.contains("fb.watch") {
        let id = url.split('/').last().unwrap_or("video").split('?').next().unwrap_or("video");
        (format!("Facebook Video {}", id), Some("https://www.facebook.com/".to_string()))
    } else {
        let default_title = url
            .split('/')
            .last()
            .and_then(|s| s.split('?').next())
            .filter(|s| !s.is_empty() && *s != "watch" && *s != "video")
            .unwrap_or("batch_media")
            .to_string();
        (default_title, None)
    }
}

/// Helper: load history from database, apply active filters, compute stats, and push it into the Slint UI
async fn refresh_history(
    db: &Database,
    state: &Arc<tokio::sync::RwLock<HistoryState>>,
    weak: slint::Weak<AppWindow>,
) {
    let (status_filter, domain_filter, search_query, selected_ids) = {
        let guard = state.read().await;
        (
            guard.status_filter,
            guard.domain_filter,
            guard.search_query.clone(),
            guard.selected_ids.clone(),
        )
    };

    match db.get_history(None).await {
        Ok(records) => {
            let total_count = records.len() as i32;
            let total_completed = records.iter().filter(|r| r.status == "Completed").count() as i32;
            let total_failed = records
                .iter()
                .filter(|r| r.status == "Failed" || r.status == "Cancelled")
                .count() as i32;
            let total_bytes: u64 = records
                .iter()
                .filter(|r| r.status == "Completed")
                .map(|r| r.downloaded_size.max(r.total_size.unwrap_or(0)))
                .sum();
            let total_bytes_text = DownloadProgress::format_size(total_bytes);
            let success_rate_text = if total_count > 0 {
                format!("{}%", (total_completed * 100) / total_count)
            } else {
                "100%".to_string()
            };

            let filtered_records: Vec<&database::HistoryRecord> = records
                .iter()
                .filter(|r| {
                    // Status filter
                    let status_ok = match status_filter {
                        1 => r.status == "Completed",
                        2 => r.status == "Failed" || r.status == "Cancelled",
                        _ => true,
                    };
                    if !status_ok {
                        return false;
                    }

                    // Domain filter
                    let domain = extract_domain_from_url(&r.url);
                    let domain_ok = match domain_filter {
                        1 => domain.contains("youtube") || domain.contains("youtu.be"),
                        2 => domain.contains("kisskh"),
                        3 => domain.contains("tiktok"),
                        4 => domain.contains("douyin"),
                        5 => domain.contains("facebook") || domain.contains("fb.watch"),
                        6 => {
                            !domain.contains("youtube")
                                && !domain.contains("youtu.be")
                                && !domain.contains("kisskh")
                                && !domain.contains("tiktok")
                                && !domain.contains("douyin")
                                && !domain.contains("facebook")
                                && !domain.contains("fb.watch")
                        }
                        _ => true,
                    };
                    if !domain_ok {
                        return false;
                    }

                    // Search query
                    if !search_query.is_empty() {
                        let q = search_query.to_lowercase();
                        let in_title = r.title.to_lowercase().contains(&q);
                        let in_file = r.filename.to_lowercase().contains(&q);
                        let in_path = r.output_path.to_lowercase().contains(&q);
                        let in_url = r.url.to_lowercase().contains(&q);
                        if !in_title && !in_file && !in_path && !in_url {
                            return false;
                        }
                    }

                    true
                })
                .collect();

            let items: Vec<HistoryItemData> = filtered_records
                .into_iter()
                .map(|r| {
                    let size_text = r
                        .total_size
                        .map(DownloadProgress::format_size)
                        .unwrap_or_else(|| DownloadProgress::format_size(r.downloaded_size));

                    let raw_date = r.completed_at.clone().unwrap_or_else(|| r.created_at.clone());
                    let date_display = database::format_history_date(&raw_date);
                    let domain = extract_domain_from_url(&r.url);
                    let is_selected = selected_ids.contains(&r.id);

                    HistoryItemData {
                        id: r.id.to_string().into(),
                        title: r.title.clone().into(),
                        status: r.status.clone().into(),
                        size_text: size_text.into(),
                        date_text: date_display.into(),
                        url: r.url.clone().into(),
                        output_path: r.output_path.clone().into(),
                        domain: domain.into(),
                        selected: is_selected,
                    }
                })
                .collect();

            let selected_count = selected_ids.len() as i32;

            let _ = weak.upgrade_in_event_loop(move |window| {
                let model = Rc::new(VecModel::from(items));
                window.set_history_items(ModelRc::from(model));
                window.set_history_total_count(total_count);
                window.set_history_completed_count(total_completed);
                window.set_history_failed_count(total_failed);
                window.set_history_total_bytes_text(total_bytes_text.into());
                window.set_history_success_rate_text(success_rate_text.into());
                window.set_history_selected_count(selected_count);
            });
        }
        Err(err) => {
            warn!("Failed to load history: {}", err);
        }
    }
}

/// Helper: Execute URL analysis and stream metadata / progress into Slint UI
fn run_url_analysis(
    url_str: String,
    window_weak: slint::Weak<AppWindow>,
    meta_clone: Arc<Mutex<Option<VideoMetadata>>>,
    client_clone: Arc<NetworkClient>,
    cookies_browser_clone: Arc<tokio::sync::RwLock<Option<String>>>,
    proxy_clone: Arc<tokio::sync::RwLock<Option<String>>>,
) {
    let url_str = downloader::normalize_media_url(url_str.trim());
    info!("Received URL analysis request: {}", url_str);

    if url_str.is_empty() {
        if let Some(window) = window_weak.upgrade() {
            window.set_status_message("Please enter a valid URL".into());
        }
        return;
    }

    if !url_str.starts_with("http://") && !url_str.starts_with("https://") {
        if let Some(window) = window_weak.upgrade() {
            window.set_status_message("Invalid URL: must start with http:// or https://".into());
        }
        return;
    }

    if let Some(window) = window_weak.upgrade() {
        window.set_input_url_text(url_str.clone().into());
        window.set_is_analyzing(true);
        window.set_has_thumbnail(false);
        window.set_thumbnail_image(slint::Image::default());
        window.set_has_metadata(false);
        window.set_has_error(false);
        window.set_error_message("".into());
        window.set_is_playlist(false);
        window.set_playlist_count(0);
        window.set_video_size_text("".into());
        window.set_video_fps_text("".into());
        window.set_video_codecs_text("".into());
        window.set_format_best_label("Best Quality".into());
        window.set_format_1080_label("1080p FHD".into());
        window.set_format_720_label("720p HD".into());
        window.set_format_480_label("480p SD".into());
        window.set_format_audio_label("Extract Audio".into());
        window.set_status_message(format!("Inspecting media at {}...", url_str).into());
    }

    let weak_for_async = window_weak.clone();
    let meta_for_async = meta_clone.clone();
    let client = client_clone.clone();

    tokio::spawn(async move {
        let cookies_browser = cookies_browser_clone.read().await.clone();
        let proxy = proxy_clone.read().await.clone();
        let inspect_result = if downloader::is_streaming_platform(&url_str) {
            let _ = weak_for_async.upgrade_in_event_loop(|w| {
                w.set_status_message("Analyzing streaming platform media (yt-dlp)...".into());
            });
            downloader::inspect_video_with_options(&url_str, cookies_browser.as_deref(), proxy.as_deref()).await
        } else {
            match client.inspect_url(&url_str).await {
                Ok(meta) if downloader::is_valid_direct_media(&meta) => Ok(meta),
                Ok(meta) => {
                    info!(
                        "Direct inspect returned non-media response (type: {:?}, length: {:?}) for {}, falling back to extractor",
                        meta.content_type, meta.content_length, url_str
                    );
                    let _ = weak_for_async.upgrade_in_event_loop(|w| {
                        w.set_status_message(
                            "Web source detected. Extracting media stream...".into(),
                        );
                    });
                    downloader::inspect_video_with_options(&url_str, cookies_browser.as_deref(), proxy.as_deref()).await
                }
                Err(err) => {
                    // Try extractor as fallback
                    match downloader::inspect_video_with_options(&url_str, cookies_browser.as_deref(), proxy.as_deref()).await {
                        Ok(extracted) => Ok(extracted),
                        Err(_) => Err(err),
                    }
                }
            }
        };

        match inspect_result {
            Ok(metadata) => {
                info!("Successfully inspected URL: {:?}", metadata);
                let is_playlist = metadata.is_playlist;
                let playlist_count = metadata.playlist_count as i32;
                let title = if is_playlist {
                    format!("{} ({} Episodes)", metadata.title, playlist_count)
                } else {
                    metadata.title.clone()
                };

                let size_str = metadata
                    .content_length
                    .map(DownloadProgress::format_size)
                    .unwrap_or_else(|| {
                        if is_playlist {
                            format!("{} episodes series", playlist_count)
                        } else {
                            "Dynamic size".to_string()
                        }
                    });

                let res_str = metadata
                    .resolution
                    .clone()
                    .or_else(|| metadata.content_type.clone())
                    .unwrap_or_else(|| "Video Stream".to_string());

                let duration_str = metadata
                    .duration_seconds
                    .map(downloader::format_duration)
                    .unwrap_or_default();

                let details_str = if is_playlist {
                    format!("Series Album  •  {} Episodes", playlist_count)
                } else if !duration_str.is_empty() {
                    format!("{}  •  Duration: {}", size_str, duration_str)
                } else if metadata.supports_ranges {
                    format!("{}  •  Resumable", size_str)
                } else {
                    format!("{}  •  Single-stream", size_str)
                };

                let format_display = if is_playlist {
                    format!("Album  •  {} Episodes", playlist_count)
                } else if metadata.is_extractor {
                    format!("Stream  •  {}", res_str)
                } else {
                    format!("Format: {}", res_str)
                };

                let has_subs = metadata.has_subtitles || is_playlist;
                let subs_summary = if !metadata.subtitles_summary.is_empty() {
                    metadata.subtitles_summary.clone()
                } else if is_playlist {
                    "Multi-language (per-episode subtitles)".to_string()
                } else {
                    String::new()
                };

                let thumb_url = metadata.thumbnail_url.clone();
                let thumb_path = if let Some(ref t_url) = thumb_url {
                    download_thumbnail_to_cache(t_url).await
                } else {
                    None
                };

                let episodes: Vec<EpisodeItemData> = if is_playlist {
                    metadata
                        .playlist_entries
                        .iter()
                        .enumerate()
                        .map(|(idx, entry)| EpisodeItemData {
                            index: idx as i32,
                            title: entry.title.clone().into(),
                            url: entry.url.clone().into(),
                            selected: true,
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                let selected_count = episodes.len() as i32;

                let video_size_text = metadata
                    .content_length
                    .or(metadata.size_best)
                    .map(|bytes| DownloadProgress::format_size(bytes))
                    .unwrap_or_default();

                let video_fps_text = metadata
                    .fps
                    .map(|f| format!("{:.0} FPS", f))
                    .unwrap_or_default();

                let video_codecs_text = match (&metadata.vcodec, &metadata.acodec) {
                    (Some(v), Some(a)) => format!("{} / {}", v, a),
                    (Some(v), None) => v.clone(),
                    (None, Some(a)) => a.clone(),
                    (None, None) => String::new(),
                };

                let format_best_label = match metadata.size_best.or(metadata.content_length) {
                    Some(s) => format!("Best Quality • ~{}", DownloadProgress::format_size(s)),
                    None => "Best Quality".to_string(),
                };

                let format_1080_label = match metadata.size_1080p {
                    Some(s) => format!("1080p FHD • ~{}", DownloadProgress::format_size(s)),
                    None => "1080p FHD".to_string(),
                };

                let format_720_label = match metadata.size_720p {
                    Some(s) => format!("720p HD • ~{}", DownloadProgress::format_size(s)),
                    None => "720p HD".to_string(),
                };

                let format_480_label = match metadata.size_480p {
                    Some(s) => format!("480p SD • ~{}", DownloadProgress::format_size(s)),
                    None => "480p SD".to_string(),
                };

                let format_audio_label = match metadata.size_audio {
                    Some(s) => format!("Extract Audio • ~{}", DownloadProgress::format_size(s)),
                    None => "Extract Audio".to_string(),
                };

                *meta_for_async.lock().await = Some(metadata);

                let _ = weak_for_async.upgrade_in_event_loop(move |window| {
                    window.set_is_analyzing(false);
                    window.set_has_metadata(true);
                    let ep_model = Rc::new(VecModel::from(episodes));
                    window.set_playlist_episodes(ModelRc::from(ep_model));
                    window.set_selected_episodes_count(selected_count);
                    window.set_episode_range_input(if selected_count > 0 {
                        format!("1-{}", selected_count).into()
                    } else {
                        "".into()
                    });

                    if let Some(ref p) = thumb_path {
                        if let Ok(img) = slint::Image::load_from_path(p) {
                            window.set_has_thumbnail(true);
                            window.set_thumbnail_image(img);
                        } else {
                            window.set_has_thumbnail(false);
                            window.set_thumbnail_image(slint::Image::default());
                        }
                        let _ = std::fs::remove_file(p);
                    } else {
                        window.set_has_thumbnail(false);
                        window.set_thumbnail_image(slint::Image::default());
                    }
                    window.set_has_error(false);
                    window.set_error_message("".into());
                    window.set_is_playlist(is_playlist);
                    window.set_playlist_count(playlist_count);
                    window.set_has_subtitles(has_subs);
                    window.set_subtitles_summary(subs_summary.into());
                    if has_subs {
                        window.set_download_subtitles(true);
                    }
                    window.set_video_title(title.into());
                    window.set_video_resolution(format_display.into());
                    window.set_video_duration(details_str.into());
                    window.set_video_size_text(video_size_text.into());
                    window.set_video_fps_text(video_fps_text.into());
                    window.set_video_codecs_text(video_codecs_text.into());
                    window.set_format_best_label(format_best_label.into());
                    window.set_format_1080_label(format_1080_label.into());
                    window.set_format_720_label(format_720_label.into());
                    window.set_format_480_label(format_480_label.into());
                    window.set_format_audio_label(format_audio_label.into());
                    window.set_status_message(
                        if is_playlist {
                            format!("Series analyzed: {} episodes found. Ready to download.", playlist_count).into()
                        } else {
                            "Media analyzed successfully. Ready to download.".into()
                        },
                    );
                });
            }
            Err(err) => {
                warn!("Failed to inspect URL {}: {}", url_str, err);
                let clean_msg = crate::error::clean_user_error(&err.to_string());
                let _ = weak_for_async.upgrade_in_event_loop(move |window| {
                    window.set_is_analyzing(false);
                    window.set_has_metadata(false);
                    window.set_playlist_episodes(ModelRc::default());
                    window.set_selected_episodes_count(0);
                    window.set_episode_range_input("".into());
                    window.set_has_thumbnail(false);
                    window.set_thumbnail_image(slint::Image::default());
                    window.set_video_size_text("".into());
                    window.set_video_fps_text("".into());
                    window.set_video_codecs_text("".into());
                    window.set_format_best_label("Best Quality".into());
                    window.set_format_1080_label("1080p FHD".into());
                    window.set_format_720_label("720p HD".into());
                    window.set_format_480_label("480p SD".into());
                    window.set_format_audio_label("Extract Audio".into());
                    window.set_has_error(true);
                    window.set_error_message(clean_msg.into());
                    window.set_status_message("Unable to analyze URL. See details above.".into());
                });
            }
        }
    });
}

/// Downloads a remote thumbnail image to a temporary file for rendering in the UI thread.
/// Automatically handles and converts AVIF, WebP, and HEIC images to standard PNG format.
async fn download_thumbnail_to_cache(url: &str) -> Option<std::path::PathBuf> {
    info!("Fetching thumbnail image preview from: {}", url);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36")
        .build()
        .ok()?;
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        warn!("Thumbnail HTTP fetch returned status: {}", resp.status());
        return None;
    }
    let bytes = resp.bytes().await.ok()?;

    let is_webp = bytes.starts_with(b"RIFF") && bytes.len() > 12 && &bytes[8..12] == b"WEBP";
    let is_avif = (bytes.len() > 12 && (&bytes[4..12] == b"ftypavif" || &bytes[4..12] == b"ftypavis"))
        || url.to_lowercase().contains(".avif");
    let is_heic = bytes.len() > 12 && (&bytes[4..12] == b"ftypheic" || &bytes[4..12] == b"ftypmif1");

    if is_webp || is_avif || is_heic {
        let raw_ext = if is_avif { "avif" } else if is_heic { "heic" } else { "webp" };
        let raw_file = std::env::temp_dir().join(format!("nvd_raw_{}.{}", uuid::Uuid::new_v4(), raw_ext));
        let png_file = std::env::temp_dir().join(format!("nvd_thumb_{}.png", uuid::Uuid::new_v4()));
        if tokio::fs::write(&raw_file, &bytes).await.is_ok() {
            #[cfg(target_os = "macos")]
            let status = tokio::process::Command::new("sips")
                .args(["-s", "format", "png"])
                .arg(&raw_file)
                .args(["--out"])
                .arg(&png_file)
                .output()
                .await;

            #[cfg(not(target_os = "macos"))]
            let status = {
                let ffmpeg_bin = crate::downloader::extractor::get_bin_dir().join("ffmpeg");
                tokio::process::Command::new(ffmpeg_bin)
                    .args(["-y", "-i"])
                    .arg(&raw_file)
                    .arg(&png_file)
                    .output()
                    .await
            };

            let _ = tokio::fs::remove_file(&raw_file).await;
            if status.map(|s| s.status.success()).unwrap_or(false) && png_file.is_file() {
                info!("Successfully converted {} thumbnail to PNG: {:?}", raw_ext, png_file);
                return Some(png_file);
            }
        }
    }

    let is_png = bytes.starts_with(b"\x89PNG") || url.to_lowercase().contains(".png");
    let ext = if is_png { "png" } else { "jpg" };
    let temp_file = std::env::temp_dir().join(format!("nvd_thumb_{}.{}", uuid::Uuid::new_v4(), ext));
    if tokio::fs::write(&temp_file, &bytes).await.is_err() {
        return None;
    }
    Some(temp_file)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize structured logging with real-time UI streaming
    let ui_log_layer = UiLogLayer::new();
    logger::init_subscribers(ui_log_layer.clone());

    info!("Starting Native Video Downloader v0.1.0 (Rust + Slint)");

    // Initialize SQLite database
    let db_path = Database::default_db_path();
    let db = Arc::new(Database::init(&db_path).await?);

    // Initialize Slint UI window with increased width
    let main_window = AppWindow::new()?;
    main_window.window().set_size(WindowSize::Logical(LogicalSize::new(1180.0, 760.0)));
    ui_log_layer.set_window(main_window.as_weak());

    // Shared state between UI callbacks and background tasks
    let current_metadata: Arc<Mutex<Option<VideoMetadata>>> = Arc::new(Mutex::new(None));
    let network_client = Arc::new(NetworkClient::new());

    // Restore max concurrency preference
    let initial_concurrency = if let Ok(Some(saved)) = db.get_setting("max_concurrency").await {
        saved.parse::<usize>().unwrap_or(3).clamp(1, 10)
    } else {
        3
    };
    main_window.set_selected_concurrency(initial_concurrency as i32);
    let download_manager = Arc::new(DownloadManager::new(initial_concurrency, db.clone()));

    // Download directory management with persistence
    let initial_download_dir = if let Ok(Some(saved)) = db.get_setting("download_dir").await {
        let p = PathBuf::from(saved);
        if p.exists() {
            p
        } else {
            filesystem::default_download_dir()
        }
    } else {
        filesystem::default_download_dir()
    };

    main_window.set_download_dir_path(initial_download_dir.to_string_lossy().to_string().into());
    let current_download_dir = Arc::new(tokio::sync::RwLock::new(initial_download_dir));

    // Restore speed limit preference
    let initial_speed_limit = if let Ok(Some(saved)) = db.get_setting("speed_limit").await {
        saved
    } else {
        String::new()
    };
    let initial_limit_idx = match initial_speed_limit.as_str() {
        "2M" => 1,
        "5M" => 2,
        "10M" => 3,
        "20M" => 4,
        _ => 0,
    };
    main_window.set_selected_speed_limit_index(initial_limit_idx);
    let manager_limit = match initial_limit_idx {
        1 => Some("2M".to_string()),
        2 => Some("5M".to_string()),
        3 => Some("10M".to_string()),
        4 => Some("20M".to_string()),
        _ => None,
    };
    download_manager.set_speed_limit(manager_limit).await;

    // Restore browser cookies authentication preference
    let initial_cookies_browser = if let Ok(Some(saved)) = db.get_setting("cookies_browser").await {
        saved
    } else {
        String::new()
    };
    let initial_cookies_idx = match initial_cookies_browser.as_str() {
        "chrome" => 1,
        "firefox" => 2,
        "safari" => 3,
        "brave" => 4,
        "edge" => 5,
        _ => 0,
    };
    main_window.set_selected_cookies_browser_index(initial_cookies_idx);
    let manager_cookies = match initial_cookies_idx {
        1 => Some("chrome".to_string()),
        2 => Some("firefox".to_string()),
        3 => Some("safari".to_string()),
        4 => Some("brave".to_string()),
        5 => Some("edge".to_string()),
        _ => None,
    };
    download_manager.set_cookies_browser(manager_cookies.clone()).await;
    let current_cookies_browser = Arc::new(tokio::sync::RwLock::new(manager_cookies));

    // Restore saved proxy settings
    let saved_proxy_enabled = db.get_setting("proxy_enabled").await.unwrap_or(None).map(|v| v == "true").unwrap_or(false);
    let saved_proxy_url = db.get_setting("proxy_url").await.unwrap_or(None).unwrap_or_default();
    let initial_proxy = if saved_proxy_enabled && !saved_proxy_url.is_empty() {
        Some(saved_proxy_url.clone())
    } else {
        None
    };
    main_window.set_proxy_enabled(saved_proxy_enabled);
    main_window.set_proxy_url_input(saved_proxy_url.clone().into());
    if let Some(ref p) = initial_proxy {
        main_window.set_proxy_status_message(format!("Active Proxy: {}", p).into());
        main_window.set_proxy_is_connected(true);
    } else {
        main_window.set_proxy_status_message("Direct connection (proxy disabled)".into());
        main_window.set_proxy_is_connected(false);
    }
    network_client.set_proxy(initial_proxy.clone());
    download_manager.set_proxy(initial_proxy.clone()).await;
    let current_proxy = Arc::new(tokio::sync::RwLock::new(initial_proxy));

    // Restore saved audio extraction defaults
    let saved_audio_fmt: i32 = db.get_setting("default_audio_format").await.unwrap_or(None).and_then(|v| v.parse().ok()).unwrap_or(0);
    let saved_audio_br: i32 = db.get_setting("default_audio_bitrate").await.unwrap_or(None).and_then(|v| v.parse().ok()).unwrap_or(0);
    let saved_embed_meta: bool = db.get_setting("default_embed_metadata").await.unwrap_or(None).map(|v| v == "true").unwrap_or(true);
    main_window.set_selected_audio_format_index(saved_audio_fmt);
    main_window.set_selected_audio_bitrate_index(saved_audio_br);
    main_window.set_embed_audio_metadata(saved_embed_meta);

    // Restore saved filename template
    let saved_template = db
        .get_setting("filename_template")
        .await
        .unwrap_or(None)
        .unwrap_or_else(|| "{title}.{ext}".to_string());
    main_window.set_filename_template(saved_template.clone().into());
    download_manager.set_filename_template(saved_template).await;

    // Restore saved asset organization mode (defaults to 0 / DedicatedVideoFolder / Approach 1)
    let saved_asset_org_mode: u8 = db
        .get_setting("asset_org_mode")
        .await
        .unwrap_or(None)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    main_window.set_selected_asset_org_mode(saved_asset_org_mode as i32);
    download_manager.set_asset_organization_mode(saved_asset_org_mode).await;

    // Restore saved file conflict & auto-resume policy (defaults to 0 / AutoResumeOrRename)
    let saved_conflict_policy_u8: u8 = db
        .get_setting("file_conflict_policy")
        .await
        .unwrap_or(None)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let conflict_policy = crate::filesystem::FileConflictPolicy::from_u8(saved_conflict_policy_u8);
    main_window.set_selected_file_conflict_policy(conflict_policy.to_u8() as i32);
    download_manager.set_file_conflict_policy(conflict_policy).await;

    // Restore saved concurrent fragments preference (default: 4)
    let saved_concurrent_fragments: u8 = db
        .get_setting("concurrent_fragments")
        .await
        .unwrap_or(None)
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    main_window.set_selected_concurrent_fragments(saved_concurrent_fragments as i32);
    download_manager.set_concurrent_fragments(saved_concurrent_fragments).await;

    // Restore saved preferred subtitle language preference (default: 0 / All Languages)
    let saved_sub_lang_str = db
        .get_setting("preferred_sub_lang")
        .await
        .unwrap_or(None)
        .unwrap_or_else(|| "0".to_string());
    let saved_sub_indices: Vec<i32> = saved_sub_lang_str
        .split(',')
        .filter_map(|s| s.trim().parse::<i32>().ok())
        .collect();
    apply_sub_langs_state(&main_window, &saved_sub_indices);

    // Restore saved theme preference (0 = Light, 1 = Dark, 2 = Auto / System Default)
    let saved_theme_mode = if let Ok(Some(mode_str)) = db.get_setting("theme_mode").await {
        mode_str.parse::<ThemeMode>().unwrap_or_default()
    } else if let Ok(Some(dark_str)) = db.get_setting("dark_mode").await {
        if dark_str == "false" {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        }
    } else {
        ThemeMode::Auto
    };
    let initial_is_dark = resolve_is_dark(saved_theme_mode);
    main_window.set_theme_mode(saved_theme_mode.to_i32());
    main_window.set_is_dark_mode(initial_is_dark);

    // Restore saved notification preferences
    let saved_notifs_enabled = db
        .get_setting("notifications_enabled")
        .await
        .unwrap_or(None)
        .map(|v| v == "true")
        .unwrap_or(true);
    let saved_sound_enabled = db
        .get_setting("notification_sound_enabled")
        .await
        .unwrap_or(None)
        .map(|v| v == "true")
        .unwrap_or(true);
    notifications::set_notifications_enabled(saved_notifs_enabled);
    notifications::set_sound_enabled(saved_sound_enabled);
    main_window.set_notifications_enabled(saved_notifs_enabled);
    main_window.set_notification_sound_enabled(saved_sound_enabled);

    // Restore scheduler and automated downloads configuration
    let saved_sched_enabled = db
        .get_setting("scheduler_enabled")
        .await
        .unwrap_or(None)
        .map(|v| v == "true")
        .unwrap_or(false);
    let saved_sched_start_h: u32 = db
        .get_setting("scheduler_start_hour")
        .await
        .unwrap_or(None)
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let saved_sched_start_m: u32 = db
        .get_setting("scheduler_start_minute")
        .await
        .unwrap_or(None)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let saved_sched_end_h: u32 = db
        .get_setting("scheduler_end_hour")
        .await
        .unwrap_or(None)
        .and_then(|v| v.parse().ok())
        .unwrap_or(7);
    let saved_sched_end_m: u32 = db
        .get_setting("scheduler_end_minute")
        .await
        .unwrap_or(None)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let saved_auto_retry_en = db
        .get_setting("auto_retry_enabled")
        .await
        .unwrap_or(None)
        .map(|v| v == "true")
        .unwrap_or(true);
    let saved_retry_interval: u64 = db
        .get_setting("auto_retry_interval_secs")
        .await
        .unwrap_or(None)
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    let saved_retry_max: u32 = db
        .get_setting("auto_retry_max_attempts")
        .await
        .unwrap_or(None)
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);

    let retry_interval_idx = match saved_retry_interval {
        30 => 0,
        60 => 1,
        120 => 2,
        300 => 3,
        _ => 1,
    };
    let retry_max_idx = match saved_retry_max {
        3 => 0,
        5 => 1,
        10 => 2,
        _ => 0,
    };

    download_manager
        .set_scheduler_config(
            saved_sched_enabled,
            saved_sched_start_h,
            saved_sched_start_m,
            saved_sched_end_h,
            saved_sched_end_m,
        )
        .await;
    download_manager
        .set_auto_retry_config(saved_auto_retry_en, saved_retry_interval, saved_retry_max)
        .await;

    main_window.set_scheduler_enabled(saved_sched_enabled);
    main_window.set_scheduler_start_hour(saved_sched_start_h as i32);
    main_window.set_scheduler_start_minute(saved_sched_start_m as i32);
    main_window.set_scheduler_end_hour(saved_sched_end_h as i32);
    main_window.set_scheduler_end_minute(saved_sched_end_m as i32);
    main_window.set_scheduler_window_status(
        format!(
            "Off-Peak Window: {:02}:{:02} – {:02}:{:02}",
            saved_sched_start_h, saved_sched_start_m, saved_sched_end_h, saved_sched_end_m
        )
        .into(),
    );
    main_window.set_auto_retry_enabled(saved_auto_retry_en);
    main_window.set_auto_retry_interval_index(retry_interval_idx);
    main_window.set_auto_retry_max_index(retry_max_idx);

    // Launch background scheduler loop (checks auto-retries and scheduled windows every second)
    download_manager.start_background_scheduler();

    // Initialize speed samples for the graph with 60 idle points (30s rolling window @ 2 Hz)
    let initial_samples: Vec<SpeedSampleData> = (0..60)
        .map(|_| SpeedSampleData {
            ratio: 0.0,
            speed_text: "0 B/s".into(),
        })
        .collect();
    main_window.set_speed_samples(ModelRc::from(Rc::new(VecModel::from(initial_samples))));

    // Crash Recovery: restore unfinished downloads from previous session
    match download_manager.restore_unfinished_jobs().await {
        Ok(count) if count > 0 => {
            info!("Crash recovery: {} unfinished downloads restored", count);
            if let Some(window) = main_window.as_weak().upgrade() {
                window.set_status_message(
                    format!("{} unfinished download(s) restored from last session", count).into(),
                );
            }
        }
        Err(err) => warn!("Crash recovery check failed: {}", err),
        _ => {}
    }

    // Asynchronously detect installed engine & core binaries on startup
    let weak_bin_init = main_window.as_weak();
    tokio::spawn(async move {
        let (y_ver, y_path) = downloader::get_ytdlp_info().await;
        let (f_ver, f_path) = downloader::get_ffmpeg_info().await;
        let _ = weak_bin_init.upgrade_in_event_loop(move |win| {
            win.set_ytdlp_version_text(y_ver.into());
            win.set_ytdlp_path_text(y_path.into());
            win.set_ffmpeg_version_text(f_ver.into());
            win.set_ffmpeg_path_text(f_path.into());
        });
    });

    // Queue filter & search state
    let queue_filter_idx: Arc<tokio::sync::RwLock<i32>> = Arc::new(tokio::sync::RwLock::new(0));
    let queue_search_term: Arc<tokio::sync::RwLock<String>> = Arc::new(tokio::sync::RwLock::new(String::new()));
    let history_state: Arc<tokio::sync::RwLock<HistoryState>> = Arc::new(tokio::sync::RwLock::new(HistoryState::default()));

    // Speed history (30 seconds rolling timeline @ 2 Hz / 60 samples), peak tracking, and session bytes
    let speed_history = Arc::new(tokio::sync::Mutex::new(VecDeque::from(vec![0.0f64; 60])));
    let peak_speed_bytes = Arc::new(AtomicU64::new(0));
    let session_downloaded_bytes = Arc::new(AtomicU64::new(0));

    // Wire up DownloadManager status updates -> Slint UI with frame-rate decoupling
    let window_weak_sync = main_window.as_weak();
    let mgr_for_sync = download_manager.clone();
    let is_rendering = Arc::new(AtomicBool::new(false));
    let needs_rerender = Arc::new(AtomicBool::new(false));
    let last_speed_shift = Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
    let filter_sync = queue_filter_idx.clone();
    let search_sync = queue_search_term.clone();
    let speed_hist_sync = speed_history.clone();
    let peak_speed_sync = peak_speed_bytes.clone();
    let session_bytes_sync = session_downloaded_bytes.clone();
    let db_for_sync = db.clone();
    let hist_for_sync = history_state.clone();
    let last_completed_count = Arc::new(AtomicUsize::new(0));
    let was_downloading = Arc::new(AtomicBool::new(false));

    download_manager
        .set_update_listener(move || {
            let weak = window_weak_sync.clone();
            let mgr = mgr_for_sync.clone();
            let rendering_flag = is_rendering.clone();
            let rerender_flag = needs_rerender.clone();
            let last_shift_lock = last_speed_shift.clone();
            let filter_lock = filter_sync.clone();
            let search_lock = search_sync.clone();
            let speed_hist_lock = speed_hist_sync.clone();
            let peak_lock = peak_speed_sync.clone();
            let session_lock = session_bytes_sync.clone();
            let db_sync = db_for_sync.clone();
            let hist_sync = hist_for_sync.clone();
            let completed_tracker = last_completed_count.clone();
            let was_dl_tracker = was_downloading.clone();

            // Skip queuing redundant frames if a frame render is already pending, but flag for follow-up
            if rendering_flag.swap(true, Ordering::SeqCst) {
                rerender_flag.store(true, Ordering::SeqCst);
                return;
            }

            tokio::spawn(async move {
                let jobs = mgr.get_jobs_snapshot().await;
                let total_queue_count = jobs.len() as i32;
                let active_count = jobs
                    .iter()
                    .filter(|j| j.status == DownloadStatus::Downloading)
                    .count() as i32;

                let completed_count = jobs
                    .iter()
                    .filter(|j| j.status == DownloadStatus::Completed)
                    .count();

                let prev_completed = completed_tracker.swap(completed_count, Ordering::Relaxed);
                if completed_count > 0 && completed_count != prev_completed {
                    let db_clone = db_sync.clone();
                    let weak_clone = weak.clone();
                    let hist_clone = hist_sync.clone();
                    let mgr_autoclear = mgr.clone();
                    tokio::spawn(async move {
                        refresh_history(&db_clone, &hist_clone, weak_clone).await;
                        // Auto-clear completed jobs from in-memory queue after a brief delay
                        // so they disappear from Queue and only live in History.
                        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
                        mgr_autoclear.clear_completed().await;
                    });
                }

                let total_current_speed: f64 = jobs
                    .iter()
                    .filter(|j| j.status == DownloadStatus::Downloading)
                    .map(|j| j.speed_bytes_sec)
                    .sum();

                let total_downloaded: u64 = jobs
                    .iter()
                    .map(|j| j.downloaded_bytes)
                    .sum();

                // Peak speed tracking
                let current_peak = peak_lock.load(Ordering::Relaxed);
                if (total_current_speed as u64) > current_peak {
                    peak_lock.store(total_current_speed as u64, Ordering::Relaxed);
                }
                let peak_val = peak_lock.load(Ordering::Relaxed) as f64;

                // Session downloaded tracking
                let cur_session = session_lock.load(Ordering::Relaxed);
                if total_downloaded > cur_session {
                    session_lock.store(total_downloaded, Ordering::Relaxed);
                }
                let session_val = session_lock.load(Ordering::Relaxed);

                // History shift with 500ms cadence (2 Hz) for smooth, high-density 30s timeline (60 samples)
                let should_shift = {
                    let mut guard = last_shift_lock.lock().unwrap();
                    if guard.elapsed() >= std::time::Duration::from_millis(480) {
                        *guard = std::time::Instant::now();
                        true
                    } else {
                        false
                    }
                };

                let mut hist = speed_hist_lock.lock().await;
                if should_shift {
                    hist.pop_front();
                    hist.push_back(total_current_speed);
                } else if let Some(last) = hist.back_mut() {
                    *last = total_current_speed;
                }

                let max_in_window = hist.iter().copied().fold(0.0f64, f64::max).max(200.0 * 1024.0);
                let speed_samples: Vec<SpeedSampleData> = hist
                    .iter()
                    .map(|&spd| {
                        let ratio = (spd / max_in_window).clamp(0.0, 1.0) as f32;
                        SpeedSampleData {
                            ratio,
                            speed_text: DownloadProgress::format_speed_val(spd).into(),
                        }
                    })
                    .collect();

                let active_speed_limit = mgr.get_speed_limit().await;
                let active_speed_limit_bytes = downloader::manager::parse_speed_limit_bytes(active_speed_limit.as_deref());
                let speed_limit_ratio: f32 = match active_speed_limit_bytes {
                    Some(cap) if max_in_window > 0.0 => (cap as f64 / max_in_window).clamp(0.0, 1.0) as f32,
                    _ => 0.0,
                };
                let speed_limit_label = active_speed_limit_bytes
                    .map(|b| DownloadProgress::format_speed_val(b as f64))
                    .unwrap_or_default();

                let recent_count = hist.len().min(10);
                let recent_sum: f64 = hist.iter().rev().take(recent_count).copied().sum();
                let avg_speed = if recent_count > 0 { recent_sum / recent_count as f64 } else { 0.0 };
                let avg_speed_text = DownloadProgress::format_speed_val(avg_speed);

                let stability_text = if active_count == 0 || avg_speed < 1000.0 {
                    "Idle".to_string()
                } else {
                    let variance: f64 = hist.iter().rev().take(recent_count)
                        .map(|&s| (s - avg_speed).powi(2))
                        .sum::<f64>() / recent_count as f64;
                    let std_dev = variance.sqrt();
                    let coeff_var = (std_dev / avg_speed).clamp(0.0, 1.0);
                    let stability_pct = ((1.0 - coeff_var * 0.7) * 100.0).clamp(50.0, 99.0) as u32;
                    format!("{}% (Stable)", stability_pct)
                };

                let cur_speed_text = DownloadProgress::format_speed_val(total_current_speed);
                let peak_speed_text = DownloadProgress::format_speed_val(peak_val);
                let session_text = DownloadProgress::format_size(session_val);
                let is_active = active_count > 0 && total_current_speed > 100.0;

                let cur_filter = *filter_lock.read().await;
                let cur_search = search_lock.read().await.trim().to_lowercase();

                let filtered_jobs: Vec<_> = jobs
                    .into_iter()
                    .filter(|j| {
                        // 1. Status Filter
                        let matches_status = match cur_filter {
                            1 => j.status == DownloadStatus::Downloading,
                            2 => j.status == DownloadStatus::Queued,
                            3 => j.status == DownloadStatus::Paused,
                            4 => j.status == DownloadStatus::Completed,
                            5 => matches!(j.status, DownloadStatus::Failed(_)),
                            _ => true,
                        };
                        if !matches_status {
                            return false;
                        }

                        // 2. Search Term Filter
                        if !cur_search.is_empty() {
                            let in_title = j.title.to_lowercase().contains(&cur_search);
                            let in_url = j.url.to_lowercase().contains(&cur_search);
                            in_title || in_url
                        } else {
                            true
                        }
                    })
                    .collect();

                let now_ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let (_, _, auto_retry_max) = mgr.get_auto_retry_config().await;
                let (sched_en, sh, sm, eh, em) = mgr.get_scheduler_config().await;
                let in_sched_window = mgr.is_in_schedule_window().await;

                let total_filtered = filtered_jobs.len();
                let items: Vec<DownloadItemData> = filtered_jobs
                    .into_iter()
                    .enumerate()
                    .map(|(idx, j)| {
                        let is_dl = j.status == DownloadStatus::Downloading;
                        let is_paused = j.status == DownloadStatus::Paused;
                        let is_queued = j.status == DownloadStatus::Queued;
                        let is_scheduled = j.status == DownloadStatus::Scheduled;
                        let is_retrying = matches!(j.status, DownloadStatus::Retrying(_));
                        let is_active = is_dl || is_paused || is_queued || is_scheduled || is_retrying;
                        let is_completed = j.status == DownloadStatus::Completed;
                        let output_path = j.output_path.to_string_lossy().to_string();
                        let size_text = j.size_display();
                        let speed_text = j.speed_display();

                        let (status_str, eta_text) = match &j.status {
                            DownloadStatus::Retrying(target_ts) => {
                                let rem = target_ts.saturating_sub(now_ts);
                                (
                                    "Retrying".to_string(),
                                    format!("Auto-retry in {}s (Attempt {}/{})", rem, j.auto_retry_count + 1, auto_retry_max),
                                )
                            }
                            DownloadStatus::Scheduled => {
                                (
                                    "Scheduled".to_string(),
                                    format!("Scheduled for {:02}:{:02} – {:02}:{:02}", sh, sm, eh, em),
                                )
                            }
                            DownloadStatus::Downloading => {
                                ("Downloading".to_string(), j.eta_display())
                            }
                            other => (other.as_str().to_string(), "".to_string()),
                        };

                        DownloadItemData {
                            id: j.id.to_string().into(),
                            title: j.title.into(),
                            status: status_str.into(),
                            progress: j.progress_ratio,
                            size_text: size_text.into(),
                            speed_text: speed_text.into(),
                            eta_text: eta_text.into(),
                            output_path: output_path.into(),
                            is_completed,
                            can_pause: is_dl,
                            can_resume: is_paused || matches!(j.status, DownloadStatus::Failed(_)) || is_retrying || is_scheduled,
                            can_cancel: is_active,
                            is_queued,
                            can_move_up: (is_queued || is_scheduled) && idx > 0,
                            can_move_down: (is_queued || is_scheduled) && idx + 1 < total_filtered,
                        }
                    })
                    .collect();

                let reset_flag = rendering_flag.clone();
                let rerender_check = rerender_flag.clone();
                let mgr_followup = mgr.clone();
                let has_waiting = items.iter().any(|item| item.status == "Scheduled") || (sched_en && !in_sched_window && total_queue_count > 0);
                let banner_active = sched_en && !in_sched_window && has_waiting;
                let banner_text = format!("🌙 Off-Peak Scheduler active — Downloads queued for {:02}:{:02} – {:02}:{:02}", sh, sm, eh, em);

                let _ = weak.upgrade_in_event_loop(move |window| {
                    let model = Rc::new(VecModel::from(items));
                    window.set_download_items(ModelRc::from(model));
                    window.set_active_downloads_count(active_count);
                    window.set_total_queue_count(total_queue_count);
                    window.set_scheduler_active(banner_active);
                    window.set_scheduler_status_banner_text(banner_text.into());

                    let speed_model = Rc::new(VecModel::from(speed_samples));
                    window.set_speed_samples(ModelRc::from(speed_model));
                    window.set_current_total_speed_text(cur_speed_text.clone().into());
                    window.set_peak_speed_text(peak_speed_text.clone().into());
                    window.set_session_downloaded_text(session_text.into());
                    window.set_is_downloading_active(is_active);
                    window.set_avg_speed_text(avg_speed_text.into());
                    window.set_network_stability_text(stability_text.into());
                    window.set_speed_limit_ratio(speed_limit_ratio);
                    window.set_speed_limit_label(speed_limit_label.into());

                    let previously_downloading = was_dl_tracker.swap(active_count > 0, Ordering::Relaxed);
                    if active_count > 0 {
                        let live_status = format!(
                            "Downloading {} item{} • {} • Peak: {}",
                            active_count,
                            if active_count > 1 { "s" } else { "" },
                            cur_speed_text,
                            peak_speed_text
                        );
                        window.set_status_message(live_status.into());
                    } else if previously_downloading || (completed_count > 0 && prev_completed != completed_count) {
                        let msg = format!(
                            "Download completed successfully • {} file{} ready",
                            completed_count,
                            if completed_count > 1 { "s" } else { "" }
                        );
                        window.set_status_message(msg.into());
                    }

                    reset_flag.store(false, Ordering::SeqCst);
                    if rerender_check.swap(false, Ordering::SeqCst) {
                        tokio::spawn(async move {
                            mgr_followup.notify_update().await;
                        });
                    }
                });
            });
        })
        .await;

    // Periodic background ticker (every 1000ms) with idle sleep to preserve CPU and battery
    let ticker_mgr = download_manager.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(1000));
        let mut zero_speed_ticks = 0;
        loop {
            interval.tick().await;
            let jobs = ticker_mgr.get_jobs_snapshot().await;
            let has_active = jobs.iter().any(|j| j.status == DownloadStatus::Downloading);
            if has_active {
                zero_speed_ticks = 0;
                ticker_mgr.notify_update().await;
            } else if zero_speed_ticks < 62 {
                // Decay the 60-sample speed graph cleanly to zero, then enter low-power sleep
                zero_speed_ticks += 1;
                ticker_mgr.notify_update().await;
            }
        }
    });

    // Callback: Analyze URL
    let window_weak = main_window.as_weak();
    let meta_clone = current_metadata.clone();
    let client_clone = network_client.clone();
    let cookies_for_analyze = current_cookies_browser.clone();
    let proxy_for_analyze = current_proxy.clone();

    main_window.on_analyze_url(move |url| {
        run_url_analysis(
            url.to_string(),
            window_weak.clone(),
            meta_clone.clone(),
            client_clone.clone(),
            cookies_for_analyze.clone(),
            proxy_for_analyze.clone(),
        );
    });

    // Callback: Clear / Cancel Analysis
    let weak_clear = main_window.as_weak();
    let meta_clear = current_metadata.clone();
    main_window.on_clear_analysis(move || {
        if let Ok(mut meta) = meta_clear.try_lock() {
            *meta = None;
        } else {
            let meta_async = meta_clear.clone();
            tokio::spawn(async move {
                let mut meta = meta_async.lock().await;
                *meta = None;
            });
        }
        if let Some(window) = weak_clear.upgrade() {
            window.set_has_metadata(false);
            window.set_is_analyzing(false);
            window.set_video_title("".into());
            window.set_video_duration("".into());
            window.set_video_resolution("".into());
            window.set_video_size_text("".into());
            window.set_video_fps_text("".into());
            window.set_video_codecs_text("".into());
            window.set_format_best_label("Best Quality".into());
            window.set_format_1080_label("1080p FHD".into());
            window.set_format_720_label("720p HD".into());
            window.set_format_480_label("480p SD".into());
            window.set_format_audio_label("Extract Audio".into());
            window.set_has_thumbnail(false);
            window.set_thumbnail_image(slint::Image::default());
            window.set_has_subtitles(false);
            window.set_subtitles_summary("".into());
            window.set_is_playlist(false);
            window.set_playlist_count(0);
            window.set_playlist_episodes(ModelRc::default());
            window.set_selected_episodes_count(0);
            window.set_episode_range_input("".into());
            window.set_status_message("Analysis cleared. Ready to download media.".into());
        }
    });

    // Callback: Preview primary stream URL
    let meta_preview = current_metadata.clone();
    main_window.on_preview_current_stream(move || {
        let meta_preview = meta_preview.clone();
        tokio::spawn(async move {
            let meta_guard = meta_preview.lock().await;
            if let Some(ref meta) = *meta_guard {
                let raw_url = meta.url.clone();
                let referer = meta.referer.clone();
                drop(meta_guard);
                let stream_url = crate::downloader::extractor::resolve_playable_stream_url(&raw_url, None).await;
                if let Err(e) = filesystem::play_stream_url(&stream_url, referer.as_deref()) {
                    warn!("Failed to preview stream URL {}: {}", stream_url, e);
                }
            }
        });
    });

    // Callback: Preview specific episode stream URL
    let meta_preview_ep = current_metadata.clone();
    main_window.on_preview_episode_stream(move |idx| {
        let meta_preview_ep = meta_preview_ep.clone();
        tokio::spawn(async move {
            let meta_guard = meta_preview_ep.lock().await;
            if let Some(ref meta) = *meta_guard {
                let idx_usize = idx as usize;
                if let Some(entry) = meta.playlist_entries.get(idx_usize) {
                    let raw_url = entry.url.clone();
                    let referer = entry.referer.clone().or_else(|| meta.referer.clone());
                    drop(meta_guard);
                    let stream_url = crate::downloader::extractor::resolve_playable_stream_url(&raw_url, None).await;
                    if let Err(e) = filesystem::play_stream_url(&stream_url, referer.as_deref()) {
                        warn!("Failed to preview episode stream URL {}: {}", stream_url, e);
                    }
                }
            }
        });
    });

    // Callback: Toggle single episode selection
    let weak_toggle = main_window.as_weak();
    main_window.on_toggle_episode(move |idx| {
        if let Some(window) = weak_toggle.upgrade() {
            let model = window.get_playlist_episodes();
            let mut items: Vec<EpisodeItemData> = (0..model.row_count())
                .filter_map(|i| model.row_data(i))
                .collect();
            if let Some(item) = items.get_mut(idx as usize) {
                item.selected = !item.selected;
            }
            let selected_count = items.iter().filter(|e| e.selected).count() as i32;
            window.set_playlist_episodes(ModelRc::from(Rc::new(VecModel::from(items))));
            window.set_selected_episodes_count(selected_count);
        }
    });

    // Callback: Select all / none episodes
    let weak_select_all = main_window.as_weak();
    main_window.on_select_all_episodes(move |select| {
        if let Some(window) = weak_select_all.upgrade() {
            let model = window.get_playlist_episodes();
            let mut items: Vec<EpisodeItemData> = (0..model.row_count())
                .filter_map(|i| model.row_data(i))
                .collect();
            for item in &mut items {
                item.selected = select;
            }
            let selected_count = if select { items.len() as i32 } else { 0 };
            window.set_playlist_episodes(ModelRc::from(Rc::new(VecModel::from(items))));
            window.set_selected_episodes_count(selected_count);
        }
    });

    // Callback: Apply episode range (e.g. "1-10", "1, 3, 5")
    let weak_range = main_window.as_weak();
    main_window.on_apply_episode_range(move |range_str| {
        if let Some(window) = weak_range.upgrade() {
            let model = window.get_playlist_episodes();
            let mut items: Vec<EpisodeItemData> = (0..model.row_count())
                .filter_map(|i| model.row_data(i))
                .collect();
            let total = items.len();
            let selected_set = downloader::parse_episode_range(&range_str, total);
            for (idx, item) in items.iter_mut().enumerate() {
                item.selected = selected_set.contains(&idx);
            }
            let selected_count = items.iter().filter(|e| e.selected).count() as i32;
            window.set_playlist_episodes(ModelRc::from(Rc::new(VecModel::from(items))));
            window.set_selected_episodes_count(selected_count);
        }
    });

    // Callback: Invert episode selection
    let weak_invert = main_window.as_weak();
    main_window.on_invert_episode_selection(move || {
        if let Some(window) = weak_invert.upgrade() {
            let model = window.get_playlist_episodes();
            let mut items: Vec<EpisodeItemData> = (0..model.row_count())
                .filter_map(|i| model.row_data(i))
                .collect();
            for item in &mut items {
                item.selected = !item.selected;
            }
            let selected_count = items.iter().filter(|e| e.selected).count() as i32;
            window.set_playlist_episodes(ModelRc::from(Rc::new(VecModel::from(items))));
            window.set_selected_episodes_count(selected_count);
        }
    });

    // Callback: Select first N episodes (e.g. 5)
    let weak_first_n = main_window.as_weak();
    main_window.on_select_first_n_episodes(move |n| {
        if let Some(window) = weak_first_n.upgrade() {
            let model = window.get_playlist_episodes();
            let mut items: Vec<EpisodeItemData> = (0..model.row_count())
                .filter_map(|i| model.row_data(i))
                .collect();
            for (idx, item) in items.iter_mut().enumerate() {
                item.selected = (idx as i32) < n;
            }
            let selected_count = items.iter().filter(|e| e.selected).count() as i32;
            window.set_playlist_episodes(ModelRc::from(Rc::new(VecModel::from(items))));
            window.set_selected_episodes_count(selected_count);
        }
    });

    let clipboard_monitor_enabled = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let last_clipboard = Arc::new(std::sync::Mutex::new(
        filesystem::read_clipboard_text().unwrap_or_default(),
    ));

    // Background Task: Live Clipboard Link Watcher
    let weak_clip_watcher = main_window.as_weak();
    let clip_enabled_watcher = clipboard_monitor_enabled.clone();
    let last_clip_watcher = last_clipboard.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(1500));
        loop {
            interval.tick().await;
            if !clip_enabled_watcher.load(std::sync::atomic::Ordering::Relaxed) {
                continue;
            }

            let current = tokio::task::spawn_blocking(filesystem::read_clipboard_text)
                .await
                .ok()
                .flatten();

            if let Some(text) = current {
                let trimmed = text.trim().to_string();
                if trimmed.is_empty() {
                    continue;
                }

                let is_new_clipboard = {
                    let mut last = last_clip_watcher.lock().unwrap();
                    if *last != trimmed {
                        *last = trimmed.clone();
                        true
                    } else {
                        false
                    }
                };

                if is_new_clipboard {
                    if let Some(candidate_url) = downloader::extractor::extract_candidate_media_url(&trimmed) {
                        tracing::info!("Clipboard watcher detected candidate media URL: {}", candidate_url);
                        let url = candidate_url.clone();
                        let _ = weak_clip_watcher.upgrade_in_event_loop(move |win| {
                            win.set_clipboard_detected_url(url.clone().into());
                            win.set_clipboard_detected_url_visible(true);
                            win.set_status_message(format!("Clipboard detected media link: {}", url).into());
                        });

                        // Auto-dismiss floating toast after 10 seconds if not clicked
                        let weak_timer = weak_clip_watcher.clone();
                        let url_timer = candidate_url.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                            let _ = weak_timer.upgrade_in_event_loop(move |win| {
                                if win.get_clipboard_detected_url().as_str() == url_timer {
                                    win.set_clipboard_detected_url_visible(false);
                                }
                            });
                        });

                        let notif_url = candidate_url.clone();
                        notifications::send_notification(
                            "Native Video Downloader",
                            "Media Link Copied",
                            &format!("Ready to inspect: {}", notif_url),
                            false,
                        );
                    }
                }
            }
        }
    });

    // Callback: Analyze Detected Clipboard URL
    let weak_analyze_clip = main_window.as_weak();
    let meta_analyze_clip = current_metadata.clone();
    let client_analyze_clip = network_client.clone();
    let cookies_analyze_clip = current_cookies_browser.clone();
    let proxy_analyze_clip = current_proxy.clone();
    main_window.on_analyze_clipboard_detected(move || {
        let weak = weak_analyze_clip.clone();
        let meta = meta_analyze_clip.clone();
        let client = client_analyze_clip.clone();
        let cookies = cookies_analyze_clip.clone();
        let proxy = proxy_analyze_clip.clone();
        let _ = weak_analyze_clip.upgrade_in_event_loop(move |win| {
            let url = win.get_clipboard_detected_url().to_string();
            win.set_clipboard_detected_url_visible(false);
            if !url.is_empty() {
                win.set_active_tab(0);
                win.set_batch_mode(false);
                win.set_input_url_text(url.clone().into());
                win.set_status_message(format!("Analyzing copied link: {}", url).into());
                run_url_analysis(url, weak, meta, client, cookies, proxy);
            }
        });
    });

    // Callback: Quick Download Detected Clipboard URL
    let weak_quick_dl = main_window.as_weak();
    let mgr_quick_dl = download_manager.clone();
    let current_dir_quick_dl = current_download_dir.clone();
    let client_quick_dl = network_client.clone();
    let cookies_quick_dl = current_cookies_browser.clone();
    let proxy_quick_dl = current_proxy.clone();

    main_window.on_quick_download_clipboard_detected(move || {
        let weak = weak_quick_dl.clone();
        let mgr = mgr_quick_dl.clone();
        let dir_lock = current_dir_quick_dl.clone();
        let client = client_quick_dl.clone();
        let cookies_lock = cookies_quick_dl.clone();
        let proxy_lock = proxy_quick_dl.clone();

        let _ = weak_quick_dl.upgrade_in_event_loop(move |win| {
            let url = win.get_clipboard_detected_url().to_string();
            win.set_clipboard_detected_url_visible(false);
            if url.is_empty() {
                return;
            }
            win.set_status_message(format!("Quick downloading media: {}", url).into());
            win.set_active_tab(1); // Switch to Downloads tab

            tokio::spawn(async move {
                let cookies = cookies_lock.read().await.clone();
                let proxy = proxy_lock.read().await.clone();

                let meta_res = if downloader::is_streaming_platform(&url) || url.contains("anyreel.app") {
                    downloader::inspect_video_with_options(&url, cookies.as_deref(), proxy.as_deref()).await
                } else {
                    match client.inspect_url(&url).await {
                        Ok(meta) if downloader::is_valid_direct_media(&meta) => Ok(meta),
                        _ => downloader::inspect_video_with_options(&url, cookies.as_deref(), proxy.as_deref()).await,
                    }
                };

                let metadata = match meta_res {
                    Ok(m) => m,
                    Err(e) => {
                        let _ = weak.upgrade_in_event_loop(move |w| {
                            w.set_status_message(format!("Quick download inspection failed: {}", e).into());
                        });
                        return;
                    }
                };

                let download_dir = dir_lock.read().await.clone();

                if metadata.is_playlist && !metadata.playlist_entries.is_empty() {
                    let series_title = metadata.title.clone();
                    let mut added = 0;
                    for (i, entry) in metadata.playlist_entries.iter().enumerate() {
                        let entry_url = crate::downloader::extractor::get_cached_playable_url(&entry.url)
                            .unwrap_or_else(|| entry.url.clone());
                        let entry_referer = if crate::downloader::extractor::is_streaming_platform(&entry_url) {
                            None
                        } else {
                            entry.referer.clone().or_else(|| metadata.referer.clone())
                        };
                        if mgr
                            .add_download_with_context(
                                entry_url,
                                entry.title.clone(),
                                &download_dir,
                                None,
                                true,
                                false,
                                None,
                                false,
                                None,
                                false,
                                None,
                                None,
                                None,
                                false,
                                Some(&series_title),
                                Some(i + 1),
                                entry_referer,
                            )
                            .await
                            .is_ok()
                        {
                            added += 1;
                        }
                    }
                    let series_title_notif = series_title.clone();
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        w.set_status_message(format!("Queued {} episodes from series", added).into());
                    });
                    notifications::send_notification(
                        "Download Started",
                        "Quick Download Queued",
                        &format!("{} episodes from {}", added, series_title_notif),
                        false,
                    );
                } else {
                    let is_extractor = metadata.is_extractor;
                    let title_clone = metadata.title.clone();
                    let title_notif = metadata.title.clone();
                    let download_subs = metadata.has_subtitles;
                    match mgr
                        .add_download_with_context(
                            metadata.url.clone(),
                            metadata.title.clone(),
                            &download_dir,
                            metadata.content_length,
                            is_extractor,
                            false,
                            None,
                            download_subs,
                            None,
                            false,
                            metadata.thumbnail_url.clone(),
                            None,
                            None,
                            false,
                            None,
                            None,
                            metadata.referer.clone(),
                        )
                        .await
                    {
                        Ok(_) => {
                            let _ = weak.upgrade_in_event_loop(move |w| {
                                w.set_status_message(format!("Queued download: {}", title_clone).into());
                            });
                            notifications::send_notification(
                                "Download Started",
                                "Quick Download Queued",
                                &title_notif,
                                false,
                            );
                        }
                        Err(e) => {
                            let _ = weak.upgrade_in_event_loop(move |w| {
                                w.set_status_message(format!("Quick download failed: {}", e).into());
                            });
                        }
                    }
                }
            });
        });
    });

    // Callback: Dismiss Detected Clipboard URL
    let weak_dismiss_clip = main_window.as_weak();
    main_window.on_dismiss_clipboard_detected(move || {
        let _ = weak_dismiss_clip.upgrade_in_event_loop(|win| {
            win.set_clipboard_detected_url_visible(false);
            win.set_status_message("Dismissed copied media link".into());
        });
    });

    // Callback: Toggle Clipboard Monitor in Settings
    let clip_enabled_setter = clipboard_monitor_enabled.clone();
    let weak_clip_set = main_window.as_weak();
    main_window.on_set_clipboard_monitor(move |enabled| {
        clip_enabled_setter.store(enabled, std::sync::atomic::Ordering::Relaxed);
        let _ = weak_clip_set.upgrade_in_event_loop(move |win| {
            win.set_status_message(
                if enabled {
                    "Live Clipboard Link Monitor enabled".into()
                } else {
                    "Live Clipboard Link Monitor disabled".into()
                },
            );
        });
    });

    // Callback: Set default audio format
    let db_audio_fmt = db.clone();
    let weak_audio_fmt = main_window.as_weak();
    main_window.on_set_default_audio_format(move |fmt_idx| {
        let db = db_audio_fmt.clone();
        let weak = weak_audio_fmt.clone();
        let name = map_audio_format(fmt_idx).to_uppercase();
        tokio::spawn(async move {
            let _ = db.set_setting("default_audio_format", &fmt_idx.to_string()).await;
            info!("Saved default audio format: {} (idx {})", name, fmt_idx);
            let _ = weak.upgrade_in_event_loop(move |win| {
                win.set_status_message(format!("Default audio format set to {}", name).into());
            });
        });
    });

    // Callback: Set default audio bitrate
    let db_audio_br = db.clone();
    let weak_audio_br = main_window.as_weak();
    main_window.on_set_default_audio_bitrate(move |br_idx| {
        let db = db_audio_br.clone();
        let weak = weak_audio_br.clone();
        let br_name = map_audio_bitrate(br_idx);
        tokio::spawn(async move {
            let _ = db.set_setting("default_audio_bitrate", &br_idx.to_string()).await;
            info!("Saved default audio bitrate: {} (idx {})", br_name, br_idx);
            let _ = weak.upgrade_in_event_loop(move |win| {
                win.set_status_message(format!("Default audio bitrate set to {}", br_name).into());
            });
        });
    });

    // Callback: Set embed audio metadata
    let db_audio_embed = db.clone();
    let weak_audio_embed = main_window.as_weak();
    main_window.on_set_embed_audio_metadata(move |embed| {
        let db = db_audio_embed.clone();
        let weak = weak_audio_embed.clone();
        tokio::spawn(async move {
            let _ = db.set_setting("default_embed_metadata", if embed { "true" } else { "false" }).await;
            info!("Saved embed audio metadata setting: {}", embed);
            let _ = weak.upgrade_in_event_loop(move |win| {
                win.set_status_message(
                    if embed {
                        "Audio cover artwork and ID3 metadata embedding enabled".into()
                    } else {
                        "Audio stream only (metadata embedding disabled)".into()
                    },
                );
            });
        });
    });

    // Callback: Set Filename Template
    let db_tpl = db.clone();
    let mgr_tpl = download_manager.clone();
    let weak_tpl = main_window.as_weak();
    main_window.on_set_filename_template(move |tpl_str| {
        let db = db_tpl.clone();
        let mgr = mgr_tpl.clone();
        let weak = weak_tpl.clone();
        let tpl = tpl_str.trim().to_string();
        tokio::spawn(async move {
            let _ = db.set_setting("filename_template", &tpl).await;
            mgr.set_filename_template(tpl.clone()).await;
            info!("Saved filename template: {}", tpl);
            let _ = weak.upgrade_in_event_loop(move |win| {
                win.set_status_message(format!("Filename template saved: {}", tpl).into());
            });
        });
    });

    // Callback: Set Subtitle & Asset Organization Mode
    let db_asset_org = db.clone();
    let mgr_asset_org = download_manager.clone();
    let weak_asset_org = main_window.as_weak();
    main_window.on_set_asset_organization_mode(move |mode_idx| {
        let db = db_asset_org.clone();
        let mgr = mgr_asset_org.clone();
        let weak = weak_asset_org.clone();
        tokio::spawn(async move {
            let mode = mode_idx.clamp(0, 2) as u8;
            let _ = db.set_setting("asset_org_mode", &mode.to_string()).await;
            mgr.set_asset_organization_mode(mode).await;
            info!("Saved asset organization mode: {}", mode);
            let msg = match mode {
                0 => "Asset Organization: Dedicated folder per video (<Title>/...)",
                1 => "Asset Organization: Grouped subtitles subfolder (Subtitles/<Title>/...)",
                _ => "Asset Organization: Flat subtitles folder (Subtitles/...)",
            };
            let _ = weak.upgrade_in_event_loop(move |win| {
                win.set_selected_asset_org_mode(mode as i32);
                win.set_status_message(msg.into());
            });
        });
    });

    // Callback: Set File Conflict & Auto-Resume Policy
    let db_conflict = db.clone();
    let mgr_conflict = download_manager.clone();
    let weak_conflict = main_window.as_weak();
    main_window.on_set_file_conflict_policy(move |policy_idx| {
        let db = db_conflict.clone();
        let mgr = mgr_conflict.clone();
        let weak = weak_conflict.clone();
        tokio::spawn(async move {
            let policy = crate::filesystem::FileConflictPolicy::from_u8(policy_idx.clamp(0, 3) as u8);
            let _ = db.set_setting("file_conflict_policy", &policy.to_u8().to_string()).await;
            mgr.set_file_conflict_policy(policy).await;
            info!("Saved file conflict policy: {:?}", policy);
            let msg = match policy {
                crate::filesystem::FileConflictPolicy::AutoResumeOrRename => "File Conflict: Auto-Resume partial downloads / Rename completed",
                crate::filesystem::FileConflictPolicy::AutoRename => "File Conflict: Always auto-rename with (1)",
                crate::filesystem::FileConflictPolicy::Overwrite => "File Conflict: Overwrite existing media",
                crate::filesystem::FileConflictPolicy::SkipExisting => "File Conflict: Skip existing completed files",
            };
            let _ = weak.upgrade_in_event_loop(move |win| {
                win.set_selected_file_conflict_policy(policy.to_u8() as i32);
                win.set_status_message(msg.into());
            });
        });
    });

    // Callback: Set concurrent fragments per video (yt-dlp --concurrent-fragments)
    let db_fragments = db.clone();
    let mgr_fragments = download_manager.clone();
    let weak_fragments = main_window.as_weak();
    main_window.on_set_concurrent_fragments(move |n| {
        let db = db_fragments.clone();
        let mgr = mgr_fragments.clone();
        let weak = weak_fragments.clone();
        tokio::spawn(async move {
            let count = (n as u8).clamp(1, 16);
            let _ = db.set_setting("concurrent_fragments", &count.to_string()).await;
            mgr.set_concurrent_fragments(count).await;
            info!("Saved concurrent fragments per video: {}", count);
            let _ = weak.upgrade_in_event_loop(move |win| {
                win.set_selected_concurrent_fragments(count as i32);
                win.set_status_message(format!("Download speed: {} parallel fragments per video", count).into());
            });
        });
    });

    // Callback: Set preferred subtitle language & persist
    // Callback: Toggle individual subtitle language selection & persist
    let db_sub_toggle = db.clone();
    let weak_sub_toggle = main_window.as_weak();
    main_window.on_toggle_sub_lang(move |idx| {
        if let Some(win) = weak_sub_toggle.upgrade() {
            handle_toggle_sub_lang(&win, idx);
            let indices = get_selected_sub_langs_indices(&win);
            let indices_str = indices.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
            let db = db_sub_toggle.clone();
            tokio::spawn(async move {
                let _ = db.set_setting("preferred_sub_lang", &indices_str).await;
            });
        }
    });

    let db_sub_lang = db.clone();
    let weak_sub_lang = main_window.as_weak();
    main_window.on_set_sub_lang(move |idx| {
        if let Some(win) = weak_sub_lang.upgrade() {
            handle_toggle_sub_lang(&win, idx);
            let indices = get_selected_sub_langs_indices(&win);
            let indices_str = indices.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
            let db = db_sub_lang.clone();
            tokio::spawn(async move {
                let _ = db.set_setting("preferred_sub_lang", &indices_str).await;
            });
        }
    });

    // Callback: Set Theme Mode (0 = Light, 1 = Dark, 2 = Auto / System) & persist
    let db_theme = db.clone();
    let weak_theme = main_window.as_weak();
    main_window.on_set_theme_mode(move |mode_val| {
        let db = db_theme.clone();
        if let Some(win) = weak_theme.upgrade() {
            let mode = ThemeMode::from_i32(mode_val);
            let is_dark = resolve_is_dark(mode);
            win.set_theme_mode(mode.to_i32());
            win.set_is_dark_mode(is_dark);

            let status = match mode {
                ThemeMode::Auto => format!(
                    "Theme: System Default ({}) - dynamic OS appearance",
                    if is_dark { "Dark" } else { "Light" }
                ),
                ThemeMode::Dark => "Theme: Dark Mode enabled".to_string(),
                ThemeMode::Light => "Theme: Light Mode enabled".to_string(),
            };
            win.set_status_message(status.into());

            tokio::spawn(async move {
                let _ = db.set_setting("theme_mode", mode.as_str()).await;
                let _ = db.set_setting("dark_mode", if is_dark { "true" } else { "false" }).await;
                info!("Saved theme mode preference: {:?} (is_dark={})", mode, is_dark);
            });
        }
    });

    // Callback: Toggle Theme (cycles through Auto -> Dark -> Light -> Auto)
    let weak_toggle = main_window.as_weak();
    main_window.on_toggle_theme(move || {
        if let Some(win) = weak_toggle.upgrade() {
            let current = ThemeMode::from_i32(win.get_theme_mode());
            let next_mode = match current {
                ThemeMode::Auto => ThemeMode::Dark,
                ThemeMode::Dark => ThemeMode::Light,
                ThemeMode::Light => ThemeMode::Auto,
            };
            win.invoke_set_theme_mode(next_mode.to_i32());
        }
    });

    // Background watcher for OS appearance changes (active when in Auto / System mode)
    let weak_auto_watcher = main_window.as_weak();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(1500));
        loop {
            interval.tick().await;
            if let Some(win) = weak_auto_watcher.upgrade() {
                if ThemeMode::from_i32(win.get_theme_mode()) == ThemeMode::Auto {
                    let sys_dark = is_system_dark_mode();
                    if win.get_is_dark_mode() != sys_dark {
                        info!("System appearance change detected! Updating app theme to is_dark={}", sys_dark);
                        win.set_is_dark_mode(sys_dark);
                    }
                }
            } else {
                break;
            }
        }
    });

    // Callback: Paste from Clipboard into URL input & Auto-Analyze
    let weak_paste = main_window.as_weak();
    let meta_paste = current_metadata.clone();
    let client_paste = network_client.clone();
    let cookies_paste = current_cookies_browser.clone();
    let proxy_paste = current_proxy.clone();
    main_window.on_paste_from_clipboard(move || {
        if let Some(text) = filesystem::read_clipboard_text() {
            let weak = weak_paste.clone();
            let is_url = text.starts_with("http://") || text.starts_with("https://");
            if is_url {
                run_url_analysis(
                    text,
                    weak,
                    meta_paste.clone(),
                    client_paste.clone(),
                    cookies_paste.clone(),
                    proxy_paste.clone(),
                );
            } else {
                let _ = weak.upgrade_in_event_loop(move |win| {
                    win.set_input_url_text(text.into());
                    win.set_status_message("Pasted from clipboard (please ensure it starts with http:// or https://)".into());
                });
            }
        } else {
            let _ = weak_paste.upgrade_in_event_loop(|win| {
                win.set_status_message("Clipboard is empty or does not contain text".into());
            });
        }
    });

    // Callback: Paste into Batch URLs input
    let weak_paste_batch = main_window.as_weak();
    main_window.on_paste_batch_from_clipboard(move || {
        if let Some(text) = filesystem::read_clipboard_text() {
            let links = filesystem::parse_links_from_text(&text);
            let to_append = if !links.is_empty() {
                links
                    .into_iter()
                    .map(|l| downloader::normalize_media_url(&l))
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                text
            };
            let _ = weak_paste_batch.upgrade_in_event_loop(move |win| {
                let current = win.get_batch_urls_text().to_string();
                let new_val = if current.trim().is_empty() {
                    to_append
                } else {
                    format!("{}\n{}", current.trim_end(), to_append)
                };
                win.set_batch_urls_text(new_val.into());
                win.set_status_message("Pasted links into batch input from clipboard".into());
            });
        }
    });

    // Callback: Import links from a .txt, .m3u, or .csv file
    let weak_import = main_window.as_weak();
    let meta_import = current_metadata.clone();
    let client_import = network_client.clone();
    let cookies_import = current_cookies_browser.clone();
    let proxy_import = current_proxy.clone();
    main_window.on_import_links_file(move || {
        let weak = weak_import.clone();
        let meta = meta_import.clone();
        let client = client_import.clone();
        let cookies = cookies_import.clone();
        let proxy = proxy_import.clone();
        tokio::spawn(async move {
            let Some(file_path) = filesystem::pick_file().await else {
                return;
            };

            let file_name = file_path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "links file".to_string());

            match tokio::fs::read_to_string(&file_path).await {
                Ok(content) => {
                    let links = filesystem::parse_links_from_text(&content);
                    if links.is_empty() {
                        let _ = weak.upgrade_in_event_loop(move |win| {
                            win.set_has_error(true);
                            win.set_error_message(
                                format!(
                                    "No valid video or audio URLs found in '{}'. Supported formats: .txt, .m3u, .csv with http:// or https:// links.",
                                    file_name
                                )
                                .into(),
                            );
                            win.set_status_message(format!("Import failed: no URLs found in {}", file_name).into());
                        });
                    } else if links.len() == 1 {
                        let single_url = links[0].clone();
                        let _ = weak.upgrade_in_event_loop(move |win| {
                            win.set_batch_mode(false);
                            win.set_input_url_text(single_url.clone().into());
                            win.set_has_error(false);
                            win.set_error_message("".into());
                            win.set_status_message(
                                format!("Imported 1 link from '{}' - analyzing...", file_name).into(),
                            );
                        });
                        run_url_analysis(
                            links[0].clone(),
                            weak.clone(),
                            meta.clone(),
                            client.clone(),
                            cookies.clone(),
                            proxy.clone(),
                        );
                    } else {
                        let count = links.len();
                        let batch_text = links.join("\n");
                        let _ = weak.upgrade_in_event_loop(move |win| {
                            win.set_batch_mode(true);
                            let current = win.get_batch_urls_text().to_string();
                            let combined = if current.trim().is_empty() {
                                batch_text
                            } else {
                                format!("{}\n{}", current.trim_end(), batch_text)
                            };
                            win.set_batch_urls_text(combined.into());
                            win.set_has_error(false);
                            win.set_error_message("".into());
                            win.set_status_message(
                                format!(
                                    "Successfully imported {} links from '{}' into batch queue",
                                    count, file_name
                                )
                                .into(),
                            );
                        });
                    }
                }
                Err(err) => {
                    tracing::error!("Failed to read imported file: {}", err);
                    let _ = weak.upgrade_in_event_loop(move |win| {
                        win.set_has_error(true);
                        win.set_error_message(format!("Failed to read file '{}': {}", file_name, err).into());
                        win.set_status_message("Error reading imported links file".into());
                    });
                }
            }
        });
    });

/// Helper: Map UI format index to (is_audio_only, quality_spec)
fn map_format_index(idx: i32) -> (bool, Option<String>) {
    match idx {
        1 => (false, Some("1080p".to_string())),
        2 => (false, Some("720p".to_string())),
        3 => (false, Some("480p".to_string())),
        4 => (true, None), // Audio Only
        _ => (false, None), // Best quality video
    }
}

/// Helper: Map audio format index to container name
fn map_audio_format(idx: i32) -> &'static str {
    match idx {
        1 => "m4a",
        2 => "flac",
        3 => "wav",
        4 => "opus",
        _ => "mp3",
    }
}

/// Helper: Map audio bitrate index to bitrate value
fn map_audio_bitrate(idx: i32) -> &'static str {
    match idx {
        1 => "256K",
        2 => "192K",
        3 => "128K",
        _ => "320K",
    }
}

fn apply_sub_langs_state(window: &AppWindow, selected_indices: &[i32]) {
    let all = selected_indices.contains(&0) || selected_indices.is_empty();
    window.set_sub_lang_all(all);
    window.set_sub_lang_en(!all && selected_indices.contains(&1));
    window.set_sub_lang_km(!all && selected_indices.contains(&2));
    window.set_sub_lang_th(!all && selected_indices.contains(&3));
    window.set_sub_lang_vi(!all && selected_indices.contains(&4));
    window.set_sub_lang_id(!all && selected_indices.contains(&5));
    window.set_sub_lang_my(!all && selected_indices.contains(&6));
    window.set_sub_lang_zh(!all && selected_indices.contains(&7));
    window.set_sub_lang_ja(!all && selected_indices.contains(&8));
    window.set_sub_lang_ko(!all && selected_indices.contains(&9));
    window.set_sub_lang_es(!all && selected_indices.contains(&10));
    window.set_sub_lang_fr(!all && selected_indices.contains(&11));
    window.set_sub_lang_de(!all && selected_indices.contains(&12));
    window.set_sub_lang_ru(!all && selected_indices.contains(&13));
    window.set_sub_lang_pt(!all && selected_indices.contains(&14));
    window.set_sub_lang_ar(!all && selected_indices.contains(&15));

    update_sub_langs_summary(window);
}

fn update_sub_langs_summary(window: &AppWindow) {
    if window.get_sub_lang_all() {
        window.set_selected_sub_langs_summary("All Languages".into());
        return;
    }
    let mut names = Vec::new();
    if window.get_sub_lang_en() { names.push("English (en)"); }
    if window.get_sub_lang_km() { names.push("Khmer (km)"); }
    if window.get_sub_lang_th() { names.push("Thai (th)"); }
    if window.get_sub_lang_vi() { names.push("Vietnamese (vi)"); }
    if window.get_sub_lang_id() { names.push("Indonesian (id)"); }
    if window.get_sub_lang_my() { names.push("Burmese (my)"); }
    if window.get_sub_lang_zh() { names.push("Chinese (zh)"); }
    if window.get_sub_lang_ja() { names.push("Japanese (ja)"); }
    if window.get_sub_lang_ko() { names.push("Korean (ko)"); }
    if window.get_sub_lang_es() { names.push("Spanish (es)"); }
    if window.get_sub_lang_fr() { names.push("French (fr)"); }
    if window.get_sub_lang_de() { names.push("German (de)"); }
    if window.get_sub_lang_ru() { names.push("Russian (ru)"); }
    if window.get_sub_lang_pt() { names.push("Portuguese (pt)"); }
    if window.get_sub_lang_ar() { names.push("Arabic (ar)"); }

    if names.is_empty() {
        window.set_sub_lang_all(true);
        window.set_selected_sub_langs_summary("All Languages".into());
    } else {
        let summary = format!("{} ({} selected)", names.join(", "), names.len());
        window.set_selected_sub_langs_summary(summary.into());
    }
}

fn handle_toggle_sub_lang(window: &AppWindow, idx: i32) {
    if idx == 0 {
        window.set_sub_lang_all(true);
        window.set_sub_lang_en(false);
        window.set_sub_lang_km(false);
        window.set_sub_lang_th(false);
        window.set_sub_lang_vi(false);
        window.set_sub_lang_id(false);
        window.set_sub_lang_my(false);
        window.set_sub_lang_zh(false);
        window.set_sub_lang_ja(false);
        window.set_sub_lang_ko(false);
        window.set_sub_lang_es(false);
        window.set_sub_lang_fr(false);
        window.set_sub_lang_de(false);
        window.set_sub_lang_ru(false);
        window.set_sub_lang_pt(false);
        window.set_sub_lang_ar(false);
        window.set_selected_sub_langs_summary("All Languages".into());
        return;
    }

    match idx {
        1 => window.set_sub_lang_en(!window.get_sub_lang_en()),
        2 => window.set_sub_lang_km(!window.get_sub_lang_km()),
        3 => window.set_sub_lang_th(!window.get_sub_lang_th()),
        4 => window.set_sub_lang_vi(!window.get_sub_lang_vi()),
        5 => window.set_sub_lang_id(!window.get_sub_lang_id()),
        6 => window.set_sub_lang_my(!window.get_sub_lang_my()),
        7 => window.set_sub_lang_zh(!window.get_sub_lang_zh()),
        8 => window.set_sub_lang_ja(!window.get_sub_lang_ja()),
        9 => window.set_sub_lang_ko(!window.get_sub_lang_ko()),
        10 => window.set_sub_lang_es(!window.get_sub_lang_es()),
        11 => window.set_sub_lang_fr(!window.get_sub_lang_fr()),
        12 => window.set_sub_lang_de(!window.get_sub_lang_de()),
        13 => window.set_sub_lang_ru(!window.get_sub_lang_ru()),
        14 => window.set_sub_lang_pt(!window.get_sub_lang_pt()),
        15 => window.set_sub_lang_ar(!window.get_sub_lang_ar()),
        _ => {}
    }

    let any_specific = window.get_sub_lang_en()
        || window.get_sub_lang_km()
        || window.get_sub_lang_th()
        || window.get_sub_lang_vi()
        || window.get_sub_lang_id()
        || window.get_sub_lang_my()
        || window.get_sub_lang_zh()
        || window.get_sub_lang_ja()
        || window.get_sub_lang_ko()
        || window.get_sub_lang_es()
        || window.get_sub_lang_fr()
        || window.get_sub_lang_de()
        || window.get_sub_lang_ru()
        || window.get_sub_lang_pt()
        || window.get_sub_lang_ar();

    if any_specific {
        window.set_sub_lang_all(false);
    } else {
        window.set_sub_lang_all(true);
    }

    update_sub_langs_summary(window);
}

fn get_selected_sub_langs(window: &AppWindow) -> String {
    if window.get_sub_lang_all() {
        return "all,-live_chat".to_string();
    }
    let mut patterns = Vec::new();
    if window.get_sub_lang_en() { patterns.push("en.*,en,und"); }
    if window.get_sub_lang_km() { patterns.push("km.*,km"); }
    if window.get_sub_lang_th() { patterns.push("th.*,th"); }
    if window.get_sub_lang_vi() { patterns.push("vi.*,vi"); }
    if window.get_sub_lang_id() { patterns.push("id.*,id,ms.*,ms"); }
    if window.get_sub_lang_my() { patterns.push("my.*,my"); }
    if window.get_sub_lang_zh() { patterns.push("zh.*,zh-Hans,zh-Hant"); }
    if window.get_sub_lang_ja() { patterns.push("ja.*,ja"); }
    if window.get_sub_lang_ko() { patterns.push("ko.*,ko"); }
    if window.get_sub_lang_es() { patterns.push("es.*,es"); }
    if window.get_sub_lang_fr() { patterns.push("fr.*,fr"); }
    if window.get_sub_lang_de() { patterns.push("de.*,de"); }
    if window.get_sub_lang_ru() { patterns.push("ru.*,ru"); }
    if window.get_sub_lang_pt() { patterns.push("pt.*,pt"); }
    if window.get_sub_lang_ar() { patterns.push("ar.*,ar"); }

    if patterns.is_empty() {
        "all,-live_chat".to_string()
    } else {
        patterns.join(",")
    }
}

fn get_selected_sub_langs_indices(window: &AppWindow) -> Vec<i32> {
    if window.get_sub_lang_all() {
        return vec![0];
    }
    let mut indices = Vec::new();
    if window.get_sub_lang_en() { indices.push(1); }
    if window.get_sub_lang_km() { indices.push(2); }
    if window.get_sub_lang_th() { indices.push(3); }
    if window.get_sub_lang_vi() { indices.push(4); }
    if window.get_sub_lang_id() { indices.push(5); }
    if window.get_sub_lang_my() { indices.push(6); }
    if window.get_sub_lang_zh() { indices.push(7); }
    if window.get_sub_lang_ja() { indices.push(8); }
    if window.get_sub_lang_ko() { indices.push(9); }
    if window.get_sub_lang_es() { indices.push(10); }
    if window.get_sub_lang_fr() { indices.push(11); }
    if window.get_sub_lang_de() { indices.push(12); }
    if window.get_sub_lang_ru() { indices.push(13); }
    if window.get_sub_lang_pt() { indices.push(14); }
    if window.get_sub_lang_ar() { indices.push(15); }
    if indices.is_empty() {
        vec![0]
    } else {
        indices
    }
}

    // Callback: Start Download (add to queue)
    let window_weak_dl = main_window.as_weak();
    let meta_clone_dl = current_metadata.clone();
    let mgr_clone_dl = download_manager.clone();
    let current_dir_dl = current_download_dir.clone();

    main_window.on_start_download(move |format_idx, download_subs, download_thumb, audio_fmt_idx, audio_br_idx, embed_meta, _legacy_sub_lang_idx| {
        let (is_audio_only, quality) = map_format_index(format_idx);
        let audio_format = if is_audio_only { Some(map_audio_format(audio_fmt_idx).to_string()) } else { None };
        let audio_bitrate = if is_audio_only { Some(map_audio_bitrate(audio_br_idx).to_string()) } else { None };
        let sub_lang = if download_subs {
            window_weak_dl.upgrade().map(|win| get_selected_sub_langs(&win))
        } else {
            None
        };

        info!(
            "User triggered 'Start Download' with format_idx: {} (audio: {}, format: {:?}, bitrate: {:?}, embed_meta: {}, subs: {}, sub_lang: {:?}, thumb: {})",
            format_idx, is_audio_only, audio_format, audio_bitrate, embed_meta, download_subs, sub_lang, download_thumb
        );

        let selected_indices: Option<HashSet<usize>> = window_weak_dl.upgrade().map(|win| {
            let model = win.get_playlist_episodes();
            (0..model.row_count())
                .filter_map(|i| {
                    model.row_data(i).and_then(|ep| if ep.selected { Some(i) } else { None })
                })
                .collect()
        });

        let weak = window_weak_dl.clone();
        let meta_arc = meta_clone_dl.clone();
        let mgr = mgr_clone_dl.clone();
        let dir_lock = current_dir_dl.clone();
        let audio_fmt_clone = audio_format.clone();
        let audio_br_clone = audio_bitrate.clone();

        // Instant visual response: switch to Downloads view immediately upon click
        if let Some(win) = window_weak_dl.upgrade() {
            win.set_status_message("Starting download...".into());
            win.set_active_tab(1);
        }

        tokio::spawn(async move {
            let maybe_meta = meta_arc.lock().await.clone();
            let metadata = match maybe_meta {
                Some(m) => m,
                None => {
                    let _ = weak.upgrade_in_event_loop(|window| {
                        window.set_status_message("No media analyzed yet.".into());
                    });
                    return;
                }
            };

            let download_dir = dir_lock.read().await.clone();

            // Preserve user's download preferences for subtitles and thumbnails
            // Even if the series index page did not list subtitles, individual episodes (e.g. Dailymotion) have subtitles.
            let download_thumb = download_thumb;
            let download_subs = download_subs;

            // If album/series playlist, queue selected episodes
            if metadata.is_playlist {
                let series_title = metadata.title.clone();
                let mut queued_count = 0;
                for (idx, entry) in metadata.playlist_entries.into_iter().enumerate() {
                    if let Some(ref sel) = selected_indices {
                        if !sel.contains(&idx) {
                            continue;
                        }
                    }
                    let entry_url = crate::downloader::extractor::get_cached_playable_url(&entry.url)
                        .unwrap_or(entry.url);
                    let entry_referer = if crate::downloader::extractor::is_streaming_platform(&entry_url) {
                        None
                    } else {
                        entry.referer.or_else(|| metadata.referer.clone())
                    };
                    if mgr
                        .add_download_with_context(
                            entry_url,
                            entry.title,
                            &download_dir,
                            None,
                            true,
                            is_audio_only,
                            quality.clone(),
                            download_subs,
                            sub_lang.clone(),
                            download_thumb,
                            metadata.thumbnail_url.clone(),
                            audio_fmt_clone.clone(),
                            audio_br_clone.clone(),
                            embed_meta,
                            Some(&series_title),
                            Some(idx + 1),
                            entry_referer,
                        )
                        .await
                        .is_ok()
                    {
                        queued_count += 1;
                    }
                }
                info!("Queued {} series episodes for download", queued_count);
                let msg = format!("Queued {} episodes for download", queued_count);
                let _ = weak.upgrade_in_event_loop(move |window| {
                    window.set_status_message(msg.into());
                });
                return;
            }

            let is_extractor = metadata.is_extractor;
            match mgr
                .add_download_with_context(
                    metadata.url,
                    metadata.title,
                    &download_dir,
                    metadata.content_length,
                    is_extractor,
                    is_audio_only,
                    quality,
                    download_subs,
                    sub_lang,
                    download_thumb,
                    metadata.thumbnail_url.clone(),
                    audio_fmt_clone,
                    audio_br_clone,
                    embed_meta,
                    None,
                    None,
                    metadata.referer.clone(),
                )
                .await
            {
                Ok(id) => {
                    info!("Download queued with id: {}", id);
                    let _ = weak.upgrade_in_event_loop(move |window| {
                        window.set_status_message("Download started".into());
                    });
                }
                Err(err) => {
                    error!("Failed to queue download: {}", err);
                    let err_msg = format!("Failed to queue download: {}", err);
                    let err_box = err_msg.clone();
                    let _ = weak.upgrade_in_event_loop(move |window| {
                        window.set_has_error(true);
                        window.set_error_message(err_box.into());
                        window.set_status_message(err_msg.into());
                    });
                }
            }
        });
    });

    // Callback: Start Batch Downloads
    let window_weak_batch = main_window.as_weak();
    let mgr_clone_batch = download_manager.clone();
    let current_dir_batch = current_download_dir.clone();

    main_window.on_start_batch_download(move |raw_text, format_idx, download_thumb, audio_fmt_idx, audio_br_idx, embed_meta, _legacy_sub_lang_idx| {
        let text_val = raw_text.to_string();
        let (is_audio_only, quality) = map_format_index(format_idx);
        let audio_format = if is_audio_only { Some(map_audio_format(audio_fmt_idx).to_string()) } else { None };
        let audio_bitrate = if is_audio_only { Some(map_audio_bitrate(audio_br_idx).to_string()) } else { None };
        let sub_lang = window_weak_batch.upgrade().map(|win| get_selected_sub_langs(&win));
        let weak = window_weak_batch.clone();
        let mgr = mgr_clone_batch.clone();
        let dir_lock = current_dir_batch.clone();

        tokio::spawn(async move {
            let parsed_links = filesystem::parse_links_from_text(&text_val);
            let mut normalized_links = Vec::new();
            let mut seen = std::collections::HashSet::new();

            for raw_url in parsed_links {
                let norm = downloader::normalize_media_url(&raw_url);
                if seen.insert(norm.clone()) {
                    normalized_links.push(norm);
                }
            }

            if normalized_links.is_empty() {
                let _ = weak.upgrade_in_event_loop(|window| {
                    window.set_has_error(true);
                    window.set_error_message(
                        "No valid URLs found in batch input (each URL must start with http:// or https://)".into(),
                    );
                });
                return;
            }

            let download_dir = dir_lock.read().await.clone();
            let mut queued_count = 0;
            let total = normalized_links.len();

            let _ = weak.upgrade_in_event_loop(move |win| {
                win.set_status_message(format!("Enqueuing {} batch download(s)...", total).into());
            });

            for url in normalized_links {
                let is_extractor = downloader::is_streaming_platform(&url);
                let (title, referer) = resolve_batch_item_title_and_referer(&url);

                if let Ok(_) = mgr
                    .add_download_with_context(
                        url,
                        title,
                        &download_dir,
                        None,
                        is_extractor,
                        is_audio_only,
                        quality.clone(),
                        true,
                        sub_lang.clone(),
                        download_thumb,
                        None,
                        audio_format.clone(),
                        audio_bitrate.clone(),
                        embed_meta,
                        None,
                        None,
                        referer,
                    )
                    .await
                {
                    queued_count += 1;
                }
            }

            info!("Batch queue completed: {} URLs queued", queued_count);
            let msg = format!("Queued {} batch download(s)", queued_count);
            let _ = weak.upgrade_in_event_loop(move |window| {
                window.set_status_message(msg.into());
                window.set_batch_urls_text("".into());
                window.set_active_tab(1); // Switch to Downloads view
            });
        });
    });

    // Callback: Pause download
    let mgr_pause = download_manager.clone();
    main_window.on_pause_download(move |id_str| {
        let mgr = mgr_pause.clone();
        let id_val = id_str.to_string();
        tokio::spawn(async move {
            if let Ok(id) = Uuid::parse_str(&id_val) {
                mgr.pause_job(id).await;
            }
        });
    });

    // Callback: Resume download
    let mgr_resume = download_manager.clone();
    main_window.on_resume_download(move |id_str| {
        let mgr = mgr_resume.clone();
        let id_val = id_str.to_string();
        tokio::spawn(async move {
            if let Ok(id) = Uuid::parse_str(&id_val) {
                mgr.resume_job(id).await;
            }
        });
    });

    // Callback: Cancel download
    let mgr_cancel = download_manager.clone();
    main_window.on_cancel_download(move |id_str| {
        let mgr = mgr_cancel.clone();
        let id_val = id_str.to_string();
        tokio::spawn(async move {
            if let Ok(id) = Uuid::parse_str(&id_val) {
                mgr.cancel_job(id).await;
            }
        });
    });

    // Callback: Pause all
    let mgr_pause_all = download_manager.clone();
    main_window.on_pause_all(move || {
        let mgr = mgr_pause_all.clone();
        tokio::spawn(async move {
            mgr.pause_all().await;
        });
    });

    // Callback: Resume all
    let mgr_resume_all = download_manager.clone();
    main_window.on_resume_all(move || {
        let mgr = mgr_resume_all.clone();
        tokio::spawn(async move {
            mgr.resume_all().await;
        });
    });

    // Callback: Clear completed
    let mgr_clear = download_manager.clone();
    main_window.on_clear_completed(move || {
        let mgr = mgr_clear.clone();
        tokio::spawn(async move {
            mgr.clear_completed().await;
        });
    });

    // Callback: Clear failed
    let mgr_clear_failed = download_manager.clone();
    main_window.on_clear_failed(move || {
        let mgr = mgr_clear_failed.clone();
        tokio::spawn(async move {
            mgr.clear_failed().await;
        });
    });

    // Callback: Cancel all active/queued downloads
    let mgr_cancel_all = download_manager.clone();
    main_window.on_cancel_all(move || {
        let mgr = mgr_cancel_all.clone();
        tokio::spawn(async move {
            mgr.cancel_all().await;
        });
    });

    // Callback: Move job up in queue
    let mgr_move_up = download_manager.clone();
    main_window.on_move_job_up(move |id_str| {
        if let Ok(id) = Uuid::parse_str(&id_str) {
            let mgr = mgr_move_up.clone();
            tokio::spawn(async move {
                mgr.move_job_up(id).await;
            });
        }
    });

    // Callback: Move job down in queue
    let mgr_move_down = download_manager.clone();
    main_window.on_move_job_down(move |id_str| {
        if let Ok(id) = Uuid::parse_str(&id_str) {
            let mgr = mgr_move_down.clone();
            tokio::spawn(async move {
                mgr.move_job_down(id).await;
            });
        }
    });

    // Callback: Prioritize job to top of queue
    let mgr_prio = download_manager.clone();
    main_window.on_prioritize_job(move |id_str| {
        if let Ok(id) = Uuid::parse_str(&id_str) {
            let mgr = mgr_prio.clone();
            tokio::spawn(async move {
                mgr.prioritize_job(id).await;
            });
        }
    });

    // Callback: Set Queue Status Filter
    let filter_set = queue_filter_idx.clone();
    let mgr_set_filter = download_manager.clone();
    main_window.on_set_queue_filter(move |idx| {
        let filter_set = filter_set.clone();
        let mgr = mgr_set_filter.clone();
        tokio::spawn(async move {
            *filter_set.write().await = idx;
            mgr.notify_update().await;
        });
    });

    // Callback: Search Queue Items
    let search_set = queue_search_term.clone();
    let mgr_set_search = download_manager.clone();
    main_window.on_search_queue(move |query| {
        let search_set = search_set.clone();
        let mgr = mgr_set_search.clone();
        let q = query.to_string();
        tokio::spawn(async move {
            *search_set.write().await = q;
            mgr.notify_update().await;
        });
    });

    // --- History callbacks ---

    // Callback: Search history
    let db_search = db.clone();
    let weak_search = main_window.as_weak();
    let state_search = history_state.clone();
    main_window.on_search_history(move |query| {
        let db = db_search.clone();
        let weak = weak_search.clone();
        let state = state_search.clone();
        let q = query.to_string();
        tokio::spawn(async move {
            {
                let mut guard = state.write().await;
                guard.search_query = q;
            }
            refresh_history(&db, &state, weak).await;
        });
    });

    // Callback: Refresh history tab on navigation
    let db_hist_nav = db.clone();
    let weak_hist_nav = main_window.as_weak();
    let state_hist_nav = history_state.clone();
    main_window.on_refresh_history_tab(move || {
        let db = db_hist_nav.clone();
        let weak = weak_hist_nav.clone();
        let state = state_hist_nav.clone();
        tokio::spawn(async move {
            refresh_history(&db, &state, weak).await;
        });
    });

    // Callback: Set history status filter (0: All, 1: Completed, 2: Failed)
    let db_status_filter = db.clone();
    let weak_status_filter = main_window.as_weak();
    let state_status_filter = history_state.clone();
    main_window.on_set_history_status_filter(move |status_idx| {
        let db = db_status_filter.clone();
        let weak = weak_status_filter.clone();
        let state = state_status_filter.clone();
        tokio::spawn(async move {
            {
                let mut guard = state.write().await;
                guard.status_filter = status_idx;
            }
            refresh_history(&db, &state, weak).await;
        });
    });

    // Callback: Set history domain filter
    let db_domain_filter = db.clone();
    let weak_domain_filter = main_window.as_weak();
    let state_domain_filter = history_state.clone();
    main_window.on_set_history_domain_filter(move |domain_idx| {
        let db = db_domain_filter.clone();
        let weak = weak_domain_filter.clone();
        let state = state_domain_filter.clone();
        tokio::spawn(async move {
            {
                let mut guard = state.write().await;
                guard.domain_filter = domain_idx;
            }
            refresh_history(&db, &state, weak).await;
        });
    });

    // Callback: Toggle history item selection
    let db_toggle = db.clone();
    let weak_toggle = main_window.as_weak();
    let state_toggle = history_state.clone();
    main_window.on_toggle_history_item_selected(move |id_str| {
        let db = db_toggle.clone();
        let weak = weak_toggle.clone();
        let state = state_toggle.clone();
        let id_val = id_str.to_string();
        tokio::spawn(async move {
            if let Ok(id) = Uuid::parse_str(&id_val) {
                {
                    let mut guard = state.write().await;
                    if guard.selected_ids.contains(&id) {
                        guard.selected_ids.remove(&id);
                    } else {
                        guard.selected_ids.insert(id);
                    }
                }
                refresh_history(&db, &state, weak).await;
            }
        });
    });

    // Callback: Select all / Deselect all history items
    let db_sel_all = db.clone();
    let weak_sel_all = main_window.as_weak();
    let state_sel_all = history_state.clone();
    main_window.on_select_all_history(move |select_all| {
        let db = db_sel_all.clone();
        let weak = weak_sel_all.clone();
        let state = state_sel_all.clone();
        tokio::spawn(async move {
            if select_all {
                if let Ok(records) = db.get_history(None).await {
                    let guard = state.read().await;
                    let status_filter = guard.status_filter;
                    let domain_filter = guard.domain_filter;
                    let search_query = guard.search_query.clone();
                    drop(guard);

                    let matching_ids: Vec<Uuid> = records
                        .iter()
                        .filter(|r| {
                            let status_ok = match status_filter {
                                1 => r.status == "Completed",
                                2 => r.status == "Failed" || r.status == "Cancelled",
                                _ => true,
                            };
                            if !status_ok { return false; }
                            let domain = extract_domain_from_url(&r.url);
                            let domain_ok = match domain_filter {
                                1 => domain.contains("youtube") || domain.contains("youtu.be"),
                                2 => domain.contains("kisskh"),
                                3 => domain.contains("tiktok"),
                                4 => domain.contains("douyin"),
                                5 => domain.contains("facebook") || domain.contains("fb.watch"),
                                6 => !domain.contains("youtube") && !domain.contains("youtu.be")
                                     && !domain.contains("kisskh") && !domain.contains("tiktok")
                                     && !domain.contains("douyin") && !domain.contains("facebook")
                                     && !domain.contains("fb.watch"),
                                _ => true,
                            };
                            if !domain_ok { return false; }
                            if !search_query.is_empty() {
                                let q = search_query.to_lowercase();
                                if !r.title.to_lowercase().contains(&q)
                                    && !r.filename.to_lowercase().contains(&q)
                                    && !r.output_path.to_lowercase().contains(&q)
                                    && !r.url.to_lowercase().contains(&q) {
                                    return false;
                                }
                            }
                            true
                        })
                        .map(|r| r.id)
                        .collect();

                    let mut guard = state.write().await;
                    for id in matching_ids {
                        guard.selected_ids.insert(id);
                    }
                }
            } else {
                let mut guard = state.write().await;
                guard.selected_ids.clear();
            }
            refresh_history(&db, &state, weak).await;
        });
    });

    // Callback: Delete selected history items
    let db_del_sel = db.clone();
    let weak_del_sel = main_window.as_weak();
    let state_del_sel = history_state.clone();
    main_window.on_delete_selected_history(move || {
        let db = db_del_sel.clone();
        let weak = weak_del_sel.clone();
        let state = state_del_sel.clone();
        tokio::spawn(async move {
            let ids_to_delete: Vec<Uuid> = {
                let guard = state.read().await;
                guard.selected_ids.iter().cloned().collect()
            };
            let count = ids_to_delete.len();
            for id in ids_to_delete {
                let _ = db.delete_record(id).await;
            }
            {
                let mut guard = state.write().await;
                guard.selected_ids.clear();
            }
            refresh_history(&db, &state, weak.clone()).await;
            let _ = weak.upgrade_in_event_loop(move |window| {
                window.set_status_message(format!("Deleted {} history item(s)", count).into());
            });
        });
    });

    // Callback: Redownload selected history items
    let db_redl_sel = db.clone();
    let mgr_redl_sel = download_manager.clone();
    let weak_redl_sel = main_window.as_weak();
    let state_redl_sel = history_state.clone();
    let current_dir_redl_sel = current_download_dir.clone();
    main_window.on_redownload_selected_history(move || {
        let db = db_redl_sel.clone();
        let mgr = mgr_redl_sel.clone();
        let weak = weak_redl_sel.clone();
        let state = state_redl_sel.clone();
        let dir_lock = current_dir_redl_sel.clone();
        tokio::spawn(async move {
            let selected_set: HashSet<Uuid> = {
                let guard = state.read().await;
                guard.selected_ids.clone()
            };
            if selected_set.is_empty() {
                return;
            }
            let download_dir = dir_lock.read().await.clone();
            let mut queued_count = 0;
            if let Ok(records) = db.get_history(None).await {
                for r in records {
                    if selected_set.contains(&r.id) {
                        let is_extractor = downloader::is_streaming_platform(&r.url);
                        if let Ok(_) = mgr.add_download(
                            r.url,
                            r.title,
                            &download_dir,
                            None,
                            is_extractor,
                            false,
                            None,
                            true,
                            None,
                            true,
                            None,
                            None,
                            None,
                            false,
                        ).await {
                            queued_count += 1;
                        }
                    }
                }
            }
            {
                let mut guard = state.write().await;
                guard.selected_ids.clear();
            }
            refresh_history(&db, &state, weak.clone()).await;
            let _ = weak.upgrade_in_event_loop(move |window| {
                window.set_status_message(format!("Queued {} item(s) for redownload", queued_count).into());
                window.set_active_tab(1);
            });
        });
    });

    // Callback: Open folder for a history item (Reveal in Finder)
    main_window.on_open_history_folder(move |path_str| {
        let path = PathBuf::from(path_str.to_string());
        let _ = filesystem::reveal_in_file_manager(&path);
    });

    // Callback: Play media file for a history item
    main_window.on_play_history_file(move |path_str| {
        let path = PathBuf::from(path_str.to_string());
        let _ = filesystem::play_media_file(&path);
    });

    // Callback: Open/play file for a completed download
    main_window.on_open_download_file(move |path_str| {
        let path = PathBuf::from(path_str.to_string());
        let _ = filesystem::play_media_file(&path);
    });

    // Callback: Reveal completed download in Finder
    main_window.on_open_download_folder(move |path_str| {
        let path = PathBuf::from(path_str.to_string());
        let _ = filesystem::reveal_in_file_manager(&path);
    });

    // Callback: Redownload from history
    let mgr_redl = download_manager.clone();
    let weak_redl = main_window.as_weak();
    let current_dir_redl = current_download_dir.clone();
    main_window.on_redownload_history(move |url_str| {
        let mgr = mgr_redl.clone();
        let weak = weak_redl.clone();
        let dir_lock = current_dir_redl.clone();
        let url = url_str.to_string();
        tokio::spawn(async move {
            let download_dir = dir_lock.read().await.clone();
            let title = url
                .split('/')
                .last()
                .and_then(|s| s.split('?').next())
                .filter(|s| !s.is_empty())
                .unwrap_or("redownload")
                .to_string();

            let is_extractor = downloader::is_streaming_platform(&url);
            match mgr
                .add_download(
                    url,
                    title,
                    &download_dir,
                    None,
                    is_extractor,
                    false,
                    None,
                    true,
                    None,
                    true,
                    None,
                    None,
                    None,
                    false,
                )
                .await
            {
                Ok(id) => {
                    info!("Redownload queued with id: {}", id);
                    let _ = weak.upgrade_in_event_loop(move |window| {
                        window.set_status_message("Redownload queued".into());
                        window.set_active_tab(1);
                    });
                }
                Err(err) => {
                    let msg = format!("Redownload failed: {}", err);
                    let _ = weak.upgrade_in_event_loop(move |window| {
                        window.set_status_message(msg.into());
                    });
                }
            }
        });
    });

    // Callback: Delete single history record
    let db_del = db.clone();
    let weak_del = main_window.as_weak();
    let state_del = history_state.clone();
    main_window.on_delete_history_item(move |id_str| {
        let db = db_del.clone();
        let weak = weak_del.clone();
        let state = state_del.clone();
        let id_val = id_str.to_string();
        tokio::spawn(async move {
            if let Ok(id) = Uuid::parse_str(&id_val) {
                let _ = db.delete_record(id).await;
                {
                    let mut guard = state.write().await;
                    guard.selected_ids.remove(&id);
                }
                refresh_history(&db, &state, weak).await;
            }
        });
    });

    // Callback: Clear all history
    let db_clear_all = db.clone();
    let weak_clear_all = main_window.as_weak();
    let state_clear_all = history_state.clone();
    main_window.on_clear_all_history(move || {
        let db = db_clear_all.clone();
        let weak = weak_clear_all.clone();
        let state = state_clear_all.clone();
        tokio::spawn(async move {
            let _ = db.clear_all_history().await;
            {
                let mut guard = state.write().await;
                guard.selected_ids.clear();
            }
            refresh_history(&db, &state, weak).await;
        });
    });

    // Callback: Export History to CSV
    let db_export = db.clone();
    let weak_export = main_window.as_weak();
    main_window.on_export_history(move || {
        let db = db_export.clone();
        let weak = weak_export.clone();
        tokio::spawn(async move {
            let records = match db.get_history(None).await {
                Ok(r) if !r.is_empty() => r,
                Ok(_) => {
                    let _ = weak.upgrade_in_event_loop(|win| {
                        win.set_status_message("No history records to export".into());
                    });
                    return;
                }
                Err(err) => {
                    let _ = weak.upgrade_in_event_loop(move |win| {
                        win.set_status_message(format!("Failed to load history: {}", err).into());
                    });
                    return;
                }
            };

            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let default_name = format!("download_history_{}.csv", timestamp);

            if let Some(mut save_path) = filesystem::pick_save_file(&default_name, "Export Download History to CSV").await {
                if save_path.extension().is_none() {
                    save_path.set_extension("csv");
                }
                match filesystem::export_history_to_csv(&records, &save_path).await {
                    Ok(count) => {
                        let path_display = save_path.display().to_string();
                        info!("Exported {} history records to {}", count, path_display);
                        notifications::send_notification(
                            "History Exported",
                            "CSV Export Successful",
                            &format!("Successfully saved {} records to {}", count, path_display),
                            false,
                        );
                        let _ = weak.upgrade_in_event_loop(move |win| {
                            win.set_status_message(format!("Exported {} records to {}", count, path_display).into());
                        });
                    }
                    Err(err) => {
                        let err_msg = format!("Failed to export history: {}", err);
                        tracing::error!("{}", err_msg);
                        let _ = weak.upgrade_in_event_loop(move |win| {
                            win.set_status_message(err_msg.into());
                        });
                    }
                }
            }
        });
    });

    // Callback: Clear Logs
    let ui_logger_clear = ui_log_layer.clone();
    main_window.on_clear_logs(move || {
        ui_logger_clear.clear();
    });

    // Callback: Copy Logs to System Clipboard
    let ui_logger_copy = ui_log_layer.clone();
    let weak_copy = main_window.as_weak();
    let last_clip_logs = last_clipboard.clone();
    main_window.on_copy_logs(move || {
        let text = ui_logger_copy.get_formatted_logs();
        if !text.is_empty() {
            let count = text.lines().count();
            let success = filesystem::write_clipboard_text(&text);
            if success {
                if let Ok(mut last) = last_clip_logs.lock() {
                    *last = text.trim().to_string();
                }
                let _ = weak_copy.upgrade_in_event_loop(move |win| {
                    win.set_status_message(format!("Copied {} log entries to clipboard", count).into());
                });
            }
        }
    });

    // Callback: Filter Logs by Level
    let ui_logger_filter = ui_log_layer.clone();
    main_window.on_set_log_filter(move |level_idx| {
        ui_logger_filter.set_filter(level_idx);
    });

    // Callback: Search Logs Text
    let ui_logger_search = ui_log_layer.clone();
    main_window.on_search_logs(move |query| {
        ui_logger_search.set_search(query.to_string());
    });

    // Callback: Browse Download Directory
    let dir_browse = current_download_dir.clone();
    let db_browse = db.clone();
    let weak_browse = main_window.as_weak();
    main_window.on_browse_download_dir(move || {
        let dir_lock = dir_browse.clone();
        let db = db_browse.clone();
        let weak = weak_browse.clone();
        tokio::spawn(async move {
            if let Some(picked) = filesystem::pick_directory().await {
                let path_str = picked.to_string_lossy().to_string();
                *dir_lock.write().await = picked.clone();
                let _ = db.set_setting("download_dir", &path_str).await;
                info!("Updated download directory to: {}", path_str);
                let path_for_ui = path_str.clone();
                let _ = weak.upgrade_in_event_loop(move |window| {
                    window.set_download_dir_path(path_for_ui.into());
                    window.set_status_message(format!("Download directory: {}", path_str).into());
                });
            }
        });
    });

    // Callback: Reset Download Directory
    let dir_reset = current_download_dir.clone();
    let db_reset = db.clone();
    let weak_reset = main_window.as_weak();
    main_window.on_reset_download_dir(move || {
        let default_dir = filesystem::default_download_dir();
        let dir_lock = dir_reset.clone();
        let db = db_reset.clone();
        let weak = weak_reset.clone();
        tokio::spawn(async move {
            let path_str = default_dir.to_string_lossy().to_string();
            *dir_lock.write().await = default_dir.clone();
            let _ = db.set_setting("download_dir", &path_str).await;
            info!("Reset download directory to default: {}", path_str);
            let path_for_ui = path_str.clone();
            let _ = weak.upgrade_in_event_loop(move |window| {
                window.set_download_dir_path(path_for_ui.into());
                window.set_status_message("Reset download directory to default Downloads".into());
            });
        });
    });

    // Callback: Open Current Download Directory in Finder
    let dir_open_curr = current_download_dir.clone();
    main_window.on_open_current_download_dir(move || {
        let dir_lock = dir_open_curr.clone();
        tokio::spawn(async move {
            let current = dir_lock.read().await.clone();
            let _ = filesystem::reveal_in_file_manager(&current);
        });
    });

    // Callback: Set Bandwidth Speed Limit
    let db_speed = db.clone();
    let mgr_speed = download_manager.clone();
    let weak_speed = main_window.as_weak();
    main_window.on_set_speed_limit(move |idx| {
        let db = db_speed.clone();
        let mgr = mgr_speed.clone();
        let weak = weak_speed.clone();
        tokio::spawn(async move {
            let (limit_str, limit_opt, label) = match idx {
                1 => ("2M", Some("2M".to_string()), "2 MB/s"),
                2 => ("5M", Some("5M".to_string()), "5 MB/s"),
                3 => ("10M", Some("10M".to_string()), "10 MB/s"),
                4 => ("20M", Some("20M".to_string()), "20 MB/s"),
                _ => ("", None, "No Limit (Unlimited)"),
            };
            mgr.set_speed_limit(limit_opt).await;
            let _ = db.set_setting("speed_limit", limit_str).await;
            info!("Updated bandwidth speed limit: {}", label);
            let _ = weak.upgrade_in_event_loop(move |window| {
                window.set_selected_speed_limit_index(idx);
                window.set_status_message(format!("Download speed limit set to: {}", label).into());
            });
        });
    });

    // Callback: Set Custom Bandwidth Speed Limit
    let db_custom_speed = db.clone();
    let mgr_custom_speed = download_manager.clone();
    let weak_custom_speed = main_window.as_weak();
    main_window.on_set_custom_speed_limit(move |input| {
        let db = db_custom_speed.clone();
        let mgr = mgr_custom_speed.clone();
        let weak = weak_custom_speed.clone();
        let input_str = input.trim().to_string();
        tokio::spawn(async move {
            if let Some(bytes) = downloader::manager::parse_speed_limit_bytes(Some(&input_str)) {
                let formatted = DownloadProgress::format_speed_val(bytes as f64);
                mgr.set_speed_limit(Some(input_str.clone())).await;
                let _ = db.set_setting("speed_limit", &input_str).await;
                info!("Custom bandwidth throttling limit applied: {} ({})", input_str, formatted);
                let _ = weak.upgrade_in_event_loop(move |window| {
                    window.set_selected_speed_limit_index(5);
                    window.set_status_message(format!("Custom speed limit active: {}", formatted).into());
                });
            } else {
                let _ = weak.upgrade_in_event_loop(move |window| {
                    window.set_status_message("Invalid speed format (e.g. use 1.5M, 500K, 20M)".into());
                });
            }
        });
    });

    // Callback: Run Network Diagnostics & Stream Health Probe
    let mgr_diag = download_manager.clone();
    let weak_diag = main_window.as_weak();
    main_window.on_run_network_diagnostics(move |target| {
        let mgr = mgr_diag.clone();
        let weak = weak_diag.clone();
        let target_str = target.trim().to_string();
        let _ = weak.upgrade_in_event_loop(|win| {
            win.set_is_running_diagnostics(true);
            win.set_diag_summary_headline("Probing network & CDN edge...".into());
            win.set_diag_summary_details("Measuring DNS lookup latency, handshake ping RTT, and byte-range throughput...".into());
            win.set_diag_overall_status("Probing...".into());
        });
        tokio::spawn(async move {
            let report = mgr.run_network_diagnostics(&target_str).await;
            let _ = weak.upgrade_in_event_loop(move |win| {
                win.set_is_running_diagnostics(false);
                win.set_diag_dns_text(format!("{} ms", report.dns_ms).into());
                win.set_diag_dns_status(report.dns_status.into());
                win.set_diag_ping_text(format!("{} ms", report.rtt_ms).into());
                win.set_diag_ping_status(report.rtt_status.into());
                win.set_diag_range_text(if report.supports_range { "Yes (206)".into() } else { "No".into() });
                win.set_diag_range_status(report.range_status.into());
                win.set_diag_throughput_text(report.throughput_speed_text.into());
                win.set_diag_throughput_status(report.throughput_status.into());
                win.set_diag_summary_headline(report.summary_headline.into());
                win.set_diag_summary_details(report.summary_details.into());
                win.set_diag_overall_status(report.overall_status.into());
                win.set_status_message("Network diagnostic probe completed".into());
            });
        });
    });

    // Callback: Set Max Concurrent Download Workers
    let db_conc = db.clone();
    let mgr_conc = download_manager.clone();
    let weak_conc = main_window.as_weak();
    main_window.on_set_max_concurrency(move |limit| {
        let db = db_conc.clone();
        let mgr = mgr_conc.clone();
        let weak = weak_conc.clone();
        let count = (limit as usize).clamp(1, 10);
        tokio::spawn(async move {
            mgr.set_max_concurrency(count).await;
            let _ = db.set_setting("max_concurrency", &count.to_string()).await;
            info!("Updated concurrent worker limit to: {}", count);
            let _ = weak.upgrade_in_event_loop(move |window| {
                window.set_status_message(format!("Concurrent workers limit set to: {} parallel", count).into());
            });
        });
    });

    // Callback: Set Browser Cookies Authentication
    let db_cookies = db.clone();
    let mgr_cookies = download_manager.clone();
    let weak_cookies = main_window.as_weak();
    let cookies_lock = current_cookies_browser.clone();
    main_window.on_set_cookies_browser(move |idx| {
        let db = db_cookies.clone();
        let mgr = mgr_cookies.clone();
        let weak = weak_cookies.clone();
        let lock = cookies_lock.clone();
        tokio::spawn(async move {
            let (setting_val, browser_opt, label) = match idx {
                1 => ("chrome", Some("chrome".to_string()), "Google Chrome account"),
                2 => ("firefox", Some("firefox".to_string()), "Mozilla Firefox account"),
                3 => ("safari", Some("safari".to_string()), "Apple Safari account"),
                4 => ("brave", Some("brave".to_string()), "Brave Browser account"),
                5 => ("edge", Some("edge".to_string()), "Microsoft Edge account"),
                _ => ("", None, "Guest mode (Public access)"),
            };
            mgr.set_cookies_browser(browser_opt.clone()).await;
            *lock.write().await = browser_opt;
            let _ = db.set_setting("cookies_browser", setting_val).await;
            info!("Updated browser account login: {}", label);
            let _ = weak.upgrade_in_event_loop(move |window| {
                window.set_status_message(format!("Account login: {}", label).into());
            });
        });
    });

    // Callback: Set Network Proxy
    let db_proxy = db.clone();
    let mgr_proxy = download_manager.clone();
    let client_proxy = network_client.clone();
    let weak_proxy = main_window.as_weak();
    let proxy_lock = current_proxy.clone();
    main_window.on_set_proxy(move |enabled, proxy_str| {
        let db = db_proxy.clone();
        let mgr = mgr_proxy.clone();
        let client = client_proxy.clone();
        let weak = weak_proxy.clone();
        let lock = proxy_lock.clone();
        let p_trimmed = proxy_str.trim().to_string();

        tokio::spawn(async move {
            let proxy_opt = if enabled && !p_trimmed.is_empty() {
                Some(p_trimmed.clone())
            } else {
                None
            };

            mgr.set_proxy(proxy_opt.clone()).await;
            client.set_proxy(proxy_opt.clone());
            *lock.write().await = proxy_opt.clone();

            let _ = db.set_setting("proxy_enabled", if enabled { "true" } else { "false" }).await;
            let _ = db.set_setting("proxy_url", &p_trimmed).await;

            let _ = weak.upgrade_in_event_loop(move |window| {
                if let Some(ref p) = proxy_opt {
                    window.set_proxy_status_message(format!("Active Proxy: {}", p).into());
                    window.set_proxy_is_connected(true);
                    window.set_status_message(format!("Custom proxy enabled: {}", p).into());
                } else {
                    window.set_proxy_status_message("Direct connection (proxy disabled)".into());
                    window.set_proxy_is_connected(false);
                    window.set_status_message("Proxy disabled - using direct network connection".into());
                }
            });
        });
    });

    // Callback: Test Proxy Connectivity & Latency
    let weak_test_proxy = main_window.as_weak();
    main_window.on_test_proxy(move |proxy_str| {
        let weak = weak_test_proxy.clone();
        let proxy_url = proxy_str.trim().to_string();
        if proxy_url.is_empty() {
            return;
        }

        let _ = weak.upgrade_in_event_loop(|win| {
            win.set_is_testing_proxy(true);
            win.set_proxy_status_message("Testing proxy ping...".into());
        });

        tokio::spawn(async move {
            let res = NetworkClient::test_proxy_connection(&proxy_url).await;
            let _ = weak.upgrade_in_event_loop(move |win| {
                win.set_is_testing_proxy(false);
                match res {
                    Ok(latency_ms) => {
                        win.set_proxy_status_message(format!("Proxy Connected (Latency: {}ms)", latency_ms).into());
                        win.set_proxy_is_connected(true);
                        win.set_status_message(format!("Proxy test succeeded: {}ms round-trip latency", latency_ms).into());
                    }
                    Err(err) => {
                        win.set_proxy_status_message(format!("Connection Failed: {}", err).into());
                        win.set_proxy_is_connected(false);
                        win.set_status_message(format!("Proxy test failed: {}", err).into());
                    }
                }
            });
        });
    });

    // Callback: Test Desktop Notification
    let weak_test_notif = main_window.as_weak();
    main_window.on_test_notification(move || {
        notifications::send_test_notification();
        let _ = weak_test_notif.upgrade_in_event_loop(|win| {
            win.set_status_message("Sent test desktop notification".into());
        });
    });

    // Callback: Toggle Desktop Notifications
    let db_notif_en = db.clone();
    let weak_notif_en = main_window.as_weak();
    main_window.on_set_notifications_enabled(move |enabled| {
        notifications::set_notifications_enabled(enabled);
        let db = db_notif_en.clone();
        let weak = weak_notif_en.clone();
        tokio::spawn(async move {
            let _ = db.set_setting("notifications_enabled", if enabled { "true" } else { "false" }).await;
            let msg = if enabled { "Desktop notifications enabled" } else { "Desktop notifications disabled" };
            let _ = weak.upgrade_in_event_loop(move |win| {
                win.set_status_message(msg.into());
            });
        });
    });

    // Callback: Toggle Notification Audio Chime
    let db_sound_en = db.clone();
    let weak_sound_en = main_window.as_weak();
    main_window.on_set_notification_sound_enabled(move |enabled| {
        notifications::set_sound_enabled(enabled);
        let db = db_sound_en.clone();
        let weak = weak_sound_en.clone();
        tokio::spawn(async move {
            let _ = db.set_setting("notification_sound_enabled", if enabled { "true" } else { "false" }).await;
            let msg = if enabled { "Notification audio chimes enabled" } else { "Notification audio chimes muted" };
            let _ = weak.upgrade_in_event_loop(move |win| {
                win.set_status_message(msg.into());
            });
        });
    });

    // Callback: Dismiss Error Alert
    let weak_dismiss = main_window.as_weak();
    main_window.on_dismiss_error(move || {
        let _ = weak_dismiss.upgrade_in_event_loop(|win| {
            win.set_has_error(false);
            win.set_error_message("".into());
        });
    });

    // Callback: Check / Update yt-dlp Extractor Engine
    let weak_update = main_window.as_weak();
    main_window.on_update_extractor(move || {
        let weak = weak_update.clone();
        tokio::spawn(async move {
            let _ = weak.upgrade_in_event_loop(|win| {
                win.set_is_updating_ytdlp(true);
                win.set_binary_update_status("Checking for yt-dlp extractor engine updates...".into());
                win.set_status_message("Checking for yt-dlp extractor engine updates...".into());
            });
            match downloader::update_ytdlp_engine().await {
                Ok(msg) => {
                    info!("Extractor engine update: {}", msg);
                    let (y_ver, y_path) = downloader::get_ytdlp_info().await;
                    let _ = weak.upgrade_in_event_loop(move |win| {
                        win.set_ytdlp_version_text(y_ver.into());
                        win.set_ytdlp_path_text(y_path.into());
                        win.set_binary_update_status(format!("yt-dlp: {}", msg).into());
                        win.set_status_message(format!("Extractor engine: {}", msg).into());
                        win.set_is_updating_ytdlp(false);
                    });
                }
                Err(err) => {
                    warn!("Failed to update extractor engine: {}", err);
                    let err_str = err.to_string();
                    let (y_ver, y_path) = downloader::get_ytdlp_info().await;
                    let _ = weak.upgrade_in_event_loop(move |win| {
                        win.set_ytdlp_version_text(y_ver.into());
                        win.set_ytdlp_path_text(y_path.into());
                        win.set_binary_update_status(format!("Update failed: {}", err_str).into());
                        win.set_status_message(format!("Extractor update check: {}", err_str).into());
                        win.set_is_updating_ytdlp(false);
                    });
                }
            }
        });
    });

    // Callback: Reinstall / Upgrade FFmpeg
    let weak_ffmpeg = main_window.as_weak();
    main_window.on_reinstall_ffmpeg(move || {
        let weak = weak_ffmpeg.clone();
        tokio::spawn(async move {
            let _ = weak.upgrade_in_event_loop(|win| {
                win.set_is_updating_ffmpeg(true);
                win.set_binary_update_status("Downloading official static FFmpeg binary...".into());
                win.set_status_message("Downloading official static FFmpeg binary...".into());
            });
            match downloader::reinstall_ffmpeg().await {
                Ok(msg) => {
                    info!("FFmpeg installation: {}", msg);
                    let (f_ver, f_path) = downloader::get_ffmpeg_info().await;
                    let _ = weak.upgrade_in_event_loop(move |win| {
                        win.set_ffmpeg_version_text(f_ver.into());
                        win.set_ffmpeg_path_text(f_path.into());
                        win.set_binary_update_status("FFmpeg installed and verified successfully".into());
                        win.set_status_message(format!("FFmpeg: {}", msg).into());
                        win.set_is_updating_ffmpeg(false);
                    });
                }
                Err(err) => {
                    warn!("Failed to install FFmpeg: {}", err);
                    let err_str = err.to_string();
                    let (f_ver, f_path) = downloader::get_ffmpeg_info().await;
                    let _ = weak.upgrade_in_event_loop(move |win| {
                        win.set_ffmpeg_version_text(f_ver.into());
                        win.set_ffmpeg_path_text(f_path.into());
                        win.set_binary_update_status(format!("FFmpeg install failed: {}", err_str).into());
                        win.set_status_message(format!("FFmpeg install failed: {}", err_str).into());
                        win.set_is_updating_ffmpeg(false);
                    });
                }
            }
        });
    });

    // Callback: Update All Core Binaries
    let weak_all = main_window.as_weak();
    main_window.on_update_all_binaries(move || {
        let weak = weak_all.clone();
        tokio::spawn(async move {
            let _ = weak.upgrade_in_event_loop(|win| {
                win.set_is_updating_ytdlp(true);
                win.set_is_updating_ffmpeg(true);
                win.set_binary_update_status("Checking and updating all core binaries (yt-dlp + FFmpeg)...".into());
                win.set_status_message("Checking and updating all core binaries...".into());
            });
            match downloader::update_all_binaries().await {
                Ok(msg) => {
                    info!("All binaries updated: {}", msg);
                    let (y_ver, y_path) = downloader::get_ytdlp_info().await;
                    let (f_ver, f_path) = downloader::get_ffmpeg_info().await;
                    let _ = weak.upgrade_in_event_loop(move |win| {
                        win.set_ytdlp_version_text(y_ver.into());
                        win.set_ytdlp_path_text(y_path.into());
                        win.set_ffmpeg_version_text(f_ver.into());
                        win.set_ffmpeg_path_text(f_path.into());
                        win.set_binary_update_status("All core binaries updated and ready".into());
                        win.set_status_message(format!("Core binaries: {}", msg).into());
                        win.set_is_updating_ytdlp(false);
                        win.set_is_updating_ffmpeg(false);
                    });
                }
                Err(err) => {
                    warn!("Failed to update all binaries: {}", err);
                    let err_str = err.to_string();
                    let (y_ver, y_path) = downloader::get_ytdlp_info().await;
                    let (f_ver, f_path) = downloader::get_ffmpeg_info().await;
                    let _ = weak.upgrade_in_event_loop(move |win| {
                        win.set_ytdlp_version_text(y_ver.into());
                        win.set_ytdlp_path_text(y_path.into());
                        win.set_ffmpeg_version_text(f_ver.into());
                        win.set_ffmpeg_path_text(f_path.into());
                        win.set_binary_update_status(format!("Update failed: {}", err_str).into());
                        win.set_status_message(format!("Binary update error: {}", err_str).into());
                        win.set_is_updating_ytdlp(false);
                        win.set_is_updating_ffmpeg(false);
                    });
                }
            }
        });
    });

    // Callback: Refresh Binary Information
    let weak_refresh = main_window.as_weak();
    main_window.on_refresh_binary_info(move || {
        let weak = weak_refresh.clone();
        tokio::spawn(async move {
            let (y_ver, y_path) = downloader::get_ytdlp_info().await;
            let (f_ver, f_path) = downloader::get_ffmpeg_info().await;
            let _ = weak.upgrade_in_event_loop(move |win| {
                win.set_ytdlp_version_text(y_ver.into());
                win.set_ytdlp_path_text(y_path.into());
                win.set_ffmpeg_version_text(f_ver.into());
                win.set_ffmpeg_path_text(f_path.into());
            });
        });
    });

    // Callback: Clear all application & engine caches
    let weak_clear = main_window.as_weak();
    main_window.on_clear_cache(move || {
        let weak = weak_clear.clone();
        tokio::spawn(async move {
            let _ = weak.upgrade_in_event_loop(|win| {
                win.set_status_message("Purging all cookies, URL cache, and temporary media files...".into());
            });
            match downloader::clear_all_caches().await {
                Ok(msg) => {
                    info!("Clear all caches success: {}", msg);
                    let _ = weak.upgrade_in_event_loop(move |win| {
                        win.set_status_message(format!("Cache purged: {}", msg).into());
                    });
                }
                Err(err) => {
                    warn!("Failed to clear caches: {}", err);
                    let err_str = err.to_string();
                    let _ = weak.upgrade_in_event_loop(move |win| {
                        win.set_status_message(format!("Cache purge error: {}", err_str).into());
                    });
                }
            }
        });
    });

    // Callback: Bypass Off-Peak Scheduler and start all waiting downloads immediately
    let mgr_bypass = download_manager.clone();
    let weak_bypass = main_window.as_weak();
    main_window.on_bypass_scheduler_now(move || {
        let mgr = mgr_bypass.clone();
        let weak = weak_bypass.clone();
        tokio::spawn(async move {
            mgr.bypass_scheduler_and_start_all().await;
            let _ = weak.upgrade_in_event_loop(|win| {
                win.set_status_message("Bypassed off-peak scheduler: Starting all queued downloads now".into());
                win.set_scheduler_active(false);
            });
        });
    });

    // Callback: Set Off-Peak Scheduler Enabled / Disabled
    let db_sched = db.clone();
    let mgr_sched = download_manager.clone();
    let weak_sched = main_window.as_weak();
    main_window.on_set_scheduler_enabled(move |enabled| {
        let db = db_sched.clone();
        let mgr = mgr_sched.clone();
        let weak = weak_sched.clone();
        tokio::spawn(async move {
            let (_, sh, sm, eh, em) = mgr.get_scheduler_config().await;
            mgr.set_scheduler_config(enabled, sh, sm, eh, em).await;
            let _ = db.set_setting("scheduler_enabled", if enabled { "true" } else { "false" }).await;
            let in_win = mgr.is_in_schedule_window().await;
            if enabled && in_win {
                mgr.process_queue().await;
            }
            mgr.notify_update().await;
            let _ = weak.upgrade_in_event_loop(move |win| {
                win.set_status_message(if enabled {
                    format!("Off-peak scheduler enabled: {:02}:{:02} – {:02}:{:02}", sh, sm, eh, em).into()
                } else {
                    "Off-peak scheduler disabled: full speed downloads anytime".into()
                });
            });
        });
    });

    // Callback: Set Scheduler Start Hour & Minute
    let db_start_time = db.clone();
    let mgr_start_time = download_manager.clone();
    let weak_start_time = main_window.as_weak();
    main_window.on_set_scheduler_start_time(move |h, m| {
        let db = db_start_time.clone();
        let mgr = mgr_start_time.clone();
        let weak = weak_start_time.clone();
        tokio::spawn(async move {
            let (en, _, _, eh, em) = mgr.get_scheduler_config().await;
            let h = (h as u32).min(23);
            let m = (m as u32).min(59);
            mgr.set_scheduler_config(en, h, m, eh, em).await;
            let _ = db.set_setting("scheduler_start_hour", &h.to_string()).await;
            let _ = db.set_setting("scheduler_start_minute", &m.to_string()).await;
            let status = format!("Off-Peak Window: {:02}:{:02} – {:02}:{:02}", h, m, eh, em);
            let _ = weak.upgrade_in_event_loop(move |win| {
                win.set_scheduler_window_status(status.into());
            });
            mgr.notify_update().await;
        });
    });

    // Callback: Set Scheduler End Hour & Minute
    let db_end_time = db.clone();
    let mgr_end_time = download_manager.clone();
    let weak_end_time = main_window.as_weak();
    main_window.on_set_scheduler_end_time(move |h, m| {
        let db = db_end_time.clone();
        let mgr = mgr_end_time.clone();
        let weak = weak_end_time.clone();
        tokio::spawn(async move {
            let (en, sh, sm, _, _) = mgr.get_scheduler_config().await;
            let h = (h as u32).min(23);
            let m = (m as u32).min(59);
            mgr.set_scheduler_config(en, sh, sm, h, m).await;
            let _ = db.set_setting("scheduler_end_hour", &h.to_string()).await;
            let _ = db.set_setting("scheduler_end_minute", &m.to_string()).await;
            let status = format!("Off-Peak Window: {:02}:{:02} – {:02}:{:02}", sh, sm, h, m);
            let _ = weak.upgrade_in_event_loop(move |win| {
                win.set_scheduler_window_status(status.into());
            });
            mgr.notify_update().await;
        });
    });

    // Callback: Set Auto-Retry Enabled / Disabled
    let db_auto_retry = db.clone();
    let mgr_auto_retry = download_manager.clone();
    let weak_auto_retry = main_window.as_weak();
    main_window.on_set_auto_retry_enabled(move |enabled| {
        let db = db_auto_retry.clone();
        let mgr = mgr_auto_retry.clone();
        let weak = weak_auto_retry.clone();
        tokio::spawn(async move {
            let (_, interval, max_a) = mgr.get_auto_retry_config().await;
            mgr.set_auto_retry_config(enabled, interval, max_a).await;
            let _ = db.set_setting("auto_retry_enabled", if enabled { "true" } else { "false" }).await;
            let _ = weak.upgrade_in_event_loop(move |win| {
                win.set_status_message(if enabled {
                    "Auto-retry enabled: failed downloads will automatically restart".into()
                } else {
                    "Auto-retry disabled".into()
                });
            });
        });
    });

    // Callback: Set Auto-Retry Delay Interval
    let db_retry_int = db.clone();
    let mgr_retry_int = download_manager.clone();
    let weak_retry_int = main_window.as_weak();
    main_window.on_set_auto_retry_interval(move |idx| {
        let db = db_retry_int.clone();
        let mgr = mgr_retry_int.clone();
        let weak = weak_retry_int.clone();
        tokio::spawn(async move {
            let (en, _, max_a) = mgr.get_auto_retry_config().await;
            let secs = match idx {
                0 => 30,
                1 => 60,
                2 => 120,
                3 => 300,
                _ => 60,
            };
            mgr.set_auto_retry_config(en, secs, max_a).await;
            let _ = db.set_setting("auto_retry_interval_secs", &secs.to_string()).await;
            let label = match idx {
                0 => "30 seconds",
                1 => "1 minute",
                2 => "2 minutes",
                3 => "5 minutes",
                _ => "1 minute",
            };
            let _ = weak.upgrade_in_event_loop(move |win| {
                win.set_status_message(format!("Auto-retry countdown delay set to: {}", label).into());
            });
        });
    });

    // Callback: Set Auto-Retry Maximum Attempts
    let db_retry_max = db.clone();
    let mgr_retry_max = download_manager.clone();
    let weak_retry_max = main_window.as_weak();
    main_window.on_set_auto_retry_max(move |idx| {
        let db = db_retry_max.clone();
        let mgr = mgr_retry_max.clone();
        let weak = weak_retry_max.clone();
        tokio::spawn(async move {
            let (en, interval, _) = mgr.get_auto_retry_config().await;
            let max_a = match idx {
                0 => 3,
                1 => 5,
                2 => 10,
                _ => 3,
            };
            mgr.set_auto_retry_config(en, interval, max_a).await;
            let _ = db.set_setting("auto_retry_max_attempts", &max_a.to_string()).await;
            let _ = weak.upgrade_in_event_loop(move |win| {
                win.set_status_message(format!("Auto-retry maximum attempts set to: {} times", max_a).into());
            });
        });
    });

    // Load initial history on startup
    let db_init_hist = db.clone();
    let weak_init_hist = main_window.as_weak();
    let hist_state_init = history_state.clone();
    tokio::spawn(async move {
        refresh_history(&db_init_hist, &hist_state_init, weak_init_hist).await;
    });

    // Background streaming dependencies verification & silent auto-update check on startup
    let weak_bg_status = main_window.as_weak();
    tokio::spawn(async move {
        // 1. Ensure external engines (yt-dlp and ffmpeg) are installed and available
        match downloader::ensure_dependencies().await {
            Ok((ytdlp, ffmpeg)) => {
                info!("Verified streaming dependencies: yt-dlp at {:?}, ffmpeg at {:?}", ytdlp, ffmpeg);
            }
            Err(err) => {
                warn!("Dependency validation warning on startup: {}", err);
            }
        }

        // 2. Perform silent background auto-update check for yt-dlp
        match downloader::update_ytdlp_engine().await {
            Ok(msg) => {
                info!("Background extractor auto-update check: {}", msg);
                if msg.contains("Updated") || msg.contains("Updating to") {
                    let _ = weak_bg_status.upgrade_in_event_loop(move |win| {
                        win.set_status_message(format!("Extractor updated: {}", msg).into());
                    });
                }
            }
            Err(err) => {
                info!("Background extractor update check deferred: {}", err);
            }
        }
    });

    // Auto-detect media URL from clipboard on app launch
    if let Some(text) = filesystem::read_clipboard_text() {
        if text.starts_with("http://") || text.starts_with("https://") {
            info!("Auto-detected URL in clipboard on startup: {}", text);
            main_window.set_input_url_text(text.clone().into());
            main_window.set_status_message(format!("Auto-detected URL in clipboard: {}", text).into());
        }
    }

    // Run Slint native event loop
    info!("Launching native Slint window");
    main_window.run()?;

    info!("Native Video Downloader terminated normally");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_domain_from_url() {
        assert_eq!(extract_domain_from_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ"), "youtube.com");
        assert_eq!(extract_domain_from_url("https://youtu.be/dQw4w9WgXcQ"), "youtube.com");
        assert_eq!(extract_domain_from_url("https://kisskh.do/Drama/Spring-of-the-Blade--2026-"), "kisskh.do");
        assert_eq!(extract_domain_from_url("https://www.tiktok.com/@user/video/1234567"), "tiktok.com");
        assert_eq!(extract_domain_from_url("https://www.douyin.com/video/71234567890"), "douyin.com");
        assert_eq!(extract_domain_from_url("https://www.facebook.com/watch/?v=123"), "facebook.com");
    }

    #[test]
    fn test_resolve_batch_item_title_and_referer() {
        let (title, referer) = resolve_batch_item_title_and_referer("https://kisskh.do/Drama/Spring-of-the-Blade--2026-/Episode-1?id=13742&ep=224512");
        assert!(title.contains("Spring of the Blade"));
        assert!(title.contains("Episode 1"));
        assert_eq!(referer, Some("https://kisskh.do/".to_string()));

        let (title_dy, ref_dy) = resolve_batch_item_title_and_referer("https://www.douyin.com/video/71234567890");
        assert_eq!(title_dy, "Douyin Video 71234567890");
        assert_eq!(ref_dy, Some("https://www.douyin.com/".to_string()));

        let (title_tt, ref_tt) = resolve_batch_item_title_and_referer("https://www.tiktok.com/@user/video/987654321");
        assert_eq!(title_tt, "TikTok Video 987654321");
        assert_eq!(ref_tt, Some("https://www.tiktok.com/".to_string()));
    }
}


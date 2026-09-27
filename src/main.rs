mod database;
mod downloader;
mod error;
mod filesystem;
mod logger;
mod models;
mod network;
pub mod notifications;

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use uuid::Uuid;

use database::Database;
use downloader::DownloadManager;
use logger::UiLogLayer;
use models::{DownloadProgress, DownloadStatus, VideoMetadata};
use network::NetworkClient;

slint::include_modules!();

/// Helper: load history from database and push it into the Slint UI
async fn refresh_history(
    db: &Database,
    search: Option<&str>,
    weak: slint::Weak<AppWindow>,
) {
    match db.get_history(search).await {
        Ok(records) => {
            let items: Vec<HistoryItemData> = records
                .into_iter()
                .map(|r| {
                    let size_text = r
                        .total_size
                        .map(DownloadProgress::format_size)
                        .unwrap_or_else(|| DownloadProgress::format_size(r.downloaded_size));

                    HistoryItemData {
                        id: r.id.to_string().into(),
                        title: r.title.into(),
                        status: r.status.into(),
                        size_text: size_text.into(),
                        date_text: r.completed_at.unwrap_or(r.created_at).into(),
                        url: r.url.into(),
                        output_path: r.output_path.into(),
                    }
                })
                .collect();

            let _ = weak.upgrade_in_event_loop(move |window| {
                let model = Rc::new(VecModel::from(items));
                window.set_history_items(ModelRc::from(model));
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
    let url_str = url_str.trim().to_string();
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

                let has_subs = metadata.has_subtitles;
                let subs_summary = metadata.subtitles_summary.clone();

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
                    window.set_video_title(title.into());
                    window.set_video_resolution(format_display.into());
                    window.set_video_duration(details_str.into());
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
                    window.set_has_error(true);
                    window.set_error_message(clean_msg.into());
                    window.set_status_message("Unable to analyze URL. See details above.".into());
                });
            }
        }
    });
}

/// Downloads a remote thumbnail image to a temporary file for rendering in the UI thread
async fn download_thumbnail_to_cache(url: &str) -> Option<std::path::PathBuf> {
    info!("Fetching thumbnail image preview from: {}", url);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .build()
        .ok()?;
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;

    // Check if webp format (magic bytes: RIFF....WEBP)
    let is_webp = bytes.starts_with(b"RIFF") && bytes.len() > 12 && &bytes[8..12] == b"WEBP";
    if is_webp {
        let raw_file = std::env::temp_dir().join(format!("nvd_raw_{}.webp", uuid::Uuid::new_v4()));
        let png_file = std::env::temp_dir().join(format!("nvd_thumb_{}.png", uuid::Uuid::new_v4()));
        if tokio::fs::write(&raw_file, &bytes).await.is_ok() {
            let status = tokio::process::Command::new("sips")
                .args(["-s", "format", "png"])
                .arg(&raw_file)
                .args(["--out"])
                .arg(&png_file)
                .output()
                .await;
            let _ = tokio::fs::remove_file(&raw_file).await;
            if status.map(|s| s.status.success()).unwrap_or(false) && png_file.is_file() {
                return Some(png_file);
            }
        }
    }

    let temp_file = std::env::temp_dir().join(format!("nvd_thumb_{}.jpg", uuid::Uuid::new_v4()));
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

    // Initialize Slint UI window
    let main_window = AppWindow::new()?;
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

    // Initialize speed samples for the graph with 30 idle points
    let initial_samples: Vec<SpeedSampleData> = (0..30)
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

    // Queue filter & search state
    let queue_filter_idx: Arc<tokio::sync::RwLock<i32>> = Arc::new(tokio::sync::RwLock::new(0));
    let queue_search_term: Arc<tokio::sync::RwLock<String>> = Arc::new(tokio::sync::RwLock::new(String::new()));

    // Speed history, peak tracking, and session bytes
    let speed_history = Arc::new(tokio::sync::Mutex::new(VecDeque::from(vec![0.0f64; 30])));
    let peak_speed_bytes = Arc::new(AtomicU64::new(0));
    let session_downloaded_bytes = Arc::new(AtomicU64::new(0));

    // Wire up DownloadManager status updates -> Slint UI with frame-rate decoupling
    let window_weak_sync = main_window.as_weak();
    let mgr_for_sync = download_manager.clone();
    let is_rendering = Arc::new(AtomicBool::new(false));
    let filter_sync = queue_filter_idx.clone();
    let search_sync = queue_search_term.clone();
    let speed_hist_sync = speed_history.clone();
    let peak_speed_sync = peak_speed_bytes.clone();
    let session_bytes_sync = session_downloaded_bytes.clone();

    download_manager
        .set_update_listener(move || {
            let weak = window_weak_sync.clone();
            let mgr = mgr_for_sync.clone();
            let rendering_flag = is_rendering.clone();
            let filter_lock = filter_sync.clone();
            let search_lock = search_sync.clone();
            let speed_hist_lock = speed_hist_sync.clone();
            let peak_lock = peak_speed_sync.clone();
            let session_lock = session_bytes_sync.clone();

            // Skip queuing redundant frames if a frame render is already pending
            if rendering_flag.swap(true, Ordering::SeqCst) {
                return;
            }

            tokio::spawn(async move {
                let jobs = mgr.get_jobs_snapshot().await;
                let total_queue_count = jobs.len() as i32;
                let active_count = jobs
                    .iter()
                    .filter(|j| j.status == DownloadStatus::Downloading)
                    .count() as i32;

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

                // History shift
                let mut hist = speed_hist_lock.lock().await;
                hist.pop_front();
                hist.push_back(total_current_speed);

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

                let items: Vec<DownloadItemData> = filtered_jobs
                    .into_iter()
                    .map(|j| {
                        let is_dl = j.status == DownloadStatus::Downloading;
                        let is_paused = j.status == DownloadStatus::Paused;
                        let is_active = is_dl || is_paused || j.status == DownloadStatus::Queued;
                        let is_completed = j.status == DownloadStatus::Completed;
                        let output_path = j.output_path.to_string_lossy().to_string();
                        let size_text = j.size_display();
                        let speed_text = j.speed_display();
                        let eta_text = j.eta_display();

                        DownloadItemData {
                            id: j.id.to_string().into(),
                            title: j.title.into(),
                            status: j.status.as_str().into(),
                            progress: j.progress_ratio,
                            size_text: size_text.into(),
                            speed_text: speed_text.into(),
                            eta_text: eta_text.into(),
                            output_path: output_path.into(),
                            is_completed,
                            can_pause: is_dl,
                            can_resume: is_paused || matches!(j.status, DownloadStatus::Failed(_)),
                            can_cancel: is_active,
                        }
                    })
                    .collect();

                let reset_flag = rendering_flag.clone();
                let _ = weak.upgrade_in_event_loop(move |window| {
                    let model = Rc::new(VecModel::from(items));
                    window.set_download_items(ModelRc::from(model));
                    window.set_active_downloads_count(active_count);
                    window.set_total_queue_count(total_queue_count);

                    let speed_model = Rc::new(VecModel::from(speed_samples));
                    window.set_speed_samples(ModelRc::from(speed_model));
                    window.set_current_total_speed_text(cur_speed_text.into());
                    window.set_peak_speed_text(peak_speed_text.into());
                    window.set_session_downloaded_text(session_text.into());
                    window.set_is_downloading_active(is_active);

                    reset_flag.store(false, Ordering::SeqCst);
                });
            });
        })
        .await;

    // Periodic background ticker (every 1000ms) to keep speed graph smooth and responsive
    let ticker_mgr = download_manager.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(1000));
        loop {
            interval.tick().await;
            ticker_mgr.notify_update().await;
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

                let is_new = {
                    let mut last = last_clip_watcher.lock().unwrap();
                    if *last != trimmed {
                        *last = trimmed.clone();
                        true
                    } else {
                        false
                    }
                };

                if is_new && downloader::extractor::is_candidate_media_url(&trimmed) {
                    tracing::info!("Clipboard watcher detected candidate media URL: {}", trimmed);
                    let url = trimmed.clone();
                    let _ = weak_clip_watcher.upgrade_in_event_loop(move |win| {
                        win.set_clipboard_detected_url(url.clone().into());
                        win.set_clipboard_detected_url_visible(true);
                        win.set_status_message(format!("Clipboard detected media link: {}", url).into());
                    });

                    let notif_url = trimmed.clone();
                    notifications::send_notification(
                        "Native Video Downloader",
                        "Media Link Copied",
                        &format!("Ready to inspect: {}", notif_url),
                        false,
                    );
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
                win.set_batch_mode(false);
                win.set_input_url_text(url.clone().into());
                win.set_status_message(format!("Analyzing copied link: {}", url).into());
                run_url_analysis(url, weak, meta, client, cookies, proxy);
            }
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
            let _ = weak_paste_batch.upgrade_in_event_loop(move |win| {
                let current = win.get_batch_urls_text().to_string();
                let new_val = if current.trim().is_empty() {
                    text
                } else {
                    format!("{}\n{}", current.trim_end(), text)
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

    // Callback: Start Download (add to queue)
    let window_weak_dl = main_window.as_weak();
    let meta_clone_dl = current_metadata.clone();
    let mgr_clone_dl = download_manager.clone();
    let current_dir_dl = current_download_dir.clone();

    main_window.on_start_download(move |format_idx, download_subs, audio_fmt_idx, audio_br_idx, embed_meta| {
        let (is_audio_only, quality) = map_format_index(format_idx);
        let audio_format = if is_audio_only { Some(map_audio_format(audio_fmt_idx).to_string()) } else { None };
        let audio_bitrate = if is_audio_only { Some(map_audio_bitrate(audio_br_idx).to_string()) } else { None };

        info!(
            "User triggered 'Start Download' with format_idx: {} (audio: {}, format: {:?}, bitrate: {:?}, embed_meta: {}, subs: {})",
            format_idx, is_audio_only, audio_format, audio_bitrate, embed_meta, download_subs
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
                    let ep_title = format!("{} - {}", series_title, entry.title);
                    if let Ok(_) = mgr
                        .add_download(
                            entry.url,
                            ep_title,
                            &download_dir,
                            None,
                            true,
                            is_audio_only,
                            quality.clone(),
                            download_subs,
                            audio_fmt_clone.clone(),
                            audio_br_clone.clone(),
                            embed_meta,
                        )
                        .await
                    {
                        queued_count += 1;
                    }
                }
                info!("Queued {} series episodes for download", queued_count);
                let msg = format!("Queued {} episodes for download", queued_count);
                let _ = weak.upgrade_in_event_loop(move |window| {
                    window.set_status_message(msg.into());
                    window.set_active_tab(1); // Switch to Downloads view
                });
                return;
            }

            let is_extractor = metadata.is_extractor;
            match mgr
                .add_download(
                    metadata.url,
                    metadata.title,
                    &download_dir,
                    metadata.content_length,
                    is_extractor,
                    is_audio_only,
                    quality,
                    download_subs,
                    audio_fmt_clone,
                    audio_br_clone,
                    embed_meta,
                )
                .await
            {
                Ok(id) => {
                    info!("Download queued with id: {}", id);
                    let _ = weak.upgrade_in_event_loop(move |window| {
                        window.set_status_message("Download queued in manager".into());
                        window.set_active_tab(1); // Switch to Downloads view
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

    main_window.on_start_batch_download(move |raw_text, format_idx, audio_fmt_idx, audio_br_idx, embed_meta| {
        let text_val = raw_text.to_string();
        let (is_audio_only, quality) = map_format_index(format_idx);
        let audio_format = if is_audio_only { Some(map_audio_format(audio_fmt_idx).to_string()) } else { None };
        let audio_bitrate = if is_audio_only { Some(map_audio_bitrate(audio_br_idx).to_string()) } else { None };
        let weak = window_weak_batch.clone();
        let mgr = mgr_clone_batch.clone();
        let dir_lock = current_dir_batch.clone();

        tokio::spawn(async move {
            let lines: Vec<String> = text_val
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty() && (l.starts_with("http://") || l.starts_with("https://")))
                .collect();

            if lines.is_empty() {
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

            for url in lines {
                let title = url
                    .split('/')
                    .last()
                    .and_then(|s| s.split('?').next())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("batch_media")
                    .to_string();

                let is_extractor = downloader::is_streaming_platform(&url);
                if let Ok(_) = mgr
                    .add_download(
                        url,
                        title,
                        &download_dir,
                        None,
                        is_extractor,
                        is_audio_only,
                        quality.clone(),
                        true,
                        audio_format.clone(),
                        audio_bitrate.clone(),
                        embed_meta,
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
    main_window.on_search_history(move |query| {
        let db = db_search.clone();
        let weak = weak_search.clone();
        let q = query.to_string();
        tokio::spawn(async move {
            let search = if q.is_empty() { None } else { Some(q.as_str()) };
            refresh_history(&db, search, weak).await;
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
                .add_download(url, title, &download_dir, None, is_extractor, false, None, true, None, None, false)
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
    main_window.on_delete_history_item(move |id_str| {
        let db = db_del.clone();
        let weak = weak_del.clone();
        let id_val = id_str.to_string();
        tokio::spawn(async move {
            if let Ok(id) = Uuid::parse_str(&id_val) {
                let _ = db.delete_record(id).await;
                refresh_history(&db, None, weak).await;
            }
        });
    });

    // Callback: Clear all history
    let db_clear_all = db.clone();
    let weak_clear_all = main_window.as_weak();
    main_window.on_clear_all_history(move || {
        let db = db_clear_all.clone();
        let weak = weak_clear_all.clone();
        tokio::spawn(async move {
            let _ = db.clear_all_history().await;
            refresh_history(&db, None, weak).await;
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
                window.set_status_message(format!("Download speed limit set to: {}", label).into());
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
                1 => ("chrome", Some("chrome".to_string()), "Google Chrome"),
                2 => ("firefox", Some("firefox".to_string()), "Mozilla Firefox"),
                3 => ("safari", Some("safari".to_string()), "Apple Safari"),
                4 => ("brave", Some("brave".to_string()), "Brave Browser"),
                5 => ("edge", Some("edge".to_string()), "Microsoft Edge"),
                _ => ("", None, "Disabled (Guest mode)"),
            };
            mgr.set_cookies_browser(browser_opt.clone()).await;
            *lock.write().await = browser_opt;
            let _ = db.set_setting("cookies_browser", setting_val).await;
            info!("Updated browser cookies authentication: {}", label);
            let _ = weak.upgrade_in_event_loop(move |window| {
                window.set_status_message(format!("Browser cookies: {}", label).into());
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
        notifications::send_notification(
            "Native Video Downloader",
            "Notification Test",
            "System notifications and audio alerts are functioning perfectly!",
            false,
        );
        let _ = weak_test_notif.upgrade_in_event_loop(|win| {
            win.set_status_message("Sent test desktop notification".into());
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
                win.set_status_message("Checking for yt-dlp extractor engine updates...".into());
            });
            match downloader::update_ytdlp_engine().await {
                Ok(msg) => {
                    info!("Extractor engine update: {}", msg);
                    let _ = weak.upgrade_in_event_loop(move |win| {
                        win.set_status_message(format!("Extractor engine: {}", msg).into());
                    });
                }
                Err(err) => {
                    warn!("Failed to update extractor engine: {}", err);
                    let err_str = err.to_string();
                    let _ = weak.upgrade_in_event_loop(move |win| {
                        win.set_status_message(format!("Extractor update check: {}", err_str).into());
                    });
                }
            }
        });
    });

    // Load initial history on startup
    let db_init_hist = db.clone();
    let weak_init_hist = main_window.as_weak();
    tokio::spawn(async move {
        refresh_history(&db_init_hist, None, weak_init_hist).await;
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

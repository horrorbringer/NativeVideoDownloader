mod database;
mod downloader;
mod error;
mod filesystem;
mod logger;
mod models;
mod network;

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use slint::{ComponentHandle, ModelRc, VecModel};
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
    let download_manager = Arc::new(DownloadManager::new(3, db.clone())); // 3 bounded concurrent workers

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

    // Wire up DownloadManager status updates -> Slint UI with frame-rate decoupling
    let window_weak_sync = main_window.as_weak();
    let mgr_for_sync = download_manager.clone();
    let is_rendering = Arc::new(std::sync::atomic::AtomicBool::new(false));

    download_manager
        .set_update_listener(move || {
            let weak = window_weak_sync.clone();
            let mgr = mgr_for_sync.clone();
            let rendering_flag = is_rendering.clone();

            // Skip queuing redundant frames if a frame render is already pending
            if rendering_flag.swap(true, std::sync::atomic::Ordering::SeqCst) {
                return;
            }

            tokio::spawn(async move {
                let jobs = mgr.get_jobs_snapshot().await;
                let active_count = jobs
                    .iter()
                    .filter(|j| j.status == DownloadStatus::Downloading)
                    .count() as i32;

                let items: Vec<DownloadItemData> = jobs
                    .into_iter()
                    .map(|j| {
                        let is_dl = j.status == DownloadStatus::Downloading;
                        let is_paused = j.status == DownloadStatus::Paused;
                        let is_active = is_dl || is_paused || j.status == DownloadStatus::Queued;
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
                    reset_flag.store(false, std::sync::atomic::Ordering::SeqCst);
                });
            });
        })
        .await;

    // Callback: Analyze URL
    let window_weak = main_window.as_weak();
    let meta_clone = current_metadata.clone();
    let client_clone = network_client.clone();

    main_window.on_analyze_url(move |url| {
        let url_str = url.to_string();
        info!("Received URL analysis request: {}", url_str);

        if url_str.trim().is_empty() {
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
            window.set_is_analyzing(true);
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
            let inspect_result = if downloader::is_streaming_platform(&url_str) {
                let _ = weak_for_async.upgrade_in_event_loop(|w| {
                    w.set_status_message("Analyzing streaming platform media (yt-dlp)...".into());
                });
                downloader::inspect_video(&url_str).await
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
                        downloader::inspect_video(&url_str).await
                    }
                    Err(err) => {
                        // Try extractor as fallback
                        match downloader::inspect_video(&url_str).await {
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

                    *meta_for_async.lock().await = Some(metadata);

                    let _ = weak_for_async.upgrade_in_event_loop(move |window| {
                        window.set_is_analyzing(false);
                        window.set_has_metadata(true);
                        window.set_has_error(false);
                        window.set_error_message("".into());
                        window.set_is_playlist(is_playlist);
                        window.set_playlist_count(playlist_count);
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
                    let err_msg = format!("Inspection failed: {}", err);
                    let err_for_box = err_msg.clone();
                    let _ = weak_for_async.upgrade_in_event_loop(move |window| {
                        window.set_is_analyzing(false);
                        window.set_has_metadata(false);
                        window.set_has_error(true);
                        window.set_error_message(err_for_box.into());
                        window.set_status_message(err_msg.into());
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
        4 => (true, None), // Audio Only (MP3)
        _ => (false, None), // Best quality video
    }
}

    // Callback: Start Download (add to queue)
    let window_weak_dl = main_window.as_weak();
    let meta_clone_dl = current_metadata.clone();
    let mgr_clone_dl = download_manager.clone();

    main_window.on_start_download(move |format_idx| {
        let (is_audio_only, quality) = map_format_index(format_idx);
        info!(
            "User triggered 'Start Download' with format_idx: {} (audio_only: {}, quality: {:?})",
            format_idx, is_audio_only, quality
        );

        let weak = window_weak_dl.clone();
        let meta_arc = meta_clone_dl.clone();
        let mgr = mgr_clone_dl.clone();

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

            let download_dir = filesystem::default_download_dir();

            // If album/series playlist, queue all episodes
            if metadata.is_playlist {
                let series_title = metadata.title.clone();
                let mut queued_count = 0;
                for entry in metadata.playlist_entries {
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

    main_window.on_start_batch_download(move |raw_text, format_idx| {
        let text_val = raw_text.to_string();
        let (is_audio_only, quality) = map_format_index(format_idx);
        let weak = window_weak_batch.clone();
        let mgr = mgr_clone_batch.clone();

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

            let download_dir = filesystem::default_download_dir();
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

    // Callback: Open folder for a history item
    main_window.on_open_history_folder(move |path_str| {
        let path = PathBuf::from(path_str.to_string());
        let _ = filesystem::open_parent_folder(&path);
    });

    // Callback: Redownload from history
    let mgr_redl = download_manager.clone();
    let weak_redl = main_window.as_weak();
    main_window.on_redownload_history(move |url_str| {
        let mgr = mgr_redl.clone();
        let weak = weak_redl.clone();
        let url = url_str.to_string();
        tokio::spawn(async move {
            let download_dir = filesystem::default_download_dir();
            let title = url
                .split('/')
                .last()
                .and_then(|s| s.split('?').next())
                .filter(|s| !s.is_empty())
                .unwrap_or("redownload")
                .to_string();

            let is_extractor = downloader::is_streaming_platform(&url);
            match mgr
                .add_download(url, title, &download_dir, None, is_extractor, false, None)
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

    // Callback: Clear Logs
    let ui_logger_clear = ui_log_layer.clone();
    main_window.on_clear_logs(move || {
        ui_logger_clear.clear();
    });

    // Callback: Dismiss Error Alert
    let weak_dismiss = main_window.as_weak();
    main_window.on_dismiss_error(move || {
        let _ = weak_dismiss.upgrade_in_event_loop(|win| {
            win.set_has_error(false);
            win.set_error_message("".into());
        });
    });

    // Load initial history on startup
    let db_init_hist = db.clone();
    let weak_init_hist = main_window.as_weak();
    tokio::spawn(async move {
        refresh_history(&db_init_hist, None, weak_init_hist).await;
    });

    // Run Slint native event loop
    info!("Launching native Slint window");
    main_window.run()?;

    info!("Native Video Downloader terminated normally");
    Ok(())
}

mod error;
mod models;
mod filesystem;
mod network;
mod downloader;
mod database;

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use slint::{ComponentHandle, ModelRc, VecModel};
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use uuid::Uuid;

use database::Database;
use downloader::DownloadManager;
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
    // Initialize structured logging
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    info!("Starting Native Video Downloader v0.1.0 (Rust + Slint)");

    // Initialize SQLite database
    let db_path = Database::default_db_path();
    let db = Arc::new(Database::init(&db_path).await?);

    // Initialize Slint UI window
    let main_window = AppWindow::new()?;

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

    // Wire up DownloadManager status updates -> Slint UI
    let window_weak_sync = main_window.as_weak();
    let mgr_for_sync = download_manager.clone();

    download_manager
        .set_update_listener(move || {
            let weak = window_weak_sync.clone();
            let mgr = mgr_for_sync.clone();

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

                let _ = weak.upgrade_in_event_loop(move |window| {
                    let model = Rc::new(VecModel::from(items));
                    window.set_download_items(ModelRc::from(model));
                    window.set_active_downloads_count(active_count);
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
            window.set_status_message(format!("Inspecting media at {}...", url_str).into());
        }

        let weak_for_async = window_weak.clone();
        let meta_for_async = meta_clone.clone();
        let client = client_clone.clone();

        tokio::spawn(async move {
            match client.inspect_url(&url_str).await {
                Ok(metadata) => {
                    info!("Successfully inspected URL: {:?}", metadata);
                    let title = metadata.title.clone();
                    let size_str = metadata
                        .content_length
                        .map(DownloadProgress::format_size)
                        .unwrap_or_else(|| "Unknown size".to_string());
                    let type_str = metadata
                        .content_type
                        .clone()
                        .unwrap_or_else(|| "media/stream".to_string());
                    let ranges_str = if metadata.supports_ranges {
                        "Resumable (Range supported)"
                    } else {
                        "Single-stream (No Range)"
                    };

                    *meta_for_async.lock().await = Some(metadata);

                    let _ = weak_for_async.upgrade_in_event_loop(move |window| {
                        window.set_is_analyzing(false);
                        window.set_has_metadata(true);
                        window.set_video_title(title.into());
                        window.set_video_resolution(format!("Format: {}", type_str).into());
                        window.set_video_duration(format!("{} • {}", size_str, ranges_str).into());
                        window.set_status_message("Media analyzed successfully. Ready to download.".into());
                    });
                }
                Err(err) => {
                    warn!("Failed to inspect URL {}: {}", url_str, err);
                    let err_msg = format!("Inspection failed: {}", err);
                    let _ = weak_for_async.upgrade_in_event_loop(move |window| {
                        window.set_is_analyzing(false);
                        window.set_has_metadata(false);
                        window.set_status_message(err_msg.into());
                    });
                }
            }
        });
    });

    // Callback: Start Download (add to queue)
    let window_weak_dl = main_window.as_weak();
    let meta_clone_dl = current_metadata.clone();
    let mgr_clone_dl = download_manager.clone();

    main_window.on_start_download(move || {
        info!("User triggered 'Start Download'");

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
            match mgr
                .add_download(metadata.url, metadata.title, &download_dir, metadata.content_length)
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
                    let _ = weak.upgrade_in_event_loop(move |window| {
                        window.set_status_message(err_msg.into());
                    });
                }
            }
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

            match mgr.add_download(url, title, &download_dir, None).await {
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

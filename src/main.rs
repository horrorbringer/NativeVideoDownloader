mod error;
mod models;
mod filesystem;
mod network;
mod downloader;

use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use models::{DownloadProgress, VideoMetadata};
use network::NetworkClient;

slint::include_modules!();

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize structured logging
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    info!("Starting Native Video Downloader v0.1.0 (Rust + Slint)");

    // Initialize Slint UI window
    let main_window = AppWindow::new()?;

    // Shared state between UI callbacks and background tasks
    let current_metadata: Arc<Mutex<Option<VideoMetadata>>> = Arc::new(Mutex::new(None));
    let active_cancel_token: Arc<Mutex<Option<CancellationToken>>> = Arc::new(Mutex::new(None));
    let network_client = Arc::new(NetworkClient::new());

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

    // Callback: Start Download
    let window_weak_dl = main_window.as_weak();
    let meta_clone_dl = current_metadata.clone();
    let cancel_clone_dl = active_cancel_token.clone();
    let client_clone_dl = network_client.clone();

    main_window.on_start_download(move || {
        info!("User triggered 'Start Download'");

        let weak = window_weak_dl.clone();
        let meta_arc = meta_clone_dl.clone();
        let cancel_arc = cancel_clone_dl.clone();
        let client = client_clone_dl.clone();

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
            let destination = match filesystem::validate_destination_path(&download_dir, &metadata.title) {
                Ok(p) => p,
                Err(err) => {
                    let err_msg = format!("Failed to resolve path: {}", err);
                    let _ = weak.upgrade_in_event_loop(move |window| {
                        window.set_status_message(err_msg.into());
                    });
                    return;
                }
            };

            let cancel_token = CancellationToken::new();
            *cancel_arc.lock().await = Some(cancel_token.clone());

            let filename = destination
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("media_download")
                .to_string();

            let filename_display = filename.clone();
            let _ = weak.upgrade_in_event_loop(move |window| {
                window.set_is_downloading(true);
                window.set_download_status("Downloading".into());
                window.set_download_filename(filename_display.into());
                window.set_download_progress(0.0);
                window.set_download_speed("-- MB/s".into());
                window.set_download_eta("--:--".into());
                window.set_download_size_text("Starting stream...".into());
                window.set_status_message("Download started...".into());
                window.set_active_tab(1); // Switch to Downloads view
            });

            // Start streaming download with progress updates
            let weak_prog = weak.clone();
            let download_res = client
                .download_file(
                    &metadata.url,
                    &destination,
                    cancel_token,
                    move |progress| {
                        let total_text = progress
                            .total_bytes
                            .map(DownloadProgress::format_size)
                            .unwrap_or_else(|| "Unknown".to_string());
                        let downloaded_text = DownloadProgress::format_size(progress.downloaded_bytes);
                        let percentage = (progress.progress_ratio * 100.0) as u32;

                        let size_text = format!("{} / {} ({}%)", downloaded_text, total_text, percentage);
                        let speed_text = progress.format_speed();
                        let eta_text = progress.format_eta();
                        let ratio = progress.progress_ratio;

                        let _ = weak_prog.upgrade_in_event_loop(move |window| {
                            window.set_download_progress(ratio);
                            window.set_download_speed(speed_text.into());
                            window.set_download_eta(eta_text.into());
                            window.set_download_size_text(size_text.into());
                        });
                    },
                )
                .await;

            *cancel_arc.lock().await = None;

            match download_res {
                Ok(()) => {
                    info!("Download completed for: {}", filename);
                    let _ = weak.upgrade_in_event_loop(move |window| {
                        window.set_is_downloading(false);
                        window.set_download_status("Completed".into());
                        window.set_download_progress(1.0);
                        window.set_status_message(format!("Saved: {}", destination.display()).into());
                    });
                }
                Err(error::AppError::Cancelled) => {
                    info!("Download cancelled for: {}", filename);
                    let _ = weak.upgrade_in_event_loop(|window| {
                        window.set_is_downloading(false);
                        window.set_download_status("Cancelled".into());
                        window.set_status_message("Download was cancelled.".into());
                    });
                }
                Err(err) => {
                    error!("Download failed for {}: {}", filename, err);
                    let err_msg = format!("Download failed: {}", err);
                    let _ = weak.upgrade_in_event_loop(move |window| {
                        window.set_is_downloading(false);
                        window.set_download_status("Failed".into());
                        window.set_status_message(err_msg.into());
                    });
                }
            }
        });
    });

    // Callback: Cancel Download
    let cancel_clone_btn = active_cancel_token.clone();
    main_window.on_cancel_download(move || {
        info!("User clicked 'Cancel Download'");
        let cancel_arc = cancel_clone_btn.clone();
        tokio::spawn(async move {
            if let Some(token) = cancel_arc.lock().await.as_ref() {
                token.cancel();
            }
        });
    });

    // Run Slint native event loop
    info!("Launching native Slint window");
    main_window.run()?;

    info!("Native Video Downloader terminated normally");
    Ok(())
}

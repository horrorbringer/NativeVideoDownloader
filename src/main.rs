use std::time::Duration;
use tracing::{info, warn};

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

    // Callback: Analyze URL
    let window_weak = main_window.as_weak();
    main_window.on_analyze_url(move |url| {
        let url_str = url.to_string();
        info!("Received URL analysis request: {}", url_str);

        if url_str.trim().is_empty() {
            if let Some(window) = window_weak.upgrade() {
                window.set_status_message("Please enter a valid URL".into());
            }
            return;
        }

        // Validate basic URL scheme
        if !url_str.starts_with("http://") && !url_str.starts_with("https://") {
            if let Some(window) = window_weak.upgrade() {
                window.set_status_message("Invalid URL: must start with http:// or https://".into());
            }
            return;
        }

        if let Some(window) = window_weak.upgrade() {
            window.set_is_analyzing(true);
            window.set_status_message(format!("Analyzing: {}...", url_str).into());
        }

        let weak_for_async = window_weak.clone();
        tokio::spawn(async move {
            // Milestone 1: Simulated non-blocking analysis pipeline
            info!("Async analysis pipeline started for {}", url_str);
            tokio::time::sleep(Duration::from_millis(1200)).await;

            // Extract simulated or basic metadata
            let filename = url_str
                .split('/')
                .last()
                .filter(|s| !s.is_empty())
                .unwrap_or("media_stream.mp4");

            info!("Analysis complete for {}, extracted filename: {}", url_str, filename);

            let _ = weak_for_async.upgrade_in_event_loop(move |window| {
                window.set_is_analyzing(false);
                window.set_has_metadata(true);
                window.set_video_title(format!("Stream: {}", filename).into());
                window.set_video_resolution("1080p (60fps)".into());
                window.set_video_duration("Simulated preview (Ready)".into());
                window.set_status_message("Analysis complete. Ready to download.".into());
            });
        });
    });

    // Callback: Start Download
    let window_weak_dl = main_window.as_weak();
    main_window.on_start_download(move || {
        info!("User triggered 'Start Download'");
        if let Some(window) = window_weak_dl.upgrade() {
            window.set_status_message("Download initiated (Queue worker idle)".into());
            window.set_active_tab(1); // Switch to Downloads tab
        }
    });

    // Run Slint native event loop
    info!("Launching native Slint window");
    main_window.run()?;

    info!("Native Video Downloader terminated normally");
    Ok(())
}

use std::path::Path;
use std::time::Duration;
use reqwest::header::{ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_TYPE, RANGE};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::downloader::progress::ProgressCalculator;
use crate::error::{AppError, Result};
use crate::filesystem::{cleanup_part_file, finalize_part_file, part_path_for};
use crate::models::{DownloadProgress, VideoMetadata};

pub struct NetworkClient {
    client: reqwest::Client,
}

impl NetworkClient {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self { client }
    }

    /// Inspects a media URL to fetch content length, content type, and range support
    pub async fn inspect_url(&self, url: &str) -> Result<VideoMetadata> {
        let resp_res = self.client.head(url).send().await;

        let resp = match resp_res {
            Ok(r) if r.status().is_success() => r,
            _ => {
                // Fallback: send GET with Range 0-0 in case HEAD is disallowed
                self.client
                    .get(url)
                    .header(RANGE, "bytes=0-0")
                    .send()
                    .await?
            }
        };

        let status = resp.status();
        if !status.is_success() && status.as_u16() != 206 {
            return Err(AppError::Http {
                status: status.as_u16(),
                message: format!("HTTP error: {}", status),
            });
        }

        let headers = resp.headers();

        let content_length = headers
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());

        let content_type = headers
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let supports_ranges = headers
            .get(ACCEPT_RANGES)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_lowercase() == "bytes")
            .unwrap_or(false);

        // Derive title from URL path or fallback
        let title = url
            .split('/')
            .last()
            .and_then(|s| s.split('?').next())
            .filter(|s| !s.is_empty())
            .unwrap_or("media_video")
            .to_string();

        Ok(VideoMetadata {
            url: url.to_string(),
            title,
            content_length,
            content_type,
            supports_ranges,
        })
    }

    /// Streams a download to disk using a temporary `.part` file, updating progress
    pub async fn download_file<F>(
        &self,
        url: &str,
        destination_path: &Path,
        cancel_token: CancellationToken,
        mut on_progress: F,
    ) -> Result<()>
    where
        F: FnMut(DownloadProgress) + Send + 'static,
    {
        info!("Starting download stream for {}", url);
        let part_path = part_path_for(destination_path);

        // Ensure parent directory exists
        if let Some(parent) = part_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let mut response = self.client.get(url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(AppError::Http {
                status: status.as_u16(),
                message: format!("Download request failed: {}", status),
            });
        }

        let total_bytes = response.content_length();
        let mut progress_calc = ProgressCalculator::new(total_bytes);

        // Open .part file for writing
        let mut file = tokio::fs::File::create(&part_path).await?;

        // Inform initial progress
        on_progress(progress_calc.update(0));

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    warn!("Download cancelled by user for: {:?}", destination_path);
                    drop(file);
                    let _ = cleanup_part_file(&part_path);
                    return Err(AppError::Cancelled);
                }
                chunk_res = response.chunk() => {
                    match chunk_res {
                        Ok(Some(chunk)) => {
                            let len = chunk.len();
                            file.write_all(&chunk).await?;
                            let prog = progress_calc.update(len);
                            on_progress(prog);
                        }
                        Ok(None) => {
                            // Stream completed successfully
                            break;
                        }
                        Err(err) => {
                            drop(file);
                            let _ = cleanup_part_file(&part_path);
                            return Err(AppError::Network(err));
                        }
                    }
                }
            }
        }

        file.flush().await?;
        drop(file);

        // Atomically rename .part file to final destination
        finalize_part_file(&part_path, destination_path)?;
        info!("Download completed successfully: {:?}", destination_path);

        Ok(())
    }
}

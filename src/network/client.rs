use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use reqwest::header::{ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_TYPE, RANGE, REFERER};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::downloader::progress::ProgressCalculator;
use crate::error::{AppError, Result};
use crate::filesystem::{cleanup_part_file, finalize_part_file, part_path_for};
use crate::models::{DownloadProgress, VideoMetadata};

fn build_http_client(proxy_url: Option<&str>) -> reqwest::Client {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .tcp_nodelay(true)
        .tcp_keepalive(Duration::from_secs(60))
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(10)
        .http2_adaptive_window(true)
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36");

    if let Some(proxy_str) = proxy_url {
        let trimmed = proxy_str.trim();
        if !trimmed.is_empty() {
            match reqwest::Proxy::all(trimmed) {
                Ok(proxy) => {
                    builder = builder.proxy(proxy);
                    info!("NetworkClient configured with proxy: {}", trimmed);
                }
                Err(err) => {
                    warn!("Failed to parse proxy URL '{}': {}", trimmed, err);
                }
            }
        }
    }

    builder.build().unwrap_or_else(|_| reqwest::Client::new())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NetworkDiagnosticReport {
    pub target: String,
    pub dns_ms: u128,
    pub dns_status: String,
    pub rtt_ms: u128,
    pub rtt_status: String,
    pub supports_range: bool,
    pub range_status: String,
    pub throughput_speed_text: String,
    pub throughput_status: String,
    pub overall_status: String,
    pub summary_headline: String,
    pub summary_details: String,
}

#[derive(Clone)]
pub struct NetworkClient {
    client: Arc<RwLock<reqwest::Client>>,
    proxy_url: Arc<RwLock<Option<String>>>,
}

impl NetworkClient {
    pub fn new() -> Self {
        Self {
            client: Arc::new(RwLock::new(build_http_client(None))),
            proxy_url: Arc::new(RwLock::new(None)),
        }
    }

    /// Sets or clears the active proxy URL for all future network operations
    pub fn set_proxy(&self, proxy: Option<String>) {
        let client = build_http_client(proxy.as_deref());
        *self.client.write().unwrap() = client;
        *self.proxy_url.write().unwrap() = proxy;
    }

    /// Returns the currently active proxy URL if set
    #[allow(dead_code)]
    pub fn get_proxy(&self) -> Option<String> {
        self.proxy_url.read().unwrap().clone()
    }

    /// Tests connection through a specified proxy URL and returns ping round-trip time in milliseconds
    pub async fn test_proxy_connection(proxy_url: &str) -> Result<u128> {
        let trimmed = proxy_url.trim();
        let proxy = reqwest::Proxy::all(trimmed)
            .map_err(|e| AppError::Generic(format!("Invalid proxy URL syntax: {}", e)))?;

        let client = reqwest::Client::builder()
            .proxy(proxy)
            .timeout(Duration::from_secs(8))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| AppError::Generic(format!("Failed to build proxy client: {}", e)))?;

        let start = std::time::Instant::now();
        // Try standard lightweight ping endpoints (Cloudflare and Google generate_204)
        let resp = client.get("https://cloudflare.com/cdn-cgi/trace").send().await;
        match resp {
            Ok(r) if r.status().is_success() => Ok(start.elapsed().as_millis()),
            _ => {
                // Secondary fallback: Google 204
                let resp2 = client.get("https://www.google.com/generate_204").send().await;
                match resp2 {
                    Ok(_) => Ok(start.elapsed().as_millis()),
                    Err(e) => {
                        let err_str = e.to_string();
                        if err_str.to_lowercase().contains("refused") {
                            Err(AppError::Generic("Connection refused. Is your proxy app (Clash/V2Ray) running?".into()))
                        } else {
                            Err(AppError::Generic(format!("Proxy unreachable: {}", e)))
                        }
                    }
                }
            }
        }
    }

    /// Performs comprehensive network diagnostic testing (DNS, HTTP Handshake RTT, Range support, Burst throughput)
    pub async fn run_diagnostics(&self, target_url: &str) -> NetworkDiagnosticReport {
        let trimmed = target_url.trim();
        let target = if trimmed.is_empty() {
            "https://cloudflare.com/cdn-cgi/trace"
        } else {
            trimmed
        };

        let host = match reqwest::Url::parse(target) {
            Ok(u) => u.host_str().unwrap_or("cloudflare.com").to_string(),
            Err(_) => "cloudflare.com".to_string(),
        };

        // 1. Measure DNS Lookup Latency
        let dns_start = std::time::Instant::now();
        let dns_port = format!("{}:443", host);
        let _ = tokio::net::lookup_host(&dns_port).await;
        let dns_ms = dns_start.elapsed().as_millis().max(1);
        let dns_status = if dns_ms < 35 {
            "Fast".to_string()
        } else if dns_ms < 120 {
            "Normal".to_string()
        } else {
            "Slow".to_string()
        };

        // 2. Measure HTTP Handshake & RTT Ping
        let client = self.client.read().unwrap().clone();
        let rtt_start = std::time::Instant::now();
        let _ = client.head(target).send().await;
        let rtt_ms = rtt_start.elapsed().as_millis().max(1);
        let rtt_status = if rtt_ms < 70 {
            "Optimal".to_string()
        } else if rtt_ms < 190 {
            "Moderate".to_string()
        } else {
            "High Latency".to_string()
        };

        // 3. Test Byte-Range Support & Micro-Burst Throughput
        let probe_start = std::time::Instant::now();
        let get_res = client
            .get(target)
            .header(reqwest::header::RANGE, "bytes=0-131071")
            .send()
            .await;

        let (supports_range, range_status, throughput_text, throughput_status, overall_status, headline, details) = match get_res {
            Ok(resp) => {
                let code = resp.status();
                let range_ok = code.as_u16() == 206
                    || resp
                        .headers()
                        .get(reqwest::header::ACCEPT_RANGES)
                        .map(|v| v == "bytes")
                        .unwrap_or(false);

                let r_stat = if range_ok { "Supported".to_string() } else { "No Resume".to_string() };

                let bytes = resp.bytes().await.unwrap_or_default();
                let elapsed = probe_start.elapsed().as_secs_f64().max(0.001);
                let bps = (bytes.len() as f64) / elapsed;
                let spd_text = DownloadProgress::format_speed_val(bps);
                let tp_stat = if bps > 10.0 * 1024.0 * 1024.0 {
                    "Fast (10M+)".to_string()
                } else if bps > 2.0 * 1024.0 * 1024.0 {
                    "Normal (2-10M)".to_string()
                } else {
                    "Limited (<2M)".to_string()
                };

                let (ov_stat, head, det) = if rtt_ms < 85 && range_ok {
                    (
                        "Optimal".to_string(),
                        "Connection is healthy and optimal for streaming".to_string(),
                        format!("Edge CDN responded in {} ms with byte-range resume verified. Burst throughput reached {}.", rtt_ms, spd_text)
                    )
                } else if !range_ok {
                    (
                        "Degraded".to_string(),
                        "Server lacks byte-range resume support".to_string(),
                        "This media server does not support HTTP 206 Partial Content. Paused or interrupted downloads cannot be resumed and will restart from byte 0.".to_string()
                    )
                } else if rtt_ms >= 200 {
                    (
                        "Degraded".to_string(),
                        "High network latency detected".to_string(),
                        format!("Round-trip latency is {} ms. Consider checking your ISP or enabling a proxy to improve routing.", rtt_ms)
                    )
                } else {
                    (
                        "Good".to_string(),
                        "Connection is stable and ready".to_string(),
                        format!("Latency: {} ms, DNS: {} ms. Streams and downloads will operate smoothly.", rtt_ms, dns_ms)
                    )
                };

                (range_ok, r_stat, spd_text, tp_stat, ov_stat, head, det)
            }
            Err(e) => {
                (
                    false,
                    "Failed".to_string(),
                    "0 B/s".to_string(),
                    "Offline".to_string(),
                    "Unreachable".to_string(),
                    "Host unreachable or connection failed".to_string(),
                    format!("Could not connect to target host: {}. Check network connection or proxy configuration.", e)
                )
            }
        };

        NetworkDiagnosticReport {
            target: target.to_string(),
            dns_ms,
            dns_status,
            rtt_ms,
            rtt_status,
            supports_range,
            range_status,
            throughput_speed_text: throughput_text,
            throughput_status,
            overall_status,
            summary_headline: headline,
            summary_details: details,
        }
    }

    /// Returns a copy of the current reqwest::Client
    #[allow(dead_code)]
    pub fn get_client(&self) -> reqwest::Client {
        self.client.read().unwrap().clone()
    }

    /// Downloads raw bytes of an image or asset URL
    pub async fn download_image_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let client = self.client.read().unwrap().clone();
        let resp = client.get(url).send().await?;
        let bytes = resp.bytes().await?;
        Ok(bytes.to_vec())
    }

    /// Inspects a media URL to fetch content length, content type, and range support
    pub async fn inspect_url(&self, url: &str) -> Result<VideoMetadata> {
        let client = self.client.read().unwrap().clone();
        let resp_res = client.head(url).send().await;

        let resp = match resp_res {
            Ok(r) if r.status().is_success() => r,
            _ => {
                // Fallback: send GET with Range 0-0 in case HEAD is disallowed
                client
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
            is_extractor: false,
            duration_seconds: None,
            resolution: None,
            ext: None,
            is_playlist: false,
            playlist_count: 0,
            playlist_entries: Vec::new(),
            has_subtitles: false,
            subtitles_summary: String::new(),
            thumbnail_url: None,
            size_best: content_length,
            ..Default::default()
        })
    }

    /// Streams a download to disk using a temporary `.part` file, updating progress
    pub async fn download_file<F>(
        &self,
        url: &str,
        destination_path: &Path,
        cancel_token: CancellationToken,
        preserve_part_on_cancel: bool,
        speed_limit_bytes: Option<u64>,
        referer: Option<&str>,
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

        // Check if partial download already exists on disk
        let existing_bytes = if part_path.exists() {
            tokio::fs::metadata(&part_path)
                .await
                .map(|m| m.len())
                .unwrap_or(0)
        } else {
            0
        };

        let client = self.client.read().unwrap().clone();
        let mut request_builder = client.get(url);

        if let Some(ref_url) = referer {
            let trimmed = ref_url.trim();
            if !trimmed.is_empty() {
                info!("Auto-forwarding Referer to native download: {}", trimmed);
                request_builder = request_builder.header(REFERER, trimmed);
                if let Ok(parsed) = reqwest::Url::parse(trimmed) {
                    let origin = format!("{}://{}", parsed.scheme(), parsed.host_str().unwrap_or(""));
                    request_builder = request_builder.header("Origin", origin);
                }
            }
        }

        let request_builder = if existing_bytes > 0 {
            info!("Attempting resume for {:?} from byte offset {}", destination_path, existing_bytes);
            request_builder.header(RANGE, format!("bytes={}-", existing_bytes))
        } else {
            request_builder
        };

        let mut response = request_builder.send().await?;
        let status = response.status();

        if !status.is_success() && status.as_u16() != 206 {
            return Err(AppError::Http {
                status: status.as_u16(),
                message: format!("Download request failed: {}", status),
            });
        }

        let is_partial = status.as_u16() == 206;
        let (file, mut progress_calc) = if is_partial && existing_bytes > 0 {
            info!("Server accepted Range request (206 Partial Content)");
            let total = response.content_length().map(|len| len + existing_bytes);
            let file = tokio::fs::OpenOptions::new()
                .write(true)
                .append(true)
                .open(&part_path)
                .await?;
            let calc = ProgressCalculator::with_initial_bytes(total, existing_bytes);
            (file, calc)
        } else {
            let total = response.content_length();
            let file = tokio::fs::File::create(&part_path).await?;
            let calc = ProgressCalculator::new(total);
            (file, calc)
        };

        // Wrap file in a 256 KB asynchronous buffer to coalesce disk writes and minimize syscall overhead
        let mut buffered_file = tokio::io::BufWriter::with_capacity(256 * 1024, file);

        // Inform initial progress
        on_progress(progress_calc.update(0));
        let mut last_prog_instant = std::time::Instant::now();

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    warn!("Download stopped/cancelled for: {:?}", destination_path);
                    drop(buffered_file);
                    if !preserve_part_on_cancel {
                        let _ = cleanup_part_file(&part_path);
                    }
                    return Err(AppError::Cancelled);
                }
                chunk_res = response.chunk() => {
                    match chunk_res {
                        Ok(Some(chunk)) => {
                            let len = chunk.len();
                            buffered_file.write_all(&chunk).await?;
                            let prog = progress_calc.update(len);
                            if last_prog_instant.elapsed() >= std::time::Duration::from_millis(60) {
                                last_prog_instant = std::time::Instant::now();
                                on_progress(prog);
                            }

                            if let Some(limit) = speed_limit_bytes {
                                if limit > 0 {
                                    let delay_secs = len as f64 / limit as f64;
                                    tokio::time::sleep(std::time::Duration::from_secs_f64(delay_secs)).await;
                                }
                            }
                        }
                        Ok(None) => {
                            // Stream completed successfully - emit final 100% progress
                            on_progress(progress_calc.update(0));
                            break;
                        }
                        Err(err) => {
                            drop(buffered_file);
                            return Err(AppError::Network(err));
                        }
                    }
                }
            }
        }

        buffered_file.flush().await?;
        drop(buffered_file);

        // Atomically rename .part file to final destination
        finalize_part_file(&part_path, destination_path)?;
        info!("Download completed successfully: {:?}", destination_path);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_diagnostic_report_defaults() {
        let report = NetworkDiagnosticReport {
            target: "https://cloudflare.com".to_string(),
            dns_ms: 12,
            dns_status: "Fast".to_string(),
            rtt_ms: 25,
            rtt_status: "Optimal".to_string(),
            supports_range: true,
            range_status: "Supported".to_string(),
            throughput_speed_text: "15.2 MB/s".to_string(),
            throughput_status: "Fast (10M+)".to_string(),
            overall_status: "Optimal".to_string(),
            summary_headline: "Optimal connection".to_string(),
            summary_details: "Connection verified".to_string(),
        };

        assert_eq!(report.target, "https://cloudflare.com");
        assert!(report.supports_range);
        assert_eq!(report.overall_status, "Optimal");
    }

    #[tokio::test]
    async fn test_network_client_diagnostics_unreachable_target() {
        let client = NetworkClient::new();
        let report = client.run_diagnostics("http://127.0.0.1:9").await;
        assert_eq!(report.target, "http://127.0.0.1:9");
        assert!(!report.supports_range);
        assert_eq!(report.overall_status, "Unreachable");
    }
}

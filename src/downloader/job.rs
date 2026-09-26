use std::path::PathBuf;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::models::{DownloadProgress, DownloadStatus};

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DownloadJob {
    pub id: Uuid,
    pub url: String,
    pub title: String,
    pub output_path: PathBuf,
    pub status: DownloadStatus,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bytes_sec: f64,
    pub eta_seconds: Option<u64>,
    pub progress_ratio: f32,
    pub retry_count: u32,
    pub max_retries: u32,
    pub cancel_token: Option<CancellationToken>,
    pub is_extractor: bool,
    pub is_audio_only: bool,
    pub quality: Option<String>,
}

impl DownloadJob {
    pub fn new(
        url: String,
        title: String,
        output_path: PathBuf,
        total_bytes: Option<u64>,
        is_extractor: bool,
        is_audio_only: bool,
        quality: Option<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            url,
            title,
            output_path,
            status: DownloadStatus::Queued,
            downloaded_bytes: 0,
            total_bytes,
            speed_bytes_sec: 0.0,
            eta_seconds: None,
            progress_ratio: 0.0,
            retry_count: 0,
            max_retries: 3,
            cancel_token: None,
            is_extractor,
            is_audio_only,
            quality,
        }
    }

    pub fn update_progress(&mut self, progress: DownloadProgress) {
        self.downloaded_bytes = progress.downloaded_bytes;
        if progress.total_bytes.is_some() {
            self.total_bytes = progress.total_bytes;
        }
        self.speed_bytes_sec = progress.speed_bytes_sec;
        self.eta_seconds = progress.eta_seconds;
        self.progress_ratio = progress.progress_ratio;
    }

    pub fn size_display(&self) -> String {
        let current = DownloadProgress::format_size(self.downloaded_bytes);
        let total = self
            .total_bytes
            .map(DownloadProgress::format_size)
            .unwrap_or_else(|| "Unknown".to_string());
        let pct = (self.progress_ratio * 100.0) as u32;
        format!("{} / {} ({}%)", current, total, pct)
    }

    pub fn speed_display(&self) -> String {
        let p = DownloadProgress {
            downloaded_bytes: self.downloaded_bytes,
            total_bytes: self.total_bytes,
            speed_bytes_sec: self.speed_bytes_sec,
            eta_seconds: self.eta_seconds,
            progress_ratio: self.progress_ratio,
        };
        p.format_speed()
    }

    pub fn eta_display(&self) -> String {
        let p = DownloadProgress {
            downloaded_bytes: self.downloaded_bytes,
            total_bytes: self.total_bytes,
            speed_bytes_sec: self.speed_bytes_sec,
            eta_seconds: self.eta_seconds,
            progress_ratio: self.progress_ratio,
        };
        p.format_eta()
    }
}

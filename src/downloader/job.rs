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
    pub download_subtitles: bool,
    pub subtitle_language: Option<String>,
    pub download_thumbnail: bool,
    pub thumbnail_url: Option<String>,
    pub audio_format: Option<String>,
    pub audio_bitrate: Option<String>,
    pub embed_artwork: bool,
}

impl DownloadJob {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        url: String,
        title: String,
        output_path: PathBuf,
        total_bytes: Option<u64>,
        is_extractor: bool,
        is_audio_only: bool,
        quality: Option<String>,
        download_subtitles: bool,
        subtitle_language: Option<String>,
        download_thumbnail: bool,
        thumbnail_url: Option<String>,
        audio_format: Option<String>,
        audio_bitrate: Option<String>,
        embed_artwork: bool,
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
            download_subtitles,
            subtitle_language,
            download_thumbnail,
            thumbnail_url,
            audio_format,
            audio_bitrate,
            embed_artwork,
        }
    }

    pub fn update_progress(&mut self, progress: DownloadProgress) {
        self.downloaded_bytes = progress.downloaded_bytes;
        if let Some(total) = progress.total_bytes {
            // Only accept total if it is at least as large as what's currently downloaded,
            // or if we have no total recorded yet. This prevents small auxiliary downloads
            // (such as 8 KB subtitles) from corrupting the media stream's total size.
            if total >= self.downloaded_bytes || self.total_bytes.is_none() {
                self.total_bytes = Some(total);
            }
        } else if let Some(existing_total) = self.total_bytes {
            // If the recorded total is strictly smaller than downloaded bytes, it is stale
            if self.downloaded_bytes > existing_total {
                self.total_bytes = None;
            }
        }
        self.speed_bytes_sec = progress.speed_bytes_sec;
        self.eta_seconds = progress.eta_seconds;
        self.progress_ratio = progress.progress_ratio;
    }

    pub fn size_display(&self) -> String {
        if self.status == DownloadStatus::Completed {
            let total = self.total_bytes.unwrap_or(self.downloaded_bytes);
            return format!("{} (100%)", DownloadProgress::format_size(total));
        }

        if self.status == DownloadStatus::Downloading && self.downloaded_bytes == 0 {
            if let Some(total) = self.total_bytes {
                return format!("Connecting & decrypting stream... ({})", DownloadProgress::format_size(total));
            }
            return "Connecting & decrypting stream...".to_string();
        }

        let current = DownloadProgress::format_size(self.downloaded_bytes);
        let pct = (self.progress_ratio * 100.0).round() as u32;
        match self.total_bytes {
            Some(total) if total >= self.downloaded_bytes => {
                let total_str = DownloadProgress::format_size(total);
                format!("{} / {} ({}%)", current, total_str, pct)
            }
            _ => {
                if pct > 0 {
                    format!("{} ({}%)", current, pct)
                } else {
                    current
                }
            }
        }
    }

    pub fn speed_display(&self) -> String {
        if self.status == DownloadStatus::Completed {
            return "Finished".to_string();
        }
        if self.status == DownloadStatus::Downloading && self.downloaded_bytes == 0 {
            return "Connecting...".to_string();
        }
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
        if self.status == DownloadStatus::Completed {
            return "Complete".to_string();
        }
        if self.status == DownloadStatus::Downloading && self.downloaded_bytes == 0 {
            return "Preparing...".to_string();
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_job_progress_and_size_display() {
        let mut job = DownloadJob::new(
            "http://example.com/video".to_string(),
            "EP01.mp4".to_string(),
            PathBuf::from("/tmp/EP01.mp4"),
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
        );

        // Subtitle finishes first: 8600 bytes
        job.update_progress(DownloadProgress {
            downloaded_bytes: 8600,
            total_bytes: Some(8600),
            speed_bytes_sec: 10000.0,
            eta_seconds: Some(0),
            progress_ratio: 1.0,
        });
        assert_eq!(job.total_bytes, Some(8600));

        // Video starts downloading: 13.8 MB downloaded, estimated total 60 MB
        job.update_progress(DownloadProgress {
            downloaded_bytes: 14_470_000,
            total_bytes: Some(62_914_560), // 60 MB
            speed_bytes_sec: 618_200.0,
            eta_seconds: Some(78),
            progress_ratio: 0.23,
        });
        assert_eq!(job.total_bytes, Some(62_914_560));
        assert_eq!(job.size_display(), "13.8 MB / 60.0 MB (23%)");

        // Stale total smaller than downloaded bytes is automatically rejected/cleared
        job.update_progress(DownloadProgress {
            downloaded_bytes: 20_000_000,
            total_bytes: None,
            speed_bytes_sec: 500_000.0,
            eta_seconds: None,
            progress_ratio: 0.30,
        });
        // Total remains 60 MB because 60 MB >= 20 MB
        assert_eq!(job.total_bytes, Some(62_914_560));
    }
}

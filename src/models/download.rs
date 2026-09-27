use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(dead_code)]
pub enum DownloadStatus {
    Queued,
    Downloading,
    Paused,
    Completed,
    Failed(String),
    Cancelled,
}

#[allow(dead_code)]
impl DownloadStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            DownloadStatus::Queued => "Queued",
            DownloadStatus::Downloading => "Downloading",
            DownloadStatus::Paused => "Paused",
            DownloadStatus::Completed => "Completed",
            DownloadStatus::Failed(_) => "Failed",
            DownloadStatus::Cancelled => "Cancelled",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bytes_sec: f64,
    pub eta_seconds: Option<u64>,
    pub progress_ratio: f32, // 0.0 to 1.0
}

impl Default for DownloadProgress {
    fn default() -> Self {
        Self {
            downloaded_bytes: 0,
            total_bytes: None,
            speed_bytes_sec: 0.0,
            eta_seconds: None,
            progress_ratio: 0.0,
        }
    }
}

impl DownloadProgress {
    pub fn format_size(bytes: u64) -> String {
        const KB: f64 = 1024.0;
        const MB: f64 = KB * 1024.0;
        const GB: f64 = MB * 1024.0;

        let b = bytes as f64;
        if b >= GB {
            format!("{:.2} GB", b / GB)
        } else if b >= MB {
            format!("{:.1} MB", b / MB)
        } else if b >= KB {
            format!("{:.1} KB", b / KB)
        } else {
            format!("{} B", bytes)
        }
    }

    pub fn format_speed_val(speed: f64) -> String {
        const KB: f64 = 1024.0;
        const MB: f64 = KB * 1024.0;

        if speed >= MB {
            format!("{:.2} MB/s", speed / MB)
        } else if speed >= KB {
            format!("{:.1} KB/s", speed / KB)
        } else {
            format!("{:.0} B/s", speed)
        }
    }

    pub fn format_speed(&self) -> String {
        Self::format_speed_val(self.speed_bytes_sec)
    }

    pub fn format_eta(&self) -> String {
        let eta = self.eta_seconds.or_else(|| {
            if self.speed_bytes_sec > 0.0 {
                self.total_bytes.and_then(|t| {
                    if t > self.downloaded_bytes {
                        Some(((t - self.downloaded_bytes) as f64 / self.speed_bytes_sec).round() as u64)
                    } else {
                        Some(0)
                    }
                })
            } else {
                None
            }
        });

        match eta {
            Some(secs) => {
                let hours = secs / 3600;
                let minutes = (secs % 3600) / 60;
                let seconds = secs % 60;
                if hours > 0 {
                    format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
                } else {
                    format!("{:02}:{:02}", minutes, seconds)
                }
            }
            None => "--:--".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct DownloadJob {
    pub id: Uuid,
    pub url: String,
    pub title: String,
    pub filename: String,
    pub output_path: String,
    pub status: DownloadStatus,
    pub progress: DownloadProgress,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistEntry {
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoMetadata {
    pub url: String,
    pub title: String,
    pub content_length: Option<u64>,
    pub content_type: Option<String>,
    pub supports_ranges: bool,
    pub is_extractor: bool,
    pub duration_seconds: Option<u64>,
    pub resolution: Option<String>,
    pub ext: Option<String>,
    pub is_playlist: bool,
    pub playlist_count: usize,
    pub playlist_entries: Vec<PlaylistEntry>,
    pub has_subtitles: bool,
    pub subtitles_summary: String,
    pub thumbnail_url: Option<String>,
}

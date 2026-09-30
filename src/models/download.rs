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
    Scheduled,
    Retrying(u64),
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
            DownloadStatus::Scheduled => "Scheduled",
            DownloadStatus::Retrying(_) => "Retrying",
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
                    if minutes > 0 {
                        format!("{}h {}m", hours, minutes)
                    } else {
                        format!("{}h", hours)
                    }
                } else if minutes > 0 {
                    format!("{}m {:02}s", minutes, seconds)
                } else if seconds > 0 {
                    format!("{}s", seconds)
                } else {
                    "Almost done".to_string()
                }
            }
            None => "--".to_string(),
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
    #[serde(default)]
    pub referer: Option<String>,
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

    #[serde(default)]
    pub fps: Option<f64>,
    #[serde(default)]
    pub vcodec: Option<String>,
    #[serde(default)]
    pub acodec: Option<String>,
    #[serde(default)]
    pub size_best: Option<u64>,
    #[serde(default)]
    pub size_1080p: Option<u64>,
    #[serde(default)]
    pub size_720p: Option<u64>,
    #[serde(default)]
    pub size_480p: Option<u64>,
    #[serde(default)]
    pub size_audio: Option<u64>,
    #[serde(default)]
    pub referer: Option<String>,
    #[serde(default)]
    pub available_formats: Vec<StreamFormatInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct StreamFormatInfo {
    pub format_id: String,
    pub quality_label: String,
    pub resolution: String,
    pub fps_text: String,
    pub video_codec: String,
    pub audio_codec: String,
    pub container: String,
    pub size_text: String,
    pub is_video: bool,
    pub is_recommended: bool,
}

impl Default for VideoMetadata {
    fn default() -> Self {
        Self {
            url: String::new(),
            title: String::new(),
            content_length: None,
            content_type: None,
            supports_ranges: false,
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
            fps: None,
            vcodec: None,
            acodec: None,
            size_best: None,
            size_1080p: None,
            size_720p: None,
            size_480p: None,
            size_audio: None,
            referer: None,
            available_formats: Vec::new(),
        }
    }
}

impl VideoMetadata {
    pub fn ensure_available_formats(&mut self) {
        if !self.available_formats.is_empty() {
            return;
        }

        let best_size_text = self.size_best.or(self.content_length)
            .map(|s| format!("~{}", crate::models::DownloadProgress::format_size(s)))
            .unwrap_or_default();
        let size_1080_text = self.size_1080p
            .map(|s| format!("~{}", crate::models::DownloadProgress::format_size(s)))
            .unwrap_or_default();
        let size_720_text = self.size_720p
            .map(|s| format!("~{}", crate::models::DownloadProgress::format_size(s)))
            .unwrap_or_default();
        let size_480_text = self.size_480p
            .map(|s| format!("~{}", crate::models::DownloadProgress::format_size(s)))
            .unwrap_or_default();
        let size_audio_text = self.size_audio
            .map(|s| format!("~{}", crate::models::DownloadProgress::format_size(s)))
            .unwrap_or_default();

        let vc = self.vcodec.clone().unwrap_or_else(|| "H.264".to_string());
        let ac = self.acodec.clone().unwrap_or_else(|| "AAC".to_string());
        let fps_text = self.fps.map(|f| format!("{:.0} FPS", f)).unwrap_or_default();

        let mut streams = Vec::new();

        streams.push(StreamFormatInfo {
            format_id: "best".to_string(),
            quality_label: "Best Available Stream".to_string(),
            resolution: self.resolution.clone().unwrap_or_else(|| "Native".to_string()),
            fps_text: fps_text.clone(),
            video_codec: vc.clone(),
            audio_codec: ac.clone(),
            container: self.ext.clone().unwrap_or_else(|| "mp4".to_string()).to_uppercase(),
            size_text: best_size_text,
            is_video: true,
            is_recommended: true,
        });

        streams.push(StreamFormatInfo {
            format_id: "1080p".to_string(),
            quality_label: "1080p Full HD".to_string(),
            resolution: "1920x1080".to_string(),
            fps_text: fps_text.clone(),
            video_codec: vc.clone(),
            audio_codec: ac.clone(),
            container: "MP4".to_string(),
            size_text: size_1080_text,
            is_video: true,
            is_recommended: false,
        });

        streams.push(StreamFormatInfo {
            format_id: "720p".to_string(),
            quality_label: "720p High Def".to_string(),
            resolution: "1280x720".to_string(),
            fps_text,
            video_codec: vc,
            audio_codec: ac,
            container: "MP4".to_string(),
            size_text: size_720_text,
            is_video: true,
            is_recommended: false,
        });

        streams.push(StreamFormatInfo {
            format_id: "480p".to_string(),
            quality_label: "480p Standard".to_string(),
            resolution: "854x480".to_string(),
            fps_text: "".to_string(),
            video_codec: "H.264".to_string(),
            audio_codec: "AAC".to_string(),
            container: "MP4".to_string(),
            size_text: size_480_text,
            is_video: true,
            is_recommended: false,
        });

        streams.push(StreamFormatInfo {
            format_id: "audio".to_string(),
            quality_label: "Audio Track Only".to_string(),
            resolution: "Audio Only".to_string(),
            fps_text: "".to_string(),
            video_codec: "None".to_string(),
            audio_codec: "MP3 / AAC".to_string(),
            container: "MP3".to_string(),
            size_text: size_audio_text,
            is_video: false,
            is_recommended: false,
        });

        self.available_formats = streams;
    }
}


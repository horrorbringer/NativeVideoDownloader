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
    pub referer: Option<String>,
    pub scheduled_at: Option<u64>,
    pub auto_retry_at: Option<u64>,
    pub auto_retry_count: u32,
    pub bypass_schedule: bool,
    pub fragment_index: Option<u32>,
    pub fragment_count: Option<u32>,
    pub format_note: Option<String>,
    pub resolution: Option<String>,
    pub phase: Option<String>,
    pub cached_thumbnail_path: Option<PathBuf>,
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
            referer: None,
            scheduled_at: None,
            auto_retry_at: None,
            auto_retry_count: 0,
            bypass_schedule: false,
            fragment_index: None,
            fragment_count: None,
            format_note: None,
            resolution: None,
            phase: None,
            cached_thumbnail_path: None,
        }
    }

    #[allow(dead_code)]
    pub fn with_referer(mut self, referer: Option<String>) -> Self {
        self.referer = referer;
        self
    }

    pub fn update_progress(&mut self, progress: DownloadProgress) {
        if progress.downloaded_bytes > 0 || self.downloaded_bytes == 0 {
            self.downloaded_bytes = progress.downloaded_bytes;
        }
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
        if progress.speed_bytes_sec > 0.0 || progress.downloaded_bytes > 0 {
            self.speed_bytes_sec = progress.speed_bytes_sec;
        }
        if progress.eta_seconds.is_some() {
            self.eta_seconds = progress.eta_seconds;
        }
        if progress.progress_ratio > 0.0 {
            self.progress_ratio = progress.progress_ratio;
        }
        if progress.fragment_index.is_some() {
            self.fragment_index = progress.fragment_index;
        }
        if progress.fragment_count.is_some() {
            self.fragment_count = progress.fragment_count;
        }
        if let Some(ref note) = progress.format_note {
            if !note.is_empty() && !note.eq_ignore_ascii_case("na") {
                self.format_note = Some(note.clone());
            }
        }
        if let Some(ref res) = progress.resolution {
            if !res.is_empty() && !res.eq_ignore_ascii_case("na") {
                self.resolution = Some(res.clone());
            }
        }
        if let Some(ref ph) = progress.phase {
            if !ph.is_empty() {
                self.phase = Some(ph.clone());
            }
        }
    }

    pub fn source_platform(&self) -> String {
        let s = self.url.trim().to_lowercase();
        if s.contains("youtube.com") || s.contains("youtu.be") {
            "YouTube".to_string()
        } else if s.contains("kisskh") {
            "KissKH".to_string()
        } else if s.contains("bilibili") {
            "Bilibili".to_string()
        } else if s.contains("tiktok.com") {
            "TikTok".to_string()
        } else if s.contains("douyin.com") {
            "Douyin".to_string()
        } else if s.contains("facebook.com") || s.contains("fb.watch") {
            "Facebook".to_string()
        } else if s.contains("twitter.com") || s.contains("x.com") {
            "X / Twitter".to_string()
        } else if s.contains("instagram.com") {
            "Instagram".to_string()
        } else if s.contains("vimeo.com") {
            "Vimeo".to_string()
        } else if s.contains("dailymotion.com") {
            "Dailymotion".to_string()
        } else if s.contains("reddit.com") {
            "Reddit".to_string()
        } else if let Ok(parsed) = reqwest::Url::parse(&self.url) {
            if let Some(host) = parsed.host_str() {
                host.trim_start_matches("www.").to_string()
            } else {
                "Direct Stream".to_string()
            }
        } else {
            "Direct Stream".to_string()
        }
    }

    pub fn quality_badge(&self) -> String {
        if self.is_audio_only {
            if let Some(ref br) = self.audio_bitrate {
                format!("Audio {}k", br)
            } else {
                "Audio Only".to_string()
            }
        } else {
            let candidate = self.quality.as_deref()
                .or(self.resolution.as_deref())
                .or(self.format_note.as_deref())
                .unwrap_or("");
            let lower = candidate.to_lowercase();
            if lower.contains("2160") || lower.contains("4k") {
                "4K UHD".to_string()
            } else if lower.contains("1440") || lower.contains("2k") {
                "1440p QHD".to_string()
            } else if lower.contains("1080") {
                "1080p FHD".to_string()
            } else if lower.contains("720") {
                "720p HD".to_string()
            } else if lower.contains("480") {
                "480p SD".to_string()
            } else if lower.contains("360") {
                "360p".to_string()
            } else if !candidate.is_empty() && !candidate.eq_ignore_ascii_case("best") {
                candidate.to_string()
            } else {
                "HD Video".to_string()
            }
        }
    }

    pub fn format_badge(&self) -> String {
        if self.is_audio_only {
            self.audio_format.as_deref().unwrap_or("MP3").to_uppercase()
        } else if let Some(ext) = self.output_path.extension().and_then(|e| e.to_str()) {
            let lower = ext.to_lowercase();
            if lower == "part" {
                if let Some(stem) = self.output_path.file_stem().and_then(|s| s.to_str()) {
                    if let Some((_, sub_ext)) = stem.rsplit_once('.') {
                        return sub_ext.to_uppercase();
                    }
                }
                "MP4".to_string()
            } else {
                ext.to_uppercase()
            }
        } else {
            "MP4".to_string()
        }
    }

    pub fn detail_status_display(&self) -> String {
        match &self.status {
            DownloadStatus::Downloading => {
                if let Some(ref ph) = self.phase {
                    ph.clone()
                } else if let (Some(idx), Some(total)) = (self.fragment_index, self.fragment_count) {
                    format!("Downloading fragment {}/{}", idx, total)
                } else if let Some(idx) = self.fragment_index {
                    format!("Downloading fragment #{}", idx)
                } else if self.downloaded_bytes == 0 {
                    "Connecting to media server...".to_string()
                } else if self.is_audio_only {
                    format!("Downloading audio stream • {}", self.format_badge())
                } else {
                    format!("Downloading video stream • {}", self.quality_badge())
                }
            }
            DownloadStatus::Paused => "Download paused".to_string(),
            DownloadStatus::Queued => "Waiting in queue".to_string(),
            DownloadStatus::Scheduled => "Scheduled for later".to_string(),
            DownloadStatus::Completed => "Complete • Ready to watch".to_string(),
            DownloadStatus::Retrying(target_ts) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let rem = target_ts.saturating_sub(now);
                format!("Connection lost • Retrying in {}s", rem)
            }
            DownloadStatus::Failed(err) => {
                if err.is_empty() {
                    "Download interrupted".to_string()
                } else {
                    format!("Failed: {}", err)
                }
            }
            DownloadStatus::Cancelled => "Download cancelled".to_string(),
        }
    }

    pub fn destination_display(&self) -> String {
        let fname = self.output_path.file_name().and_then(|s| s.to_str()).unwrap_or(&self.title);
        let path_str = self.output_path.to_string_lossy();
        if let Ok(home) = std::env::var("HOME") {
            if path_str.starts_with(&home) {
                return path_str.replacen(&home, "~", 1);
            }
        }
        if let Some(parent) = self.output_path.parent() {
            let parent_name = parent.file_name().and_then(|s| s.to_str()).unwrap_or("Downloads");
            format!("{}/{}", parent_name, fname)
        } else {
            fname.to_string()
        }
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
            fragment_index: self.fragment_index,
            fragment_count: self.fragment_count,
            format_note: self.format_note.clone(),
            resolution: self.resolution.clone(),
            phase: self.phase.clone(),
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
            fragment_index: self.fragment_index,
            fragment_count: self.fragment_count,
            format_note: self.format_note.clone(),
            resolution: self.resolution.clone(),
            phase: self.phase.clone(),
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
            ..Default::default()
        });
        assert_eq!(job.total_bytes, Some(8600));

        // Video starts downloading: 13.8 MB downloaded, estimated total 60 MB
        job.update_progress(DownloadProgress {
            downloaded_bytes: 14_470_000,
            total_bytes: Some(62_914_560), // 60 MB
            speed_bytes_sec: 618_200.0,
            eta_seconds: Some(78),
            progress_ratio: 0.23,
            ..Default::default()
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
            ..Default::default()
        });
        // Total remains 60 MB because 60 MB >= 20 MB
        assert_eq!(job.total_bytes, Some(62_914_560));
    }

    #[test]
    fn test_job_referer() {
        let job = DownloadJob::new(
            "https://cdn.example.com/video.mp4".to_string(),
            "Sample Video".to_string(),
            PathBuf::from("/tmp/video.mp4"),
            None,
            false,
            false,
            None,
            false,
            None,
            false,
            None,
            None,
            None,
            false,
        ).with_referer(Some("https://example.com/watch/123".to_string()));

        assert_eq!(job.referer, Some("https://example.com/watch/123".to_string()));
    }

    #[test]
    fn test_job_ux_badges_and_detail_status() {
        let mut job = DownloadJob::new(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ".to_string(),
            "Never Gonna Give You Up".to_string(),
            PathBuf::from("/Users/test/Downloads/Never Gonna Give You Up.mp4"),
            Some(100_000_000),
            true,
            false,
            Some("1080p".to_string()),
            false,
            None,
            false,
            None,
            None,
            None,
            false,
        );

        assert_eq!(job.source_platform(), "YouTube");
        assert_eq!(job.quality_badge(), "1080p FHD");
        assert_eq!(job.format_badge(), "MP4");
        assert!(job.destination_display().contains("Never Gonna Give You Up.mp4"));

        // When downloading without fragments or phase
        job.status = DownloadStatus::Downloading;
        job.downloaded_bytes = 10_000_000;
        assert_eq!(job.detail_status_display(), "Downloading video stream • 1080p FHD");

        // When fragment progress arrives
        job.update_progress(DownloadProgress {
            downloaded_bytes: 20_000_000,
            total_bytes: Some(100_000_000),
            speed_bytes_sec: 5_000_000.0,
            eta_seconds: Some(16),
            progress_ratio: 0.2,
            fragment_index: Some(15),
            fragment_count: Some(75),
            format_note: Some("1080p60".to_string()),
            resolution: Some("1920x1080".to_string()),
            phase: None,
        });
        assert_eq!(job.detail_status_display(), "Downloading fragment 15/75");

        // When postprocessing phase arrives
        job.update_progress(DownloadProgress {
            downloaded_bytes: 100_000_000,
            total_bytes: Some(100_000_000),
            speed_bytes_sec: 0.0,
            eta_seconds: None,
            progress_ratio: 0.99,
            fragment_index: None,
            fragment_count: None,
            format_note: None,
            resolution: None,
            phase: Some("Merging video & audio streams...".to_string()),
        });
        assert_eq!(job.detail_status_display(), "Merging video & audio streams...");

        // Audio-only job testing
        let audio_job = DownloadJob::new(
            "https://kisskh.co/Drama/Episode-1".to_string(),
            "Theme Song".to_string(),
            PathBuf::from("/Users/test/Downloads/Theme Song.mp3"),
            None,
            true,
            true,
            None,
            false,
            None,
            false,
            None,
            Some("mp3".to_string()),
            Some("320".to_string()),
            false,
        );
        assert_eq!(audio_job.source_platform(), "KissKH");
        assert_eq!(audio_job.quality_badge(), "Audio 320k");
        assert_eq!(audio_job.format_badge(), "MP3");
    }
}

use std::time::Instant;
use crate::models::DownloadProgress;

pub struct ProgressCalculator {
    total_bytes: Option<u64>,
    downloaded_bytes: u64,
    _start_time: Instant,
    last_sample_time: Instant,
    last_sample_bytes: u64,
    current_speed: f64,
}

impl ProgressCalculator {
    pub fn new(total_bytes: Option<u64>) -> Self {
        Self::with_initial_bytes(total_bytes, 0)
    }

    pub fn with_initial_bytes(total_bytes: Option<u64>, downloaded_bytes: u64) -> Self {
        let now = Instant::now();
        Self {
            total_bytes,
            downloaded_bytes,
            _start_time: now,
            last_sample_time: now,
            last_sample_bytes: downloaded_bytes,
            current_speed: 0.0,
        }
    }

    /// Dynamically update total expected bytes if discovered after stream connection
    #[allow(dead_code)]
    pub fn set_total_bytes(&mut self, total_bytes: Option<u64>) {
        self.total_bytes = total_bytes;
    }

    /// Total elapsed duration since the download started
    #[allow(dead_code)]
    pub fn elapsed(&self) -> std::time::Duration {
        self._start_time.elapsed()
    }

    /// Current smoothed download speed in bytes/sec
    #[allow(dead_code)]
    pub fn current_speed(&self) -> f64 {
        self.current_speed
    }

    /// Total bytes downloaded so far
    #[allow(dead_code)]
    pub fn downloaded_bytes(&self) -> u64 {
        self.downloaded_bytes
    }

    pub fn update(&mut self, chunk_len: usize) -> DownloadProgress {
        self.downloaded_bytes += chunk_len as u64;
        let now = Instant::now();
        let elapsed_since_sample = now.duration_since(self.last_sample_time).as_secs_f64();

        // Update speed calculation every 250ms to smooth out spikes
        if elapsed_since_sample >= 0.25 {
            let bytes_in_interval = (self.downloaded_bytes - self.last_sample_bytes) as f64;
            let instant_speed = bytes_in_interval / elapsed_since_sample;

            if bytes_in_interval == 0.0 && elapsed_since_sample >= 1.0 {
                // Decay speed towards zero if stalled or idle
                self.current_speed *= 0.5;
                if self.current_speed < 1.0 {
                    self.current_speed = 0.0;
                }
            } else if self.current_speed == 0.0 {
                self.current_speed = instant_speed;
            } else {
                // Exponential moving average for smooth display
                self.current_speed = self.current_speed * 0.7 + instant_speed * 0.3;
            }

            self.last_sample_time = now;
            self.last_sample_bytes = self.downloaded_bytes;
        }

        let progress_ratio = match self.total_bytes {
            Some(total) if total > 0 => (self.downloaded_bytes as f32 / total as f32).min(1.0),
            _ => 0.0,
        };

        let eta_seconds = if self.current_speed > 0.0 {
            self.total_bytes.and_then(|total| {
                if total > self.downloaded_bytes {
                    let remaining = (total - self.downloaded_bytes) as f64;
                    Some((remaining / self.current_speed) as u64)
                } else {
                    Some(0)
                }
            })
        } else {
            None
        };

        DownloadProgress {
            downloaded_bytes: self.downloaded_bytes,
            total_bytes: self.total_bytes,
            speed_bytes_sec: self.current_speed,
            eta_seconds,
            progress_ratio,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_calculation() {
        let total = Some(1000);
        let mut calc = ProgressCalculator::new(total);
        let p1 = calc.update(250);

        assert_eq!(p1.downloaded_bytes, 250);
        assert_eq!(p1.total_bytes, Some(1000));
        assert!((p1.progress_ratio - 0.25).abs() < 0.001);
    }

    #[test]
    fn test_format_size_and_eta() {
        assert_eq!(DownloadProgress::format_size(500), "500 B");
        assert_eq!(DownloadProgress::format_size(1024), "1.0 KB");
        assert_eq!(DownloadProgress::format_size(1024 * 1024 * 5), "5.0 MB");

        let p = DownloadProgress {
            downloaded_bytes: 50,
            total_bytes: Some(100),
            speed_bytes_sec: 1024.0 * 1024.0 * 2.5,
            eta_seconds: Some(65),
            progress_ratio: 0.5,
        };

        assert_eq!(p.format_eta(), "1m 05s");
        assert_eq!(p.format_speed(), "2.50 MB/s");
    }

    #[test]
    fn test_progress_calculator_methods() {
        let mut calc = ProgressCalculator::new(None);
        assert_eq!(calc.downloaded_bytes(), 0);
        assert!(calc.elapsed().as_millis() < 5000);

        calc.set_total_bytes(Some(2000));
        let p = calc.update(500);
        assert_eq!(p.downloaded_bytes, 500);
        assert_eq!(p.total_bytes, Some(2000));
        assert_eq!(calc.downloaded_bytes(), 500);
    }
}

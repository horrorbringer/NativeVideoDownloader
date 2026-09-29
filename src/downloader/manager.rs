use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::database::Database;
use crate::downloader::job::DownloadJob;
use crate::downloader::queue::DownloadQueue;
use crate::downloader::retry::RetryPolicy;
use crate::error::AppError;
use crate::filesystem::part_path_for;
use crate::models::DownloadStatus;
use crate::network::NetworkClient;

pub type StatusUpdateCallback = Arc<dyn Fn() + Send + Sync + 'static>;

pub fn parse_speed_limit_bytes(limit_str: Option<&str>) -> Option<u64> {
    let s = limit_str?;
    let s = s.trim().to_uppercase();
    if s.is_empty() || s == "UNLIMITED" {
        return None;
    }
    if s.ends_with('M') {
        let mb: f64 = s.trim_end_matches('M').parse().ok()?;
        Some((mb * 1024.0 * 1024.0) as u64)
    } else if s.ends_with('K') {
        let kb: f64 = s.trim_end_matches('K').parse().ok()?;
        Some((kb * 1024.0) as u64)
    } else {
        s.parse::<u64>().ok()
    }
}

pub struct DownloadManager {
    queue: Arc<Mutex<DownloadQueue>>,
    max_concurrency: Arc<RwLock<usize>>,
    network_client: Arc<NetworkClient>,
    retry_policy: RetryPolicy,
    db: Arc<Database>,
    speed_limit: Arc<RwLock<Option<String>>>,
    cookies_browser: Arc<RwLock<Option<String>>>,
    proxy: Arc<RwLock<Option<String>>>,
    filename_template: Arc<RwLock<String>>,
    asset_org_mode: Arc<RwLock<u8>>,
    concurrent_fragments: Arc<RwLock<u8>>,
    file_conflict_policy: Arc<RwLock<crate::filesystem::FileConflictPolicy>>,
    scheduler_enabled: Arc<RwLock<bool>>,
    schedule_start_hour: Arc<RwLock<u32>>,
    schedule_start_minute: Arc<RwLock<u32>>,
    schedule_end_hour: Arc<RwLock<u32>>,
    schedule_end_minute: Arc<RwLock<u32>>,
    auto_retry_enabled: Arc<RwLock<bool>>,
    auto_retry_interval_secs: Arc<RwLock<u64>>,
    auto_retry_max_attempts: Arc<RwLock<u32>>,
    on_update: Mutex<Option<StatusUpdateCallback>>,
}

impl DownloadManager {
    pub fn new(max_concurrent: usize, db: Arc<Database>) -> Self {
        Self {
            queue: Arc::new(Mutex::new(DownloadQueue::new())),
            max_concurrency: Arc::new(RwLock::new(max_concurrent.clamp(1, 10))),
            network_client: Arc::new(NetworkClient::new()),
            retry_policy: RetryPolicy::default(),
            db,
            speed_limit: Arc::new(RwLock::new(None)),
            cookies_browser: Arc::new(RwLock::new(None)),
            proxy: Arc::new(RwLock::new(None)),
            filename_template: Arc::new(RwLock::new("{title}.{ext}".to_string())),
            asset_org_mode: Arc::new(RwLock::new(0)),
            concurrent_fragments: Arc::new(RwLock::new(4)),
            file_conflict_policy: Arc::new(RwLock::new(crate::filesystem::FileConflictPolicy::AutoResumeOrRename)),
            scheduler_enabled: Arc::new(RwLock::new(false)),
            schedule_start_hour: Arc::new(RwLock::new(2)),
            schedule_start_minute: Arc::new(RwLock::new(0)),
            schedule_end_hour: Arc::new(RwLock::new(7)),
            schedule_end_minute: Arc::new(RwLock::new(0)),
            auto_retry_enabled: Arc::new(RwLock::new(true)),
            auto_retry_interval_secs: Arc::new(RwLock::new(60)),
            auto_retry_max_attempts: Arc::new(RwLock::new(3)),
            on_update: Mutex::new(None),
        }
    }

    #[allow(dead_code)]
    pub fn database(&self) -> Arc<Database> {
        self.db.clone()
    }

    pub async fn set_update_listener(&self, callback: impl Fn() + Send + Sync + 'static) {
        let mut guard = self.on_update.lock().await;
        *guard = Some(Arc::new(callback));
    }

    pub async fn notify_update(&self) {
        let guard = self.on_update.lock().await;
        if let Some(cb) = guard.as_ref() {
            cb();
        }
    }

    pub async fn set_speed_limit(&self, limit: Option<String>) {
        let mut guard = self.speed_limit.write().await;
        *guard = limit;
    }

    pub async fn get_speed_limit(&self) -> Option<String> {
        let guard = self.speed_limit.read().await;
        guard.clone()
    }

    pub async fn set_cookies_browser(&self, browser: Option<String>) {
        let mut guard = self.cookies_browser.write().await;
        *guard = browser;
    }

    pub async fn get_cookies_browser(&self) -> Option<String> {
        let guard = self.cookies_browser.read().await;
        guard.clone()
    }

    pub async fn set_proxy(&self, proxy: Option<String>) {
        let mut guard = self.proxy.write().await;
        *guard = proxy;
    }

    pub async fn get_proxy(&self) -> Option<String> {
        let guard = self.proxy.read().await;
        guard.clone()
    }

    pub async fn set_filename_template(&self, template: String) {
        let mut guard = self.filename_template.write().await;
        *guard = template;
    }

    pub async fn get_filename_template(&self) -> String {
        let guard = self.filename_template.read().await;
        guard.clone()
    }

    pub async fn set_asset_organization_mode(&self, mode: u8) {
        let mut guard = self.asset_org_mode.write().await;
        *guard = mode;
    }

    pub async fn get_asset_organization_mode(&self) -> u8 {
        let guard = self.asset_org_mode.read().await;
        *guard
    }

    pub async fn set_concurrent_fragments(&self, count: u8) {
        let mut guard = self.concurrent_fragments.write().await;
        *guard = count.clamp(1, 16);
    }

    pub async fn get_concurrent_fragments(&self) -> u8 {
        let guard = self.concurrent_fragments.read().await;
        *guard
    }

    pub async fn set_file_conflict_policy(&self, policy: crate::filesystem::FileConflictPolicy) {
        let mut guard = self.file_conflict_policy.write().await;
        *guard = policy;
    }

    pub async fn get_file_conflict_policy(&self) -> crate::filesystem::FileConflictPolicy {
        let guard = self.file_conflict_policy.read().await;
        *guard
    }

    pub async fn set_scheduler_config(&self, enabled: bool, start_h: u32, start_m: u32, end_h: u32, end_m: u32) {
        *self.scheduler_enabled.write().await = enabled;
        *self.schedule_start_hour.write().await = start_h.min(23);
        *self.schedule_start_minute.write().await = start_m.min(59);
        *self.schedule_end_hour.write().await = end_h.min(23);
        *self.schedule_end_minute.write().await = end_m.min(59);
    }

    pub async fn get_scheduler_config(&self) -> (bool, u32, u32, u32, u32) {
        (
            *self.scheduler_enabled.read().await,
            *self.schedule_start_hour.read().await,
            *self.schedule_start_minute.read().await,
            *self.schedule_end_hour.read().await,
            *self.schedule_end_minute.read().await,
        )
    }

    pub async fn is_in_schedule_window(&self) -> bool {
        let (enabled, sh, sm, eh, em) = self.get_scheduler_config().await;
        if !enabled {
            return true;
        }
        use chrono::Timelike;
        let now = chrono::Local::now();
        let now_mins = now.hour() * 60 + now.minute();
        let start_mins = sh * 60 + sm;
        let end_mins = eh * 60 + em;

        if start_mins <= end_mins {
            now_mins >= start_mins && now_mins < end_mins
        } else {
            // Crosses midnight, e.g. 23:00 to 06:00
            now_mins >= start_mins || now_mins < end_mins
        }
    }

    pub async fn set_auto_retry_config(&self, enabled: bool, interval_secs: u64, max_attempts: u32) {
        *self.auto_retry_enabled.write().await = enabled;
        *self.auto_retry_interval_secs.write().await = interval_secs.max(5);
        *self.auto_retry_max_attempts.write().await = max_attempts.max(1);
    }

    pub async fn get_auto_retry_config(&self) -> (bool, u64, u32) {
        (
            *self.auto_retry_enabled.read().await,
            *self.auto_retry_interval_secs.read().await,
            *self.auto_retry_max_attempts.read().await,
        )
    }

    pub async fn bypass_scheduler_and_start_all(self: &Arc<Self>) {
        {
            let mut queue = self.queue.lock().await;
            for j in queue.all_jobs_mut() {
                if j.status == DownloadStatus::Scheduled || j.status == DownloadStatus::Queued {
                    j.bypass_schedule = true;
                    j.status = DownloadStatus::Queued;
                }
            }
        }
        self.notify_update().await;
        self.process_queue().await;
    }

    pub fn start_background_scheduler(self: &Arc<Self>) {
        let manager = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
            loop {
                interval.tick().await;

                let now_ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                let (sched_enabled, _, _, _, _) = manager.get_scheduler_config().await;
                let in_window = manager.is_in_schedule_window().await;

                let mut queue_changed = false;
                let mut has_retrying_or_scheduled = false;

                {
                    let mut queue = manager.queue.lock().await;
                    for j in queue.all_jobs_mut() {
                        // Check auto-retry timers
                        if let Some(target_ts) = j.auto_retry_at {
                            if now_ts >= target_ts {
                                info!("Auto-retry timer reached for job {}. Re-queueing...", j.id);
                                j.auto_retry_at = None;
                                j.auto_retry_count += 1;
                                j.status = DownloadStatus::Queued;
                                queue_changed = true;
                            } else {
                                has_retrying_or_scheduled = true;
                            }
                        }

                        // Check scheduled jobs
                        if j.status == DownloadStatus::Scheduled {
                            if let Some(target_ts) = j.scheduled_at {
                                if now_ts >= target_ts {
                                    info!("Scheduled start timestamp reached for job {}. Re-queueing...", j.id);
                                    j.scheduled_at = None;
                                    j.status = DownloadStatus::Queued;
                                    queue_changed = true;
                                } else {
                                    has_retrying_or_scheduled = true;
                                }
                            } else if !sched_enabled || in_window {
                                info!("Off-peak window open for scheduled job {}. Re-queueing...", j.id);
                                j.status = DownloadStatus::Queued;
                                queue_changed = true;
                            } else {
                                has_retrying_or_scheduled = true;
                            }
                        }
                    }
                }

                if queue_changed {
                    manager.notify_update().await;
                    manager.process_queue().await;
                } else if has_retrying_or_scheduled {
                    manager.notify_update().await;
                }
            }
        });
    }

    pub async fn run_network_diagnostics(&self, target_url: &str) -> crate::network::NetworkDiagnosticReport {
        self.network_client.run_diagnostics(target_url).await
    }

    pub async fn set_max_concurrency(self: &Arc<Self>, count: usize) {
        let valid_count = count.clamp(1, 10);
        *self.max_concurrency.write().await = valid_count;
        info!("Updated max concurrent download workers to: {}", valid_count);
        self.process_queue().await;
        self.notify_update().await;
    }

    #[allow(dead_code)]
    pub async fn get_max_concurrency(&self) -> usize {
        *self.max_concurrency.read().await
    }

    pub async fn get_jobs_snapshot(&self) -> Vec<DownloadJob> {
        let queue = self.queue.lock().await;
        queue.all_jobs().to_vec()
    }

    /// Restore unfinished jobs from previous session (Crash Recovery)
    pub async fn restore_unfinished_jobs(self: &Arc<Self>) -> Result<usize, AppError> {
        let unfinished = self.db.get_unfinished_jobs().await?;
        let count = unfinished.len();
        if count == 0 {
            return Ok(0);
        }

        let mut queue = self.queue.lock().await;
        for mut job in unfinished {
            // Check if partial file exists on disk
            let part_path = part_path_for(&job.output_path);
            if part_path.exists() {
                if let Ok(metadata) = tokio::fs::metadata(&part_path).await {
                    job.downloaded_bytes = metadata.len();
                    if let Some(total) = job.total_bytes {
                        if total > 0 {
                            job.progress_ratio = (job.downloaded_bytes as f32 / total as f32).min(1.0);
                        }
                    }
                }
            }
            job.status = DownloadStatus::Paused; // Ready for user to resume
            queue.add_job(job);
        }
        drop(queue);

        info!("Crash recovery: restored {} unfinished downloads to queue", count);
        self.notify_update().await;
        self.process_queue().await;
        Ok(count)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn add_download(
        self: &Arc<Self>,
        url: String,
        title: String,
        output_dir: &PathBuf,
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
    ) -> Result<Uuid, AppError> {
        self.add_download_with_context(
            url,
            title,
            output_dir,
            total_bytes,
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
            None,
            None,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn add_download_with_context(
        self: &Arc<Self>,
        url: String,
        title: String,
        output_dir: &Path,
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
        series: Option<&str>,
        index: Option<usize>,
        referer: Option<String>,
    ) -> Result<Uuid, AppError> {
        let ext = if is_audio_only {
            audio_format.as_deref().unwrap_or("mp3")
        } else {
            crate::filesystem::split_stem_and_ext(&title).1.unwrap_or("mp4")
        };

        let template = self.get_filename_template().await;
        let ctx = crate::filesystem::FilenameContext {
            title: &title,
            ext,
            resolution: quality.as_deref(),
            series,
            index,
            date: None,
            group_subtitles: download_subtitles && series.is_none() && !is_audio_only,
        };

        let policy = self.get_file_conflict_policy().await;
        let resolved = crate::filesystem::resolve_template_destination_with_policy(
            output_dir,
            &template,
            &ctx,
            policy,
        )?;
        let destination = resolved.path;
        let final_title = destination.file_name().and_then(|s| s.to_str()).unwrap_or(&title).to_string();

        if resolved.should_skip {
            info!("Skipping download for '{}': completed file exists on disk and policy is SkipExisting", final_title);
            let existing_size = std::fs::metadata(&destination).map(|m| m.len()).unwrap_or(0);
            let mut job = DownloadJob::new(
                url,
                final_title,
                destination,
                Some(existing_size),
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
            );
            job.referer = referer;
            job.status = DownloadStatus::Completed;
            job.progress_ratio = 1.0;
            job.downloaded_bytes = existing_size;
            let id = job.id;

            let db = self.db.clone();
            let job_for_db = job.clone();
            tokio::spawn(async move {
                if let Err(err) = db.upsert_job(&job_for_db).await {
                    warn!("Failed to persist skipped job {} to database: {}", id, err);
                }
            });

            {
                let mut queue = self.queue.lock().await;
                queue.add_job(job);
            }
            self.notify_update().await;
            return Ok(id);
        }

        if resolved.is_resuming {
            info!("Resuming existing partial download for '{}' at {:?}", final_title, destination);
        }

        let mut job = DownloadJob::new(
            url,
            final_title,
            destination,
            total_bytes,
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
        );
        job.referer = referer;
        if *self.scheduler_enabled.read().await && !self.is_in_schedule_window().await {
            job.status = DownloadStatus::Scheduled;
        }
        let id = job.id;

        // Persist to database asynchronously so worker starts immediately
        let db = self.db.clone();
        let job_for_db = job.clone();
        tokio::spawn(async move {
            if let Err(err) = db.upsert_job(&job_for_db).await {
                warn!("Failed to persist new job {} to database: {}", id, err);
            }
        });

        {
            let mut queue = self.queue.lock().await;
            queue.add_job(job);
        }

        self.notify_update().await;
        self.process_queue().await;

        Ok(id)
    }

    pub async fn pause_job(self: &Arc<Self>, id: Uuid) {
        let mut queue = self.queue.lock().await;
        if let Some(job) = queue.get_job_mut(id) {
            if job.status == DownloadStatus::Downloading 
                || job.status == DownloadStatus::Queued 
                || job.status == DownloadStatus::Scheduled 
                || matches!(job.status, DownloadStatus::Retrying(_)) {
                info!("Pausing download job {}", id);
                if let Some(token) = job.cancel_token.take() {
                    token.cancel();
                }
                job.auto_retry_at = None;
                job.status = DownloadStatus::Paused;
                job.speed_bytes_sec = 0.0;
                job.eta_seconds = None;

                let _ = self.db.upsert_job(job).await;
            }
        }
        drop(queue);
        self.notify_update().await;
        self.process_queue().await;
    }

    pub async fn resume_job(self: &Arc<Self>, id: Uuid) {
        {
            let mut queue = self.queue.lock().await;
            if let Some(job) = queue.get_job_mut(id) {
                if job.status == DownloadStatus::Paused 
                    || matches!(job.status, DownloadStatus::Failed(_))
                    || matches!(job.status, DownloadStatus::Retrying(_))
                    || job.status == DownloadStatus::Scheduled {
                    info!("Resuming download job {}", id);
                    job.auto_retry_at = None;
                    job.bypass_schedule = true; // User manual trigger bypasses scheduler
                    job.status = DownloadStatus::Queued;
                    let _ = self.db.upsert_job(job).await;
                }
            }
        }
        self.notify_update().await;
        self.process_queue().await;
    }

    pub async fn cancel_job(self: &Arc<Self>, id: Uuid) {
        let mut queue = self.queue.lock().await;
        if let Some(job) = queue.get_job_mut(id) {
            info!("Cancelling download job {}", id);
            if let Some(token) = job.cancel_token.take() {
                token.cancel();
            }
            job.auto_retry_at = None;
            job.status = DownloadStatus::Cancelled;
            job.speed_bytes_sec = 0.0;
            job.eta_seconds = None;

            let _ = self.db.mark_cancelled(id).await;
        }
        drop(queue);
        self.notify_update().await;
        self.process_queue().await;
    }

    pub async fn pause_all(self: &Arc<Self>) {
        let ids: Vec<Uuid> = {
            let queue = self.queue.lock().await;
            queue
                .all_jobs()
                .iter()
                .filter(|j| matches!(j.status, DownloadStatus::Downloading | DownloadStatus::Queued | DownloadStatus::Scheduled | DownloadStatus::Retrying(_)))
                .map(|j| j.id)
                .collect()
        };
        for id in ids {
            self.pause_job(id).await;
        }
    }

    pub async fn resume_all(self: &Arc<Self>) {
        {
            let mut queue = self.queue.lock().await;
            for job in queue.all_jobs_mut() {
                if job.status == DownloadStatus::Paused 
                    || matches!(job.status, DownloadStatus::Failed(_))
                    || matches!(job.status, DownloadStatus::Retrying(_))
                    || job.status == DownloadStatus::Scheduled {
                    job.auto_retry_at = None;
                    job.bypass_schedule = true;
                    job.status = DownloadStatus::Queued;
                    let _ = self.db.upsert_job(job).await;
                }
            }
        }
        self.notify_update().await;
        self.process_queue().await;
    }

    pub async fn clear_completed(&self) {
        let mut queue = self.queue.lock().await;
        queue.clear_completed();
        drop(queue);
        self.notify_update().await;
    }

    pub async fn clear_failed(&self) {
        let mut queue = self.queue.lock().await;
        queue.clear_failed();
        drop(queue);
        self.notify_update().await;
    }

    pub async fn cancel_all(self: &Arc<Self>) {
        let ids: Vec<Uuid> = {
            let queue = self.queue.lock().await;
            queue
                .all_jobs()
                .iter()
                .filter(|j| matches!(j.status, DownloadStatus::Downloading | DownloadStatus::Queued | DownloadStatus::Paused | DownloadStatus::Scheduled | DownloadStatus::Retrying(_)))
                .map(|j| j.id)
                .collect()
        };
        for id in ids {
            self.cancel_job(id).await;
        }
    }

    pub async fn move_job_up(&self, id: Uuid) -> bool {
        let mut queue = self.queue.lock().await;
        let moved = queue.move_job_up(id);
        drop(queue);
        if moved {
            self.notify_update().await;
        }
        moved
    }

    pub async fn move_job_down(&self, id: Uuid) -> bool {
        let mut queue = self.queue.lock().await;
        let moved = queue.move_job_down(id);
        drop(queue);
        if moved {
            self.notify_update().await;
        }
        moved
    }

    pub async fn prioritize_job(self: &Arc<Self>, id: Uuid) -> bool {
        let mut queue = self.queue.lock().await;
        let moved = queue.prioritize_job(id);
        drop(queue);
        if moved {
            self.notify_update().await;
            self.process_queue().await;
        }
        moved
    }


    pub async fn process_queue(self: &Arc<Self>) {
        let max = *self.max_concurrency.read().await;
        let sched_enabled = *self.scheduler_enabled.read().await;
        let in_window = self.is_in_schedule_window().await;

        loop {
            let job_id = {
                let mut queue = self.queue.lock().await;
                if queue.active_downloads_count() < max {
                    if let Some(id) = queue.next_eligible_job(sched_enabled, in_window) {
                        if let Some(j) = queue.get_job_mut(id) {
                            j.status = DownloadStatus::Downloading;
                            Some(id)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            match job_id {
                Some(id) => {
                    self.spawn_worker(id);
                }
                None => break,
            }
        }
    }

    fn spawn_worker(self: &Arc<Self>, id: Uuid) {
        let manager = self.clone();

        tokio::spawn(async move {
            let (
                url,
                destination,
                _total_bytes,
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
                referer,
            ) = {
                let mut queue = manager.queue.lock().await;
                let job = match queue.get_job_mut(id) {
                    Some(j) => j,
                    None => return,
                };

                let cancel_token = CancellationToken::new();
                job.cancel_token = Some(cancel_token.clone());
                job.status = DownloadStatus::Downloading;
                (
                    job.url.clone(),
                    job.output_path.clone(),
                    job.total_bytes,
                    job.is_extractor,
                    job.is_audio_only,
                    job.quality.clone(),
                    job.download_subtitles,
                    job.subtitle_language.clone(),
                    job.download_thumbnail,
                    job.thumbnail_url.clone(),
                    job.audio_format.clone(),
                    job.audio_bitrate.clone(),
                    job.embed_artwork,
                    job.referer.clone(),
                )
            };

            manager.notify_update().await;

            let mut retry_count = 0;
            loop {
                let token = {
                    let queue = manager.queue.lock().await;
                    queue.get_job(id).and_then(|j| j.cancel_token.clone())
                };

                let cancel_token = match token {
                    Some(t) => t,
                    None => CancellationToken::new(),
                };

                let last_notify_stream = Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
                let last_notify_net = Arc::new(std::sync::Mutex::new(std::time::Instant::now()));

                let active_speed_limit = manager.get_speed_limit().await;
                let active_speed_limit_bytes = parse_speed_limit_bytes(active_speed_limit.as_deref());
                let active_cookies_browser = manager.get_cookies_browser().await;
                let active_proxy = manager.get_proxy().await;
                let active_concurrent_fragments = manager.get_concurrent_fragments().await;

                let mgr_progress = manager.clone();
                let use_extractor = is_extractor || is_audio_only;
                let download_res = if use_extractor {
                    let last_time = last_notify_stream.clone();
                    let stream_res = crate::downloader::extractor::download_stream(
                        &url,
                        &destination,
                        is_audio_only,
                        quality.as_deref(),
                        download_subtitles,
                        subtitle_language.as_deref(),
                        download_thumbnail,
                        audio_format.as_deref(),
                        audio_bitrate.as_deref(),
                        embed_artwork,
                        active_speed_limit.as_deref(),
                        active_cookies_browser.as_deref(),
                        active_proxy.as_deref(),
                        active_concurrent_fragments,
                        referer.as_deref(),
                        cancel_token,
                        move |progress| {
                            let mgr = mgr_progress.clone();
                            let last_t = last_time.clone();
                            tokio::spawn(async move {
                                let mut queue = mgr.queue.lock().await;
                                if let Some(j) = queue.get_job_mut(id) {
                                    if j.status == DownloadStatus::Downloading {
                                        j.update_progress(progress);
                                    }
                                }
                                drop(queue);

                                let should_notify = {
                                    let mut guard = last_t.lock().unwrap();
                                    if guard.elapsed() >= std::time::Duration::from_millis(60) {
                                        *guard = std::time::Instant::now();
                                        true
                                    } else {
                                        false
                                    }
                                };
                                if should_notify {
                                    mgr.notify_update().await;
                                }
                            });
                        },
                    )
                    .await;

                    if let Ok(ref resolved_path) = stream_res {
                        let mut queue = manager.queue.lock().await;
                        if let Some(j) = queue.get_job_mut(id) {
                            j.output_path = resolved_path.clone();
                        }
                    }

                    stream_res.map(|_| ())
                } else {
                    let last_time = last_notify_net.clone();
                    manager
                        .network_client
                        .download_file(
                            &url,
                            &destination,
                            cancel_token,
                            true, // Preserve .part file on cancel/pause for resume
                            active_speed_limit_bytes,
                            referer.as_deref(),
                            move |progress| {
                                let mgr = mgr_progress.clone();
                                let last_t = last_time.clone();
                                tokio::spawn(async move {
                                    let mut queue = mgr.queue.lock().await;
                                    if let Some(j) = queue.get_job_mut(id) {
                                        if j.status == DownloadStatus::Downloading {
                                            j.update_progress(progress);
                                        }
                                    }
                                    drop(queue);

                                    let should_notify = {
                                        let mut guard = last_t.lock().unwrap();
                                        if guard.elapsed() >= std::time::Duration::from_millis(60) {
                                            *guard = std::time::Instant::now();
                                            true
                                        } else {
                                            false
                                        }
                                    };
                                    if should_notify {
                                        mgr.notify_update().await;
                                    }
                                });
                            },
                        )
                        .await
                };

                match download_res {
                    Ok(()) => {
                        let (final_size, out_path) = {
                            let mut queue = manager.queue.lock().await;
                            if let Some(j) = queue.get_job_mut(id) {
                                (j.downloaded_bytes, j.output_path.clone())
                            } else {
                                (0, destination.clone())
                            }
                        };

                        // Auto-cleaner: If downloaded file is 0 bytes, discard and fail
                        if final_size == 0 {
                            warn!("Job {} resulted in 0 bytes; cleaning up corrupt file {:?}", id, out_path);
                            let _ = tokio::fs::remove_file(&out_path).await;
                            let err_msg = "Download aborted: server returned 0 bytes of media".to_string();
                            {
                                let mut queue = manager.queue.lock().await;
                                if let Some(j) = queue.get_job_mut(id) {
                                    j.status = DownloadStatus::Failed(err_msg.clone());
                                    j.speed_bytes_sec = 0.0;
                                    j.eta_seconds = None;
                                }
                            }
                            let _ = manager.db.mark_failed(id, &err_msg).await;
                            manager.notify_update().await;
                            break;
                        }

                        // If thumbnail downloading is enabled, ensure companion image is saved
                        if download_thumbnail {
                            let parent = out_path.parent().unwrap_or(Path::new("."));
                            let stem = out_path.file_stem().and_then(|s| s.to_str()).unwrap_or("thumb");
                            let has_thumb = ["png", "jpg", "jpeg", "webp", "avif"].iter().any(|ext| {
                                parent.join(format!("{}.{}", stem, ext)).exists()
                            });
                            if !has_thumb {
                                if let Some(ref thumb_url) = thumbnail_url {
                                    if let Ok(bytes) = manager.network_client.download_image_bytes(thumb_url).await {
                                        let thumb_path = parent.join(format!("{}.jpg", stem));
                                        let _ = tokio::fs::write(&thumb_path, &bytes).await;
                                        info!("Saved companion thumbnail to {:?}", thumb_path);
                                    }
                                }
                            }
                        }

                        info!("Job {} completed successfully with size {} bytes", id, final_size);
                        let org_mode = manager.get_asset_organization_mode().await;
                        let final_out_path = match crate::filesystem::organize_media_assets_with_mode(&out_path, org_mode.into()).await {
                            Ok((_, new_p)) => new_p,
                            Err(_) => out_path.clone(),
                        };

                        let job_title = {
                            let mut queue = manager.queue.lock().await;
                            if let Some(j) = queue.get_job_mut(id) {
                                j.status = DownloadStatus::Completed;
                                j.progress_ratio = 1.0;
                                j.downloaded_bytes = final_size;
                                j.total_bytes = Some(final_size);
                                j.speed_bytes_sec = 0.0;
                                j.eta_seconds = Some(0);
                                j.output_path = final_out_path.clone();
                                j.title.clone()
                            } else {
                                "Media file".to_string()
                            }
                        };
                        crate::notifications::send_notification(
                            "Native Video Downloader",
                            "Download Complete",
                            &format!("\"{}\" has finished downloading.", job_title),
                            false,
                        );
                        if let Err(e) = manager.db.mark_completed(id, final_size, Some(&final_out_path)).await {
                            error!("Failed to mark job {} as completed in database: {}", id, e);
                        }
                        manager.notify_update().await;
                        break;
                    }
                    Err(AppError::Cancelled) => {
                        info!("Job {} was cancelled or paused", id);
                        break;
                    }
                    Err(err) => {
                        warn!("Job {} failed with error: {}", id, err);

                        if manager.retry_policy.should_retry(&err, retry_count) {
                            retry_count += 1;
                            let delay = manager.retry_policy.delay_for_retry(retry_count);
                            warn!("Retrying job {} in {:?} (attempt {}/{})", id, delay, retry_count, manager.retry_policy.max_retries);

                            {
                                let mut queue = manager.queue.lock().await;
                                if let Some(j) = queue.get_job_mut(id) {
                                    j.retry_count = retry_count;
                                }
                            }
                            manager.notify_update().await;

                            tokio::time::sleep(delay).await;
                            continue;
                        } else {
                            error!("Job {} exhausted initial worker retries: {}", id, err);
                            let err_str = err.to_string();
                            let clean_err = crate::downloader::extractor::clean_extractor_error(&err_str);

                            let (auto_retry_en, auto_interval, auto_max) = manager.get_auto_retry_config().await;
                            let can_auto_retry = auto_retry_en && {
                                let queue = manager.queue.lock().await;
                                queue.get_job(id).map(|j| j.auto_retry_count < auto_max).unwrap_or(false)
                            };

                            if can_auto_retry {
                                let now_ts = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_secs();
                                let target_ts = now_ts + auto_interval;
                                info!("Scheduling automated retry for job {} in {}s (target ts: {})", id, auto_interval, target_ts);
                                {
                                    let mut queue = manager.queue.lock().await;
                                    if let Some(j) = queue.get_job_mut(id) {
                                        j.auto_retry_at = Some(target_ts);
                                        j.status = DownloadStatus::Retrying(target_ts);
                                        j.speed_bytes_sec = 0.0;
                                        j.eta_seconds = None;
                                    }
                                }
                                manager.notify_update().await;
                                break;
                            } else {
                                let job_title = {
                                    let mut queue = manager.queue.lock().await;
                                    if let Some(j) = queue.get_job_mut(id) {
                                        j.auto_retry_at = None;
                                        j.status = DownloadStatus::Failed(clean_err.clone());
                                        j.speed_bytes_sec = 0.0;
                                        j.eta_seconds = None;
                                        j.title.clone()
                                    } else {
                                        "Media download".to_string()
                                    }
                                };
                                crate::notifications::send_notification(
                                    "Native Video Downloader",
                                    "Download Failed",
                                    &format!("\"{}\" failed: {}", job_title, clean_err),
                                    true,
                                );
                                let _ = manager.db.mark_failed(id, &clean_err).await;
                                manager.notify_update().await;
                                break;
                            }
                        }
                    }
                }
            }

            manager.process_queue().await;
            manager.notify_update().await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_speed_limit_bytes() {
        assert_eq!(parse_speed_limit_bytes(None), None);
        assert_eq!(parse_speed_limit_bytes(Some("")), None);
        assert_eq!(parse_speed_limit_bytes(Some("unlimited")), None);
        assert_eq!(parse_speed_limit_bytes(Some("2M")), Some(2 * 1024 * 1024));
        assert_eq!(parse_speed_limit_bytes(Some("5M")), Some(5 * 1024 * 1024));
        assert_eq!(parse_speed_limit_bytes(Some("10M")), Some(10 * 1024 * 1024));
        assert_eq!(parse_speed_limit_bytes(Some("20m")), Some(20 * 1024 * 1024));
        assert_eq!(parse_speed_limit_bytes(Some("1.5M")), Some((1.5 * 1024.0 * 1024.0) as u64));
        assert_eq!(parse_speed_limit_bytes(Some("500K")), Some(500 * 1024));
    }

    #[tokio::test]
    async fn test_cookies_browser_config() {
        let db_path = std::env::temp_dir().join(format!("test_mgr_cookies_{}.db", Uuid::new_v4()));
        let db = Arc::new(Database::init(&db_path).await.unwrap());
        let mgr = DownloadManager::new(3, db.clone());

        assert_eq!(mgr.get_cookies_browser().await, None);
        mgr.set_cookies_browser(Some("chrome".to_string())).await;
        assert_eq!(mgr.get_cookies_browser().await, Some("chrome".to_string()));
        mgr.set_cookies_browser(Some("firefox".to_string())).await;
        assert_eq!(mgr.get_cookies_browser().await, Some("firefox".to_string()));
        mgr.set_cookies_browser(None).await;
        assert_eq!(mgr.get_cookies_browser().await, None);

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn test_proxy_config() {
        let db_path = std::env::temp_dir().join(format!("test_mgr_proxy_{}.db", Uuid::new_v4()));
        let db = Arc::new(Database::init(&db_path).await.unwrap());
        let mgr = DownloadManager::new(3, db.clone());

        assert_eq!(mgr.get_proxy().await, None);
        mgr.set_proxy(Some("http://127.0.0.1:7890".to_string())).await;
        assert_eq!(mgr.get_proxy().await, Some("http://127.0.0.1:7890".to_string()));
        mgr.set_proxy(Some("socks5://127.0.0.1:1080".to_string())).await;
        assert_eq!(mgr.get_proxy().await, Some("socks5://127.0.0.1:1080".to_string()));
        mgr.set_proxy(None).await;
        assert_eq!(mgr.get_proxy().await, None);

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn test_filename_template_config() {
        let db_path = std::env::temp_dir().join(format!("test_mgr_tpl_{}.db", Uuid::new_v4()));
        let db = Arc::new(Database::init(&db_path).await.unwrap());
        let mgr = DownloadManager::new(3, db.clone());

        assert_eq!(mgr.get_filename_template().await, "{title}.{ext}");
        mgr.set_filename_template("{series}/{index} - {title}.{ext}".to_string()).await;
        assert_eq!(mgr.get_filename_template().await, "{series}/{index} - {title}.{ext}");

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn test_scheduler_and_auto_retry_config() {
        let db_path = std::env::temp_dir().join(format!("test_mgr_sched_{}.db", Uuid::new_v4()));
        let db = Arc::new(Database::init(&db_path).await.unwrap());
        let mgr = DownloadManager::new(3, db.clone());

        assert_eq!(mgr.get_scheduler_config().await, (false, 2, 0, 7, 0));
        mgr.set_scheduler_config(true, 1, 30, 6, 45).await;
        assert_eq!(mgr.get_scheduler_config().await, (true, 1, 30, 6, 45));

        assert_eq!(mgr.get_auto_retry_config().await, (true, 60, 3));
        mgr.set_auto_retry_config(false, 120, 5).await;
        assert_eq!(mgr.get_auto_retry_config().await, (false, 120, 5));

        let _ = std::fs::remove_file(db_path);
    }
}
